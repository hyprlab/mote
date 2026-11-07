//! The clipboard over VNC. The original cut text carries Latin-1 text and
//! goes out as soon as something is copied; the extended clipboard, which
//! TigerVNC and wayvnc have, carries UTF-8 text and HTML and, like RDP's,
//! announces what was copied and sends it only when something pastes.
//! Each side's copies reach the interface as RDP's do.

use std::time::{Duration, Instant};

use flate2::{Compress, Compression, Decompress, FlushCompress};
use tokio::sync::{mpsc, oneshot};

use super::decode::inflate;
use crate::rdp::clipboard::formats::{cf_html_from_html, html_from_cf_html};
use crate::remote::Event;
use crate::remote::clipboard::{ClipboardCommand, ClipboardEvent, Content, Kind};

/// The extended clipboard's formats this side takes, as flag bits.
const TEXT: u32 = 1;
const HTML: u32 = 1 << 2;
const FORMATS: u32 = 0xFFFF;
/// Its actions.
const CAPS: u32 = 1 << 24;
const REQUEST: u32 = 1 << 25;
const PEEK: u32 = 1 << 26;
const NOTIFY: u32 = 1 << 27;
const PROVIDE: u32 = 1 << 28;

/// The most clipboard taken from the server at once.
const MOST: usize = 20 << 20;
/// How long a paste waits for the server.
const PASTE_WITHIN: Duration = Duration::from_secs(30);

fn bit(kind: Kind) -> u32 {
    match kind {
        Kind::Text => TEXT,
        Kind::Html => HTML,
        Kind::Image | Kind::Files => 0,
    }
}

fn kinds(flags: u32) -> Vec<Kind> {
    [(TEXT, Kind::Text), (HTML, Kind::Html)]
        .into_iter()
        .filter(|(bit, _)| flags & bit != 0)
        .map(|(_, kind)| kind)
        .collect()
}

/// What the interface read of the local clipboard, and why it was asked.
pub(super) struct Answer {
    kind: Kind,
    content: Option<Content>,
    /// For the original cut text, which goes out at once; otherwise for
    /// the server's request.
    original: bool,
}

pub(super) struct Clipboard {
    /// The flags the server's extended clipboard announced: the formats
    /// and actions it takes. None for a server with the original alone.
    server: Option<u32>,
    /// What the server copied, as far as it has sent it.
    held: Vec<(Kind, Content)>,
    /// The formats the server says it holds.
    offered: u32,
    /// Pastes here waiting for the server's data.
    waiting: Vec<(Kind, Instant, oneshot::Sender<Option<Content>>)>,
    /// What is copied here, as offered to the server.
    local: Vec<Kind>,
    /// The server has had its chance to say it has the extended
    /// clipboard: copies here wait until then, not to go out as the
    /// original cut text first.
    settled: bool,
    /// A request of the server's being gathered: the formats still to
    /// come, and those come.
    gathering: Option<(u32, Vec<(Kind, Content)>)>,
    answers_tx: mpsc::UnboundedSender<Answer>,
    pub(super) answers: mpsc::UnboundedReceiver<Answer>,
}

impl Clipboard {
    pub(super) fn new() -> Self {
        let (answers_tx, answers) = mpsc::unbounded_channel();
        Self {
            server: None,
            held: Vec::new(),
            offered: 0,
            waiting: Vec::new(),
            local: Vec::new(),
            settled: false,
            gathering: None,
            answers_tx,
            answers,
        }
    }

    /// The original cut text from the server, the whole of it: Latin-1 as
    /// RFB says, or UTF-8, as wayvnc and libvncserver's servers send. Text
    /// that reads as UTF-8 is taken as UTF-8: Latin-1 with letters beyond
    /// ASCII almost never does.
    pub(super) fn cut_text(&mut self, data: &[u8], events: &mut Vec<Event>) {
        let text = match std::str::from_utf8(data) {
            Ok(text) => text.to_owned(),
            Err(_) => data.iter().map(|&byte| char::from(byte)).collect(),
        };
        self.held = vec![(Kind::Text, Content::Text(text))];
        self.offered = TEXT;
        events.push(Event::Clipboard(ClipboardEvent::Copied(vec![Kind::Text])));
    }

