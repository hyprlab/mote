//! The shared clipboard (MS-RDPECLIP), on the session's side. IronRDP's
//! channel speaks the protocol; this keeps what each side has copied, turns
//! the formats Windows defined into text, HTML and pictures and back, and
//! moves files. The interface hears what was copied on the remote desktop
//! and answers for the local clipboard; everything is asked for only when
//! something is pasted.

mod files;
pub(crate) mod formats;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use ironrdp_cliprdr::backend::CliprdrBackend;
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardFormatName, ClipboardGeneralCapabilityFlags,
    FileContentsRequest, FileContentsResponse, FileDescriptor, FormatDataRequest,
    FormatDataResponse, LockDataId, OwnedFormatDataResponse,
};
use ironrdp_cliprdr::{CliprdrClient, CliprdrSvcMessages};
use ironrdp_core::IntoOwned as _;
use ironrdp_session::{ActiveStage, ActiveStageOutput};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

pub(crate) use crate::remote::clipboard::{ClipboardCommand, ClipboardEvent, Content, Kind};

use super::{Event, Failure, describe};

/// The ids Mote gives HTML and PNG in its format lists; the names are what
/// count. GNOME Remote Desktop hands a format named for a mime type to its
/// desktop's applications as it is.
const HTML_ID: u32 = 0xC0A0;
const PNG_ID: u32 = 0xC0A1;
/// How long the interface has to answer for the local clipboard.
const ANSWER_WITHIN: Duration = Duration::from_secs(10);
/// How soon after taking the local clipboard a server's own copy of it is
/// taken as an echo: KRdp copies what it took, and announces it back.
const ECHO_WITHIN: Duration = Duration::from_secs(1);

/// What IronRDP's channel calls back with, passed on to the session loop.
#[derive(Debug)]
pub(super) enum Note {
    /// The server is ready for the first format list.
    Asked,
    Ready,
    Copied(Vec<ClipboardFormat>),
    Wanted(ClipboardFormatId),
    Data(Option<Vec<u8>>),
    FileList(Vec<FileDescriptor>, Option<u32>),
    FileWanted(FileContentsRequest),
    FileData(FileContentsResponse<'static>),
}

#[derive(Debug)]
pub(super) struct Backend {
    notes: mpsc::UnboundedSender<Note>,
}

ironrdp_core::impl_as_any!(Backend);

impl Backend {
    fn note(&self, note: Note) {
        let _ = self.notes.send(note);
    }
}

impl CliprdrBackend for Backend {
    fn temporary_directory(&self) -> &str {
        // Where a Windows server would leave files for a client that reads
        // paths; the file list carries no paths, so it is never used.
        "/tmp"
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES
            | ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED
            | ClipboardGeneralCapabilityFlags::FILECLIP_NO_FILE_PATHS
            | ClipboardGeneralCapabilityFlags::CAN_LOCK_CLIPDATA
            | ClipboardGeneralCapabilityFlags::HUGE_FILE_SUPPORT_ENABLED
    }

    fn on_ready(&mut self) {
        self.note(Note::Ready);
    }

    fn on_request_format_list(&mut self) {
        self.note(Note::Asked);
    }

    fn on_process_negotiated_capabilities(
        &mut self,
        capabilities: ClipboardGeneralCapabilityFlags,
    ) {
        tracing::debug!("clipboard capabilities {capabilities:?}");
    }

    fn on_remote_copy(&mut self, formats: &[ClipboardFormat]) {
        self.note(Note::Copied(formats.to_vec()));
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        self.note(Note::Wanted(request.format));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        self.note(Note::Data(
            (!response.is_error()).then(|| response.data().to_vec()),
        ));
    }

    fn on_file_contents_request(&mut self, request: FileContentsRequest) {
        self.note(Note::FileWanted(request));
    }

    fn on_file_contents_response(&mut self, response: FileContentsResponse<'_>) {
        self.note(Note::FileData(response.into_owned()));
    }

    fn on_lock(&mut self, _: LockDataId) {}

    fn on_unlock(&mut self, _: LockDataId) {}

