//! The shared clipboard as the interface sees it: what was copied on either
//! side, fetched only when something is pasted.

use std::path::PathBuf;

use tokio::sync::oneshot;

/// A picture: straight (not premultiplied) RGBA, rows top first, no
/// padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Image {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

/// What can be on a clipboard, as far as both sides understand each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Text,
    Html,
    Image,
    Files,
}

#[derive(Debug)]
pub(crate) enum Content {
    Text(String),
    Html(String),
    Image(Image),
    /// A picture the remote side had as PNG already.
    Png(Vec<u8>),
}

pub(crate) enum ClipboardEvent {
    /// Something was copied on the remote desktop.
    Copied(Vec<Kind>),
    /// Something is being pasted on the remote desktop: what the local
    /// clipboard holds, as `kind`. A picture is wanted as
    /// [`Content::Png`] when `png` is set, else as [`Content::Image`].
    Wanted {
        kind: Kind,
        png: bool,
        reply: oneshot::Sender<Option<Content>>,
    },
}

pub(crate) enum ClipboardCommand {
    /// Something was copied here; `files` are the paths when it is files.
    Copied {
        kinds: Vec<Kind>,
        files: Vec<PathBuf>,
    },
    /// Fetch what the remote desktop copied, as `kind`.
    Paste {
        kind: Kind,
        reply: oneshot::Sender<Option<Content>>,
    },
    /// Fetch the remote desktop's files into `dir`; the reply has what is at
    /// its top.
    PasteFiles {
        dir: PathBuf,
        reply: oneshot::Sender<Option<Vec<PathBuf>>>,
    },
}
