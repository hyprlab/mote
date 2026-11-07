//! A running VNC session: the reader decodes what the server sends into
//! the desktop, and this loop sends input, asks for the next update as
//! soon as one has been read, and hands frames to the interface one at a
//! time.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncWriteExt, WriteHalf};
use tokio::sync::mpsc;

use super::decode::Desktop;
use super::handshake::ServerInit;
use super::input::Keys;
use super::reader::{self, Note, encoding};
use crate::remote::frames::Frames;
use crate::remote::route::Stream;
use crate::remote::{Command, Cursor, Ending, Event, Failure, Frame, Rect, describe};

/// Tight's JPEG quality, 0 to 9, until section 15 makes it a choice.
const JPEG_QUALITY: i32 = 8;
/// How hard the server compresses, 0 to 9: TigerVNC's own default.
const COMPRESS_LEVEL: i32 = 2;

/// The encodings asked for, the one most wanted first.
const ENCODINGS: [i32; 15] = [
    encoding::TIGHT,
    encoding::ZRLE,
    encoding::HEXTILE,
    encoding::ZLIB,
    encoding::COPY_RECT,
    encoding::RAW,
    encoding::JPEG_QUALITY_0 + JPEG_QUALITY,
    encoding::COMPRESS_LEVEL_0 + COMPRESS_LEVEL,
    encoding::CURSOR,
    encoding::DESKTOP_SIZE,
    encoding::EXTENDED_DESKTOP_SIZE,
    encoding::LAST_RECT,
    encoding::DESKTOP_NAME,
    encoding::QEMU_KEYS,
    encoding::EXTENDED_MOUSE_BUTTONS,
];

/// The pixel format asked for: 32 bits, little-endian, true color, red in
/// the third byte and blue in the first, which lays pixels out as BGRX.
const PIXEL_FORMAT: [u8; 16] = [32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0];

/// Aborts the reader when the session ends, however it ends.
struct Reading(tokio::task::JoinHandle<()>);

