//! A running VNC session: the reader decodes what the server sends into
//! the desktop, and this loop sends input, keeps updates coming, changes
//! the desktop's size and screens where the server allows it, and hands
//! frames to the interface one at a time.

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf, WriteHalf};
use tokio::sync::mpsc;

use super::apple::pasteboard;
use super::clipboard::Clipboard;
use super::decode::Desktop;
use super::h264::Contexts;
use super::handshake::{MAX_SIDE, Opened};
use super::input::Keys;
use super::reader::{self, Note, Screen, encoding};
use crate::connection::VncCodec;
use crate::remote::frames::Frames;
use crate::remote::route::Stream;
use crate::remote::{
    ClipboardCommand, Command, Cursor, Ending, Event, Failure, Frame, Monitor, Rect, Target,
    describe,
};

/// By this many updates, a server that can change its screens has said
/// so: TigerVNC and GNOME do in the first, neatvnc once a frame shows its
/// layout.
const SAID_BY: u32 = 3;

/// The status of a change of screens the server passed on to someone else
/// to make (QEMU's guest, or wayvnc's compositor), which may still happen.
const FORWARDED: u16 = 4;

/// A fence the server wants back carries this flag; the answer doesn't.
const FENCE_REQUEST: u32 = 1 << 31;
/// The flags a fence's answer keeps: BlockBefore, BlockAfter, SyncNext.
const FENCE_KEPT: u32 = 0b111;

/// The pixel format asked for: 32 bits, little-endian, true color, red in
/// the third byte and blue in the first, which lays pixels out as BGRX.
const PIXEL_FORMAT: [u8; 16] = [32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0];

/// The encodings to ask for, the one most wanted first. Tight sends JPEG
/// only to a client that names a quality, so Lossless names none.
fn encodings(codec: VncCodec, h264: bool, clipboard: bool) -> Vec<i32> {
    #[cfg(test)]
    if let Ok(only) = std::env::var("MOTE_TEST_ENCODING") {
        return test_encodings(&only, h264);
    }
    let mut list = Vec::new();
    if h264 && codec != VncCodec::Lossless {
        list.push(encoding::OPEN_H264);
    }
    let (first, second) = match codec {
        VncCodec::Lossless => (encoding::ZRLE, encoding::TIGHT),
        _ => (encoding::TIGHT, encoding::ZRLE),
    };
    list.extend([
        first,
        second,
        encoding::HEXTILE,
        encoding::ZLIB,
        encoding::COPY_RECT,
        encoding::RAW,
    ]);
    // Compression 2 is TigerVNC's own default; 9, the server's hardest.
    match codec {
        VncCodec::Best => {
            list.extend([encoding::JPEG_QUALITY_0 + 8, encoding::COMPRESS_LEVEL_0 + 2])
        }
        VncCodec::Lossless => list.push(encoding::COMPRESS_LEVEL_0 + 2),
        VncCodec::LowBandwidth => {
            list.extend([encoding::JPEG_QUALITY_0 + 3, encoding::COMPRESS_LEVEL_0 + 9])
        }
    }
    list.extend(PSEUDO);
    if clipboard {
        list.push(encoding::EXTENDED_CLIPBOARD);
    }
    list
}

/// What this side understands beyond pictures.
const PSEUDO: [i32; 11] = [
    encoding::CURSOR,
    encoding::DESKTOP_SIZE,
    encoding::EXTENDED_DESKTOP_SIZE,
    encoding::LAST_RECT,
    encoding::DESKTOP_NAME,
    encoding::QEMU_KEYS,
    encoding::EXTENDED_MOUSE_BUTTONS,
    encoding::CONTINUOUS_UPDATES,
    encoding::FENCE,
    encoding::QEMU_LEDS,
    encoding::VMWARE_LEDS,
];

/// In tests, `MOTE_TEST_ENCODING` (tight, zrle, hextile, zlib, h264 or
/// raw) asks for that one alone, beside the pseudo-encodings.
#[cfg(test)]
fn test_encodings(only: &str, h264: bool) -> Vec<i32> {
    let kind = match only {
        "tight" => encoding::TIGHT,
        "zrle" => encoding::ZRLE,
        "hextile" => encoding::HEXTILE,
        "zlib" => encoding::ZLIB,
        "h264" if h264 => encoding::OPEN_H264,
        _ => encoding::RAW,
    };
    std::iter::once(kind)
        .chain([encoding::RAW, encoding::JPEG_QUALITY_0 + 8])
        .chain(PSEUDO)
        .collect()
}

