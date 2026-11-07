//! Open H.264 (encoding 50), which wayvnc sends when it can encode on the
//! GPU, decoded by whichever H.264 decoder the system has, as RDP's are.
//! Each rectangle the server sends H.264 for has a decoder of its own,
//! found by the rectangle's place and size; the server says when to start
//! one afresh.

use ironrdp_egfx::decode::H264Decoder;

use super::decode::Bad;

/// A rectangle's place and size.
type Place = (u16, u16, u16, u16);

/// Of the decoders the server can ask for, the most kept at once.
const MOST: usize = 8;

pub(super) struct Contexts {
    contexts: Vec<(Place, Box<dyn H264Decoder>)>,
    /// A decoder made already, to show there is one; the first rectangle
    /// takes it.
    spare: Option<Box<dyn H264Decoder>>,
}

impl Contexts {
    /// Whether this system can decode H.264 at all, keeping the decoder it
    /// found for the first rectangle.
    pub(super) fn new() -> Option<Self> {
        let (source, spare) = crate::rdp::h264::decoder()?;
        tracing::info!("H.264 for VNC from {source:?}");
        Some(Self {
            contexts: Vec::new(),
            spare: Some(spare),
        })
    }

    pub(super) fn reset(&mut self, place: Place) {
        self.contexts.retain(|(p, _)| *p != place);
    }

    pub(super) fn reset_all(&mut self) {
        self.contexts.clear();
    }

    /// Decode `data` for the rectangle at `place`, into its BGRX pixels.
    pub(super) fn decode(&mut self, place: Place, data: &[u8]) -> Result<Vec<u8>, Bad> {
        let at = match self.contexts.iter().position(|(p, _)| *p == place) {
            Some(at) => at,
            None => {
                if self.contexts.len() >= MOST {
                    self.contexts.remove(0);
                }
                let decoder = match self.spare.take() {
                    Some(decoder) => decoder,
                    None => {
                        crate::rdp::h264::decoder()
                            .ok_or("no H.264 decoder for another rectangle")?
                            .1
                    }
                };
                self.contexts.push((place, decoder));
                self.contexts.len() - 1
            }
        };
        let frame = self.contexts[at]
            .1
            .decode(data)
            .map_err(|e| format!("H.264: {e:?}"))?;
        let (_, _, width, height) = place;
        let (w, h) = (usize::from(width), usize::from(height));
        let (frame_w, frame_h) = (frame.width() as usize, frame.height() as usize);
        if frame_w < w || frame_h < h {
            return Err(format!(
                "an H.264 picture of {frame_w}x{frame_h} for {w}x{h}"
            ));
        }
        // The picture can be larger, to whole macroblocks: only the
        // rectangle's part counts.
        let rgba = frame.data();
        let mut bgrx = Vec::with_capacity(w * h * 4);
        for row in rgba.chunks_exact(frame_w * 4).take(h) {
            for pixel in row[..w * 4].as_chunks::<4>().0 {
                bgrx.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 0xFF]);
            }
        }
        Ok(bgrx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes `MOTE_TEST_H264` (an Annex B stream of one picture) for a
    /// rectangle of `MOTE_TEST_H264_SIZE` (`<w>x<h>`) and compares it with
    /// `MOTE_TEST_H264_RGB`, the same picture as raw RGB: the picture's
    /// colors, their order and the cropping. Needs an H.264 decoder.
    #[test]
    #[ignore]
    fn decodes_a_picture_as_its_source_has_it() {
        let var = |name| std::env::var(name).expect(name);
        let (w, h) = var("MOTE_TEST_H264_SIZE")
            .split_once('x')
            .map(|(w, h)| (w.parse::<u16>().unwrap(), h.parse::<u16>().unwrap()))
            .unwrap();
        let stream = std::fs::read(var("MOTE_TEST_H264")).unwrap();
        let rgb = std::fs::read(var("MOTE_TEST_H264_RGB")).unwrap();
        let mut contexts = Contexts::new().expect("an H.264 decoder");
        let bgrx = contexts.decode((0, 0, w, h), &stream).unwrap();
        assert_eq!(bgrx.len(), usize::from(w) * usize::from(h) * 4);
        let worst = bgrx
            .as_chunks::<4>()
            .0
            .iter()
            .zip(rgb.as_chunks::<3>().0)
            .flat_map(|(ours, theirs)| {
                [
                    ours[2].abs_diff(theirs[0]),
                    ours[1].abs_diff(theirs[1]),
                    ours[0].abs_diff(theirs[2]),
                ]
            })
            .max()
            .unwrap();
        eprintln!("largest difference from the source: {worst}");
        assert!(worst < 48, "decoded colors differ by {worst}");
    }
}
