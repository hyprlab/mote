//! H.264 for the Graphics Pipeline, from whichever decoder the system has,
//! loaded when a session starts: FFmpeg's (the Flatpak's GNOME runtime has
//! it), the system's OpenH264 (Fedora's), or Cisco's OpenH264 downloaded on
//! request. Without one, servers send RemoteFX instead, or, like KRdp,
//! refuse the session.

pub(crate) mod cisco;
mod ffmpeg;
mod openh264;

use std::sync::Mutex;

use ironrdp_egfx::decode::{DecodedFrame, DecoderResult, H264Decoder, Yuv420Frame};

/// Where H.264 comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Source {
    #[serde(rename = "ffmpeg")]
    Ffmpeg,
    #[serde(rename = "system-openh264")]
    SystemOpenH264,
    #[serde(rename = "cisco-openh264")]
    CiscoOpenH264,
}

impl Source {
    /// The order sources are tried in when none is chosen.
    pub(crate) const ALL: [Self; 3] = [Self::Ffmpeg, Self::SystemOpenH264, Self::CiscoOpenH264];

    fn name(self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg",
            Self::SystemOpenH264 => "openh264",
            Self::CiscoOpenH264 => "cisco",
        }
    }

    fn load(self) -> Option<Box<dyn H264Decoder>> {
        match self {
            Self::Ffmpeg => {
                let dir = std::env::var_os("MOTE_FFMPEG_DIR").map(std::path::PathBuf::from);
                dir.as_deref()
                    .and_then(|d| ffmpeg::Ffmpeg::load(Some(d)))
                    .or_else(|| ffmpeg::Ffmpeg::load(None))
                    .map(|d| Box::new(d) as Box<dyn H264Decoder>)
            }
            Self::SystemOpenH264 => openh264::OpenH264::system().map(|d| Box::new(d) as _),
            Self::CiscoOpenH264 => cisco::downloaded()
                .then(|| openh264::OpenH264::checked(&cisco::path()))
                .flatten()
                .map(|d| Box::new(d) as _),
        }
    }
}

/// The source Settings chose, tried before the others; set by the main
/// window, read by sessions as their graphics channel opens.
static CHOSEN: Mutex<Option<Source>> = Mutex::new(None);

pub(crate) fn choose(source: Option<Source>) {
    *CHOSEN.lock().unwrap_or_else(|e| e.into_inner()) = source;
}

/// The sources to try, the chosen one first. `MOTE_H264` set to `ffmpeg`,
/// `openh264` or `cisco` tries that source alone, and `MOTE_FFMPEG_DIR`
/// names a directory to load FFmpeg from.
fn order() -> Vec<Source> {
    let only = std::env::var("MOTE_H264").ok();
    let chosen = *CHOSEN.lock().unwrap_or_else(|e| e.into_inner());
    chosen
        .into_iter()
        .chain(Source::ALL.into_iter().filter(|s| Some(*s) != chosen))
        .filter(|s| only.as_deref().is_none_or(|o| o == s.name()))
        .collect()
}

/// A decoder from the first source that has one.
pub(crate) fn decoder() -> Option<(Source, Box<dyn H264Decoder>)> {
    let found = order().into_iter().find_map(|s| s.load().map(|d| (s, d)));
    match &found {
        Some((source, _)) => tracing::info!(target: "mote::stats", "H.264 from {source:?}"),
        None => tracing::info!("no H.264 decoder: servers will send RemoteFX instead"),
    }
    found
}

/// The sources this system has, in the order they are tried when none is
/// chosen, for Settings to offer. Loads and drops a decoder from each, so
/// call it off the main thread.
pub(crate) fn available() -> Vec<Source> {
    Source::ALL
        .into_iter()
        .filter(|s| s.load().is_some())
        .collect()
}

/// The decoder the graphics pipeline gets: timed for `mote::stats`, and,
/// for AVC420 only, saying it can't give planes, which is what keeps
/// IronRDP from offering AVC444.
pub(super) struct Decoder {
    inner: Box<dyn H264Decoder>,
    planes: bool,
    stats: crate::stats::Stats,
}

impl Decoder {
    pub(super) fn new(inner: Box<dyn H264Decoder>, avc444: bool) -> Self {
        Self {
            inner,
            planes: avc444,
            stats: crate::stats::Stats::new("h264"),
        }
    }
}

impl H264Decoder for Decoder {
    fn decode(&mut self, data: &[u8]) -> DecoderResult<DecodedFrame> {
        let start = std::time::Instant::now();
        let frame = self.inner.decode(data);
        self.stats.time("decode", start);
        frame
    }

    fn decode_yuv420(&mut self, data: &[u8]) -> DecoderResult<Yuv420Frame> {
        let start = std::time::Instant::now();
        let frame = self.inner.decode_yuv420(data);
        self.stats.time("decode", start);
        frame
    }

    fn supports_yuv420(&self) -> bool {
        self.planes && self.inner.supports_yuv420()
    }

    fn wait_until_ready(&mut self) -> bool {
        self.inner.wait_until_ready()
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}
