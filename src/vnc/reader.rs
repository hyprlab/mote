//! What the server sends, read as it comes: framebuffer updates decoded
//! into the desktop, and the rest passed on to the session as notes. It
//! runs as a task of its own so the session can send input and frames
//! while a large update is still arriving; it holds the desktop's lock
//! only to put a decoded rectangle in place, never across a read.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flate2::Decompress;
use tokio::io::{AsyncRead, AsyncReadExt, BufReader};
use tokio::sync::mpsc;

use super::decode::{self, Bad, Desktop, Filter};
use super::h264::Contexts;
use super::handshake::MAX_SIDE;
use crate::remote::{Ending, Failure, Pointer, Rect, describe};

/// The largest compressed rectangle taken: anything larger is more likely
/// damage than a picture.
const MAX_COMPRESSED: usize = 64 << 20;
/// The longest clipboard text taken, until section 16 shares it.
const MAX_CUT_TEXT: usize = 16 << 20;
/// The largest pointer shape taken.
const MAX_CURSOR: u16 = 1024;

pub(super) mod encoding {
    pub(in super::super) const RAW: i32 = 0;
    pub(in super::super) const COPY_RECT: i32 = 1;
    pub(in super::super) const HEXTILE: i32 = 5;
    pub(in super::super) const ZLIB: i32 = 6;
    pub(in super::super) const TIGHT: i32 = 7;
    pub(in super::super) const ZRLE: i32 = 16;
    pub(in super::super) const OPEN_H264: i32 = 50;
    pub(in super::super) const CURSOR: i32 = -239;
    pub(in super::super) const DESKTOP_SIZE: i32 = -223;
    pub(in super::super) const LAST_RECT: i32 = -224;
    pub(in super::super) const QEMU_KEYS: i32 = -258;
    pub(in super::super) const DESKTOP_NAME: i32 = -307;
    pub(in super::super) const EXTENDED_DESKTOP_SIZE: i32 = -308;
    pub(in super::super) const EXTENDED_MOUSE_BUTTONS: i32 = -316;
    pub(in super::super) const FENCE: i32 = -312;
    pub(in super::super) const CONTINUOUS_UPDATES: i32 = -313;
    /// Tight's JPEG quality, 0 to 9, as this plus the level.
    pub(in super::super) const JPEG_QUALITY_0: i32 = -32;
    /// The server's compression effort, 0 to 9, as this plus the level.
    pub(in super::super) const COMPRESS_LEVEL_0: i32 = -256;
}

/// A screen of the remote desktop, as ExtendedDesktopSize describes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Screen {
    pub(super) id: u32,
    pub(super) x: u16,
    pub(super) y: u16,
    pub(super) width: u16,
    pub(super) height: u16,
    pub(super) flags: u32,
}

pub(super) enum Note {
    /// An update has been read; these parts of the desktop changed, and
    /// decoding them took this long, waiting on the network left out.
    Updated(Vec<Rect>, Duration),
    /// The server's screens, which say it can change them: after a change
    /// this client asked for (`asked`), and with a status not 0 when the
    /// change failed.
    Layout {
        asked: bool,
        status: u16,
        screens: Vec<Screen>,
    },
    /// The server takes continuous updates, or has stopped sending them.
    ContinuousUpdates,
    /// The server asks for its fence to come back.
    Fence {
        flags: u32,
        payload: Vec<u8>,
    },
    /// The desktop has a new size, and holds nothing yet.
    Resized(u16, u16),
    /// The server's pointer; none for a hidden one.
    Cursor(Option<Pointer>),
    Bell,
    /// The server takes QEMU's extended key events.
    QemuKeys,
    /// The server takes the back and forward buttons as extended pointer
    /// events.
    ExtendedButtons,
    /// The server is gone, or sent what this side can't read.
    Ended(Result<Ending, Failure>),
}

/// Why reading stopped.
enum Stop {
    Io(std::io::Error),
    Bad(Bad),
    /// The session went away.
    Gone,
}

