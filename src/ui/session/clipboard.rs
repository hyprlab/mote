//! The clipboard between this desktop and the remote one, on the interface's
//! side: what is copied here is announced to the server and read when it
//! pastes; what is copied there goes on the local clipboard as a promise,
//! fetched only when something here pastes it.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::pin::Pin;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, gio, glib};
use tokio::sync::oneshot;

use super::{Session, SessionOutput};
use crate::i18n::i18n;
use crate::rdp;
use crate::remote::clipboard::{Content, Image, Kind};
use crate::remote::{self, ClipboardCommand, ClipboardEvent};

/// The most read from the local clipboard for one paste.
const MAX_READ: usize = 256 << 20;
/// How long a paste here waits for the remote desktop's text or picture.
const FETCH_WITHIN: std::time::Duration = std::time::Duration::from_secs(30);

/// Where files pasted from a remote desktop are kept, until the app starts
/// again.
pub(crate) fn fetched_files() -> PathBuf {
    glib::user_cache_dir().join("mote").join("clipboard")
}

/// One session's share of the clipboard. Dropped, it stops listening, and
/// takes back what it put on the clipboard, which can't be fetched now.
pub(super) struct Sharing {
    clipboard: gdk::Clipboard,
    changed: Option<glib::SignalHandlerId>,
    handle: remote::Handle,
    /// Marks what this session put on the clipboard.
    tag: u64,
}

thread_local! {
    static TAGS: Cell<u64> = const { Cell::new(0) };
}

impl Sharing {
    /// `share_files` offers files copied here; off, the server never hears
    /// of them, nor of their paths.
    pub(super) fn new(
        widget: &impl IsA<gtk::Widget>,
        handle: remote::Handle,
        share_files: bool,
    ) -> Self {
        let clipboard = widget.clipboard();
        let tag = TAGS.with(|tags| {
            tags.set(tags.get() + 1);
            tags.get()
        });
        let changed = clipboard.connect_changed({
            let handle = handle.clone();
            move |clipboard| announce(clipboard, &handle, tag, share_files)
        });
        // The server hears what is already there once it can.
        announce(&clipboard, &handle, tag, share_files);
        Self {
            clipboard,
            changed: Some(changed),
            handle,
            tag,
        }
    }

    pub(super) fn event(&self, event: ClipboardEvent) {
        match event {
            ClipboardEvent::Copied(kinds) => {
                let content = RemoteContent::new(kinds, self.handle.clone(), self.tag);
                if let Err(e) = self.clipboard.set_content(Some(&content)) {
                    tracing::warn!("taking the clipboard: {e}");
                }
            }
            ClipboardEvent::Wanted { kind, png, reply } => {
                let clipboard = self.clipboard.clone();
                glib::spawn_future_local(async move {
                    let content = read(&clipboard, kind, png).await;
                    let _ = reply.send(content);
                });
            }
        }
    }
}

impl Drop for Sharing {
    fn drop(&mut self) {
        if let Some(changed) = self.changed.take() {
            self.clipboard.disconnect(changed);
        }
        if ours(&self.clipboard, self.tag) {
            let _ = self.clipboard.set_content(None::<&gdk::ContentProvider>);
        }
    }
}

impl Session {
    /// Say, at a connection's first session that shares the clipboard,
    /// that the server reads what is copied here without asking.
    pub(super) fn tell_clipboard(&mut self, sender: &relm4::ComponentSender<Self>) {
        if !self.clipboard_notice || self.clipboard.is_none() || self.connection.clipboard_told {
            return;
        }
        self.connection.clipboard_told = true;
        let toast = adw::Toast::new(&i18n("The remote desktop can read what is copied here"));
        toast.set_timeout(10);
        self.toasts.add_toast(toast);
        let _ = sender.output(SessionOutput::ClipboardTold(self.connection.id.clone()));
    }
}

/// Whether the clipboard holds this session's promise.
fn ours(clipboard: &gdk::Clipboard, tag: u64) -> bool {
    clipboard
        .content()
        .and_downcast::<RemoteContent>()
        .is_some_and(|c| c.imp().tag.get() == tag)
}

