//! The desktop and the work of decoding into it that needs no more bytes
//! from the server: inflating, ZRLE's tiles, Tight's filters and JPEG.
//! The pixel format asked for puts each pixel in memory as B, G, R and a
//! spare byte, which is BGRX, what the frames and GDK take as they are.

use flate2::{Decompress, FlushDecompress, Status};
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

/// What went wrong decoding, as the engine logs it.
pub(super) type Bad = String;

/// The remote desktop as this side has it, in BGRX.
pub(super) struct Desktop {
    pub(super) width: u16,
    pub(super) height: u16,
    pub(super) pixels: Vec<u8>,
}

impl Desktop {
    pub(super) fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; usize::from(width) * usize::from(height) * 4],
        }
    }

    pub(super) fn stride(&self) -> usize {
        usize::from(self.width) * 4
    }

    /// Whether a rectangle lies wholly on the desktop.
    pub(super) fn holds(&self, x: u16, y: u16, width: u16, height: u16) -> bool {
        u32::from(x) + u32::from(width) <= u32::from(self.width)
            && u32::from(y) + u32::from(height) <= u32::from(self.height)
    }

    /// Put a rectangle's pixels, `width * 4` bytes to a row, in place.
    pub(super) fn put(&mut self, x: u16, y: u16, width: u16, rect: &[u8]) {
        let stride = self.stride();
        let row = usize::from(width) * 4;
        if row == 0 {
            return;
        }
        for (i, line) in rect.chunks_exact(row).enumerate() {
            let at = (usize::from(y) + i) * stride + usize::from(x) * 4;
            self.pixels[at..at + row].copy_from_slice(line);
        }
    }

    /// Copy a rectangle from one place on the desktop to another. The two
    /// may overlap: rows go in the order that reads each before it is
    /// written over.
    pub(super) fn copy(&mut self, from: (u16, u16), to: (u16, u16), width: u16, height: u16) {
        let stride = self.stride();
        let row = usize::from(width) * 4;
        let line = |x: u16, y: u16, i: usize| (usize::from(y) + i) * stride + usize::from(x) * 4;
        let mut each = |i: usize| {
            let source = line(from.0, from.1, i);
            self.pixels
                .copy_within(source..source + row, line(to.0, to.1, i));
        };
        if from.1 < to.1 {
            (0..usize::from(height)).rev().for_each(&mut each);
        } else {
            (0..usize::from(height)).for_each(&mut each);
        }
    }
}

/// A rectangle of one color.
pub(super) fn solid(pixel: [u8; 4], width: u16, height: u16) -> Vec<u8> {
    pixel.repeat(usize::from(width) * usize::from(height))
}

/// A pixel from three bytes in B, G, R order: ZRLE's compressed pixel.
fn bgr(bytes: &[u8; 3]) -> [u8; 4] {
    [bytes[0], bytes[1], bytes[2], 0xFF]
}

/// A pixel from three bytes in R, G, B order: Tight's.
pub(super) fn rgb(bytes: &[u8; 3]) -> [u8; 4] {
    [bytes[2], bytes[1], bytes[0], 0xFF]
}

/// Inflate the next part of a zlib stream that runs through the whole
/// connection: all of `input`, into at most `limit` bytes.
pub(super) fn inflate(stream: &mut Decompress, input: &[u8], limit: usize) -> Result<Vec<u8>, Bad> {
    let start = stream.total_in();
    let taken = |stream: &Decompress| (stream.total_in() - start) as usize;
    let mut out = Vec::with_capacity(limit.min(input.len().saturating_mul(4) + 4096));
    loop {
        if out.len() == out.capacity() {
            if out.len() >= limit {
                // Full: right only if nothing is left to inflate.
                return if taken(stream) == input.len() {
                    Ok(out)
                } else {
                    Err(format!("more than {limit} bytes inflated"))
                };
            }
            out.reserve((limit - out.len()).min(out.len().max(4096)));
        }
        let (before_in, before_out) = (taken(stream), out.len());
        let status = stream
            .decompress_vec(&input[before_in..], &mut out, FlushDecompress::Sync)
            .map_err(|e| format!("inflating: {e}"))?;
        // Room left over means zlib wanted more input, and there is none.
        if status == Status::StreamEnd
            || (taken(stream) == input.len() && out.len() < out.capacity())
        {
            return Ok(out);
        }
        if out.len() == before_out && taken(stream) == before_in {
            return Err("inflating went nowhere".into());
        }
    }
}