impl From<std::io::Error> for Stop {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<Bad> for Stop {
    fn from(bad: Bad) -> Self {
        Self::Bad(bad)
    }
}

pub(super) async fn run<R: AsyncRead + Unpin>(
    read: R,
    desktop: Arc<Mutex<Desktop>>,
    notes: mpsc::Sender<Note>,
    h264: Option<Contexts>,
) {
    let mut reader = Reader {
        read: BufReader::with_capacity(64 << 10, read),
        desktop,
        notes: notes.clone(),
        zlib: Decompress::new(true),
        zrle: Decompress::new(true),
        tight: std::array::from_fn(|_| Decompress::new(true)),
        h264,
        spent: Duration::ZERO,
    };
    let ending = match reader.serve().await {
        Stop::Gone => return,
        Stop::Io(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            // A VNC server ends a session by closing the connection.
            Ok(Ending::ServerClosed(None))
        }
        Stop::Io(e) => Err(Failure::Lost(describe(&e))),
        Stop::Bad(bad) => Err(Failure::Protocol(bad)),
    };
    let _ = notes.send(Note::Ended(ending)).await;
}

struct Reader<R> {
    read: BufReader<R>,
    desktop: Arc<Mutex<Desktop>>,
    notes: mpsc::Sender<Note>,
    /// The connection's zlib streams: one each for Zlib and ZRLE, four for
    /// Tight.
    zlib: Decompress,
    zrle: Decompress,
    tight: [Decompress; 4],
    /// H.264's decoders, when the system has one and it was asked for.
    h264: Option<Contexts>,
    /// Time spent decoding this update so far.
    spent: Duration,
}

impl<R: AsyncRead + Unpin> Reader<R> {
    async fn serve(&mut self) -> Stop {
        loop {
            if let Err(stop) = self.message().await {
                return stop;
            }
        }
    }

    async fn note(&mut self, note: Note) -> Result<(), Stop> {
        self.notes.send(note).await.map_err(|_| Stop::Gone)
    }

    async fn message(&mut self) -> Result<(), Stop> {
        match self.read.read_u8().await? {
            0 => self.update().await,
            // SetColourMapEntries: only for a palette, which isn't asked for.
            1 => {
                self.skip(1).await?;
                let _first = self.read.read_u16().await?;
                let count = self.read.read_u16().await?;
                self.skip(usize::from(count) * 6).await
            }
            2 => self.note(Note::Bell).await,
            // ServerCutText, shared once section 16 comes.
            3 => {
                self.skip(3).await?;
                let length = self.read.read_i32().await?.unsigned_abs() as usize;
                if length > MAX_CUT_TEXT {
                    return Err(Stop::Bad(format!("{length} bytes of clipboard")));
                }
                self.skip(length).await
            }
            // EndOfContinuousUpdates.
            150 => self.note(Note::ContinuousUpdates).await,
            // ServerFence.
            248 => {
                self.skip(3).await?;
                let flags = self.read.read_u32().await?;
                let length = self.read.read_u8().await?;
                if length > 64 {
                    return Err(Stop::Bad(format!("a fence of {length} bytes")));
                }
                let payload = self.bytes(length.into()).await?;
                self.note(Note::Fence { flags, payload }).await
            }
            other => Err(Stop::Bad(format!("message type {other}"))),
        }
    }

    async fn update(&mut self) -> Result<(), Stop> {
        self.skip(1).await?;
        let count = self.read.read_u16().await?;
        let mut damage = Vec::new();
        for _ in 0..count {
            let x = self.read.read_u16().await?;
            let y = self.read.read_u16().await?;
            let width = self.read.read_u16().await?;
            let height = self.read.read_u16().await?;
            let kind = self.read.read_i32().await?;
            if kind == encoding::LAST_RECT {
                break;
            }
            if self.rectangle(kind, x, y, width, height).await? {
                damage.push(Rect {
                    x: x.into(),
                    y: y.into(),
                    width: width.into(),
                    height: height.into(),
                });
            }
        }
        let spent = std::mem::take(&mut self.spent);
        self.note(Note::Updated(damage, spent)).await
    }

