//! The RDP engine: one session's connection, run on the tokio runtime, away
//! from the GTK main thread. The interface starts it with [`start`], talks
//! to it with [`Command`]s and hears back through [`Event`]s; nothing here
//! touches a widget.

mod audio;
pub(crate) mod clipboard;
mod compat;
mod connect;
mod devices;
mod failure;
mod frames;
pub(crate) mod h264;
mod input;
pub(crate) mod keymap;
mod rdstls;
mod route;
pub(crate) mod server;
mod session;
mod tap;

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

pub(crate) use clipboard::{ClipboardCommand, ClipboardEvent};
pub(crate) use failure::{Disconnect, Ending, Failure, Security, Tunnel, Unreachable};

/// Where to connect, and as whom.
#[derive(Clone)]
pub(crate) struct Target {
    /// Where the connection is going right now.
    pub(crate) host: String,
    pub(crate) port: u16,
    /// Everywhere the computer may be, in the order to try.
    pub(crate) addresses: Vec<(String, u16)>,
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) domain: String,
    /// The desktop size to ask for, in the screen's own pixels.
    pub(crate) width: u16,
    pub(crate) height: u16,
    /// The screen's scale, in percent: 200 on a 2× screen.
    pub(crate) scale: u32,
    pub(crate) quality: crate::connection::Quality,
    pub(crate) network: crate::connection::Network,
    /// Share the clipboard.
    pub(crate) clipboard: bool,
    /// Offer files copied here through the clipboard.
    pub(crate) clipboard_files: bool,
    /// Play the remote desktop's sound here; send the microphone there.
    pub(crate) sound: bool,
    pub(crate) microphone: bool,
    /// A folder to lend the remote desktop.
    pub(crate) folder: Option<std::path::PathBuf>,
    /// The way there, when it isn't straight to the host.
    pub(crate) via: Via,
    /// The MAC address to send a Wake-on-LAN packet to before connecting.
    pub(crate) wake: Option<[u8; 6]>,
    /// Connect again when the network drops.
    pub(crate) reconnect: bool,
    /// The keyboard layout to tell the server about, as a Windows ID.
    pub(crate) keyboard_layout: u32,
}

/// What stands between this computer and the server.
#[derive(Clone)]
pub(crate) enum Via {
    Direct,
    /// An SSH server, which forwards the connection the way `ssh -L` does.
    Ssh(Hop),
    /// An RD Gateway.
    Gateway(Hop),
}

/// An SSH server or an RD Gateway, and the account it takes.
#[derive(Clone)]
pub(crate) struct Hop {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) username: String,
    pub(crate) password: String,
}

pub(crate) enum Event {
    /// The server's certificate, checked before any credentials go out; or
    /// the key of the SSH server, or the gateway's certificate.
    Certificate {
        host: String,
        port: u16,
        fingerprint: String,
        of: Identity,
        /// The server's other addresses, under which the user may already
        /// have trusted it, written the way `host` is.
        also: Vec<(String, u16)>,
        reply: oneshot::Sender<Trust>,
    },
    /// The address now being tried, when there are several.
    Trying(String),
    /// The computer doesn't answer yet, after a Wake-on-LAN packet.
    Waking,
    /// The connection dropped, and is being made again.
    Reconnecting,
    /// The session is up, or the server changed the desktop's size.
    Connected {
        width: u16,
        height: u16,
    },
    /// A new picture of the desktop. The next one comes after
    /// [`Command::FrameDone`].
    Frame(Frame),
    Cursor(Cursor),
    Clipboard(ClipboardEvent),
    /// The server can't spread its desktop over several monitors.
    OneMonitor,
    /// The server keeps the desktop's size: it has no Display Control.
    KeepsSize,
    /// Which server it is, guessed from its certificate.
    Server(server::Server),
    /// What the computer runs, found through an SSH tunnel that ends on it.
    System(server::System),
    /// The graphics channel opened, offering H.264 or not.
    Graphics {
        h264: bool,
    },
    /// The server was offered H.264 and sends its picture another way.
    NoH264,
    /// The last event of a session.
    Ended(Ending),
}

