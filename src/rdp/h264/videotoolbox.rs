//! H.264 through VideoToolbox, the Mac's own decoder, on its media engine:
//! every Mac has it, so the macOS app needs no other.
//!
//! VideoToolbox takes length-prefixed NAL units with the parameter sets
//! given apart, as MP4 carries them, where the Graphics Pipeline sends an
//! Annex B stream with them inline; each picture is taken apart here. A new
//! sequence or picture parameter set starts a new decoding session. Pictures
//! are decoded one at a time and come back before the call returns, so each
//! is the one its data carried, as AVC444 needs, and their planes are
//! handed over as they are, for the Graphics Pipeline to convert.

use std::ffi::c_void;
use std::sync::Mutex;

use core_foundation::array::CFArray;
use core_foundation::base::TCFType;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use ironrdp_egfx::decode::{DecodedFrame, DecoderError, DecoderResult, H264Decoder, Yuv420Frame};

type OSStatus = i32;
type Ref = *mut c_void;

/// CMTime, which the output callback is passed by value.
#[repr(C)]
#[derive(Clone, Copy)]
struct Time {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

type Output = extern "C" fn(Ref, Ref, OSStatus, u32, Ref, Time, Time);

/// VTDecompressionOutputCallbackRecord.
#[repr(C)]
struct Callback {
    output: Output,
    refcon: *mut c_void,
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTDecompressionSessionCreate(
        allocator: Ref,
        format: Ref,
        decoder: Ref,
        attributes: Ref,
        callback: *const Callback,
        session: *mut Ref,
    ) -> OSStatus;
    fn VTDecompressionSessionDecodeFrame(
        session: Ref,
        sample: Ref,
        flags: u32,
        refcon: *mut c_void,
        info: *mut u32,
    ) -> OSStatus;
    fn VTDecompressionSessionWaitForAsynchronousFrames(session: Ref) -> OSStatus;
    fn VTDecompressionSessionInvalidate(session: Ref);
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
        allocator: Ref,
        count: usize,
        sets: *const *const u8,
        sizes: *const usize,
        length_size: i32,
        format: *mut Ref,
    ) -> OSStatus;
    fn CMBlockBufferCreateWithMemoryBlock(
        allocator: Ref,
        memory: *mut c_void,
        length: usize,
        block_allocator: Ref,
        source: *const c_void,
        offset: usize,
        data_length: usize,
        flags: u32,
        buffer: *mut Ref,
    ) -> OSStatus;
    fn CMBlockBufferReplaceDataBytes(
        source: *const c_void,
        buffer: Ref,
        offset: usize,
        length: usize,
    ) -> OSStatus;
    fn CMSampleBufferCreateReady(
        allocator: Ref,
        data: Ref,
        format: Ref,
        samples: isize,
        timings: isize,
        timing: *const c_void,
        sizes: isize,
        size: *const usize,
        sample: *mut Ref,
    ) -> OSStatus;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: CFStringRef;
    fn CVPixelBufferLockBaseAddress(buffer: Ref, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: Ref, flags: u64) -> i32;
    fn CVPixelBufferGetPixelFormatType(buffer: Ref) -> u32;
    fn CVPixelBufferGetWidth(buffer: Ref) -> usize;
    fn CVPixelBufferGetHeight(buffer: Ref) -> usize;
    fn CVPixelBufferGetPlaneCount(buffer: Ref) -> usize;
    fn CVPixelBufferGetBaseAddressOfPlane(buffer: Ref, plane: usize) -> *const u8;
    fn CVPixelBufferGetBytesPerRowOfPlane(buffer: Ref, plane: usize) -> usize;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: Ref);
}

/// Two-plane 4:2:0, luma then interleaved chroma: in video range and in
/// full range. Asking for both lets VideoToolbox keep the stream's own, so
/// the samples arrive as encoded, without a range conversion.
const NV12_VIDEO: u32 = u32::from_be_bytes(*b"420v");
const NV12_FULL: u32 = u32::from_be_bytes(*b"420f");
const READ_ONLY: u64 = 1;
const ASSURE_MEMORY_NOW: u32 = 1;
/// kVTInvalidSessionErr: the session is gone, as after the Mac sleeps.
const INVALID_SESSION: OSStatus = -12903;

const SPS: u8 = 7;
const PPS: u8 = 8;
const ACCESS_UNIT_DELIMITER: u8 = 9;

/// What the output callback leaves for the call that decoded.
type Slot = Mutex<Option<Result<Yuv420Frame, String>>>;