/// ZRLE's tiles, inflated, into a rectangle's BGRX pixels.
pub(super) fn zrle(data: &[u8], width: u16, height: u16) -> Result<Vec<u8>, Bad> {
    const TILE: u16 = 64;
    let (w, h) = (usize::from(width), usize::from(height));
    let mut out = vec![0; w * h * 4];
    let mut data = Bytes(data);
    for ty in (0..height).step_by(TILE.into()) {
        for tx in (0..width).step_by(TILE.into()) {
            let (tw, th) = (
                usize::from(TILE.min(width - tx)),
                usize::from(TILE.min(height - ty)),
            );
            let tile = zrle_tile(&mut data, tw, th)?;
            for (row, line) in tile.chunks_exact(tw * 4).enumerate() {
                let at = ((usize::from(ty) + row) * w + usize::from(tx)) * 4;
                out[at..at + tw * 4].copy_from_slice(line);
            }
        }
    }
    Ok(out)
}

fn zrle_tile(data: &mut Bytes<'_>, width: usize, height: usize) -> Result<Vec<u8>, Bad> {
    let pixels = width * height;
    let kind = data.byte()?;
    let palette = |data: &mut Bytes<'_>, size: usize| -> Result<Vec<[u8; 4]>, Bad> {
        (0..size).map(|_| Ok(bgr(data.pixel()?))).collect()
    };
    Ok(match kind {
        // Raw.
        0 => data
            .take(pixels * 3)?
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(bgr)
            .collect(),
        1 => solid(bgr(data.pixel()?), width as u16, height as u16),
        // A palette, and packed indices: rows start on a byte.
        2..=16 => {
            let palette = palette(data, usize::from(kind))?;
            let bits = match kind {
                2 => 1,
                3 | 4 => 2,
                _ => 4,
            };
            let row = (width * bits).div_ceil(8);
            let packed = data.take(row * height)?;
            let mut out = Vec::with_capacity(pixels * 4);
            for line in packed.chunks_exact(row) {
                for x in 0..width {
                    let bit = x * bits;
                    let index = (line[bit / 8] >> (8 - bits - bit % 8)) & ((1 << bits) - 1);
                    let pixel = palette
                        .get(usize::from(index))
                        .ok_or("a palette index past its end")?;
                    out.extend_from_slice(pixel);
                }
            }
            out
        }
        // Runs of one pixel each.
        128 => {
            let mut out = Vec::with_capacity(pixels * 4);
            while out.len() < pixels * 4 {
                let pixel = bgr(data.pixel()?);
                let run = data.run()?;
                push_run(&mut out, pixel, run, pixels)?;
            }
            out
        }
        // Runs of palette entries: an index alone is a run of one.
        130.. => {
            let palette = palette(data, usize::from(kind - 128))?;
            let mut out = Vec::with_capacity(pixels * 4);
            while out.len() < pixels * 4 {
                let index = data.byte()?;
                let pixel = *palette
                    .get(usize::from(index & 0x7F))
                    .ok_or("a palette index past its end")?;
                let run = if index & 0x80 != 0 { data.run()? } else { 1 };
                push_run(&mut out, pixel, run, pixels)?;
            }
            out
        }
        other => return Err(format!("ZRLE subencoding {other}")),
    })
}

fn push_run(out: &mut Vec<u8>, pixel: [u8; 4], run: usize, pixels: usize) -> Result<(), Bad> {
    if out.len() / 4 + run > pixels {
        return Err("a run past the end of its tile".into());
    }
    for _ in 0..run {
        out.extend_from_slice(&pixel);
    }
    Ok(())
}

/// Bytes read in order, failing at their end.
struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], Bad> {
        if self.0.len() < count {
            return Err("data ended early".into());
        }
        let (taken, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(taken)
    }

    fn pixel(&mut self) -> Result<&'a [u8; 3], Bad> {
        Ok(self.take(3)?.try_into().expect("3 bytes"))
    }

    fn byte(&mut self) -> Result<u8, Bad> {
        Ok(self.take(1)?[0])
    }

    /// A run length: bytes of 255 and a last one below it, added up, plus
    /// one.
    fn run(&mut self) -> Result<usize, Bad> {
        let mut run = 1;
        loop {
            let byte = self.byte()?;
            run += usize::from(byte);
            if byte != 255 {
                return Ok(run);
            }
        }
    }
}

/// How the bytes of a Tight rectangle sent with BasicCompression stand
/// for its pixels.
pub(super) enum Filter {
    Copy,
    Palette(Vec<[u8; 4]>),
    Gradient,
}

impl Filter {
    /// How many bytes the rectangle's pixels take before compression.
    pub(super) fn size(&self, width: u16, height: u16) -> usize {
        let (w, h) = (usize::from(width), usize::from(height));
        match self {
            Self::Palette(colors) if colors.len() == 2 => w.div_ceil(8) * h,
            Self::Palette(_) => w * h,
            Self::Copy | Self::Gradient => w * h * 3,
        }
    }