/// Aborts the reader when the session ends, however it ends.
struct Reading(tokio::task::JoinHandle<()>);

impl Drop for Reading {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The connection's reading side, counting what comes in for the stats.
struct Counting<R> {
    inner: R,
    count: Arc<AtomicU64>,
}

impl<R: AsyncRead + Unpin> AsyncRead for Counting<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let poll = Pin::new(&mut this.inner).poll_read(cx, buf);
        let read = buf.filled().len() - before;
        this.count.fetch_add(read as u64, Ordering::Relaxed);
        poll
    }
}

/// What was last asked of the server's screens, to know what a refusal
/// means.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Asked {
    Size,
    Monitors,
}

struct Live<'a> {
    write: WriteHalf<Box<dyn Stream>>,
    desktop: Arc<Mutex<Desktop>>,
    events: &'a mpsc::Sender<Event>,
    frames: Frames,
    /// Changed since the last frame went out, and since when.
    damage: Vec<Rect>,
    since: Option<std::time::Instant>,
    /// The interface has the last frame and hasn't shown it yet.
    in_flight: bool,
    /// The server's screens, once it has said it can change them.
    screens: Option<Vec<Screen>>,
    asked: Option<Asked>,
    /// A size to ask for as soon as the server says it can take one.
    wanted: Option<(u16, u16)>,
    /// Updates read so far, up to [`SAID_BY`]: a server that can change
    /// its screens has said so by then.
    updates: u32,
    /// The server sends updates as they happen, unasked.
    continuous: bool,
    received: Arc<AtomicU64>,
    counted: u64,
    stats: crate::stats::Stats,
    /// Shared with the server, when the connection shares it.
    clipboard: Option<Clipboard>,
}