/// The whole desktop in BGRX, and the parts of it that changed since the
/// last frame.
pub(crate) struct Frame {
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) stride: usize,
    pub(crate) pixels: frames::Pixels,
    pub(crate) damage: Vec<Rect>,
    /// When the oldest change in it was decoded, to time its way to the
    /// screen.
    pub(crate) made: std::time::Instant,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Rect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: i32,
    pub(crate) height: i32,
}

/// Whose identity [`Event::Certificate`] asks about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Identity {
    Server,
    Ssh,
    Gateway,
}

/// The answer to [`Event::Certificate`].
pub(crate) enum Trust {
    /// Trusted before.
    Known,
    /// Trusted just now, by the user.
    Accepted,
    Rejected,
}

#[derive(Clone)]
pub(crate) enum Cursor {
    Default,
    Hidden,
    Bitmap(Arc<crate::remote::Pointer>),
}

pub(crate) enum Command {
    Input(Vec<crate::remote::input::Input>),
    /// Make the remote desktop this size, in the screen's own pixels, at
    /// this scale in percent.
    Resize {
        width: u32,
        height: u32,
        scale: u32,
    },
    /// Spread the remote desktop over these monitors, the first of them
    /// the primary one.
    Layout(Vec<Monitor>),
    /// The last frame is on screen; the next can come.
    FrameDone,
    Clipboard(ClipboardCommand),
    Close,
}

/// A monitor of a layout: where it is on the remote desktop, relative to
/// the primary monitor's top left corner, and its size, in its own pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Monitor {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Its scale, in percent.
    pub(crate) scale: u32,
}

/// A running session. It ends on [`Command::Close`], or once every copy of
/// its handle is gone.
#[derive(Clone)]
pub(crate) struct Handle {
    commands: mpsc::UnboundedSender<Command>,
}

impl Handle {
    pub(crate) fn send(&self, command: Command) {
        // A session that has ended has nobody to hear it.
        let _ = self.commands.send(command);
    }
}

/// The gateway's NTLM brings a second crypto provider into rustls, which
/// then can't choose; ring is the one the rest of the engine was built on.
/// Called before any TLS; only the first call does anything.
pub(crate) fn use_ring() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Connect to `target`, and keep the session going until it ends.
pub(crate) fn start(target: Target) -> (Handle, mpsc::Receiver<Event>) {
    use_ring();
    let (commands, command_rx) = mpsc::unbounded_channel();
    let (event_tx, events) = mpsc::channel(8);
    // A thread of its own per session, on a single-threaded runtime: the
    // NLA exchange holds state that can't move between threads, and one
    // busy session can't slow another.
    let spawned = std::thread::Builder::new()
        .name("rdp-session".into())
        .spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime.block_on(run(target, event_tx, command_rx)),
                Err(e) => {
                    let failure = Failure::Protocol(e.to_string());
                    let _ = event_tx.blocking_send(Event::Ended(Ending::Failed(failure)));
                }
            }
        });
    if let Err(e) = spawned {
        tracing::error!("starting a session thread: {e}");
    }
    (Handle { commands }, events)
}

/// How many redirections one connection follows before giving up.
const MAX_HOPS: usize = 4;

/// How long a dropped session keeps trying to come back.
const RECONNECT_FOR: std::time::Duration = std::time::Duration::from_secs(120);

async fn run(
    mut target: Target,
    events: mpsc::Sender<Event>,
    mut commands: mpsc::UnboundedReceiver<Command>,
) {
    let ending = match drive(&mut target, &events, &mut commands).await {
        Ok(ending) => ending,
        Err(failure) => Ending::Failed(failure),
    };
    let _ = events.send(Event::Ended(ending)).await;
}