    /// The rectangle's BGRX pixels from those bytes.
    pub(super) fn pixels(&self, data: &[u8], width: u16, height: u16) -> Result<Vec<u8>, Bad> {
        let (w, h) = (usize::from(width), usize::from(height));
        if data.len() != self.size(width, height) {
            return Err(format!("{} bytes of Tight pixels", data.len()));
        }
        Ok(match self {
            Self::Copy => data.as_chunks::<3>().0.iter().flat_map(rgb).collect(),
            Self::Palette(colors) if colors.len() == 2 => {
                let row = w.div_ceil(8);
                let mut out = Vec::with_capacity(w * h * 4);
                for line in data.chunks_exact(row) {
                    for x in 0..w {
                        let bit = (line[x / 8] >> (7 - x % 8)) & 1;
                        out.extend_from_slice(&colors[usize::from(bit)]);
                    }
                }
                out
            }
            Self::Palette(colors) => {
                let mut out = Vec::with_capacity(w * h * 4);
                for &index in data {
                    let pixel = colors
                        .get(usize::from(index))
                        .ok_or("a palette index past its end")?;
                    out.extend_from_slice(pixel);
                }
                out
            }
            Self::Gradient => gradient(data, w),
        })
    }
}

/// Undo Tight's gradient filter: each component was sent as its difference
/// from a prediction made from the pixels to its left and above.
fn gradient(data: &[u8], width: usize) -> Vec<u8> {
    let row = width * 3;
    let mut values = vec![0u8; data.len()];
    for y in 0..data.len() / row.max(1) {
        for x in 0..width {
            for c in 0..3 {
                let at = y * row + x * 3 + c;
                let left = if x > 0 { i32::from(values[at - 3]) } else { 0 };
                let up = if y > 0 {
                    i32::from(values[at - row])
                } else {
                    0
                };
                let corner = if x > 0 && y > 0 {
                    i32::from(values[at - row - 3])
                } else {
                    0
                };
                let predicted = (left + up - corner).clamp(0, 255) as u8;
                values[at] = predicted.wrapping_add(data[at]);
            }
        }
    }
    values.as_chunks::<3>().0.iter().flat_map(rgb).collect()
}

/// A JPEG picture as BGRX, if it is the size it should be.
pub(super) fn jpeg(data: &[u8], width: u16, height: u16) -> Result<Vec<u8>, Bad> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::BGRA);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(data), options);
    let pixels = decoder.decode().map_err(|e| format!("JPEG: {e:?}"))?;
    match decoder.dimensions() {
        Some((w, h)) if w == usize::from(width) && h == usize::from(height) => Ok(pixels),
        other => Err(format!("a JPEG of {other:?} for {width}x{height}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [u8; 4] = [0, 0, 0xFF, 0xFF];
    const BLUE: [u8; 4] = [0xFF, 0, 0, 0xFF];

    #[test]
    fn a_copy_down_reads_each_row_before_writing_over_it() {
        let mut desktop = Desktop::new(1, 3);
        desktop.put(0, 0, 1, &[RED, BLUE, BLUE].concat());
        desktop.copy((0, 0), (0, 1), 1, 2);
        assert_eq!(desktop.pixels, [RED, RED, BLUE].concat());
    }

    #[test]
    fn reads_zrle_runs_and_palettes() {
        // A 3x1 tile of plain runs: red twice (a run byte of 1), then blue.
        let runs = [128, 0, 0, 0xFF, 1, 0xFF, 0, 0, 0];
        assert_eq!(zrle(&runs, 3, 1).unwrap(), [RED, RED, BLUE].concat());
        // A two-color palette, one bit per pixel: blue, red, blue.
        let packed = [2, 0xFF, 0, 0, 0, 0, 0xFF, 0b0100_0000];
        assert_eq!(zrle(&packed, 3, 1).unwrap(), [BLUE, RED, BLUE].concat());
        // A palette run past the end of the tile.
        assert!(zrle(&[130, 0, 0, 0, 0, 0, 0, 0x80, 9], 3, 1).is_err());
    }

    #[test]
    fn undoes_the_gradient_filter() {
        // 2x1: the first pixel is sent as itself, the second as its
        // difference from the first.
        let sent = [10, 20, 30, 5, 0, 251];
        assert_eq!(
            gradient(&sent, 2),
            [[30, 20, 10, 0xFF], [25, 20, 15, 0xFF]].concat()
        );
    }

    #[test]
    fn inflates_a_stream_in_parts() {
        use flate2::{Compress, Compression, FlushCompress};
        let mut compress = Compress::new(Compression::default(), true);
        let mut part = |input: &[u8]| {
            let mut out = Vec::with_capacity(1024);
            compress
                .compress_vec(input, &mut out, FlushCompress::Sync)
                .unwrap();
            out
        };
        let (first, second) = (part(b"hello "), part(b"world"));
        let mut stream = Decompress::new(true);
        assert_eq!(inflate(&mut stream, &first, 64).unwrap(), b"hello ");
        // Exactly as much as it may hold.
        assert_eq!(inflate(&mut stream, &second, 5).unwrap(), b"world");
        let third = part(b"too long");
        assert!(inflate(&mut stream, &third, 3).is_err());
    }
}