pub(super) struct VideoToolbox {
    session: Option<Session>,
    sps: Vec<u8>,
    pps: Vec<u8>,
    annex_b: Vec<u8>,
    /// The picture's slices, each after its length.
    sample: Vec<u8>,
    /// Boxed so the callback's pointer to it holds while this moves.
    slot: Box<Slot>,
}

impl VideoToolbox {
    pub(super) fn new() -> Self {
        Self {
            session: None,
            sps: Vec::new(),
            pps: Vec::new(),
            annex_b: Vec::new(),
            sample: Vec::new(),
            slot: Box::default(),
        }
    }

    /// One picture through the session, made first if there is none.
    fn decode_sample(&mut self) -> Result<Yuv420Frame, String> {
        if self.session.is_none() {
            if self.sps.is_empty() || self.pps.is_empty() {
                return Err("a picture before its parameter sets".into());
            }
            self.session = Some(Session::new(&self.sps, &self.pps, &self.slot)?);
        }
        let Some(session) = &self.session else {
            unreachable!("made above");
        };
        let sample = Sample::new(&self.sample, session.format)?;
        *lock(&self.slot) = None;
        let mut info = 0;
        // SAFETY: the session and the sample are alive; without the
        // asynchronous flag the callback runs before this returns.
        let status = unsafe {
            VTDecompressionSessionDecodeFrame(
                session.session,
                sample.0,
                0,
                std::ptr::null_mut(),
                &mut info,
            )
        };
        if status == INVALID_SESSION {
            self.session = None;
            return Err("the decoding session ended; it starts again".into());
        }
        check(status, "decoding")?;
        // SAFETY: the session is alive.
        unsafe { VTDecompressionSessionWaitForAsynchronousFrames(session.session) };
        lock(&self.slot)
            .take()
            .unwrap_or_else(|| Err("no picture".into()))
    }
}

impl H264Decoder for VideoToolbox {
    fn decode(&mut self, data: &[u8]) -> DecoderResult<DecodedFrame> {
        Ok(self.decode_yuv420(data)?.to_rgba())
    }

    fn decode_yuv420(&mut self, data: &[u8]) -> DecoderResult<Yuv420Frame> {
        let stream = if ironrdp_egfx::pdu::is_avc_format(data) {
            ironrdp_egfx::pdu::avc_to_annex_b_into(data, &mut self.annex_b);
            std::mem::take(&mut self.annex_b)
        } else {
            data.to_vec()
        };
        self.sample.clear();
        for nal in nal_units(&stream) {
            match nal[0] & 0x1f {
                SPS if nal != self.sps.as_slice() => {
                    self.sps = nal.to_vec();
                    self.session = None;
                }
                PPS if nal != self.pps.as_slice() => {
                    self.pps = nal.to_vec();
                    self.session = None;
                }
                SPS | PPS | ACCESS_UNIT_DELIMITER => {}
                _ => {
                    self.sample
                        .extend_from_slice(&(nal.len() as u32).to_be_bytes());
                    self.sample.extend_from_slice(nal);
                }
            }
        }
        self.annex_b = stream;
        if self.sample.is_empty() {
            return Err(DecoderError::msg("no picture in the data"));
        }
        self.decode_sample()
            .map_err(|e| DecoderError::msg(format!("VideoToolbox: {e}")))
    }

    fn supports_yuv420(&self) -> bool {
        true
    }

    fn reset(&mut self) {
        self.session = None;
        self.sps.clear();
        self.pps.clear();
    }
}

/// A decompression session and the format it was made for.
struct Session {
    session: Ref,
    format: Ref,
}

// SAFETY: VideoToolbox's sessions may be used from any thread, one call at
// a time, which `&mut self` keeps to.
unsafe impl Send for Session {}
unsafe impl Send for VideoToolbox {}