/// Tell the server what the local clipboard holds now. What another
/// session put there is passed on like anything else. Files go only when
/// `share_files` is set; otherwise a copy of files is offered as nothing,
/// since its text is their paths.
fn announce(clipboard: &gdk::Clipboard, handle: &remote::Handle, tag: u64, share_files: bool) {
    if ours(clipboard, tag) {
        return;
    }
    // What another application offers is listed by mime type; this adds
    // what GDK can turn those into.
    let formats = clipboard.formats().union_deserialize_types();
    let holds_files = formats.contains_type(gdk::FileList::static_type());
    let mut kinds = Vec::new();
    if formats.contains_type(glib::Type::STRING) {
        kinds.push(Kind::Text);
    }
    if formats.contain_mime_type("text/html") {
        kinds.push(Kind::Html);
    }
    if formats.contains_type(gdk::Texture::static_type()) {
        kinds.push(Kind::Image);
    }
    tracing::debug!("the local clipboard holds {}", formats.to_str());
    if holds_files && !share_files {
        tracing::debug!("files are not shared: offering nothing");
        kinds.clear();
    }
    if !share_files || !holds_files {
        handle.send(remote::Command::Clipboard(ClipboardCommand::Copied {
            kinds,
            files: Vec::new(),
        }));
        return;
    }
    // Files are offered by path, so the paths are read now; through the
    // file transfer portal inside a sandbox.
    let (clipboard, handle) = (clipboard.clone(), handle.clone());
    glib::spawn_future_local(async move {
        let files = clipboard
            .read_value_future(gdk::FileList::static_type(), glib::Priority::DEFAULT)
            .await
            .ok()
            .and_then(|value| value.get::<gdk::FileList>().ok())
            .map(|list| {
                list.files()
                    .iter()
                    .filter_map(gio::File::path)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !files.is_empty() {
            kinds.insert(0, Kind::Files);
        }
        handle.send(remote::Command::Clipboard(ClipboardCommand::Copied {
            kinds,
            files,
        }));
    });
}

/// The local clipboard's content as `kind`, for the server; a picture as
/// PNG when `png` is set.
async fn read(clipboard: &gdk::Clipboard, kind: Kind, png: bool) -> Option<Content> {
    let priority = glib::Priority::DEFAULT;
    match kind {
        Kind::Text => {
            let text = clipboard.read_text_future().await.ok().flatten()?;
            Some(Content::Text(text.into()))
        }
        Kind::Html => {
            let (stream, _) = clipboard.read_future(&["text/html"], priority).await.ok()?;
            let data = read_all(&stream).await?;
            Some(Content::Html(rdp::clipboard::formats::html_from_local(
                &data,
            )))
        }
        // Most applications copy PNG; it goes as it is.
        Kind::Image if png && clipboard.formats().contain_mime_type("image/png") => {
            let (stream, _) = clipboard.read_future(&["image/png"], priority).await.ok()?;
            read_all(&stream).await.map(Content::Png)
        }
        Kind::Image if png => {
            let texture = clipboard.read_texture_future().await.ok().flatten()?;
            gio::spawn_blocking(move || texture.save_to_png_bytes().to_vec())
                .await
                .ok()
                .map(Content::Png)
        }
        Kind::Image => {
            let texture = clipboard.read_texture_future().await.ok().flatten()?;
            gio::spawn_blocking(move || rgba(&texture))
                .await
                .ok()
                .map(Content::Image)
        }
        Kind::Files => None,
    }
}

async fn read_all(stream: &gio::InputStream) -> Option<Vec<u8>> {
    let mut data = Vec::new();
    loop {
        let bytes = stream
            .read_bytes_future(64 << 10, glib::Priority::DEFAULT)
            .await
            .ok()?;
        if bytes.is_empty() {
            return Some(data);
        }
        data.extend_from_slice(&bytes);
        if data.len() > MAX_READ {
            tracing::warn!("the clipboard holds more than the server is sent");
            return None;
        }
    }
}

/// A texture's pixels, straight RGBA without padding.
fn rgba(texture: &gdk::Texture) -> Image {
    let mut downloader = gdk::TextureDownloader::new(texture);
    downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
    let (bytes, stride) = downloader.download_bytes();
    let (width, height) = (texture.width() as usize, texture.height() as usize);
    let rgba = bytes
        .chunks(stride)
        .take(height)
        .flat_map(|row| &row[..width * 4])
        .copied()
        .collect();
    Image {
        width: width as u32,
        height: height as u32,
        rgba,
    }
}

/// PNG, for a picture from the remote desktop.
fn png(image: Image) -> glib::Bytes {
    let stride = image.width as usize * 4;
    let texture = gdk::MemoryTexture::new(
        image.width as i32,
        image.height as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(image.rgba),
        stride,
    );
    texture.save_to_png_bytes()
}

/// The mime types offered for each kind of content.
fn mime_types(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Text => &["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"],
        Kind::Html => &["text/html"],
        Kind::Image => &["image/png"],
        Kind::Files => &["text/uri-list", "x-special/gnome-copied-files"],
    }
}

/// Files being fetched for a paste, or fetched already.
#[derive(Default)]
enum Fetched {
    #[default]
    No,
    Waiting(Vec<oneshot::Sender<Option<Vec<PathBuf>>>>),
    Here(Option<Vec<PathBuf>>),
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub(crate) struct RemoteContent {
        pub(super) kinds: RefCell<Vec<Kind>>,
        pub(super) handle: RefCell<Option<remote::Handle>>,
        pub(super) tag: Cell<u64>,
        pub(super) files: RefCell<Fetched>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RemoteContent {
        const NAME: &'static str = "MoteRemoteContent";
        type Type = super::RemoteContent;
        type ParentType = gdk::ContentProvider;
    }

    impl ObjectImpl for RemoteContent {}

    impl gdk::subclass::prelude::ContentProviderImpl for RemoteContent {
        fn formats(&self) -> gdk::ContentFormats {
            let mut builder = gdk::ContentFormatsBuilder::new();
            for kind in self.kinds.borrow().iter() {
                for mime in mime_types(*kind) {
                    builder = builder.add_mime_type(*mime);
                }
            }
            builder.build()
        }

        fn write_mime_type_future(
            &self,
            mime_type: &str,
            stream: &gio::OutputStream,
            io_priority: glib::Priority,
        ) -> Pin<Box<dyn Future<Output = Result<(), glib::Error>> + 'static>> {
            let (content, mime, stream) =
                (self.obj().clone(), mime_type.to_owned(), stream.clone());
            Box::pin(async move {
                let data = content.fetch(&mime).await.ok_or_else(|| {
                    glib::Error::new(
                        gio::IOErrorEnum::NotFound,
                        "The remote desktop sent nothing",
                    )
                })?;
                stream
                    .write_all_future(data, io_priority)
                    .await
                    .map_err(|(_, e)| e)?;
                Ok(())
            })
        }
    }
}

glib::wrapper! {
    /// What was copied on the remote desktop, fetched when pasted.
    pub(crate) struct RemoteContent(ObjectSubclass<imp::RemoteContent>)
        @extends gdk::ContentProvider;
}

impl RemoteContent {
    fn new(kinds: Vec<Kind>, handle: remote::Handle, tag: u64) -> Self {
        let content: Self = glib::Object::new();
        let imp = content.imp();
        imp.kinds.replace(kinds);
        imp.handle.replace(Some(handle));
        imp.tag.set(tag);
        content
    }

    fn send(&self, command: ClipboardCommand) {
        if let Some(handle) = self.imp().handle.borrow().as_ref() {
            handle.send(remote::Command::Clipboard(command));
        }
    }

    /// The content as `mime`, from the remote desktop.
    async fn fetch(&self, mime: &str) -> Option<Vec<u8>> {
        let kind = [Kind::Text, Kind::Html, Kind::Image, Kind::Files]
            .into_iter()
            .find(|kind| mime_types(*kind).contains(&mime))?;
        if kind == Kind::Files {
            let files = self.files().await?;
            let uris: Vec<String> = files
                .iter()
                .map(|path| gio::File::for_path(path).uri().into())
                .collect();
            return Some(if mime == "x-special/gnome-copied-files" {
                format!("copy\n{}", uris.join("\n")).into_bytes()
            } else {
                uris.iter()
                    .map(|uri| format!("{uri}\r\n"))
                    .collect::<String>()
                    .into_bytes()
            });
        }
        let (reply, answer) = oneshot::channel();
        self.send(ClipboardCommand::Paste { kind, reply });
        // A server can leave a request unanswered, as xrdp does when what
        // was copied there is gone; the application pasting isn't kept
        // waiting for it.
        let answer = glib::future_with_timeout(FETCH_WITHIN, answer).await.ok()?;
        match answer.ok()?? {
            Content::Text(text) | Content::Html(text) => Some(text.into_bytes()),
            Content::Png(png) => Some(png),
            Content::Image(image) => gio::spawn_blocking(move || png(image))
                .await
                .ok()
                .map(|bytes| bytes.to_vec()),
        }
    }

    /// The remote files, fetched once however often they are pasted.
    async fn files(&self) -> Option<Vec<PathBuf>> {
        let (reply, answer) = oneshot::channel();
        let start = {
            let mut files = self.imp().files.borrow_mut();
            match &mut *files {
                Fetched::Here(paths) => return paths.clone(),
                Fetched::Waiting(waiting) => {
                    waiting.push(reply);
                    false
                }
                Fetched::No => {
                    *files = Fetched::Waiting(vec![reply]);
                    true
                }
            }
        };
        if start {
            let dir = fetched_files().join(glib::uuid_string_random().as_str());
            let (fetched, done) = oneshot::channel();
            self.send(ClipboardCommand::PasteFiles {
                dir,
                reply: fetched,
            });
            let paths = done.await.ok().flatten();
            // A fetch that failed is tried again at the next paste.
            let now = match &paths {
                Some(_) => Fetched::Here(paths.clone()),
                None => Fetched::No,
            };
            let waiting = std::mem::replace(&mut *self.imp().files.borrow_mut(), now);
            if let Fetched::Waiting(waiting) = waiting {
                for reply in waiting {
                    let _ = reply.send(paths.clone());
                }
            }
        }
        answer.await.ok().flatten()
    }
}
