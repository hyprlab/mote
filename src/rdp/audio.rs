//! The remote desktop's sound, played here (MS-RDPEA), and the microphone,
//! sent there (MS-RDPEAI). IronRDP's channels speak the protocol; a thread
//! per stream plays or records through PulseAudio's API, as PipeWire serves
//! it, so a slow sound server never holds up the session. On macOS the
//! same calls go to Audio Queue Services instead.

#[cfg(target_os = "macos")]
mod coreaudio;
mod opus;
#[cfg(not(target_os = "macos"))]
mod pulse;
#[cfg(target_os = "macos")]
use coreaudio as backend;
#[cfg(not(target_os = "macos"))]
use pulse as backend;

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use ironrdp_core::impl_as_any;
use ironrdp_dvc::{DvcClientProcessor, DvcMessage, DvcProcessor};
use ironrdp_pdu::PduResult;
use ironrdp_pdu::gcc::ChannelName;
use ironrdp_rdpeai::client::{AudioPacketSink, DvcUplink, RdpeaiCaptureHandler, RdpeaiClient};
use ironrdp_rdpsnd::client::{Rdpsnd, RdpsndClientHandler, RdpsndDvc};
use ironrdp_rdpsnd::pdu::{
    AudioFormat, AudioFormatFlags, ClientAudioOutputPdu, PitchPdu, TrainingConfirmPdu, VolumePdu,
    WaveFormat,
};
use ironrdp_svc::{CompressionCondition, SvcClientProcessor, SvcMessage, SvcProcessor};
use tokio::sync::mpsc as tokio_mpsc;

/// How much sound is kept queued ahead of the speakers: enough to ride
/// out a network's jitter, little enough to stay with the picture.
const PLAY_AHEAD: Duration = Duration::from_millis(100);
/// Blocks waiting to be played before more are dropped, a few seconds'
/// worth: the server sends in real time, so a backlog means the speakers
/// fell behind.
const MAX_QUEUED: usize = 128;
/// The microphone is read and sent this much at a time.
const CAPTURE_FRAGMENT: Duration = Duration::from_millis(20);
/// An HRESULT for a microphone that can't be opened.
const E_FAIL: i32 = 0x8000_4005_u32 as i32;

/// 16-bit PCM, the formats offered, most preferred first. Linux servers
/// offer PCM beside their compressed formats.
fn pcm_formats() -> Vec<AudioFormat> {
    [
        (48_000, 2),
        (44_100, 2),
        (22_050, 2),
        (48_000, 1),
        (44_100, 1),
        (22_050, 1),
    ]
    .into_iter()
    .map(|(rate, channels)| pcm(rate, channels))
    .collect()
}

fn pcm(rate: u32, channels: u16) -> AudioFormat {
    AudioFormat {
        format: WaveFormat::PCM,
        n_channels: channels,
        n_samples_per_sec: rate,
        n_avg_bytes_per_sec: rate * u32::from(channels) * 2,
        n_block_align: channels * 2,
        bits_per_sample: 16,
        data: None,
    }
}

fn is_opus(format: &AudioFormat) -> bool {
    format.format == WaveFormat::OPUS && matches!(format.n_channels, 1 | 2)
}

fn is_pcm16(format: &AudioFormat) -> bool {
    format.format == WaveFormat::PCM
        && format.bits_per_sample == 16
        && matches!(format.n_channels, 1 | 2)
}

/// What a connection does with sound, and the microphone's way to the
/// session loop, which sends its packets.
pub(super) struct Audio {
    play: bool,
    record: bool,
    uplink: tokio_mpsc::UnboundedSender<Vec<SvcMessage>>,
    pub(super) packets: tokio_mpsc::UnboundedReceiver<Vec<SvcMessage>>,
}

impl Audio {
    pub(super) fn new(play: bool, record: bool) -> Self {
        let available = (play || record) && backend::available();
        let (uplink, packets) = tokio_mpsc::unbounded_channel();
        Self {
            play: play && available,
            record: record && available,
            uplink,
            packets,
        }
    }

    pub(super) fn plays(&self) -> bool {
        self.play
    }

    pub(super) fn records(&self) -> bool {
        self.record
    }