impl Session {
    fn new(sps: &[u8], pps: &[u8], slot: &Slot) -> Result<Self, String> {
        let sets = [sps.as_ptr(), pps.as_ptr()];
        let sizes = [sps.len(), pps.len()];
        let mut format = std::ptr::null_mut();
        // SAFETY: two parameter sets, valid for the call; four-byte lengths.
        check(
            unsafe {
                CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    std::ptr::null_mut(),
                    2,
                    sets.as_ptr(),
                    sizes.as_ptr(),
                    4,
                    &mut format,
                )
            },
            "reading the parameter sets",
        )?;
        // SAFETY: CoreVideo's constant key.
        let key = unsafe { CFString::wrap_under_get_rule(kCVPixelBufferPixelFormatTypeKey) };
        let formats = CFArray::from_CFTypes(&[
            CFNumber::from(NV12_VIDEO as i32),
            CFNumber::from(NV12_FULL as i32),
        ]);
        let attributes = CFDictionary::from_CFType_pairs(&[(key, formats.as_CFType())]);
        let callback = Callback {
            output: decoded,
            refcon: std::ptr::from_ref(slot).cast_mut().cast(),
        };
        let mut session = std::ptr::null_mut();
        // SAFETY: the format and attributes are alive for the call, and the
        // slot outlives the session (its owner drops the session first).
        let status = unsafe {
            VTDecompressionSessionCreate(
                std::ptr::null_mut(),
                format,
                std::ptr::null_mut(),
                attributes.as_concrete_TypeRef().cast_mut().cast(),
                &callback,
                &mut session,
            )
        };
        if let Err(e) = check(status, "starting the decoder") {
            // SAFETY: made above, not used again.
            unsafe { CFRelease(format) };
            return Err(e);
        }
        Ok(Self { session, format })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: both were made by `new` and are let go of once.
        unsafe {
            VTDecompressionSessionInvalidate(self.session);
            CFRelease(self.session);
            CFRelease(self.format);
        }
    }
}

/// One picture's slices as a sample buffer.
struct Sample(Ref);