impl Drop for Reading {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Changes waiting for the interface, and whether it has the last frame.
struct Pending {
    frames: Frames,
    damage: Vec<Rect>,
    since: Option<std::time::Instant>,
    in_flight: bool,
}

pub(super) async fn run(
    stream: Box<dyn Stream>,
    init: ServerInit,
    events: &mpsc::Sender<Event>,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    keys: &mut Keys,
) -> Result<Ending, Failure> {
    let (read, mut write) = tokio::io::split(stream);
    let desktop = Arc::new(Mutex::new(Desktop::new(init.width, init.height)));
    let (notes_tx, mut notes) = mpsc::channel(16);
    let _reading = Reading(tokio::spawn(reader::run(read, desktop.clone(), notes_tx)));
    keys.connected();

    let mut hello = Vec::with_capacity(64);
    hello.extend_from_slice(&[0, 0, 0, 0]);
    hello.extend_from_slice(&PIXEL_FORMAT);
    hello.extend_from_slice(&[2, 0]);
    let encodings = encodings();
    hello.extend_from_slice(&(encodings.len() as u16).to_be_bytes());
    for kind in encodings {
        hello.extend_from_slice(&kind.to_be_bytes());
    }
    request(&mut hello, false, init.width, init.height);
    send(&mut write, &hello).await?;

    for event in [
        Event::Connected {
            width: init.width,
            height: init.height,
        },
        // Resizing the remote desktop comes with section 15.
        Event::KeepsSize,
    ] {
        if events.send(event).await.is_err() {
            return Ok(Ending::Closed);
        }
    }
    let mut pending = Pending {
        frames: Frames::new(),
        damage: Vec::new(),
        since: None,
        in_flight: false,
    };
    let mut stats = crate::stats::Stats::new("engine");
    loop {
        let event = tokio::select! {
            note = notes.recv() => {
                let Some(note) = note else {
                    return Err(Failure::Protocol("the reader stopped".into()));
                };
                match note {
                    Note::Updated(damage) => {
                        // Asked for at once, so the server can be at work
                        // on the next while this one is drawn.
                        let (width, height) = {
                            let desktop = desktop.lock().expect("desktop");
                            (desktop.width, desktop.height)
                        };
                        let mut next = Vec::with_capacity(10);
                        request(&mut next, true, width, height);
                        send(&mut write, &next).await?;
                        if !damage.is_empty() {
                            pending.since.get_or_insert_with(std::time::Instant::now);
                            pending.damage.extend(damage);
                        }
                        frame(&desktop, &mut pending, &mut stats)
                    }
                    Note::Resized(width, height) => {
                        // A new size makes new frames, from all of it.
                        pending.frames = Frames::new();
                        pending.damage = vec![Rect {
                            x: 0,
                            y: 0,
                            width: width.into(),
                            height: height.into(),
                        }];
                        Some(Event::Connected { width, height })
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
                    Note::Ended(ending) => return ending,
                }
            }
            command = commands.recv() => match command {
                Some(Command::Input(input)) => {
                    let mut out = Vec::new();
                    for input in input {
                        keys.message(input, &mut out);
                    }
                    send(&mut write, &out).await?;
                    None
                }
                Some(Command::FrameDone) => {
                    pending.in_flight = false;
                    frame(&desktop, &mut pending, &mut stats)
                }
                // Until section 15, the desktop keeps the server's size,
                // and section 16 brings the clipboard.
                Some(Command::Resize { .. } | Command::Layout(_) | Command::Clipboard(_)) => None,
                Some(Command::Close) | None => return Ok(Ending::Closed),
            },
        };
        if let Some(event) = event
            && events.send(event).await.is_err()
        {
            return Ok(Ending::Closed);
        }
    }
}

/// The next frame, when there are changes and the interface has shown the
/// last one.
fn frame(
    desktop: &Mutex<Desktop>,
    pending: &mut Pending,
    stats: &mut crate::stats::Stats,
) -> Option<Event> {
    if pending.in_flight || pending.damage.is_empty() {
        return None;
    }
    let start = std::time::Instant::now();
    let desktop = desktop.lock().expect("desktop");
    let pixels = pending
        .frames
        .make(&desktop.pixels, desktop.stride(), &pending.damage);
    stats.time("copy", start);
    stats.count("frames", 1);
    pending.in_flight = true;
    Some(Event::Frame(Frame {
        width: desktop.width,
        height: desktop.height,
        stride: desktop.stride(),
        pixels,
        damage: std::mem::take(&mut pending.damage),
        made: pending.since.take().unwrap_or(start),
    }))
}

/// The encodings to ask for. In tests, `MOTE_TEST_ENCODING` (tight, zrle,
/// hextile, zlib or raw) asks for that one alone, beside the pseudo ones.
fn encodings() -> Vec<i32> {
    #[cfg(test)]
    if let Ok(only) = std::env::var("MOTE_TEST_ENCODING") {
        let kind = match only.as_str() {
            "tight" => encoding::TIGHT,
            "zrle" => encoding::ZRLE,
            "hextile" => encoding::HEXTILE,
            "zlib" => encoding::ZLIB,
            _ => encoding::RAW,
        };
        return std::iter::once(kind)
            .chain(ENCODINGS.into_iter().filter(|&e| e < 0))
            .collect();
    }
    ENCODINGS.to_vec()
}

/// A FramebufferUpdateRequest for the whole desktop.
fn request(out: &mut Vec<u8>, incremental: bool, width: u16, height: u16) {
    out.extend_from_slice(&[3, u8::from(incremental), 0, 0, 0, 0]);
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
}

async fn send(write: &mut WriteHalf<Box<dyn Stream>>, bytes: &[u8]) -> Result<(), Failure> {
    if bytes.is_empty() {
        return Ok(());
    }
    write
        .write_all(bytes)
        .await
        .map_err(|e| Failure::Lost(describe(&e)))
}