    /// Read one rectangle; whether it changed the picture.
    async fn rectangle(
        &mut self,
        kind: i32,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    ) -> Result<bool, Stop> {
        let pixels = match kind {
            encoding::DESKTOP_SIZE => return self.resize(width, height).await,
            encoding::EXTENDED_DESKTOP_SIZE => {
                let count = self.read.read_u8().await?;
                self.skip(3).await?;
                let mut screens = Vec::with_capacity(count.into());
                for _ in 0..count {
                    screens.push(Screen {
                        id: self.read.read_u32().await?,
                        x: self.read.read_u16().await?,
                        y: self.read.read_u16().await?,
                        width: self.read.read_u16().await?,
                        height: self.read.read_u16().await?,
                        flags: self.read.read_u32().await?,
                    });
                }
                // `x` says why: 1 for a change this client asked for. `y`
                // is the status: not 0 when that change failed, and then
                // the size is the one the desktop already has.
                let (asked, status) = (x == 1, y);
                self.note(Note::Layout {
                    asked,
                    status,
                    screens,
                })
                .await?;
                return if status == 0 {
                    self.resize(width, height).await
                } else {
                    tracing::info!("the server answered the layout asked for with {status}");
                    Ok(false)
                };
            }
            encoding::CURSOR => return self.cursor(x, y, width, height).await,
            encoding::DESKTOP_NAME => {
                let length = self.read.read_u32().await? as usize;
                self.skip(length.min(MAX_COMPRESSED)).await?;
                return Ok(false);
            }
            encoding::QEMU_KEYS => {
                self.note(Note::QemuKeys).await?;
                return Ok(false);
            }
            encoding::EXTENDED_MOUSE_BUTTONS => {
                self.note(Note::ExtendedButtons).await?;
                return Ok(false);
            }
            _ if !self
                .desktop
                .lock()
                .expect("desktop")
                .holds(x, y, width, height) =>
            {
                return Err(Stop::Bad(format!(
                    "a rectangle at {x},{y} of {width}x{height}, off the desktop"
                )));
            }
            encoding::COPY_RECT => {
                let from = (self.read.read_u16().await?, self.read.read_u16().await?);
                let mut desktop = self.desktop.lock().expect("desktop");
                if !desktop.holds(from.0, from.1, width, height) {
                    return Err(Stop::Bad("a copy from off the desktop".into()));
                }
                desktop.copy(from, (x, y), width, height);
                return Ok(true);
            }
            encoding::RAW => {
                self.bytes(usize::from(width) * usize::from(height) * 4)
                    .await?
            }
            encoding::HEXTILE => self.hextile(width, height).await?,
            encoding::ZLIB => {
                let data = self.counted().await?;
                let size = usize::from(width) * usize::from(height) * 4;
                let start = Instant::now();
                let pixels = exactly(decode::inflate(&mut self.zlib, &data, size)?, size)?;
                self.spent += start.elapsed();
                pixels
            }
            encoding::ZRLE => {
                let data = self.counted().await?;
                // Runs of one pixel each are the largest: four bytes a
                // pixel, and a tile's palette at most 382 bytes.
                let tiles = usize::from(width.div_ceil(64)) * usize::from(height.div_ceil(64));
                let limit = usize::from(width) * usize::from(height) * 4 + tiles * 382;
                let start = Instant::now();
                let tiles = decode::inflate(&mut self.zrle, &data, limit)?;
                let pixels = decode::zrle(&tiles, width, height)?;
                self.spent += start.elapsed();
                pixels
            }
            encoding::TIGHT => self.tight(width, height).await?,
            encoding::OPEN_H264 => {
                let length = self.read.read_u32().await? as usize;
                let flags = self.read.read_u32().await?;
                let Some(h264) = &mut self.h264 else {
                    return Err(Stop::Bad("H.264, not asked for".into()));
                };
                if flags & 2 != 0 {
                    h264.reset_all();
                }
                if flags & 1 != 0 {
                    h264.reset((x, y, width, height));
                }
                // Empty: only the flags were to be heard.
                if length == 0 {
                    return Ok(false);
                }
                let data = self.bytes(length).await?;
                let start = Instant::now();
                let pixels = self
                    .h264
                    .as_mut()
                    .expect("checked above")
                    .decode((x, y, width, height), &data)?;
                self.spent += start.elapsed();
                pixels
            }
            other => return Err(Stop::Bad(format!("encoding {other}, not asked for"))),
        };
        let start = Instant::now();
        self.desktop
            .lock()
            .expect("desktop")
            .put(x, y, width, &pixels);
        self.spent += start.elapsed();
        Ok(true)
    }