pub(super) async fn run(
    opened: Opened,
    target: &Target,
    events: &mpsc::Sender<Event>,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    keys: &mut Keys,
    early: Option<ClipboardCommand>,
) -> Result<Ending, Failure> {
    let Opened {
        stream,
        init,
        rekeys,
        ..
    } = opened;
    let apple = rekeys.is_some();
    let (read, write) = tokio::io::split(stream);
    let desktop = Arc::new(Mutex::new(Desktop::new(init.width, init.height)));
    let (notes_tx, mut notes) = mpsc::channel(16);
    let h264 = (target.vnc_codec != VncCodec::Lossless)
        .then(Contexts::new)
        .flatten();
    let encodings = encodings(target.vnc_codec, h264.is_some(), target.clipboard);
    let received = Arc::new(AtomicU64::new(0));
    let read = Counting {
        inner: read,
        count: received.clone(),
    };
    let _reading = Reading(tokio::spawn(reader::run(
        read,
        desktop.clone(),
        notes_tx,
        h264,
        rekeys,
    )));
    keys.connected();
    let mut live = Live {
        write,
        desktop,
        events,
        frames: Frames::new(),
        damage: Vec::new(),
        since: None,
        in_flight: false,
        screens: None,
        asked: None,
        wanted: target
            .resize
            .then_some((target.width, target.height))
            .filter(|&size| size != (init.width, init.height)),
        updates: 0,
        continuous: false,
        received,
        counted: 0,
        stats: crate::stats::Stats::new("engine"),
        clipboard: target.clipboard.then(|| Clipboard::new(apple)),
    };

    let mut hello = Vec::with_capacity(128);
    hello.extend_from_slice(&[0, 0, 0, 0]);
    hello.extend_from_slice(&PIXEL_FORMAT);
    hello.extend_from_slice(&[2, 0]);
    hello.extend_from_slice(&(encodings.len() as u16).to_be_bytes());
    for kind in encodings {
        hello.extend_from_slice(&kind.to_be_bytes());
    }
    request(&mut hello, false, init.width, init.height);
    if apple && target.clipboard {
        hello.extend_from_slice(&pasteboard::WATCH);
    }
    live.send(&hello).await?;
    let connected = Event::Connected {
        width: init.width,
        height: init.height,
    };
    if events.send(connected).await.is_err() {
        return Ok(Ending::Closed);
    }
    // What was copied here while connecting.
    if let Some(command) = early {
        live.clipboard_command(command).await?;
    }

    let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
    loop {
        let event = tokio::select! {
            note = notes.recv() => {
                let Some(note) = note else {
                    return Err(Failure::Protocol("the reader stopped".into()));
                };
                match note {
                    Note::Ended(ending) => return ending,
                    note => live.note(note, keys).await?,
                }
            }
            command = commands.recv() => match command {
                Some(Command::Input(input)) => {
                    let mut out = Vec::new();
                    for input in input {
                        keys.message(input, &mut out);
                    }
                    live.send(&out).await?;
                    None
                }
                Some(Command::FrameDone) => {
                    live.in_flight = false;
                    live.frame()
                }
                Some(Command::Resize { width, height, .. }) => {
                    live.resize(width, height).await?
                }
                Some(Command::Layout(monitors)) => live.layout(&monitors).await?,
                Some(Command::Clipboard(command)) => {
                    live.clipboard_command(command).await?;
                    None
                }
                Some(Command::Close) | None => return Ok(Ending::Closed),
            },
            Some(answer) = async {
                match &mut live.clipboard {
                    Some(clipboard) => clipboard.answers.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                let mut out = Vec::new();
                if let Some(clipboard) = &mut live.clipboard {
                    clipboard.answered(answer, &mut out);
                }
                live.send(&out).await?;
                None
            }
            _ = tick.tick() => {
                if let Some(clipboard) = &mut live.clipboard {
                    clipboard.tick();
                }
                None
            }
        };
        if let Some(event) = event
            && events.send(event).await.is_err()
        {
            return Ok(Ending::Closed);
        }
    }
}

impl Live<'_> {
    async fn note(&mut self, note: Note, keys: &mut Keys) -> Result<Option<Event>, Failure> {
        Ok(match note {
            Note::Updated(damage, spent) => {
                self.stats.duration("decode", spent);
                let received = self.received.load(Ordering::Relaxed);
                self.stats.count("kB in", (received - self.counted) / 1024);
                self.counted = received - (received - self.counted) % 1024;
                if !self.continuous {
                    // Asked for at once, so the server can be at work on
                    // the next while this one is drawn.
                    let (width, height) = self.size();
                    let mut next = Vec::with_capacity(10);
                    request(&mut next, true, width, height);
                    self.send(&next).await?;
                }
                if !damage.is_empty() {
                    self.since.get_or_insert_with(std::time::Instant::now);
                    self.damage.extend(damage);
                }
                if self.updates == 0
                    && let Some(clipboard) = &mut self.clipboard
                {
                    let (mut out, mut events) = (Vec::new(), Vec::new());
                    clipboard.settle(&mut out, &mut events);
                    self.send(&out).await?;
                    for event in events {
                        self.tell(event).await?;
                    }
                }
                if self.updates < SAID_BY {
                    self.updates += 1;
                    if self.updates == SAID_BY && self.screens.is_none() {
                        tracing::info!("the server keeps its own size");
                        self.wanted = None;
                        self.tell(Event::KeepsSize).await?;
                    }
                }
                self.frame()
            }
            Note::Layout {
                asked,
                status,
                screens,
            } => {
                let first = self.screens.is_none();
                self.screens = Some(screens);
                if asked && status == FORWARDED {
                    tracing::debug!("the server passed the change on; it may still come");
                    return Ok(None);
                }
                if status != 0 && asked {
                    return Ok(match self.asked.take() {
                        Some(Asked::Monitors) => Some(Event::OneMonitor),
                        _ => Some(Event::KeepsSize),
                    });
                }
                if first && let Some((width, height)) = self.wanted.take() {
                    return self.resize(width.into(), height.into()).await;
                }
                None
            }
            Note::Resized(width, height) => {
                // A new size makes new frames, from all of it.
                self.frames = Frames::new();
                self.damage = vec![Rect {
                    x: 0,
                    y: 0,
                    width: width.into(),
                    height: height.into(),
                }];
                if self.continuous {
                    self.send(&continuous(true, width, height)).await?;
                }
                Some(Event::Connected { width, height })
            }
            Note::ContinuousUpdates => {
                let (width, height) = self.size();
                if self.continuous {
                    // The server stopped: back to asking.
                    self.continuous = false;
                    let mut next = Vec::with_capacity(10);
                    request(&mut next, true, width, height);
                    self.send(&next).await?;
                } else {
                    tracing::info!("the server sends updates as they happen");
                    self.continuous = true;
                    self.send(&continuous(true, width, height)).await?;
                }
                None
            }
            Note::Fence { flags, payload } => {
                if flags & FENCE_REQUEST != 0 {
                    let mut answer = vec![248, 0, 0, 0];
                    answer.extend_from_slice(&(flags & FENCE_KEPT).to_be_bytes());
                    answer.push(payload.len() as u8);
                    answer.extend(payload);
                    self.send(&answer).await?;
                }
                None
            }
            Note::CutText(data) => {
                let mut events = Vec::new();
                if let Some(clipboard) = &mut self.clipboard {
                    clipboard.cut_text(&data, &mut events);
                }
                for event in events {
                    self.tell(event).await?;
                }
                None
            }
            Note::ExtendedClipboard(data) => {
                let (mut out, mut events) = (Vec::new(), Vec::new());
                if let Some(clipboard) = &mut self.clipboard {
                    clipboard.extended(&data, &mut out, &mut events);
                }
                self.send(&out).await?;
                for event in events {
                    self.tell(event).await?;
                }
                None
            }
            Note::AppleClipboardChanged => {
                if self.clipboard.is_some() {
                    self.send(&pasteboard::FETCH).await?;
                }
                None
            }
            Note::AppleClipboard(text) => {
                let mut events = Vec::new();
                if let Some(clipboard) = &mut self.clipboard {
                    clipboard.copied(text, &mut events);
                }
                for event in events {
                    self.tell(event).await?;
                }
                None
            }
            Note::Cursor(Some(pointer)) => Some(Event::Cursor(Cursor::Bitmap(pointer.into()))),
            Note::Cursor(None) => Some(Event::Cursor(Cursor::Hidden)),
            Note::Bell => Some(Event::Bell),
            Note::QemuKeys => {
                tracing::info!("the server takes scancodes");
                keys.qemu = true;
                None
            }
            Note::ExtendedButtons => {
                keys.extended_buttons = true;
                None
            }
            Note::Leds(state) => {
                tracing::debug!("the server's lock keys: {state:#05b}");
                keys.leds = Some(state);
                None
            }
            // The loop ends the session on it before it gets here.
            Note::Ended(_) => None,
        })
    }

    async fn clipboard_command(&mut self, command: ClipboardCommand) -> Result<(), Failure> {
        let Some(clipboard) = &mut self.clipboard else {
            return Ok(());
        };
        let (mut out, mut events) = (Vec::new(), Vec::new());
        clipboard.command(command, &mut out, &mut events);
        self.send(&out).await?;
        for event in events {
            self.tell(event).await?;
        }
        Ok(())
    }

    /// Ask for the desktop to be this size, on one screen.
    async fn resize(&mut self, width: u32, height: u32) -> Result<Option<Event>, Failure> {
        if self.screens.is_none() {
            // Not known yet whether the server can: ask once it says so.
            if self.updates < SAID_BY {
                self.wanted = Some((clamp(width), clamp(height)));
            }
            return Ok(None);
        }
        let (width, height) = (clamp(width), clamp(height));
        if self.size() == (width, height) && self.screens.as_ref().is_some_and(|s| s.len() == 1) {
            return Ok(None);
        }
        tracing::debug!("asking for {width}×{height}");
        self.asked = Some(Asked::Size);
        self.send_screens(&[(0, 0, width, height)]).await?;
        Ok(None)
    }

    /// Spread the desktop over these monitors, the first the primary.
    async fn layout(&mut self, monitors: &[Monitor]) -> Result<Option<Event>, Failure> {
        if self.screens.is_none() {
            return Ok(Some(Event::OneMonitor));
        }
        // Their places from the top left of them all.
        let left = monitors.iter().map(|m| m.x).min().unwrap_or(0);
        let top = monitors.iter().map(|m| m.y).min().unwrap_or(0);
        let places: Vec<_> = monitors
            .iter()
            .map(|m| {
                (
                    place(m.x - left),
                    place(m.y - top),
                    clamp(m.width),
                    clamp(m.height),
                )
            })
            .collect();
        self.asked = Some(Asked::Monitors);
        self.send_screens(&places).await?;
        Ok(None)
    }

    /// SetDesktopSize: the whole desktop, and each screen on it. Screens
    /// keep the ids the server gave them, in order; new ones take the next.
    async fn send_screens(&mut self, places: &[(u16, u16, u16, u16)]) -> Result<(), Failure> {
        let known = self.screens.clone().unwrap_or_default();
        let width = places.iter().map(|p| p.0 + p.2).max().unwrap_or(0);
        let height = places.iter().map(|p| p.1 + p.3).max().unwrap_or(0);
        let mut next_id = known.iter().map(|s| s.id).max().unwrap_or(0) + 1;
        let mut message = vec![251, 0];
        message.extend_from_slice(&width.min(MAX_SIDE).to_be_bytes());
        message.extend_from_slice(&height.min(MAX_SIDE).to_be_bytes());
        message.extend_from_slice(&[places.len() as u8, 0]);
        for (i, &(x, y, w, h)) in places.iter().enumerate() {
            let (id, flags) = match known.get(i) {
                Some(screen) => (screen.id, screen.flags),
                None => {
                    next_id += 1;
                    (next_id - 1, 0)
                }
            };
            message.extend_from_slice(&id.to_be_bytes());
            for value in [x, y, w, h] {
                message.extend_from_slice(&value.to_be_bytes());
            }
            message.extend_from_slice(&flags.to_be_bytes());
        }
        self.send(&message).await
    }

    /// The next frame, when there are changes and the interface has shown
    /// the last one.
    fn frame(&mut self) -> Option<Event> {
        if self.in_flight || self.damage.is_empty() {
            return None;
        }
        let start = std::time::Instant::now();
        let desktop = self.desktop.lock().expect("desktop");
        let pixels = self
            .frames
            .make(&desktop.pixels, desktop.stride(), &self.damage);
        self.stats.time("copy", start);
        self.stats.count("frames", 1);
        self.in_flight = true;
        Some(Event::Frame(Frame {
            width: desktop.width,
            height: desktop.height,
            stride: desktop.stride(),
            pixels,
            damage: std::mem::take(&mut self.damage),
            made: self.since.take().unwrap_or(start),
        }))
    }

    fn size(&self) -> (u16, u16) {
        let desktop = self.desktop.lock().expect("desktop");
        (desktop.width, desktop.height)
    }

    async fn tell(&self, event: Event) -> Result<(), Failure> {
        // A closed interface ends the session through its commands.
        let _ = self.events.send(event).await;
        Ok(())
    }

    async fn send(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        if bytes.is_empty() {
            return Ok(());
        }
        // Flushed too: an encrypted stream may hold bytes back until then.
        let sent = async {
            self.write.write_all(bytes).await?;
            self.write.flush().await
        };
        sent.await.map_err(|e| Failure::Lost(describe(&e)))
    }
}

