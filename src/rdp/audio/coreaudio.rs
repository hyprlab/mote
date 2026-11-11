//! Sound on macOS, through Audio Queue Services (AudioToolbox), with the
//! blocking calls pulse.rs gives on Linux, so the sound and microphone
//! threads in audio.rs run unchanged. A queue takes the server's 16-bit
//! samples at the server's rate and channel count and converts them for
//! whatever the Mac plays through or records from.
//!
//! Playing keeps a handful of short buffers: `write` fills one when the
//! queue gives one back, so it blocks once about the asked-for time is
//! queued, as PulseAudio's does. Recording queues buffers that the queue
//! fills; its callback moves what they hold where `read` waits for it.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

type OSStatus = i32;
type AudioQueueRef = *mut c_void;

/// AudioStreamBasicDescription.
#[repr(C)]
struct Format {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

/// AudioQueueBuffer.
#[repr(C)]
struct QueueBuffer {
    capacity: u32,
    data: *mut c_void,
    size: u32,
    user_data: *mut c_void,
    packet_description_capacity: u32,
    packet_descriptions: *mut c_void,
    packet_description_count: u32,
}

type OutputCallback = extern "C" fn(*mut c_void, AudioQueueRef, *mut QueueBuffer);
type InputCallback =
    extern "C" fn(*mut c_void, AudioQueueRef, *mut QueueBuffer, *const c_void, u32, *const c_void);

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioQueueNewOutput(
        format: *const Format,
        callback: OutputCallback,
        user_data: *mut c_void,
        run_loop: *const c_void,
        run_loop_mode: *const c_void,
        flags: u32,
        queue: *mut AudioQueueRef,
    ) -> OSStatus;
    fn AudioQueueNewInput(
        format: *const Format,
        callback: InputCallback,
        user_data: *mut c_void,
        run_loop: *const c_void,
        run_loop_mode: *const c_void,
        flags: u32,
        queue: *mut AudioQueueRef,
    ) -> OSStatus;
    fn AudioQueueAllocateBuffer(
        queue: AudioQueueRef,
        size: u32,
        buffer: *mut *mut QueueBuffer,
    ) -> OSStatus;
    fn AudioQueueEnqueueBuffer(
        queue: AudioQueueRef,
        buffer: *mut QueueBuffer,
        descriptions: u32,
        description: *const c_void,
    ) -> OSStatus;
    fn AudioQueueStart(queue: AudioQueueRef, start_time: *const c_void) -> OSStatus;
    fn AudioQueueDispose(queue: AudioQueueRef, immediate: u8) -> OSStatus;
}

/// 'lpcm', and signed, packed little-endian integers.
const LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
const SIGNED_PACKED: u32 = (1 << 2) | (1 << 3);
/// Each playing buffer holds this much: short enough that the queue's
/// fill tracks the asked-for time closely.
const PLAY_BUFFER: Duration = Duration::from_millis(10);
/// Buffers the microphone fills in turn.
const RECORD_BUFFERS: usize = 4;
/// Recorded sound kept for `read`, at most; older is dropped.
const RECORD_KEPT: Duration = Duration::from_secs(1);
/// How long `write` or `read` waits on a queue that has stopped moving
/// before giving up on it.
const STALLED: Duration = Duration::from_secs(2);

/// Whether sound can be played and recorded here at all: macOS always has
/// Audio Queue Services.
pub(super) fn available() -> bool {
    true
}

/// A buffer the queue hands back, between threads.
struct Buffer(*mut QueueBuffer);
// SAFETY: a buffer belongs to whichever side holds it; the queue's thread
// gives it back and the stream's thread fills it, never both at once.
unsafe impl Send for Buffer {}

/// What the playing queue's callback shares with `write`.
struct Playing {
    free: Mutex<Vec<Buffer>>,
    returned: Condvar,
    /// Bytes enqueued and not yet played.
    queued: AtomicU32,
}

/// What the recording queue's callback shares with `read`.
struct Recording {
    samples: Mutex<VecDeque<u8>>,
    arrived: Condvar,
    kept: usize,
}

enum Direction {
    Play {
        shared: Arc<Playing>,
        buffer: Duration,
        started: bool,
    },
    Record {
        shared: Arc<Recording>,
    },
}

/// One stream of 16-bit samples, to the speakers or from the microphone.
pub(super) struct Stream {
    queue: AudioQueueRef,
    rate: u32,
    channels: u8,
    direction: Direction,
}

// SAFETY: the queue is used by one thread at a time; Audio Queue Services
// may be called from any thread.
unsafe impl Send for Stream {}