async fn drive(
    target: &mut Target,
    events: &mpsc::Sender<Event>,
    commands: &mut mpsc::UnboundedReceiver<Command>,
) -> Result<Ending, Failure> {
    // Where a dropped session goes back to: the server first asked for,
    // not one it redirected to, whose credentials were for one use.
    let mut first = target.clone();
    let mut redirect = None;
    let mut clipboard = clipboard::Clipboard::new(target.clipboard, target.clipboard_files);
    let mut early = Vec::new();
    let mut audio = audio::Audio::new(target.sound, target.microphone);
    // Kept across reconnections, as the interface's own record of what is
    // held down is.
    let mut input = ironrdp_input::Database::new();
    let mut hops = 0;
    // Since when the session has been trying to come back, and how long it
    // waits before the next try.
    let mut lost: Option<(std::time::Instant, std::time::Duration)> = None;
    loop {
        let connected = tokio::select! {
            connected = connect::connect(target, redirect.take(), events, &mut clipboard, &audio) => connected,
            () = closed(commands, &mut early) => return Ok(Ending::Closed),
        };
        let (result, framed) = match connected {
            Ok(connected) => connected,
            Err(failure) => match &mut lost {
                Some((since, wait)) if failure.passing() && since.elapsed() < RECONNECT_FOR => {
                    tracing::info!("reconnecting failed; trying again in {wait:?}");
                    tokio::select! {
                        () = tokio::time::sleep(*wait) => {}
                        () = closed(commands, &mut early) => return Ok(Ending::Closed),
                    }
                    *wait = (*wait * 2).min(std::time::Duration::from_secs(10));
                    continue;
                }
                _ => return Err(failure),
            },
        };
        lost = None;
        for command in early.drain(..) {
            clipboard.early(command);
        }
        let mut size = (
            u32::from(target.width),
            u32::from(target.height),
            target.scale,
        );
        let outcome = session::run(
            framed,
            result,
            events,
            commands,
            &mut size,
            session::Kept {
                input: &mut input,
                clipboard: &mut clipboard,
                audio: &mut audio,
            },
        )
        .await;
        match outcome {
            Ok(session::Outcome::Ended(ending)) => return Ok(ending),
            // A server can hand the session on after it starts: GNOME's
            // Remote Login does, once someone logs in at its login screen.
            Ok(session::Outcome::Redirect(pdu)) => {
                hops += 1;
                if hops > MAX_HOPS {
                    return Err(Failure::Protocol("too many redirections".into()));
                }
                tracing::info!("the server redirected the session");
                // Only GNOME's Remote Login does, among Linux servers.
                if events
                    .send(Event::Server(server::Server::GnomeLogin))
                    .await
                    .is_err()
                {
                    return Ok(Ending::Closed);
                }
                connect::follow(target, &pdu)?;
                redirect = Some(pdu);
            }
            // The network went away: come back to the same desktop, at the
            // size it had, once it returns.
            Err(Failure::Lost(why)) if target.reconnect => {
                tracing::info!("the connection dropped ({why}); reconnecting");
                (first.width, first.height, first.scale) = (
                    u16::try_from(size.0).unwrap_or(first.width),
                    u16::try_from(size.1).unwrap_or(first.height),
                    size.2,
                );
                *target = first.clone();
                hops = 0;
                lost = Some((std::time::Instant::now(), std::time::Duration::from_secs(1)));
                if events.send(Event::Reconnecting).await.is_err() {
                    return Ok(Ending::Closed);
                }
            }
            Err(failure) => return Err(failure),
        }
    }
}

/// Resolves when the interface closes the session, ignoring input sent
/// before there is a session to take it. What was copied meanwhile is kept
/// for the session.
async fn closed(
    commands: &mut mpsc::UnboundedReceiver<Command>,
    early: &mut Vec<ClipboardCommand>,
) {
    while let Some(command) = commands.recv().await {
        match command {
            Command::Close => return,
            Command::Clipboard(command) => early.push(command),
            _ => {}
        }
    }
}

/// The physical size, in millimeters, of a screen with this many pixels at
/// this scale: the size at which 96 DPI times the scale comes out. X11
/// desktops behind xrdp take their DPI from it, as they ignore the scale.
fn physical_size(width: u32, height: u32, scale: u32) -> (u32, u32) {
    let millimeters =
        |pixels: u32| (u64::from(pixels) * 2540 / (96 * u64::from(scale.max(100)))) as u32;
    (millimeters(width), millimeters(height))
}

/// The kind of I/O error at the root of an error, if there is one.
fn io_kind(error: &(dyn std::error::Error + 'static)) -> Option<std::io::ErrorKind> {
    io_error(error).map(std::io::Error::kind)
}

/// The I/O error at the root of an error, if there is one.
fn io_error<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a std::io::Error> {
    let mut current = Some(error);
    while let Some(e) = current {
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            return Some(io);
        }
        current = e.source();
    }
    None
}