    /// The static sound channel, for servers that send on it, as xrdp does.
    pub(super) fn static_channel(&self) -> Option<SoundChannel> {
        self.play
            .then(|| SoundChannel(Rdpsnd::new(Box::new(Speaker::new()))))
    }

    /// Makes the sound channel's dynamic counterpart, for GNOME Remote
    /// Desktop, each time the server opens it.
    pub(super) fn playback(&self) -> Option<impl Fn() -> SoundDvc + Send + 'static> {
        self.play
            .then_some(|| SoundDvc(RdpsndDvc::new(Box::new(Speaker::new()))))
    }

    /// Makes the microphone's channel each time the server opens it.
    pub(super) fn capture(&self) -> Option<impl Fn() -> RdpeaiClient + Send + 'static> {
        let uplink = self.uplink.clone();
        self.record.then_some(move || {
            let uplink = uplink.clone();
            let send: DvcUplink = Box::new(move |_, messages| {
                // A session that has ended doesn't send them.
                let _ = uplink.send(messages);
                Ok(())
            });
            RdpeaiClient::new(Box::new(Microphone::default()), send)
        })
    }
}

/// The reply to the server's Training PDU, when that is what `payload` is.
/// The confirmation carries back the `wPackSize` the server sent
/// ([MS-RDPEA] 2.2.3.2). IronRDP takes that size to count the PDU's eight
/// bytes of header and answers with eight bytes less; FreeRDP, and with it
/// GNOME Remote Desktop, counts the data alone, and sends no sound after an
/// answer that doesn't match.
fn training_confirm(payload: &[u8]) -> Option<ClientAudioOutputPdu> {
    const SNDC_TRAINING: u8 = 0x06;
    let u16_at = |at: usize| {
        Some(u16::from_le_bytes(
            payload.get(at..at + 2)?.try_into().ok()?,
        ))
    };
    let (body, timestamp, pack_size) = (u16_at(2)?, u16_at(4)?, u16_at(6)?);
    // The sizes are checked too, so wave data that happens to start with
    // the same byte isn't taken for training.
    let data = payload.len().saturating_sub(8);
    let fits = usize::from(body) + 4 == payload.len()
        && [0, data, payload.len()].contains(&usize::from(pack_size));
    (payload[0] == SNDC_TRAINING && fits).then_some(ClientAudioOutputPdu::TrainingConfirm(
        TrainingConfirmPdu {
            timestamp,
            pack_size,
        },
    ))
}

/// IronRDP's static sound channel, with the training answered correctly.
#[derive(Debug)]
pub(super) struct SoundChannel(Rdpsnd);

impl_as_any!(SoundChannel);

impl SvcProcessor for SoundChannel {
    fn channel_name(&self) -> ChannelName {
        self.0.channel_name()
    }

    fn compression_condition(&self) -> CompressionCondition {
        self.0.compression_condition()
    }

    fn start(&mut self) -> PduResult<Vec<SvcMessage>> {
        self.0.start()
    }

    fn process(&mut self, payload: &[u8]) -> PduResult<Vec<SvcMessage>> {
        let replies = self.0.process(payload)?;
        Ok(match training_confirm(payload) {
            Some(confirm) => vec![SvcMessage::from(confirm)],
            None => replies,
        })
    }
}

impl SvcClientProcessor for SoundChannel {}

/// IronRDP's dynamic sound channel, with the training answered correctly.
#[derive(Debug)]
pub(super) struct SoundDvc(RdpsndDvc);

impl_as_any!(SoundDvc);

impl DvcProcessor for SoundDvc {
    fn channel_name(&self) -> &str {
        self.0.channel_name()
    }

    fn start(&mut self, channel_id: u32) -> PduResult<Vec<DvcMessage>> {
        self.0.start(channel_id)
    }

    fn process(&mut self, channel_id: u32, payload: &[u8]) -> PduResult<Vec<DvcMessage>> {
        let replies = self.0.process(channel_id, payload)?;
        Ok(match training_confirm(payload) {
            Some(confirm) => vec![Box::new(confirm) as DvcMessage],
            None => replies,
        })
    }

    fn close(&mut self, channel_id: u32) {
        self.0.close(channel_id);
    }
}

impl DvcClientProcessor for SoundDvc {}