    async fn resize(&mut self, width: u16, height: u16) -> Result<bool, Stop> {
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return Err(Stop::Bad(format!("a desktop of {width}x{height}")));
        }
        {
            let mut desktop = self.desktop.lock().expect("desktop");
            if (desktop.width, desktop.height) == (width, height) {
                return Ok(false);
            }
            *desktop = Desktop::new(width, height);
        }
        tracing::info!("the desktop is now {width}x{height}");
        self.note(Note::Resized(width, height)).await?;
        Ok(false)
    }

    /// The Cursor pseudo-encoding: the pointer's pixels, then a mask of
    /// which ones show; the rectangle's place is its hotspot.
    async fn cursor(&mut self, x: u16, y: u16, width: u16, height: u16) -> Result<bool, Stop> {
        if width > MAX_CURSOR || height > MAX_CURSOR {
            return Err(Stop::Bad(format!("a pointer of {width}x{height}")));
        }
        let (w, h) = (usize::from(width), usize::from(height));
        let pixels = self.bytes(w * h * 4).await?;
        let row = w.div_ceil(8);
        let mask = self.bytes(row * h).await?;
        let pointer = (w > 0 && h > 0).then(|| {
            let mut rgba = Vec::with_capacity(w * h * 4);
            for (i, bgrx) in pixels.as_chunks::<4>().0.iter().enumerate() {
                let (px, py) = (i % w, i / w);
                let shown = mask[py * row + px / 8] & (0x80 >> (px % 8)) != 0;
                rgba.extend_from_slice(&[bgrx[2], bgrx[1], bgrx[0], if shown { 0xFF } else { 0 }]);
            }
            Pointer {
                width,
                height,
                hotspot_x: x.min(width - 1),
                hotspot_y: y.min(height - 1),
                rgba,
            }
        });
        self.note(Note::Cursor(pointer)).await?;
        Ok(false)
    }

    /// Hextile: 16x16 tiles, each raw or a background with rectangles on
    /// it. A tile's colors carry over to the next unless it says otherwise.
    async fn hextile(&mut self, width: u16, height: u16) -> Result<Vec<u8>, Stop> {
        const TILE: u16 = 16;
        let w = usize::from(width);
        let mut out = vec![0; w * usize::from(height) * 4];
        let (mut background, mut foreground) = (None, None);
        for ty in (0..height).step_by(TILE.into()) {
            for tx in (0..width).step_by(TILE.into()) {
                let (tw, th) = (TILE.min(width - tx), TILE.min(height - ty));
                let mask = self.read.read_u8().await?;
                let mut fill = |color: [u8; 4], sx: u16, sy: u16, sw: u16, sh: u16| {
                    for row in sy..sy + sh {
                        let at = ((usize::from(ty + row)) * w + usize::from(tx + sx)) * 4;
                        for pixel in out[at..at + usize::from(sw) * 4].as_chunks_mut::<4>().0 {
                            pixel.copy_from_slice(&color);
                        }
                    }
                };
                if mask & 1 != 0 {
                    let raw = self.bytes(usize::from(tw) * usize::from(th) * 4).await?;
                    for (row, line) in raw.chunks_exact(usize::from(tw) * 4).enumerate() {
                        let at = ((usize::from(ty) + row) * w + usize::from(tx)) * 4;
                        out[at..at + line.len()].copy_from_slice(line);
                    }
                    (background, foreground) = (None, None);
                    continue;
                }
                if mask & 2 != 0 {
                    background = Some(self.pixel().await?);
                }
                let color = background
                    .ok_or_else(|| Stop::Bad("a Hextile tile without a background".into()))?;
                fill(color, 0, 0, tw, th);
                if mask & 4 != 0 {
                    foreground = Some(self.pixel().await?);
                }
                if mask & 8 == 0 {
                    continue;
                }
                let colored = mask & 16 != 0;
                for _ in 0..self.read.read_u8().await? {
                    let color = if colored {
                        self.pixel().await?
                    } else {
                        foreground.ok_or_else(|| {
                            Stop::Bad("a Hextile tile without a foreground".into())
                        })?
                    };
                    let (place, size) = (self.read.read_u8().await?, self.read.read_u8().await?);
                    let (sx, sy) = (u16::from(place >> 4), u16::from(place & 15));
                    let (sw, sh) = (u16::from(size >> 4) + 1, u16::from(size & 15) + 1);
                    if sx + sw > tw || sy + sh > th {
                        return Err(Stop::Bad("a Hextile rectangle off its tile".into()));
                    }
                    fill(color, sx, sy, sw, sh);
                }
                if colored {
                    foreground = None;
                }
            }
        }
        Ok(out)
    }