/// An error and its causes, on one line.
fn describe(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_size_carries_the_scale() {
        // 96 DPI at 100%: 960 pixels are 254 mm; at 200% the same pixels
        // are half that.
        assert_eq!(physical_size(960, 960, 100), (254, 254));
        assert_eq!(physical_size(960, 960, 200), (127, 127));
    }

    /// The frame as a PPM picture, for `MOTE_TEST_PICTURE=<file.ppm>`.
    fn save_ppm(path: &str, frame: &Frame) {
        let mut out = format!("P6\n{} {}\n255\n", frame.width, frame.height).into_bytes();
        for row in frame.pixels.as_ref().chunks(frame.stride) {
            for bgrx in row[..usize::from(frame.width) * 4].chunks(4) {
                out.extend_from_slice(&[bgrx[2], bgrx[1], bgrx[0]]);
            }
        }
        std::fs::write(path, out).expect("writes the picture");
    }

    /// `MOTE_TEST_CLIPBOARD=<text>`: offer the server that text, with HTML
    /// and a picture beside it, or the files in `MOTE_TEST_FILES` (paths
    /// split by `:`), and fetch whatever the remote desktop copies, files
    /// into `MOTE_TEST_FETCH` (a directory).
    mod clipboard_test {
        use super::super::clipboard::{Content, Image, Kind};
        use super::super::{ClipboardCommand, ClipboardEvent, Command, Handle};

        pub(super) fn offer(handle: &Handle) {
            if std::env::var_os("MOTE_TEST_CLIPBOARD").is_none() {
                return;
            }
            let files: Vec<std::path::PathBuf> = std::env::var("MOTE_TEST_FILES")
                .map(|f| f.split(':').map(Into::into).collect())
                .unwrap_or_default();
            let kinds = if files.is_empty() {
                vec![Kind::Text, Kind::Html, Kind::Image]
            } else {
                vec![Kind::Files]
            };
            handle.send(Command::Clipboard(ClipboardCommand::Copied {
                kinds,
                files,
            }));
        }

        pub(super) fn event(handle: &Handle, event: ClipboardEvent) {
            match event {
                ClipboardEvent::Copied(kinds) => {
                    eprintln!("remote copied {kinds:?}");
                    for kind in kinds {
                        if kind == Kind::Files {
                            let dir = std::env::var("MOTE_TEST_FETCH")
                                .unwrap_or("/tmp/mote-fetch".into());
                            let (reply, answer) = tokio::sync::oneshot::channel();
                            handle.send(Command::Clipboard(ClipboardCommand::PasteFiles {
                                dir: dir.into(),
                                reply,
                            }));
                            std::thread::spawn(move || {
                                eprintln!("pasted files {:?}", answer.blocking_recv());
                            });
                            continue;
                        }
                        let (reply, answer) = tokio::sync::oneshot::channel();
                        handle.send(Command::Clipboard(ClipboardCommand::Paste { kind, reply }));
                        std::thread::spawn(move || match answer.blocking_recv() {
                            Ok(Some(Content::Text(text))) => eprintln!("pasted text {text:?}"),
                            Ok(Some(Content::Html(html))) => eprintln!("pasted html {html:?}"),
                            Ok(Some(Content::Image(image))) => {
                                eprintln!("pasted image {}x{}", image.width, image.height);
                            }
                            Ok(Some(Content::Png(png))) => {
                                eprintln!("pasted png, {} bytes", png.len())
                            }
                            _ => eprintln!("pasted nothing as {kind:?}"),
                        });
                    }
                }
                ClipboardEvent::Wanted { kind, png, reply } => {
                    eprintln!("server wants {kind:?}{}", if png { " as PNG" } else { "" });
                    let text = std::env::var("MOTE_TEST_CLIPBOARD").unwrap_or_default();
                    let _ = reply.send(match kind {
                        Kind::Text => Some(Content::Text(text)),
                        Kind::Html => Some(Content::Html(format!("<b>{text}</b>"))),
                        // One red pixel.
                        Kind::Image if png => Some(Content::Png(
                            b"\x89PNG\x0d\x0a\x1a\x0a\x00\x00\x00\x0dIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\x0dIDATx\x9cc\xf8\xcf\xc0\xf0\x1f\x00\x05\x00\x01\xff\x89\x99=\x1d\x00\x00\x00\x00IEND\xaeB`\x82".to_vec(),
                        )),
                        // MOTE_TEST_IMAGE=<width>x<height>: a larger picture.
                        Kind::Image => Some(Content::Image(
                            std::env::var("MOTE_TEST_IMAGE")
                                .ok()
                                .and_then(|s| {
                                    let (w, h) = s.split_once('x')?;
                                    let (width, height): (u32, u32) = (w.parse().ok()?, h.parse().ok()?);
                                    let rgba = (0..width * height)
                                        .flat_map(|i| [(i % 251) as u8, (i % 241) as u8, (i % 239) as u8, 255])
                                        .collect();
                                    Some(Image { width, height, rgba })
                                })
                                .unwrap_or(Image {
                                    width: 2,
                                    height: 1,
                                    rgba: vec![255, 0, 0, 255, 0, 0, 255, 255],
                                }),
                        )),
                        Kind::Files => None,
                    });
                }
            }
        }
    }

    /// `MOTE_TEST_SSH=<user>@<host>[:port]` (with `MOTE_TEST_SSH_PASSWORD`
    /// if the agent's keys aren't enough) goes through that SSH server;
    /// `MOTE_TEST_GATEWAY=<host>:<port>` through that RD Gateway, as
    /// `MOTE_TEST_GATEWAY_USER` with `MOTE_TEST_GATEWAY_PASSWORD`.
    fn test_via() -> Via {
        let var = |name: &str| std::env::var(name).unwrap_or_default();
        if let Ok(ssh) = std::env::var("MOTE_TEST_SSH") {
            let (username, server) = ssh.split_once('@').expect("<user>@<host>");
            let (host, port) = crate::connection::split_address(server).expect("an address");
            return Via::Ssh(Hop {
                host,
                port: port.unwrap_or(22),
                username: username.into(),
                password: var("MOTE_TEST_SSH_PASSWORD"),
            });
        }
        if let Ok(gateway) = std::env::var("MOTE_TEST_GATEWAY") {
            let (host, port) = crate::connection::split_address(&gateway).expect("an address");
            return Via::Gateway(Hop {
                host,
                port: port.unwrap_or(443),
                username: var("MOTE_TEST_GATEWAY_USER"),
                password: var("MOTE_TEST_GATEWAY_PASSWORD"),
            });
        }
        Via::Direct
    }

    /// Connects to the server in `MOTE_TEST_SERVER` (`host:port`) as
    /// `MOTE_TEST_USER` with `MOTE_TEST_PASSWORD` (both `mote` by default,
    /// as `temp/grd/run.sh` sets them up), and expects the session to end
    /// as `MOTE_TEST_EXPECT` says: `closed` (by default) after the first
    /// frame, or `refused`. `MOTE_LOG` sets the log filter.
    /// `MOTE_TEST_SERVER=localhost:3399 cargo test -- --ignored live`.
    #[test]
    #[ignore]
    fn live() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_env("MOTE_LOG")
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("mote=info")),
            )
            .with_test_writer()
            .try_init();
        let var = |name: &str, default: &str| std::env::var(name).unwrap_or(default.into());
        let server = std::env::var("MOTE_TEST_SERVER").expect("MOTE_TEST_SERVER");
        let (host, port) = server.rsplit_once(':').expect("host:port");
        // MOTE_TEST_SIZE=<width>x<height>: the desktop to ask for.
        let size = var("MOTE_TEST_SIZE", "1024x768");
        let (width, height) = size.split_once('x').expect("<width>x<height>");
        let target = Target {
            host: host.into(),
            port: port.parse().expect("port"),
            addresses: vec![(host.into(), port.parse().expect("port"))],
            username: var("MOTE_TEST_USER", "mote"),
            password: var("MOTE_TEST_PASSWORD", "mote"),
            domain: String::new(),
            width: width.parse().expect("width"),
            height: height.parse().expect("height"),
            // MOTE_TEST_SCALE: the scale to ask for, in percent.
            scale: var("MOTE_TEST_SCALE", "100").parse().expect("scale"),
            // MOTE_TEST_QUALITY: best, avc420 or remotefx.
            quality: match var("MOTE_TEST_QUALITY", "best").as_str() {
                "avc420" => crate::connection::Quality::Avc420,
                "remotefx" => crate::connection::Quality::RemoteFx,
                _ => crate::connection::Quality::Best,
            },
            // MOTE_TEST_NETWORK: lan, wan, broadband-high, satellite,
            // broadband-low or modem.
            network: {
                use crate::connection::Network;
                match var("MOTE_TEST_NETWORK", "lan").as_str() {
                    "wan" => Network::Wan,
                    "broadband-high" => Network::BroadbandHigh,
                    "satellite" => Network::Satellite,
                    "broadband-low" => Network::BroadbandLow,
                    "modem" => Network::Modem,
                    _ => Network::Lan,
                }
            },
            clipboard: true,
            clipboard_files: true,
            // MOTE_TEST_SOUND=1 plays the server's sound; MOTE_TEST_MICROPHONE=1
            // sends the microphone.
            sound: std::env::var_os("MOTE_TEST_SOUND").is_some(),
            microphone: std::env::var_os("MOTE_TEST_MICROPHONE").is_some(),
            // MOTE_TEST_FOLDER=<dir>: lend that folder.
            folder: std::env::var_os("MOTE_TEST_FOLDER").map(Into::into),
            via: test_via(),
            // MOTE_TEST_WAKE=<mac>: wake it first.
            wake: std::env::var("MOTE_TEST_WAKE")
                .ok()
                .and_then(|mac| crate::connection::parse_mac(&mac)),
            reconnect: true,
            // MOTE_TEST_LAYOUT_ID=<hex>: the keyboard layout to send.
            keyboard_layout: std::env::var("MOTE_TEST_LAYOUT_ID")
                .ok()
                .and_then(|id| u32::from_str_radix(id.trim_start_matches("0x"), 16).ok())
                .unwrap_or(0x409),
        };
        // MOTE_TEST_DOWNLOAD: fetch Cisco's OpenH264 first.
        if std::env::var_os("MOTE_TEST_DOWNLOAD").is_some() {
            h264::cisco::download().expect("downloads Cisco's OpenH264");
            eprintln!("downloaded to {}", h264::cisco::path().display());
        }
        let (handle, mut events) = start(target);
        // MOTE_TEST_SECONDS: stay connected that long, for a desktop that
        // takes a while to draw itself.
        let stay = std::env::var("MOTE_TEST_SECONDS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok());
        if let Some(seconds) = stay {
            let closer = handle.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(seconds));
                closer.send(Command::Close);
            });
        }
        let mut frames = 0;
        let mut last_size = (0, 0);
        while let Some(event) = events.blocking_recv() {
            match event {
                Event::Certificate {
                    fingerprint,
                    of,
                    reply,
                    ..
                } => {
                    eprintln!("certificate {of:?} {fingerprint}");
                    let _ = reply.send(Trust::Accepted);
                }
                Event::Waking => eprintln!("waking"),
                Event::OneMonitor => eprintln!("one monitor only"),
                Event::KeepsSize => eprintln!("keeps its size"),
                Event::Server(server) => eprintln!("server {server:?}"),
                Event::System(system) => eprintln!("system {system:?}"),
                Event::Graphics { h264 } => eprintln!("graphics, H.264 offered: {h264}"),
                Event::NoH264 => eprintln!("no H.264"),
                Event::Reconnecting => eprintln!("reconnecting"),
                Event::Connected { width, height } => {
                    eprintln!("connected {width}x{height}");
                    clipboard_test::offer(&handle);
                }
                Event::Clipboard(event) => clipboard_test::event(&handle, event),
                Event::Frame(frame) => {
                    frames += 1;
                    if let Ok(path) = std::env::var("MOTE_TEST_PICTURE") {
                        save_ppm(&path, &frame);
                    }
                    if frames == 1 {
                        eprintln!("frame 1 damage {:?}", frame.damage.first());
                        // MOTE_TEST_RESIZE=<width>x<height>@<scale>: ask for
                        // that size after the first frame.
                        if let Some((width, height, scale)) =
                            std::env::var("MOTE_TEST_RESIZE").ok().and_then(|s| {
                                let (size, scale) = s.split_once('@')?;
                                let (w, h) = size.split_once('x')?;
                                Some((w.parse().ok()?, h.parse().ok()?, scale.parse().ok()?))
                            })
                        {
                            handle.send(Command::Resize {
                                width,
                                height,
                                scale,
                            });
                        }
                        // MOTE_TEST_LAYOUT=<w>x<h>,<w>x<h>…: monitors side
                        // by side, the first the primary.
                        if let Ok(layout) = std::env::var("MOTE_TEST_LAYOUT") {
                            let mut x = 0;
                            let monitors = layout
                                .split(',')
                                .map(|size| {
                                    let (w, h) = size.split_once('x').expect("<w>x<h>");
                                    let (width, height) = (w.parse().unwrap(), h.parse().unwrap());
                                    let monitor = Monitor {
                                        x,
                                        y: 0,
                                        width,
                                        height,
                                        scale: 100,
                                    };
                                    x += width as i32;
                                    monitor
                                })
                                .collect();
                            handle.send(Command::Layout(monitors));
                        }
                    }
                    if frame.width != last_size.0 || frame.height != last_size.1 {
                        eprintln!("frame size {}x{}", frame.width, frame.height);
                        last_size = (frame.width, frame.height);
                    }
                    if stay.is_some() {
                        handle.send(Command::FrameDone);
                    } else {
                        // An idle desktop sends nothing more.
                        handle.send(Command::Close);
                    }
                }
                Event::Cursor(_) => eprintln!("cursor"),
                Event::Trying(address) => eprintln!("trying {address}"),
                Event::Ended(ending) => {
                    let text = match ending {
                        Ending::Closed => "closed".to_owned(),
                        Ending::ServerClosed(Some(Disconnect::Graphics)) => "needs h264".to_owned(),
                        Ending::ServerClosed(_) => "server closed".to_owned(),
                        Ending::Failed(Failure::Refused) => "refused".to_owned(),
                        Ending::Failed(Failure::Untrusted) => "untrusted".to_owned(),
                        Ending::Failed(Failure::Unreachable(why)) => match why {
                            Unreachable::NotFound => "not found",
                            Unreachable::Refused => "connection refused",
                            Unreachable::NoAnswer => "no answer",
                            Unreachable::Closed => "closed at once",
                            Unreachable::NotRdp => "not rdp",
                            Unreachable::Other(_) => "unreachable",
                        }
                        .to_owned(),
                        Ending::Failed(Failure::Lost(e) | Failure::Protocol(e)) => {
                            format!("failed: {e}")
                        }
                        Ending::Failed(Failure::NotAllowed) => "not allowed".to_owned(),
                        Ending::Failed(Failure::Security(security)) => match security {
                            Security::NoCertificate => "no certificate",
                            Security::LegacyOnly => "legacy only",
                            Security::OldTls => "old tls",
                            Security::Other(_) => "security",
                        }
                        .to_owned(),
                        Ending::Failed(Failure::Rejected) => "rejected".to_owned(),
                        Ending::Failed(Failure::Addresses(_)) => "no address led there".to_owned(),
                        Ending::Failed(Failure::Ssh(why) | Failure::Gateway(why)) => match why {
                            Tunnel::Unreachable(_) => "hop unreachable".to_owned(),
                            Tunnel::Refused => "hop refused".to_owned(),
                            Tunnel::KeyChanged => "key changed".to_owned(),
                            Tunnel::Denied => "hop denied".to_owned(),
                            Tunnel::NoRoute(e) => format!("no route: {e}"),
                            Tunnel::Other(e) => format!("hop failed: {e}"),
                        },
                    };
                    eprintln!("ended: {text}");
                    let expected = var("MOTE_TEST_EXPECT", "closed");
                    assert_eq!(text, expected);
                    if expected != "closed" {
                        return;
                    }
                    break;
                }
            }
        }
        eprintln!("frames {frames}");
        assert!(frames >= 1);
    }
}