    /// An extended clipboard message from the server.
    pub(super) fn extended(&mut self, data: &[u8], out: &mut Vec<u8>, events: &mut Vec<Event>) {
        let Some((flags, rest)) = data.split_first_chunk::<4>() else {
            return;
        };
        let flags = u32::from_be_bytes(*flags);
        if flags & CAPS != 0 {
            tracing::info!("the server's clipboard takes {:#x}", flags);
            self.server = Some(flags);
            // Every size 0: the server announces each copy, and sends it
            // only when asked.
            let mut caps = (CAPS | TEXT | HTML | REQUEST | PEEK | NOTIFY | PROVIDE)
                .to_be_bytes()
                .to_vec();
            caps.extend_from_slice(&[0; 8]);
            extended(out, &caps);
            // What was copied here before the server said so.
            if self.settled && !self.local.is_empty() {
                self.notify(out);
            }
        } else if flags & REQUEST != 0 {
            self.requested(flags & FORMATS, out, events);
        } else if flags & PEEK != 0 {
            self.notify(out);
        } else if flags & NOTIFY != 0 {
            self.offered = flags & (TEXT | HTML);
            self.held.clear();
            // Nothing to paste here, as when the server's clipboard went
            // empty: the clipboard here stays as it is.
            if self.offered != 0 {
                events.push(Event::Clipboard(ClipboardEvent::Copied(kinds(
                    self.offered,
                ))));
            }
        } else if flags & PROVIDE != 0 {
            self.provided(flags & FORMATS, rest, events);
        }
    }