    fn on_remote_file_list(&mut self, files: &[FileDescriptor], lock: Option<u32>) {
        self.note(Note::FileList(files.to_vec(), lock));
    }
}

/// A paste to ask the server for. They are asked one at a time: GNOME
/// Remote Desktop fails a request that comes while it answers another.
enum Paste {
    Content {
        format: ClipboardFormatId,
        reply: oneshot::Sender<Option<Content>>,
    },
    Files {
        format: ClipboardFormatId,
        dir: PathBuf,
        reply: oneshot::Sender<Option<Vec<PathBuf>>>,
    },
}

impl Paste {
    fn format(&self) -> ClipboardFormatId {
        match self {
            Self::Content { format, .. } | Self::Files { format, .. } => *format,
        }
    }
}

/// The server's request for the local clipboard, waiting on the interface.
/// They are answered in the order asked, as the protocol pairs them.
pub(super) struct Answer {
    format: ClipboardFormatId,
    reply: oneshot::Receiver<Option<Content>>,
    by: Instant,
}

/// The clipboard's state across a session's connections.
pub(super) struct Clipboard {
    on: bool,
    /// Files copied here are offered to the server. Off, their paths and
    /// contents never leave this computer, whatever the interface sends.
    share_files: bool,
    pub(super) notes: mpsc::UnboundedReceiver<Note>,
    /// Kept so that `notes` waits, rather than ends, between connections.
    _sender: mpsc::UnboundedSender<Note>,
    /// What the local clipboard holds.
    local: Vec<Kind>,
    files: Vec<PathBuf>,
    /// The server asked for the first format list; the channel is ready.
    asked: bool,
    ready: bool,
    /// The local clipboard changed since the server last heard.
    stale: bool,
    /// What the remote clipboard holds.
    remote: Vec<ClipboardFormat>,
    pastes: VecDeque<Paste>,
    /// The first of `pastes` has been asked for.
    asking: bool,
    pub(super) answers: VecDeque<Answer>,
    /// When the server last took the local clipboard.
    served: Option<Instant>,
    offered: Vec<files::Offered>,
    fetch: Option<(files::Fetch, oneshot::Sender<Option<Vec<PathBuf>>>)>,
    streams: u32,
}

type Outputs = Result<Vec<ActiveStageOutput>, Failure>;

impl Clipboard {
    pub(super) fn new(on: bool, share_files: bool) -> Self {
        let (sender, notes) = mpsc::unbounded_channel();
        Self {
            on,
            share_files,
            notes,
            _sender: sender,
            local: Vec::new(),
            files: Vec::new(),
            asked: false,
            ready: false,
            stale: false,
            remote: Vec::new(),
            pastes: VecDeque::new(),
            asking: false,
            answers: VecDeque::new(),
            served: None,
            offered: Vec::new(),
            fetch: None,
            streams: 0,
        }
    }

    pub(super) fn on(&self) -> bool {
        self.on
    }

    /// The channel for a new connection, which starts over.
    pub(super) fn channel(&mut self) -> Option<CliprdrClient> {
        if !self.on {
            return None;
        }
        let (sender, notes) = mpsc::unbounded_channel();
        self.notes = notes;
        self._sender = sender.clone();
        self.asked = false;
        self.ready = false;
        self.stale = true;
        self.remote.clear();
        self.pastes.clear();
        self.asking = false;
        self.answers.clear();
        self.offered.clear();
        self.fetch = None;
        Some(CliprdrClient::new(Box::new(Backend { notes: sender })))
    }

    /// A command that came while there was no connection to take it.
    pub(super) fn early(&mut self, command: ClipboardCommand) {
        if let ClipboardCommand::Copied { kinds, files } = command {
            self.copied(kinds, files);
        }
    }

    /// What the local clipboard holds now. Files that aren't shared take
    /// the rest with them, since a copy of files reads as their paths.
    fn copied(&mut self, mut kinds: Vec<Kind>, mut files: Vec<PathBuf>) {
        if !self.share_files && (kinds.contains(&Kind::Files) || !files.is_empty()) {
            kinds.clear();
            files.clear();
        }
        self.local = kinds;
        self.files = files;
        self.stale = true;
    }