impl Sample {
    fn new(data: &[u8], format: Ref) -> Result<Self, String> {
        let mut block = std::ptr::null_mut();
        // SAFETY: a block of the data's size, allocated by CoreMedia, then
        // filled with a copy of it.
        unsafe {
            check(
                CMBlockBufferCreateWithMemoryBlock(
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    data.len(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    0,
                    data.len(),
                    ASSURE_MEMORY_NOW,
                    &mut block,
                ),
                "making a block buffer",
            )?;
            if let Err(e) = check(
                CMBlockBufferReplaceDataBytes(data.as_ptr().cast(), block, 0, data.len()),
                "filling a block buffer",
            ) {
                CFRelease(block);
                return Err(e);
            }
        }
        let size = data.len();
        let mut sample = std::ptr::null_mut();
        // SAFETY: one sample of `size` bytes, without timing.
        let status = unsafe {
            CMSampleBufferCreateReady(
                std::ptr::null_mut(),
                block,
                format,
                1,
                0,
                std::ptr::null(),
                1,
                &size,
                &mut sample,
            )
        };
        // SAFETY: the sample holds its own reference to the block.
        unsafe { CFRelease(block) };
        check(status, "making a sample buffer")?;
        Ok(Self(sample))
    }
}

impl Drop for Sample {
    fn drop(&mut self) {
        // SAFETY: made by `new`, let go of once.
        unsafe { CFRelease(self.0) };
    }
}

/// The session's output: the picture's planes, copied out, into the slot.
extern "C" fn decoded(refcon: Ref, _: Ref, status: OSStatus, _: u32, image: Ref, _: Time, _: Time) {
    // SAFETY: the refcon is the decoder's slot, which outlives the session.
    let slot = unsafe { &*refcon.cast::<Slot>() };
    let result = if status != 0 {
        Err(format!("the decoder said {status}"))
    } else if image.is_null() {
        Err("the decoder dropped the picture".into())
    } else {
        // SAFETY: the image is a pixel buffer, alive for the callback.
        unsafe { planes(image) }
    };
    *lock(slot) = Some(result);
}

/// SAFETY: `image` must be a live CVPixelBuffer.
unsafe fn planes(image: Ref) -> Result<Yuv420Frame, String> {
    // SAFETY: the caller's.
    unsafe {
        let format = CVPixelBufferGetPixelFormatType(image);
        if !matches!(format, NV12_VIDEO | NV12_FULL) || CVPixelBufferGetPlaneCount(image) != 2 {
            return Err(format!("a picture in format {format:#x}"));
        }
        if CVPixelBufferLockBaseAddress(image, READ_ONLY) != 0 {
            return Err("the picture could not be read".into());
        }
        let width = CVPixelBufferGetWidth(image);
        let height = CVPixelBufferGetHeight(image);
        let luma = CVPixelBufferGetBaseAddressOfPlane(image, 0);
        let luma_stride = CVPixelBufferGetBytesPerRowOfPlane(image, 0);
        let chroma = CVPixelBufferGetBaseAddressOfPlane(image, 1);
        let chroma_stride = CVPixelBufferGetBytesPerRowOfPlane(image, 1);
        let picture = if luma.is_null() || chroma.is_null() {
            Err("the picture had no planes".into())
        } else {
            let luma = std::slice::from_raw_parts(luma, luma_stride * height);
            let chroma = std::slice::from_raw_parts(chroma, chroma_stride * height.div_ceil(2));
            split(width, height, luma, luma_stride, chroma, chroma_stride)
        };
        CVPixelBufferUnlockBaseAddress(image, READ_ONLY);
        picture
    }
}

/// Planar 4:2:0 from luma and interleaved chroma.
fn split(
    width: usize,
    height: usize,
    luma: &[u8],
    luma_stride: usize,
    chroma: &[u8],
    chroma_stride: usize,
) -> Result<Yuv420Frame, String> {
    let (Ok(w), Ok(h)) = (u16::try_from(width), u16::try_from(height)) else {
        return Err("a picture larger than 65535 pixels".into());
    };
    let chroma_width = width.div_ceil(2);
    let mut u = Vec::with_capacity(chroma_width * height.div_ceil(2));
    let mut v = Vec::with_capacity(u.capacity());
    for row in chroma.chunks(chroma_stride) {
        for [cb, cr] in row[..chroma_width * 2].as_chunks::<2>().0 {
            u.push(*cb);
            v.push(*cr);
        }
    }
    Yuv420Frame::new(w, h, luma.to_vec(), luma_stride, u, v, chroma_width)
        .map_err(|e| e.to_string())
}

/// The NAL units of an Annex B stream, without their start codes.
fn nal_units(stream: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= stream.len() {
        if stream[i] == 0 && stream[i + 1] == 0 && stream[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let ends: Vec<usize> = starts
        .iter()
        .skip(1)
        .map(|&next| {
            // The zero before a four-byte start code isn't the unit's.
            let mut end = next - 3;
            while end > 0 && stream[end - 1] == 0 {
                end -= 1;
            }
            end
        })
        .chain(std::iter::once(stream.len()))
        .collect();
    starts
        .into_iter()
        .zip(ends)
        .filter(|(start, end)| end > start)
        .map(move |(start, end)| &stream[start..end])
}

fn lock(slot: &Slot) -> std::sync::MutexGuard<'_, Option<Result<Yuv420Frame, String>>> {
    slot.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn check(status: OSStatus, doing: &str) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!("{doing}: {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three pictures of one color, 66 by 50 (cropped from 80 by 64), from
    /// x264: the parameter sets, then a slice for each.
    const SOLID: &[u8] = include_bytes!("testdata/solid-66x50.h264");

    /// The stream as the server sends it, a picture at a time: whatever
    /// comes before each slice, and the slice.
    fn pictures() -> Vec<Vec<u8>> {
        let mut pictures = Vec::new();
        let mut picture = Vec::new();
        for nal in nal_units(SOLID) {
            picture.extend_from_slice(&[0, 0, 0, 1]);
            picture.extend_from_slice(nal);
            if matches!(nal[0] & 0x1f, 1 | 5) {
                pictures.push(std::mem::take(&mut picture));
            }
        }
        pictures
    }

    #[test]
    fn finds_the_nal_units() {
        let units: Vec<u8> = nal_units(SOLID).map(|n| n[0] & 0x1f).collect();
        assert_eq!(units.iter().filter(|&&t| t == SPS).count(), 1);
        assert_eq!(units.iter().filter(|&&t| t == PPS).count(), 1);
        assert_eq!(units.iter().filter(|&&t| matches!(t, 1 | 5)).count(), 3);
    }

    #[test]
    fn decodes_each_picture_as_it_was_encoded() {
        let mut decoder = VideoToolbox::new();
        let pictures = pictures();
        assert_eq!(pictures.len(), 3);
        for data in &pictures {
            let frame = decoder.decode_yuv420(data).expect("decodes");
            assert_eq!((frame.width(), frame.height()), (66, 50));
            // ffmpeg decodes the same stream to Y 100, U 180, V 98.
            let first = |row: Option<&[u8]>| row.expect("a row")[0];
            let (y, u, v) = (
                first(frame.luma_row(0)),
                first(frame.u_row(0)),
                first(frame.v_row(0)),
            );
            assert!(y.abs_diff(100) <= 1 && u.abs_diff(180) <= 1 && v.abs_diff(98) <= 1);
        }
    }
}