    /// The server's data, in a zlib stream of its own: a length and the
    /// bytes, for each format in the flags, lowest bit first.
    fn provided(&mut self, formats: u32, compressed: &[u8], events: &mut Vec<Event>) {
        let data = match inflate(&mut Decompress::new(true), compressed, MOST) {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!("the server's clipboard: {e}");
                return;
            }
        };
        let mut rest = data.as_slice();
        let mut held = Vec::new();
        for format in (0..16).map(|n| 1 << n).filter(|bit| formats & bit != 0) {
            let Some((size, after)) = rest.split_first_chunk::<4>() else {
                break;
            };
            let size = u32::from_be_bytes(*size) as usize;
            if after.len() < size {
                break;
            }
            let (bytes, after) = after.split_at(size);
            rest = after;
            match format {
                TEXT => {
                    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                    let text = String::from_utf8_lossy(&bytes[..end]).replace("\r\n", "\n");
                    held.push((Kind::Text, Content::Text(text)));
                }
                HTML => {
                    if let Some(html) = html_from_cf_html(bytes) {
                        held.push((Kind::Html, Content::Html(html)));
                    }
                }
                _ => {}
            }
        }
        let unasked = self.waiting.is_empty();
        for (kind, content) in held {
            for (_, _, reply) in self.waiting.extract_if(.., |(k, _, _)| *k == kind) {
                let _ = reply.send(Some(copy(&content)));
            }
            self.held.retain(|(k, _)| *k != kind);
            self.held.push((kind, content));
        }
        if unasked {
            // A server that sends its copy whole, unannounced.
            self.offered = self
                .held
                .iter()
                .fold(0, |flags, (kind, _)| flags | bit(*kind));
            let held = self.held.iter().map(|(kind, _)| *kind).collect();
            events.push(Event::Clipboard(ClipboardEvent::Copied(held)));
        }
    }

    /// The server asks for what is copied here: the interface reads it,
    /// and it goes back together once all of it has come.
    fn requested(&mut self, formats: u32, out: &mut Vec<u8>, events: &mut Vec<Event>) {
        let wanted = formats & self.local.iter().fold(0, |flags, kind| flags | bit(*kind));
        if wanted == 0 {
            provide(out, &[]);
            return;
        }
        self.gathering = Some((wanted, Vec::new()));
        for kind in kinds(wanted) {
            self.ask(kind, false, events);
        }
    }

    /// Ask the interface for the local clipboard as `kind`.
    fn ask(&self, kind: Kind, original: bool, events: &mut Vec<Event>) {
        let (reply, answer) = oneshot::channel();
        let answers = self.answers_tx.clone();
        tokio::spawn(async move {
            let content = answer.await.ok().flatten();
            let _ = answers.send(Answer {
                kind,
                content,
                original,
            });
        });
        events.push(Event::Clipboard(ClipboardEvent::Wanted {
            kind,
            png: false,
            reply,
        }));
    }

    /// The interface read the local clipboard.
    pub(super) fn answered(&mut self, answer: Answer, out: &mut Vec<u8>) {
        if answer.original {
            if let Some(Content::Text(text)) = answer.content {
                // Latin-1: what it can't hold becomes a question mark.
                let bytes: Vec<u8> = text
                    .chars()
                    .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
                    .collect();
                out.extend_from_slice(&[6, 0, 0, 0]);
                out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
                out.extend(bytes);
            }
            return;
        }
        let Some((left, gathered)) = &mut self.gathering else {
            return;
        };
        *left &= !bit(answer.kind);
        if let Some(content) = answer.content {
            gathered.push((answer.kind, content));
        }
        if *left == 0 {
            let gathered = std::mem::take(gathered);
            self.gathering = None;
            provide(out, &gathered);
        }
    }

    /// What the interface asks of the server's clipboard, or tells it of
    /// this one's.
    pub(super) fn command(
        &mut self,
        command: ClipboardCommand,
        out: &mut Vec<u8>,
        events: &mut Vec<Event>,
    ) {
        match command {
            ClipboardCommand::Copied { kinds, .. } => {
                self.local = kinds.into_iter().filter(|kind| bit(*kind) != 0).collect();
                if !self.settled {
                    return;
                }
                match self.server {
                    Some(flags) if flags & NOTIFY != 0 => self.notify(out),
                    // The original cut text goes out whole, at once.
                    _ if self.local.contains(&Kind::Text) => self.ask(Kind::Text, true, events),
                    _ => {}
                }
            }
            ClipboardCommand::Paste { kind, reply } => {
                if let Some((_, content)) = self.held.iter().find(|(k, _)| *k == kind) {
                    let _ = reply.send(Some(copy(content)));
                } else if self.server.is_some() && self.offered & bit(kind) != 0 {
                    let flags = REQUEST | bit(kind);
                    extended(out, &flags.to_be_bytes());
                    self.waiting.push((kind, Instant::now(), reply));
                } else {
                    let _ = reply.send(None);
                }
            }
            ClipboardCommand::PasteFiles { reply, .. } => {
                let _ = reply.send(None);
            }
        }
    }

    /// The server's first update has come, and with it, where it has one,
    /// its extended clipboard: what was copied here meanwhile goes out.
    pub(super) fn settle(&mut self, out: &mut Vec<u8>, events: &mut Vec<Event>) {
        if self.settled {
            return;
        }
        self.settled = true;
        match self.server {
            Some(flags) if flags & NOTIFY != 0 => self.notify(out),
            _ if self.local.contains(&Kind::Text) => self.ask(Kind::Text, true, events),
            _ => {}
        }
    }

    /// Tell the server what is copied here.
    fn notify(&self, out: &mut Vec<u8>) {
        let flags = self
            .local
            .iter()
            .fold(NOTIFY, |flags, kind| flags | bit(*kind));
        extended(out, &flags.to_be_bytes());
    }

    /// Give up on pastes the server hasn't answered.
    pub(super) fn tick(&mut self) {
        for (_, _, reply) in self
            .waiting
            .extract_if(.., |(_, at, _)| at.elapsed() > PASTE_WITHIN)
        {
            let _ = reply.send(None);
        }
    }
}