    pub(super) fn command(
        &mut self,
        command: ClipboardCommand,
        stage: &mut ActiveStage,
    ) -> Outputs {
        match command {
            ClipboardCommand::Copied { kinds, files } => {
                self.copied(kinds, files);
                self.advertise(stage)
            }
            ClipboardCommand::Paste { kind, reply } => {
                let Some(format) = self.ready.then(|| pick(&self.remote, kind)).flatten() else {
                    let _ = reply.send(None);
                    return Ok(Vec::new());
                };
                self.pastes.push_back(Paste::Content { format, reply });
                self.ask(stage)
            }
            ClipboardCommand::PasteFiles { dir, reply } => {
                let format = self
                    .ready
                    .then(|| pick(&self.remote, Kind::Files))
                    .flatten();
                let (Some(format), None) = (format, &self.fetch) else {
                    let _ = reply.send(None);
                    return Ok(Vec::new());
                };
                self.pastes.push_back(Paste::Files { format, dir, reply });
                self.ask(stage)
            }
        }
    }

    /// Ask for the next paste, unless one is being answered.
    fn ask(&mut self, stage: &mut ActiveStage) -> Outputs {
        let Some(format) = self
            .pastes
            .front()
            .map(Paste::format)
            .filter(|_| !self.asking)
        else {
            return Ok(Vec::new());
        };
        self.asking = true;
        let messages = channel(stage).map(|c| c.initiate_paste(format));
        send(stage, messages)
    }

    /// Tell the server what the local clipboard holds, once it can hear it.
    fn advertise(&mut self, stage: &mut ActiveStage) -> Outputs {
        if !self.ready || !self.stale {
            return Ok(Vec::new());
        }
        self.stale = false;
        if self.share_files && self.local.contains(&Kind::Files) && !self.files.is_empty() {
            let (list, offered) = files::describe(&self.files);
            self.offered = offered;
            tracing::debug!("offering {} files and folders", list.len());
            let messages = channel(stage).map(|c| c.initiate_file_copy(list));
            return send(stage, messages);
        }
        let formats = advertised(&self.local);
        let messages = channel(stage).map(|c| c.initiate_copy(&formats));
        send(stage, messages)
    }

