//! Opus decoding through libopus, loaded at run time: GNOME Remote Desktop
//! sends a twelfth of the bandwidth with it than with PCM. Distributions
//! and the Flatpak's runtime carry the library, and so does the macOS app.

use std::ffi::{c_int, c_void};
use std::sync::OnceLock;

use libloading::Library;

/// The longest packet Opus makes: 120 ms at 48 kHz.
const MAX_FRAMES: usize = 5760;

/// Where libopus is: by its name on Linux; on macOS in the app's
/// Frameworks (the binary's @rpath), or Homebrew's for a bare binary.
const LIBRARIES: &[&str] = if cfg!(target_os = "macos") {
    &[
        "@rpath/libopus.0.dylib",
        "/opt/homebrew/lib/libopus.0.dylib",
    ]
} else {
    &["libopus.so.0"]
};

struct Api {
    create: unsafe extern "C" fn(i32, c_int, *mut c_int) -> *mut c_void,
    decode: unsafe extern "C" fn(*mut c_void, *const u8, i32, *mut i16, c_int, c_int) -> c_int,
    destroy: unsafe extern "C" fn(*mut c_void),
    // Kept loaded for as long as the functions above are used.
    _library: Library,
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let mut loaded = Err(String::new());
        for name in LIBRARIES {
            // SAFETY: the library by that name is libopus.
            loaded = unsafe {
                Library::new(name).and_then(|library| {
                    Ok(Api {
                        create: *library.get(b"opus_decoder_create\0")?,
                        decode: *library.get(b"opus_decode\0")?,
                        destroy: *library.get(b"opus_decoder_destroy\0")?,
                        _library: library,
                    })
                })
            }
            .map_err(|e| e.to_string());
            if loaded.is_ok() {
                break;
            }
        }
        match loaded {
            Ok(api) => Some(api),
            Err(e) => {
                tracing::info!("sound as PCM only: libopus: {e}");
                None
            }
        }
    })
    .as_ref()
}

pub(super) fn available() -> bool {
    api().is_some()
}

pub(super) struct Decoder {
    api: &'static Api,
    handle: *mut c_void,
    channels: usize,
}

// SAFETY: a decoder is used by one thread at a time.
unsafe impl Send for Decoder {}

impl Decoder {
    pub(super) fn new(rate: u32, channels: u8) -> Option<Self> {
        let api = api()?;
        let mut error = 0;
        // SAFETY: plain values and a valid pointer for the error code.
        let handle = unsafe { (api.create)(rate as i32, c_int::from(channels), &mut error) };
        if handle.is_null() || error != 0 {
            tracing::warn!("opus_decoder_create failed: {error}");
            return None;
        }
        Some(Self {
            api,
            handle,
            channels: usize::from(channels),
        })
    }

    /// One packet as interleaved 16-bit samples, little-endian bytes.
    pub(super) fn decode(&mut self, packet: &[u8]) -> Option<Vec<u8>> {
        let mut samples = vec![0i16; MAX_FRAMES * self.channels];
        // SAFETY: the decoder is open; the packet and the sample buffer are
        // valid for the lengths given, the buffer for MAX_FRAMES frames.
        let frames = unsafe {
            (self.api.decode)(
                self.handle,
                packet.as_ptr(),
                packet.len() as i32,
                samples.as_mut_ptr(),
                MAX_FRAMES as c_int,
                0,
            )
        };
        if frames < 0 {
            tracing::debug!("an Opus packet didn't decode: {frames}");
            return None;
        }
        samples.truncate(frames as usize * self.channels);
        Some(samples.iter().flat_map(|s| s.to_le_bytes()).collect())
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: the decoder is open, and not used again.
        unsafe { (self.api.destroy)(self.handle) };
    }
}
