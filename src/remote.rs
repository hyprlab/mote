//! What the interface knows of a session, whatever its protocol: where it
//! goes, the input it sends and the events it hears back. The engines in
//! `rdp` (and, later, `vnc`) run each session on a thread of its own and
//! translate between these and their wire formats; the way to the server,
//! the frame buffers and the reasons a session ends are theirs to share.

pub(crate) mod clipboard;
pub(crate) mod failure;
pub(crate) mod frames;
pub(crate) mod input;
pub(crate) mod route;

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

pub(crate) use clipboard::{ClipboardCommand, ClipboardEvent};
pub(crate) use failure::{Disconnect, Ending, Failure, Security, Tunnel, Unreachable};

/// Connect to `target`, and keep the session going until it ends.
pub(crate) fn start(target: Target) -> (Handle, mpsc::Receiver<Event>) {
    match target.protocol {
        crate::connection::Protocol::Rdp => crate::rdp::start(target),
        crate::connection::Protocol::Vnc => crate::vnc::start(target),
    }
}

/// A pointer shape from the server.
pub(crate) struct Pointer {
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) hotspot_x: u16,
    pub(crate) hotspot_y: u16,
    /// RGBA, not premultiplied, `width * 4` bytes to a row.
    pub(crate) rgba: Vec<u8>,
}

/// Where to connect, and as whom.
#[derive(Clone)]
pub(crate) struct Target {
    pub(crate) protocol: crate::connection::Protocol,
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
    /// The server rang its bell.
    Bell,
    Clipboard(ClipboardEvent),
    /// The server can't spread its desktop over several monitors.
    OneMonitor,
    /// The server keeps the desktop's size: it has no Display Control.
    KeepsSize,
    /// Which server it is, guessed from its certificate.
    Server(crate::rdp::server::Server),
    /// What the computer runs, found through an SSH tunnel that ends on it.
    System(crate::rdp::server::System),
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
    Bitmap(Arc<Pointer>),
}

pub(crate) enum Command {
    Input(Vec<input::Input>),
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
    pub(crate) fn new(commands: mpsc::UnboundedSender<Command>) -> Self {
        Self { commands }
    }

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

/// The kind of I/O error at the root of an error, if there is one.
pub(crate) fn io_kind(error: &(dyn std::error::Error + 'static)) -> Option<std::io::ErrorKind> {
    io_error(error).map(std::io::Error::kind)
}

/// The I/O error at the root of an error, if there is one.
pub(crate) fn io_error<'a>(
    error: &'a (dyn std::error::Error + 'static),
) -> Option<&'a std::io::Error> {
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
pub(crate) fn describe(error: &(dyn std::error::Error + 'static)) -> String {
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