/// An extended ClientCutText: a negative length, then the message.
fn extended(out: &mut Vec<u8>, message: &[u8]) {
    out.extend_from_slice(&[6, 0, 0, 0]);
    out.extend_from_slice(&(-(message.len() as i32)).to_be_bytes());
    out.extend_from_slice(message);
}

/// A provide message with the data gathered: text as UTF-8 with Windows'
/// line endings and a NUL, HTML as Windows' HTML Format.
fn provide(out: &mut Vec<u8>, gathered: &[(Kind, Content)]) {
    let mut flags = PROVIDE;
    let mut plain = Vec::new();
    for want in [Kind::Text, Kind::Html] {
        let Some((_, content)) = gathered.iter().find(|(kind, _)| *kind == want) else {
            continue;
        };
        let bytes = match content {
            Content::Text(text) => {
                let mut bytes = text
                    .replace("\r\n", "\n")
                    .replace('\n', "\r\n")
                    .into_bytes();
                bytes.push(0);
                bytes
            }
            Content::Html(html) => cf_html_from_html(html),
            _ => continue,
        };
        flags |= bit(want);
        plain.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        plain.extend(bytes);
    }
    let mut compressed = Vec::with_capacity(plain.len() + 64);
    let mut stream = Compress::new(Compression::default(), true);
    loop {
        let before = compressed.len();
        if compressed.capacity() == before {
            compressed.reserve(4096);
        }
        let taken = stream.total_in() as usize;
        match stream.compress_vec(&plain[taken..], &mut compressed, FlushCompress::Finish) {
            Ok(flate2::Status::StreamEnd) => break,
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("compressing the clipboard: {e}");
                return;
            }
        }
    }
    let mut message = flags.to_be_bytes().to_vec();
    message.extend(compressed);
    extended(out, &message);
}

/// Content is passed on as owned values; a paste can ask twice.
fn copy(content: &Content) -> Content {
    match content {
        Content::Text(text) => Content::Text(text.clone()),
        Content::Html(html) => Content::Html(html.clone()),
        Content::Image(image) => Content::Image(image.clone()),
        Content::Png(png) => Content::Png(png.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The extended message inside a ClientCutText.
    fn message(out: &[u8]) -> &[u8] {
        assert_eq!(out[0], 6);
        let length = i32::from_be_bytes(out[4..8].try_into().unwrap());
        assert!(length < 0);
        &out[8..8 + length.unsigned_abs() as usize]
    }

    #[test]
    fn a_provide_carries_text_with_windows_line_endings_and_comes_back() {
        let mut out = Vec::new();
        provide(&mut out, &[(Kind::Text, Content::Text("a\nb".into()))]);
        let sent = message(&out);
        assert_eq!(
            u32::from_be_bytes(sent[..4].try_into().unwrap()),
            PROVIDE | TEXT
        );
        let mut ours = Clipboard::new();
        let mut events = Vec::new();
        ours.provided(TEXT, &sent[4..], &mut events);
        assert!(matches!(
            ours.held.first(),
            Some((Kind::Text, Content::Text(text))) if text == "a\nb"
        ));
    }

    #[test]
    fn original_cut_text_is_latin_1() {
        let mut clipboard = Clipboard::new();
        let mut events = Vec::new();
        clipboard.cut_text(&[b'c', 0xE9], &mut events);
        assert!(matches!(
            clipboard.held.first(),
            Some((Kind::Text, Content::Text(text))) if text == "cé"
        ));
        // UTF-8 where it reads as such.
        clipboard.cut_text("cü".as_bytes(), &mut events);
        assert!(matches!(
            clipboard.held.first(),
            Some((Kind::Text, Content::Text(text))) if text == "cü"
        ));
        let mut out = Vec::new();
        clipboard.answered(
            Answer {
                kind: Kind::Text,
                content: Some(Content::Text("é€".into())),
                original: true,
            },
            &mut out,
        );
        assert_eq!(out, [6, 0, 0, 0, 0, 0, 0, 2, 0xE9, b'?']);
    }
}