impl Stream {
    /// A stream to the default speakers that keeps about `buffer` queued.
    pub(super) fn play(
        _name: &str,
        rate: u32,
        channels: u8,
        buffer: Duration,
    ) -> Result<Self, String> {
        let shared = Arc::new(Playing {
            free: Mutex::new(Vec::new()),
            returned: Condvar::new(),
            queued: AtomicU32::new(0),
        });
        let mut queue = std::ptr::null_mut();
        // SAFETY: the format is valid for the call, and `shared` outlives
        // the queue, which is disposed of before the stream lets go of it.
        check(unsafe {
            AudioQueueNewOutput(
                &format(rate, channels),
                played,
                Arc::as_ptr(&shared).cast_mut().cast(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut queue,
            )
        })?;
        let stream = Self {
            queue,
            rate,
            channels,
            direction: Direction::Play {
                shared: shared.clone(),
                buffer,
                started: false,
            },
        };
        let size = bytes_for(PLAY_BUFFER, rate, channels).max(2);
        let count = (buffer.as_millis() / PLAY_BUFFER.as_millis()).max(2) as usize;
        let mut free = lock(&shared.free);
        for _ in 0..count {
            free.push(Buffer(stream.allocate(size)?));
        }
        drop(free);
        Ok(stream)
    }

    /// A stream from the default microphone, read `fragment` at a time.
    pub(super) fn record(
        _name: &str,
        rate: u32,
        channels: u8,
        fragment: Duration,
    ) -> Result<Self, String> {
        let shared = Arc::new(Recording {
            samples: Mutex::new(VecDeque::new()),
            arrived: Condvar::new(),
            kept: bytes_for(RECORD_KEPT, rate, channels) as usize,
        });
        let mut queue = std::ptr::null_mut();
        // SAFETY: as in `play`.
        check(unsafe {
            AudioQueueNewInput(
                &format(rate, channels),
                recorded,
                Arc::as_ptr(&shared).cast_mut().cast(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut queue,
            )
        })?;
        let stream = Self {
            queue,
            rate,
            channels,
            direction: Direction::Record { shared },
        };
        let size = bytes_for(fragment, rate, channels).max(2);
        for _ in 0..RECORD_BUFFERS {
            let buffer = stream.allocate(size)?;
            // SAFETY: a buffer of this queue, empty, as recording wants.
            check(unsafe { AudioQueueEnqueueBuffer(queue, buffer, 0, std::ptr::null()) })?;
        }
        // SAFETY: the queue is open; null starts it now.
        check(unsafe { AudioQueueStart(queue, std::ptr::null()) })?;
        Ok(stream)
    }

    fn allocate(&self, size: u32) -> Result<*mut QueueBuffer, String> {
        let mut buffer = std::ptr::null_mut();
        // SAFETY: the queue is open.
        check(unsafe { AudioQueueAllocateBuffer(self.queue, size, &mut buffer) })?;
        Ok(buffer)
    }

    /// Queue samples; blocks while the buffer is full.
    pub(super) fn write(&mut self, data: &[u8]) -> Result<(), String> {
        let Direction::Play {
            shared, started, ..
        } = &mut self.direction
        else {
            return Err("not a stream to the speakers".into());
        };
        let mut rest = data;
        while !rest.is_empty() {
            let mut free = lock(&shared.free);
            while free.is_empty() {
                let (next, waited) = shared
                    .returned
                    .wait_timeout(free, STALLED)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                free = next;
                if waited.timed_out() && free.is_empty() {
                    return Err("the speakers stopped taking sound".into());
                }
            }
            let Some(Buffer(buffer)) = free.pop() else {
                continue;
            };
            drop(free);
            // SAFETY: the queue gave the buffer back, so it is ours to fill,
            // up to its capacity.
            let taken = unsafe {
                let capacity = (*buffer).capacity as usize;
                // Whole frames only.
                let frame = usize::from(self.channels) * 2;
                let taken = rest.len().min(capacity - capacity % frame);
                std::ptr::copy_nonoverlapping(rest.as_ptr(), (*buffer).data.cast(), taken);
                (*buffer).size = taken as u32;
                taken
            };
            shared.queued.fetch_add(taken as u32, Ordering::Relaxed);
            // SAFETY: a filled buffer of this queue.
            if let Err(e) =
                check(unsafe { AudioQueueEnqueueBuffer(self.queue, buffer, 0, std::ptr::null()) })
            {
                shared.queued.fetch_sub(taken as u32, Ordering::Relaxed);
                lock(&shared.free).push(Buffer(buffer));
                return Err(e);
            }
            rest = &rest[taken..];
        }
        if !*started {
            // SAFETY: the queue is open; null starts it now.
            check(unsafe { AudioQueueStart(self.queue, std::ptr::null()) })?;
            *started = true;
        }
        Ok(())
    }

    /// Fill `data` with recorded samples; blocks until there are enough.
    pub(super) fn read(&mut self, data: &mut [u8]) -> Result<(), String> {
        let Direction::Record { shared } = &self.direction else {
            return Err("not a stream from the microphone".into());
        };
        let mut samples = lock(&shared.samples);
        while samples.len() < data.len() {
            let (next, waited) = shared
                .arrived
                .wait_timeout(samples, STALLED)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            samples = next;
            if waited.timed_out() && samples.len() < data.len() {
                return Err("the microphone stopped sending sound".into());
            }
        }
        let wanted = data.len();
        for (to, from) in data.iter_mut().zip(samples.drain(..wanted)) {
            *to = from;
        }
        Ok(())
    }

    /// How long until what is queued now is heard.
    pub(super) fn latency(&mut self) -> Duration {
        let Direction::Play { shared, .. } = &self.direction else {
            return Duration::ZERO;
        };
        let bytes = u64::from(shared.queued.load(Ordering::Relaxed));
        let per_second = u64::from(self.rate) * u64::from(self.channels) * 2;
        Duration::from_micros(bytes * 1_000_000 / per_second.max(1))
    }

    /// Drop what is queued and not yet played: a new queue in place of
    /// this one, which Audio Queue Services makes simpler than taking
    /// back buffers already handed over.
    pub(super) fn flush(&mut self) {
        let Direction::Play { buffer, .. } = self.direction else {
            return;
        };
        match Self::play("", self.rate, self.channels, buffer) {
            Ok(fresh) => *self = fresh,
            Err(e) => tracing::warn!("starting the speakers again: {e}"),
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: the queue is open; disposing of it at once stops its
        // callbacks before `direction`, which they use, is dropped.
        unsafe { AudioQueueDispose(self.queue, 1) };
    }
}

/// The playing queue is done with a buffer: hand it back to `write`.
extern "C" fn played(user_data: *mut c_void, _: AudioQueueRef, buffer: *mut QueueBuffer) {
    // SAFETY: `user_data` is the stream's `Playing`, alive while its queue
    // is, and the buffer the queue's own.
    let (shared, size) = unsafe { (&*user_data.cast::<Playing>(), (*buffer).size) };
    shared.queued.fetch_sub(size, Ordering::Relaxed);
    lock(&shared.free).push(Buffer(buffer));
    shared.returned.notify_one();
}

/// The recording queue filled a buffer: keep what it holds for `read`,
/// and give it back to be filled again.
extern "C" fn recorded(
    user_data: *mut c_void,
    queue: AudioQueueRef,
    buffer: *mut QueueBuffer,
    _: *const c_void,
    _: u32,
    _: *const c_void,
) {
    // SAFETY: as in `played`; the buffer holds `size` recorded bytes.
    let shared = unsafe { &*user_data.cast::<Recording>() };
    let bytes =
        unsafe { std::slice::from_raw_parts((*buffer).data.cast::<u8>(), (*buffer).size as usize) };
    let mut samples = lock(&shared.samples);
    samples.extend(bytes);
    let over = samples.len().saturating_sub(shared.kept);
    samples.drain(..over);
    drop(samples);
    shared.arrived.notify_one();
    // SAFETY: the queue's own buffer, emptied, back to it. It fails only
    // as the queue is disposed of, when nothing more is wanted.
    unsafe { AudioQueueEnqueueBuffer(queue, buffer, 0, std::ptr::null()) };
}

fn format(rate: u32, channels: u8) -> Format {
    let frame = u32::from(channels) * 2;
    Format {
        sample_rate: f64::from(rate),
        format_id: LINEAR_PCM,
        format_flags: SIGNED_PACKED,
        bytes_per_packet: frame,
        frames_per_packet: 1,
        bytes_per_frame: frame,
        channels_per_frame: u32::from(channels),
        bits_per_channel: 16,
        reserved: 0,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// An OSStatus as an error, by its four characters when it has them.
fn check(status: OSStatus) -> Result<(), String> {
    if status == 0 {
        return Ok(());
    }
    let code = status.to_be_bytes();
    if code.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        Err(format!(
            "Audio Queue Services said '{}'",
            String::from_utf8_lossy(&code)
        ))
    } else {
        Err(format!("Audio Queue Services said {status}"))
    }
}

fn bytes_for(duration: Duration, rate: u32, channels: u8) -> u32 {
    let frames = u64::from(rate) * duration.as_millis() as u64 / 1000;
    (frames * u64::from(channels) * 2) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second of silence through the speakers: `write` keeps pace with
    /// what is heard, and what is queued stays near the time asked for.
    #[test]
    #[ignore = "plays through the Mac's speakers"]
    fn plays_in_real_time() {
        let (rate, channels) = (48_000, 2);
        let mut stream =
            Stream::play("test", rate, channels, Duration::from_millis(100)).expect("opens");
        let block = vec![0u8; bytes_for(Duration::from_millis(20), rate, channels) as usize];
        let start = std::time::Instant::now();
        let mut most = Duration::ZERO;
        for _ in 0..50 {
            stream.write(&block).expect("writes");
            most = most.max(stream.latency());
        }
        let took = start.elapsed();
        assert!(
            took >= Duration::from_millis(850),
            "wrote a second in {took:?}"
        );
        assert!(most <= Duration::from_millis(120), "queued {most:?}");
        stream.flush();
        stream.write(&block).expect("writes after a flush");
    }
}