    /// Tight: a control byte, then a single color, a JPEG picture, or
    /// pixels filtered and compressed with one of four zlib streams.
    async fn tight(&mut self, width: u16, height: u16) -> Result<Vec<u8>, Stop> {
        let control = self.read.read_u8().await?;
        for (i, stream) in self.tight.iter_mut().enumerate() {
            if control & (1 << i) != 0 {
                *stream = Decompress::new(true);
            }
        }
        let method = control >> 4;
        if method == 0b1000 {
            let color = self.bytes(3).await?;
            let color: [u8; 3] = color.try_into().expect("3 bytes");
            return Ok(decode::solid(decode::rgb(&color), width, height));
        }
        if method == 0b1001 {
            let length = self.compact().await?;
            let data = self.bytes(length).await?;
            let start = Instant::now();
            let pixels = decode::jpeg(&data, width, height)?;
            self.spent += start.elapsed();
            return Ok(pixels);
        }
        if method & 0b1000 != 0 {
            return Err(Stop::Bad(format!("Tight compression {method:#06b}")));
        }
        let filter = if method & 0b0100 != 0 {
            match self.read.read_u8().await? {
                0 => Filter::Copy,
                1 => {
                    let count = usize::from(self.read.read_u8().await?) + 1;
                    let colors = self.bytes(count * 3).await?;
                    Filter::Palette(colors.as_chunks::<3>().0.iter().map(decode::rgb).collect())
                }
                2 => Filter::Gradient,
                other => return Err(Stop::Bad(format!("Tight filter {other}"))),
            }
        } else {
            Filter::Copy
        };
        let size = filter.size(width, height);
        // Less than 12 bytes go as they are, uncompressed.
        let data = if size < 12 {
            self.bytes(size).await?
        } else {
            let length = self.compact().await?;
            let compressed = self.bytes(length).await?;
            let stream = &mut self.tight[usize::from(method & 0b11)];
            let start = Instant::now();
            let data = exactly(decode::inflate(stream, &compressed, size)?, size)?;
            self.spent += start.elapsed();
            data
        };
        let start = Instant::now();
        let pixels = filter.pixels(&data, width, height)?;
        self.spent += start.elapsed();
        Ok(pixels)
    }

    /// Tight's length in one to three bytes, seven bits at a time.
    async fn compact(&mut self) -> Result<usize, Stop> {
        let mut length = 0;
        for (i, shift) in [0, 7, 14].into_iter().enumerate() {
            let byte = self.read.read_u8().await?;
            if i == 2 {
                length |= usize::from(byte) << shift;
                break;
            }
            length |= usize::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                break;
            }
        }
        Ok(length)
    }

    /// A length, and that many bytes of compressed data.
    async fn counted(&mut self) -> Result<Vec<u8>, Stop> {
        let length = self.read.read_u32().await? as usize;
        self.bytes(length).await
    }

    async fn bytes(&mut self, count: usize) -> Result<Vec<u8>, Stop> {
        if count > MAX_COMPRESSED {
            return Err(Stop::Bad(format!("{count} bytes in one rectangle")));
        }
        let mut data = vec![0; count];
        self.read.read_exact(&mut data).await?;
        Ok(data)
    }

    async fn pixel(&mut self) -> Result<[u8; 4], Stop> {
        let mut pixel = [0; 4];
        self.read.read_exact(&mut pixel).await?;
        pixel[3] = 0xFF;
        Ok(pixel)
    }

    async fn skip(&mut self, count: usize) -> Result<(), Stop> {
        let copied = tokio::io::copy(
            &mut (&mut self.read).take(count as u64),
            &mut tokio::io::sink(),
        )
        .await?;
        if copied < count as u64 {
            return Err(Stop::Io(std::io::ErrorKind::UnexpectedEof.into()));
        }
        Ok(())
    }
}