/// A size, from 1 to the largest side taken.
fn clamp(value: u32) -> u16 {
    value.clamp(1, u32::from(MAX_SIDE)) as u16
}

/// A place on the desktop, from 0.
fn place(value: i32) -> u16 {
    value.clamp(0, i32::from(MAX_SIDE)) as u16
}

/// A FramebufferUpdateRequest for the whole desktop.
fn request(out: &mut Vec<u8>, incremental: bool, width: u16, height: u16) {
    out.extend_from_slice(&[3, u8::from(incremental), 0, 0, 0, 0]);
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
}

/// EnableContinuousUpdates, for the whole desktop.
fn continuous(on: bool, width: u16, height: u16) -> Vec<u8> {
    let mut message = vec![150, u8::from(on), 0, 0, 0, 0];
    message.extend_from_slice(&width.to_be_bytes());
    message.extend_from_slice(&height.to_be_bytes());
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lossless_names_no_jpeg_quality_and_no_h264() {
        let lossless = encodings(VncCodec::Lossless, true, false);
        assert_eq!(lossless[0], encoding::ZRLE);
        assert!(!lossless.contains(&encoding::OPEN_H264));
        assert!(!lossless.iter().any(|e| (-32..=-23).contains(e)));
        let best = encodings(VncCodec::Best, true, true);
        assert_eq!(best[0], encoding::OPEN_H264);
        assert!(best.contains(&(encoding::JPEG_QUALITY_0 + 8)));
    }
}