    pub(super) async fn note(
        &mut self,
        note: Note,
        stage: &mut ActiveStage,
        events: &mpsc::Sender<Event>,
    ) -> Outputs {
        match note {
            Note::Asked => {
                // The first list goes with the channel's own start; files
                // can be offered only once it is ready.
                self.asked = true;
                let formats = advertised(&self.local);
                self.stale = self.local.contains(&Kind::Files);
                let messages = channel(stage).map(|c| c.initiate_copy(&formats));
                send(stage, messages)
            }
            Note::Ready => {
                tracing::debug!("the clipboard is ready");
                self.ready = true;
                self.advertise(stage)
            }
            Note::Copied(formats) => {
                tracing::debug!("the remote clipboard holds {}", names(&formats));
                let kinds: Vec<Kind> = [Kind::Text, Kind::Html, Kind::Image, Kind::Files]
                    .into_iter()
                    .filter(|kind| pick(&formats, *kind).is_some())
                    .collect();
                self.remote = formats;
                let echo = self.served.is_some_and(|at| at.elapsed() < ECHO_WITHIN)
                    && kinds.iter().all(|kind| self.local.contains(kind));
                if echo {
                    tracing::debug!("taking that as the server's echo of the local clipboard");
                }
                // A server can hold nothing Mote reads; the local clipboard
                // stays as it was then.
                if !kinds.is_empty() && !echo {
                    let _ = events
                        .send(Event::Clipboard(ClipboardEvent::Copied(kinds)))
                        .await;
                }
                Ok(Vec::new())
            }
            Note::Wanted(format) => {
                tracing::debug!("the server asks for format {:#x}", format.value());
                let (reply, receiver) = oneshot::channel();
                self.answers.push_back(Answer {
                    format,
                    reply: receiver,
                    by: Instant::now() + ANSWER_WITHIN,
                });
                match wanted(format).filter(|kind| self.local.contains(kind)) {
                    Some(kind) => {
                        let png = format.value() == PNG_ID;
                        let _ = events
                            .send(Event::Clipboard(ClipboardEvent::Wanted {
                                kind,
                                png,
                                reply,
                            }))
                            .await;
                    }
                    None => {
                        tracing::debug!("nothing here as format {:#x}", format.value());
                        let _ = reply.send(None);
                    }
                }
                Ok(Vec::new())
            }
            Note::Data(data) => {
                self.asking = false;
                match self.pastes.pop_front() {
                    Some(Paste::Content { format, reply }) => {
                        let content = data.as_deref().and_then(|data| decode(format, data));
                        if content.is_none() {
                            let head = data.as_deref().map(|d| &d[..d.len().min(16)]);
                            tracing::debug!("nothing read as {:#x}: {head:02x?}", format.value());
                        }
                        let _ = reply.send(content);
                    }
                    Some(Paste::Files { reply, .. }) => {
                        let _ = reply.send(None);
                    }
                    None => {}
                }
                self.ask(stage)
            }
            Note::FileList(list, lock) => {
                self.asking = false;
                let Some(Paste::Files { dir, reply, .. }) = self.pastes.pop_front() else {
                    return self.ask(stage);
                };
                tracing::debug!("fetching {} files and folders", list.len());
                self.streams = self.streams.wrapping_add(1);
                let mut fetch = files::Fetch::new(dir, &list, lock, self.streams);
                let step = fetch.next();
                self.fetch = Some((fetch, reply));
                let mut outputs = self.fetched(step, stage)?;
                outputs.extend(self.ask(stage)?);
                Ok(outputs)
            }
            Note::FileData(response) => {
                let Some((fetch, _)) = self
                    .fetch
                    .as_mut()
                    .filter(|(f, _)| f.stream() == response.stream_id())
                else {
                    return Ok(Vec::new());
                };
                let step = fetch.received(&response);
                self.fetched(step, stage)
            }
            Note::FileWanted(request) => {
                tracing::debug!(
                    "the server asks for file {} {:?} at {}, {} bytes",
                    request.index,
                    request.flags,
                    request.position,
                    request.requested_size
                );
                let response = files::serve(&self.offered, &request);
                let messages = channel(stage).map(|c| c.submit_file_contents(response));
                send(stage, messages)
            }
        }
    }

    fn fetched(&mut self, step: files::Step, stage: &mut ActiveStage) -> Outputs {
        match step {
            files::Step::Request(request) => {
                let messages = channel(stage).map(|c| c.request_file_contents(request));
                send(stage, messages)
            }
            files::Step::Done(top) => {
                if let Some((_, reply)) = self.fetch.take() {
                    let _ = reply.send(Some(top));
                }
                Ok(Vec::new())
            }
            files::Step::Failed => {
                if let Some((_, reply)) = self.fetch.take() {
                    let _ = reply.send(None);
                }
                Ok(Vec::new())
            }
        }
    }

    /// The interface's answer for the oldest request.
    pub(super) fn answer(
        &mut self,
        format: ClipboardFormatId,
        content: Option<Content>,
        stage: &mut ActiveStage,
    ) -> Outputs {
        let response = match content.and_then(|c| encode(format, c)) {
            Some(data) => {
                self.served = Some(Instant::now());
                OwnedFormatDataResponse::new_data(data)
            }
            None => OwnedFormatDataResponse::new_error(),
        };
        let messages = channel(stage).map(|c| c.submit_format_data(response));
        send(stage, messages)
    }

