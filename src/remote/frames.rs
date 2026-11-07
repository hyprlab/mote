//! The pixels a frame hands to the interface, from a few buffers used in
//! turn. GDK lets a buffer go once the texture made from it is gone; it
//! comes back here, and only what changed since it was last filled is
//! copied into it again, rather than the whole desktop each frame.

use std::collections::VecDeque;
use std::sync::mpsc;

use super::Rect;

/// Frames of damage remembered, to bring a returning buffer up to date.
const HISTORY: usize = 8;
/// Buffers kept for reuse; more than this come back only when GDK holds
/// on to textures, and are let go.
const POOL: usize = 4;

/// A frame's pixels. Dropped, they go back to the session that made them.
pub(crate) struct Pixels {
    data: Vec<u8>,
    filled: u64,
    home: mpsc::Sender<(u64, Vec<u8>)>,
}

impl AsRef<[u8]> for Pixels {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl Drop for Pixels {
    fn drop(&mut self) {
        // A session that has ended doesn't take them back.
        let _ = self
            .home
            .send((self.filled, std::mem::take(&mut self.data)));
    }
}

pub(crate) struct Frames {
    /// Buffers back from the interface, each with the frame it holds.
    pool: Vec<(u64, Vec<u8>)>,
    home: mpsc::Sender<(u64, Vec<u8>)>,
    returned: mpsc::Receiver<(u64, Vec<u8>)>,
    /// The last frame made.
    made: u64,
    /// Each recent frame's damage, newest last.
    history: VecDeque<(u64, Vec<Rect>)>,
}

impl Frames {
    pub(crate) fn new() -> Self {
        let (home, returned) = mpsc::channel();
        Self {
            pool: Vec::new(),
            home,
            returned,
            made: 0,
            history: VecDeque::new(),
        }
    }

    /// The image as it is now, with `damage` the change since the last
    /// frame. A new desktop size makes a new `Frames`.
    pub(crate) fn make(&mut self, image: &[u8], stride: usize, damage: &[Rect]) -> Pixels {
        self.pool.extend(self.returned.try_iter());
        self.made += 1;
        self.history.push_back((self.made, damage.to_vec()));
        if self.history.len() > HISTORY {
            self.history.pop_front();
        }
        let data = match self.freshest(image.len()) {
            Some((filled, mut data)) => {
                self.update(&mut data, filled, image, stride);
                data
            }
            None => image.to_vec(),
        };
        // Older buffers would need more copying; keep only a few.
        self.pool
            .sort_unstable_by_key(|(filled, _)| std::cmp::Reverse(*filled));
        self.pool.truncate(POOL);
        Pixels {
            data,
            filled: self.made,
            home: self.home.clone(),
        }
    }

    /// The buffer of the right size holding the most recent frame.
    fn freshest(&mut self, len: usize) -> Option<(u64, Vec<u8>)> {
        self.pool.retain(|(_, data)| data.len() == len);
        let index = (0..self.pool.len()).max_by_key(|i| self.pool[*i].0)?;
        Some(self.pool.swap_remove(index))
    }

    /// Copy into `data`, which holds frame `filled`, what changed since.
    fn update(&self, data: &mut [u8], filled: u64, image: &[u8], stride: usize) {
        let oldest = self.history.front().map_or(u64::MAX, |(n, _)| *n);
        let rects: Vec<&Rect> = self
            .history
            .iter()
            .filter(|(n, _)| *n > filled)
            .flat_map(|(_, rects)| rects)
            .collect();
        let area: usize = rects
            .iter()
            .map(|r| r.width.max(0) as usize * r.height.max(0) as usize * 4)
            .sum();
        // Past what the history covers, or most of the desktop: all of it.
        if filled + 1 < oldest || area * 2 > image.len() {
            data.copy_from_slice(image);
            return;
        }
        let rows = image.len() / stride.max(1);
        for rect in rects {
            let x = rect.x.max(0) as usize * 4;
            let end = (x + rect.width.max(0) as usize * 4).min(stride);
            let top = (rect.y.max(0) as usize).min(rows);
            let bottom = (top + rect.height.max(0) as usize).min(rows);
            for row in top..bottom {
                let span = row * stride + x..row * stride + end;
                if span.start < span.end {
                    data[span.clone()].copy_from_slice(&image[span]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_returned_buffer_catches_up() {
        let mut frames = Frames::new();
        // Four pixels a row, two rows.
        let mut image = vec![0u8; 32];
        let first = frames.make(&image, 16, &[rect(0, 0, 4, 2)]);
        drop(first);
        image[4..8].copy_from_slice(&[1; 4]);
        let second = frames.make(&image, 16, &[rect(1, 0, 1, 1)]);
        assert_eq!(second.as_ref(), image.as_slice());
        // Kept by the interface: the next frame takes a new buffer, and the
        // one after catches up on both changes.
        image[20..24].copy_from_slice(&[2; 4]);
        let third = frames.make(&image, 16, &[rect(1, 1, 1, 1)]);
        assert_eq!(third.as_ref(), image.as_slice());
        drop(second);
        image[0..4].copy_from_slice(&[3; 4]);
        let fourth = frames.make(&image, 16, &[rect(0, 0, 1, 1)]);
        assert_eq!(fourth.as_ref(), image.as_slice());
        drop(third);
    }
}