fn exactly(data: Vec<u8>, size: usize) -> Result<Vec<u8>, Bad> {
    if data.len() == size {
        Ok(data)
    } else {
        Err(format!("{} bytes inflated for {size}", data.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small, seeded generator, so a failure repeats.
    struct Noise(u64);

    impl Noise {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn bytes(&mut self, most: usize) -> Vec<u8> {
            let length = self.next() as usize % (most + 1);
            (0..length).map(|_| self.next() as u8).collect()
        }
    }

    /// Read `data` as everything a server sent, to its end.
    async fn read_all(data: Vec<u8>) {
        let desktop = Arc::new(Mutex::new(Desktop::new(64, 48)));
        let (notes, mut heard) = mpsc::channel(16);
        let listening = tokio::spawn(async move { while heard.recv().await.is_some() {} });
        run(std::io::Cursor::new(data), desktop, notes, None).await;
        listening.await.unwrap();
    }

    /// A framebuffer update holding one rectangle of `kind`, whose body is
    /// noise: the message's frame is right, so the noise reaches the
    /// decoder itself.
    fn update(kind: i32, noise: &mut Noise) -> Vec<u8> {
        let mut data = vec![0, 0, 0, 1];
        let width = (noise.next() % 65) as u16;
        let height = (noise.next() % 49) as u16;
        for value in [0, 0, width, height] {
            data.extend_from_slice(&value.to_be_bytes());
        }
        data.extend_from_slice(&kind.to_be_bytes());
        // A plausible length first, for the encodings that start with one.
        data.extend_from_slice(&((noise.next() % 600) as u32).to_be_bytes());
        data.extend(noise.bytes(600));
        data
    }

    #[tokio::test]
    async fn noise_from_a_server_ends_the_session_without_a_panic() {
        let mut noise = Noise(0x9E37_79B9_7F4A_7C15);
        for _ in 0..2000 {
            read_all(noise.bytes(256)).await;
        }
        let kinds = [
            encoding::RAW,
            encoding::COPY_RECT,
            encoding::HEXTILE,
            encoding::ZLIB,
            encoding::TIGHT,
            encoding::ZRLE,
            encoding::CURSOR,
            encoding::DESKTOP_SIZE,
            encoding::EXTENDED_DESKTOP_SIZE,
            encoding::DESKTOP_NAME,
        ];
        for round in 0..6000 {
            let kind = kinds[round % kinds.len()];
            read_all(update(kind, &mut noise)).await;
        }
        // Noise compressed as it should be, so ZRLE's tiles and Zlib's
        // pixels see it.
        for round in 0..3000 {
            let kind = [encoding::ZRLE, encoding::ZLIB][round % 2];
            let mut data = update(kind, &mut noise);
            data.truncate(16);
            let mut compressed = Vec::with_capacity(2048);
            flate2::Compress::new(flate2::Compression::fast(), true)
                .compress_vec(
                    &noise.bytes(1500),
                    &mut compressed,
                    flate2::FlushCompress::Sync,
                )
                .unwrap();
            data.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
            data.extend(compressed);
            read_all(data).await;
        }
    }

    #[test]
    fn noise_in_compressed_data_is_refused_without_a_panic() {
        let mut noise = Noise(0x2545_F491_4F6C_DD1D);
        for _ in 0..3000 {
            let data = noise.bytes(400);
            let (width, height) = ((noise.next() % 80) as u16, (noise.next() % 80) as u16);
            let _ = decode::zrle(&data, width, height);
            let _ = decode::jpeg(&data, width, height);
            let _ = decode::inflate(&mut Decompress::new(true), &data, 4096);
            let colors = (0..noise.next() % 4).map(|_| [0, 0, 0, 0xFF]).collect();
            for filter in [Filter::Copy, Filter::Gradient, Filter::Palette(colors)] {
                let _ = filter.pixels(&data, width, height);
            }
        }
    }
}