/// Plays what the server sends, on a thread of its own.
#[derive(Debug)]
struct Speaker {
    formats: Vec<AudioFormat>,
    blocks: mpsc::SyncSender<Play>,
    /// How long what was queued last takes to be heard, in microseconds.
    latency: Arc<AtomicU64>,
}

enum Play {
    Samples {
        rate: u32,
        channels: u8,
        data: Vec<u8>,
    },
    Opus {
        rate: u32,
        channels: u8,
        packet: Vec<u8>,
    },
    /// The server stopped; what is queued is dropped.
    Stop,
}

impl Speaker {
    fn new() -> Self {
        let (blocks, queue) = mpsc::sync_channel(MAX_QUEUED);
        let latency = Arc::new(AtomicU64::new(0));
        let reported = latency.clone();
        let spawned = std::thread::Builder::new()
            .name("rdp-sound".into())
            .spawn(move || play(&queue, &reported));
        if let Err(e) = spawned {
            tracing::warn!("starting the sound thread: {e}");
        }
        Self {
            formats: pcm_formats(),
            blocks,
            latency,
        }
    }
}

impl RdpsndClientHandler for Speaker {
    fn get_flags(&self) -> AudioFormatFlags {
        AudioFormatFlags::ALIVE
    }

    fn get_formats(&self) -> &[AudioFormat] {
        &self.formats
    }

    /// Opus first, as the server describes it, which GNOME Remote Desktop
    /// compares field by field; then PCM.
    fn choose_formats(&self, server: &[AudioFormat]) -> Vec<AudioFormat> {
        let opus = server
            .iter()
            .filter(|f| is_opus(f) && opus::available())
            .cloned();
        opus.chain(self.formats.iter().cloned()).collect()
    }

    fn wave(&mut self, format: &AudioFormat, _: u32, data: Cow<'_, [u8]>) {
        let (rate, channels) = (format.n_samples_per_sec, format.n_channels as u8);
        let block = if is_opus(format) {
            Play::Opus {
                rate,
                channels,
                packet: data.into_owned(),
            }
        } else if is_pcm16(format) {
            Play::Samples {
                rate,
                channels,
                data: data.into_owned(),
            }
        } else {
            return;
        };
        if self.blocks.try_send(block).is_err() {
            tracing::debug!("the speakers fell behind; dropping sound");
        }
    }

    fn playback_latency(&self) -> Duration {
        Duration::from_micros(self.latency.load(Ordering::Relaxed))
    }

    fn set_volume(&mut self, volume: VolumePdu) {
        tracing::debug!("the server set the volume to {volume:?}");
    }

    fn set_pitch(&mut self, _: PitchPdu) {}

    fn close(&mut self) {
        let _ = self.blocks.try_send(Play::Stop);
    }
}

/// The sound thread: opens a stream for each format the server sends in,
/// and ends with the channel.
fn play(queue: &mpsc::Receiver<Play>, latency: &AtomicU64) {
    let mut stream: Option<((u32, u8), backend::Stream)> = None;
    let mut decoder: Option<((u32, u8), opus::Decoder)> = None;
    while let Ok(block) = queue.recv() {
        let (rate, channels, data) = match block {
            Play::Samples {
                rate,
                channels,
                data,
            } => (rate, channels, data),
            Play::Opus {
                rate,
                channels,
                packet,
            } => {
                if decoder
                    .as_ref()
                    .is_none_or(|(spec, _)| *spec != (rate, channels))
                {
                    decoder = opus::Decoder::new(rate, channels).map(|d| ((rate, channels), d));
                }
                let Some(data) = decoder.as_mut().and_then(|(_, d)| d.decode(&packet)) else {
                    continue;
                };
                (rate, channels, data)
            }
            Play::Stop => {
                if let Some((_, stream)) = stream.as_mut() {
                    stream.flush();
                }
                continue;
            }
        };
        if stream
            .as_ref()
            .is_none_or(|(spec, _)| *spec != (rate, channels))
        {
            stream = None;
            match backend::Stream::play("Remote Desktop", rate, channels, PLAY_AHEAD) {
                Ok(opened) => {
                    tracing::debug!("playing sound at {rate} Hz, {channels} channels");
                    stream = Some(((rate, channels), opened));
                }
                Err(e) => {
                    tracing::warn!("playing sound: {e}");
                    continue;
                }
            }
        }
        let Some((_, playing)) = stream.as_mut() else {
            continue;
        };
        if let Err(e) = playing.write(&data) {
            tracing::warn!("playing sound: {e}");
            stream = None;
            continue;
        }
        latency.store(playing.latency().as_micros() as u64, Ordering::Relaxed);
    }
}