    /// Let go of locks and transfers the server stopped answering.
    pub(super) fn tick(&mut self, stage: &mut ActiveStage) -> Outputs {
        if !self.ready {
            return Ok(Vec::new());
        }
        let messages = channel(stage).map(CliprdrClient::drive_timeouts);
        send(stage, messages)
    }
}

/// The oldest request's answer, once the interface has it, or nothing once
/// it has taken too long.
pub(super) async fn answered(
    answers: &mut VecDeque<Answer>,
) -> (ClipboardFormatId, Option<Content>) {
    let Some(front) = answers.front_mut() else {
        return std::future::pending().await;
    };
    let content = tokio::time::timeout_at(front.by, &mut front.reply)
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
    let format = front.format;
    answers.pop_front();
    (format, content)
}

fn channel(stage: &mut ActiveStage) -> Option<&mut CliprdrClient> {
    stage.get_svc_processor_mut::<CliprdrClient>()
}

/// Encode what the channel has to send. A clipboard that fails is logged
/// and the session goes on.
fn send(
    stage: &mut ActiveStage,
    messages: Option<ironrdp_pdu::PduResult<CliprdrSvcMessages<ironrdp_cliprdr::Client>>>,
) -> Outputs {
    let messages = match messages {
        Some(Ok(messages)) => messages,
        Some(Err(e)) => {
            tracing::warn!("clipboard: {}", describe(&e));
            return Ok(Vec::new());
        }
        None => return Ok(Vec::new()),
    };
    let frame = stage
        .process_svc_processor_messages(messages)
        .map_err(|e| Failure::Protocol(describe(&e)))?;
    Ok(if frame.is_empty() {
        Vec::new()
    } else {
        vec![ActiveStageOutput::ResponseFrame(frame)]
    })
}

/// The formats offered for what the local clipboard holds, files aside.
fn advertised(kinds: &[Kind]) -> Vec<ClipboardFormat> {
    kinds
        .iter()
        .flat_map(|kind| match kind {
            Kind::Text => vec![ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)],
            Kind::Html => vec![
                ClipboardFormat::new(ClipboardFormatId::new(HTML_ID))
                    .with_name(ClipboardFormatName::HTML),
            ],
            // PNG for GNOME Remote Desktop, which would otherwise give its
            // applications BMP, and not all of them read it.
            Kind::Image => vec![
                ClipboardFormat::new(ClipboardFormatId::new(PNG_ID))
                    .with_name(ClipboardFormatName::new_static("image/png")),
                ClipboardFormat::new(ClipboardFormatId::CF_DIB),
            ],
            Kind::Files => Vec::new(),
        })
        .collect()
}

/// What the server asks for, by the formats Mote offers.
fn wanted(format: ClipboardFormatId) -> Option<Kind> {
    match format {
        ClipboardFormatId::CF_UNICODETEXT
        | ClipboardFormatId::CF_TEXT
        | ClipboardFormatId::CF_OEMTEXT => Some(Kind::Text),
        ClipboardFormatId::CF_DIB => Some(Kind::Image),
        f if f.value() == PNG_ID => Some(Kind::Image),
        f if f.value() == HTML_ID => Some(Kind::Html),
        _ => None,
    }
}

fn named<'a>(formats: &'a [ClipboardFormat], names: &[&str]) -> Option<&'a ClipboardFormat> {
    formats.iter().find(|f| {
        f.name().is_some_and(|n| {
            names
                .iter()
                .any(|name| n.value().eq_ignore_ascii_case(name))
        })
    })
}

fn with_id(formats: &[ClipboardFormat], id: ClipboardFormatId) -> Option<ClipboardFormatId> {
    formats.iter().any(|f| f.id() == id).then_some(id)
}

/// The remote format to fetch `kind` in, the best there is.
fn pick(formats: &[ClipboardFormat], kind: Kind) -> Option<ClipboardFormatId> {
    match kind {
        Kind::Text => with_id(formats, ClipboardFormatId::CF_UNICODETEXT)
            .or_else(|| {
                named(formats, &["UTF8_STRING", "text/plain;charset=utf-8"]).map(|f| f.id())
            })
            .or_else(|| with_id(formats, ClipboardFormatId::CF_TEXT))
            .or_else(|| with_id(formats, ClipboardFormatId::CF_OEMTEXT)),
        Kind::Html => named(formats, &[ClipboardFormatName::HTML.value()]).map(|f| f.id()),
        // CF_DIB before CF_DIBV5, which xrdp lists beside it without
        // sending.
        Kind::Image => named(formats, &["PNG", "image/png"])
            .map(|f| f.id())
            .or_else(|| with_id(formats, ClipboardFormatId::CF_DIB))
            .or_else(|| with_id(formats, ClipboardFormatId::CF_DIBV5)),
        Kind::Files => named(formats, &[ClipboardFormatName::FILE_LIST.value()]).map(|f| f.id()),
    }
}