/// Records the microphone on a thread of its own while the server listens.
#[derive(Default)]
struct Microphone {
    recording: Option<Recording>,
}

struct Recording {
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

impl RdpeaiCaptureHandler for Microphone {
    fn supported_formats(&self) -> &[AudioFormat] {
        static FORMATS: std::sync::OnceLock<Vec<AudioFormat>> = std::sync::OnceLock::new();
        FORMATS.get_or_init(pcm_formats)
    }

    fn open(
        &mut self,
        capture: &AudioFormat,
        encode: &AudioFormat,
        packet_size: usize,
        sink: AudioPacketSink,
    ) -> i32 {
        self.close();
        let same = (capture.n_samples_per_sec, capture.n_channels)
            == (encode.n_samples_per_sec, encode.n_channels);
        if !is_pcm16(encode) || !same {
            tracing::debug!("microphone formats: capture {capture:?}, encode {encode:?}");
            tracing::warn!("the server asked for a microphone format Mote doesn't send");
            return E_FAIL;
        }
        let (rate, channels) = (encode.n_samples_per_sec, encode.n_channels as u8);
        let stream =
            match backend::Stream::record("Remote Desktop", rate, channels, CAPTURE_FRAGMENT) {
                Ok(stream) => stream,
                Err(e) => {
                    tracing::warn!("recording the microphone: {e}");
                    return E_FAIL;
                }
            };
        tracing::debug!(
            "sending the microphone at {rate} Hz, {channels} channels, {packet_size} bytes a packet"
        );
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        match std::thread::Builder::new()
            .name("rdp-microphone".into())
            .spawn(move || record(stream, packet_size, sink, &stopped))
        {
            Ok(thread) => {
                self.recording = Some(Recording { stop, thread });
                0
            }
            Err(e) => {
                tracing::warn!("starting the microphone thread: {e}");
                E_FAIL
            }
        }
    }

    fn set_format(&mut self, encode: &AudioFormat, _: usize) -> bool {
        // Formats change only on Open, which starts the recording again.
        is_pcm16(encode)
    }

    fn close(&mut self) {
        if let Some(recording) = self.recording.take() {
            recording.stop.store(true, Ordering::Relaxed);
            // The thread notices within a packet.
            let _ = recording.thread.join();
        }
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        self.close();
    }
}

fn record(
    mut stream: backend::Stream,
    packet_size: usize,
    mut sink: AudioPacketSink,
    stop: &AtomicBool,
) {
    let mut packet = vec![0; packet_size.max(2)];
    let mut sent = 0u64;
    while !stop.load(Ordering::Relaxed) {
        if let Err(e) = stream.read(&mut packet) {
            tracing::warn!("recording the microphone: {e}");
            return;
        }
        sent += 1;
        if sent.is_multiple_of(50) {
            let peak = packet
                .as_chunks::<2>()
                .0
                .iter()
                .map(|s| i16::from_le_bytes(*s).unsigned_abs())
                .max();
            tracing::trace!("{sent} microphone packets sent, the last peaking at {peak:?}");
        }
        sink(packet.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn training_is_confirmed_with_the_servers_size() {
        // GNOME Remote Desktop's: 8 bytes of header, then 1024 of data,
        // and 1024 in wPackSize.
        let mut training = vec![0x06, 0, 0x04, 0x04, 0x34, 0x12, 0x00, 0x04];
        training.resize(1032, 0);
        let Some(ClientAudioOutputPdu::TrainingConfirm(confirm)) = training_confirm(&training)
        else {
            panic!("not taken for training");
        };
        assert_eq!((confirm.timestamp, confirm.pack_size), (0x1234, 1024));
        // Wave data that starts with the same byte is left alone.
        assert!(training_confirm(&[0x06, 1, 2, 3, 4, 5, 6, 7, 8]).is_none());
    }
}