/// The remote clipboard's data, in the format `pick` chose.
fn decode(format: ClipboardFormatId, data: &[u8]) -> Option<Content> {
    match format {
        ClipboardFormatId::CF_UNICODETEXT => Some(Content::Text(formats::text_from_unicode(data))),
        ClipboardFormatId::CF_TEXT | ClipboardFormatId::CF_OEMTEXT => {
            Some(Content::Text(formats::text_from_ansi(data)))
        }
        ClipboardFormatId::CF_DIB | ClipboardFormatId::CF_DIBV5 => {
            formats::image_from_dib(data).map(Content::Image)
        }
        // A registered format: text, HTML or PNG by name, which `pick`
        // matched. PNG starts with its signature; HTML Format with its
        // header.
        _ if data.starts_with(b"\x89PNG") => Some(Content::Png(data.to_vec())),
        _ if data.starts_with(b"Version:") => formats::html_from_cf_html(data).map(Content::Html),
        _ => Some(Content::Text(formats::text_from_ansi(data))),
    }
}

/// The local clipboard's content, in the format the server asked for.
fn encode(format: ClipboardFormatId, content: Content) -> Option<Vec<u8>> {
    match (format, content) {
        (ClipboardFormatId::CF_UNICODETEXT, Content::Text(text)) => {
            Some(formats::unicode_from_text(&text))
        }
        (ClipboardFormatId::CF_TEXT | ClipboardFormatId::CF_OEMTEXT, Content::Text(text)) => {
            let mut data = text.into_bytes();
            data.push(0);
            Some(data)
        }
        (ClipboardFormatId::CF_DIB, Content::Image(image)) => Some(formats::dib_from_image(&image)),
        (f, Content::Html(html)) if f.value() == HTML_ID => Some(formats::cf_html_from_html(&html)),
        (f, Content::Png(png)) if f.value() == PNG_ID => Some(png),
        _ => None,
    }
}

/// A format list, for the log.
fn names(formats: &[ClipboardFormat]) -> String {
    formats
        .iter()
        .map(|f| match f.name() {
            Some(name) => format!("{:#x} {:?}", f.id().value(), name.value()),
            None => format!("{:#x}", f.id().value()),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_best_remote_format_is_picked() {
        let formats = [
            ClipboardFormat::new(ClipboardFormatId::CF_TEXT),
            ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT),
            ClipboardFormat::new(ClipboardFormatId::new(0xc004))
                .with_name(ClipboardFormatName::new("HTML Format")),
            ClipboardFormat::new(ClipboardFormatId::CF_DIB),
        ];
        assert_eq!(
            pick(&formats, Kind::Text),
            Some(ClipboardFormatId::CF_UNICODETEXT)
        );
        assert_eq!(
            pick(&formats, Kind::Html),
            Some(ClipboardFormatId::new(0xc004))
        );
        assert_eq!(pick(&formats, Kind::Image), Some(ClipboardFormatId::CF_DIB));
        assert_eq!(pick(&formats, Kind::Files), None);
    }

    #[test]
    fn files_are_kept_back_unless_shared() {
        let copied = || ClipboardCommand::Copied {
            kinds: vec![Kind::Files, Kind::Text],
            files: vec![PathBuf::from("/home/someone/secret.txt")],
        };
        let mut kept = Clipboard::new(true, false);
        kept.early(copied());
        assert!(kept.local.is_empty());
        assert!(kept.files.is_empty());
        // Text alone still goes.
        kept.early(ClipboardCommand::Copied {
            kinds: vec![Kind::Text],
            files: Vec::new(),
        });
        assert_eq!(kept.local, vec![Kind::Text]);

        let mut shared = Clipboard::new(true, true);
        shared.early(copied());
        assert_eq!(shared.local, vec![Kind::Files, Kind::Text]);
        assert_eq!(shared.files.len(), 1);
    }

    #[test]
    fn what_is_offered_is_what_is_answered() {
        for kind in [Kind::Text, Kind::Html, Kind::Image] {
            for format in advertised(&[kind]) {
                assert_eq!(wanted(format.id()), Some(kind));
            }
        }
    }
}
