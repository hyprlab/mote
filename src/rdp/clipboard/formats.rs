//! The clipboard's content as Windows puts it on the wire, and as a Linux
//! desktop has it: UTF-16 text, the HTML Format with its byte-offset
//! header, and device-independent bitmaps.
//!
//! Text goes out with the line endings it has. Windows writes CRLF, which
//! GNOME Remote Desktop and KRdp turn into LF; xrdp passes CRLF on to the
//! applications on its desktop, and all three take LF as it is.

pub(crate) use crate::remote::clipboard::Image;

/// The largest picture taken from the other side, 16384 pixels square:
/// anything larger is more likely damage than a picture.
const MAX_SIDE: u32 = 16384;

/// CF_UNICODETEXT: UTF-16LE up to the first NUL, CRLF to LF.
pub(super) fn text_from_unicode(data: &[u8]) -> String {
    let units: Vec<u16> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|unit| *unit != 0)
        .collect();
    String::from_utf16_lossy(&units).replace("\r\n", "\n")
}

/// CF_TEXT and CF_OEMTEXT: bytes up to the first NUL. Linux servers send
/// UTF-8 there when they send it at all.
pub(super) fn text_from_ansi(data: &[u8]) -> String {
    let end = data.iter().position(|b| *b == 0).unwrap_or(data.len());
    String::from_utf8_lossy(&data[..end]).replace("\r\n", "\n")
}

/// CF_UNICODETEXT.
pub(super) fn unicode_from_text(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// The HTML in an HTML Format payload: from `StartHTML` to `EndHTML`, or
/// the fragment alone when those are missing.
pub(super) fn html_from_cf_html(data: &[u8]) -> Option<String> {
    let end = data.iter().position(|b| *b == 0).unwrap_or(data.len());
    let data = &data[..end];
    let header = String::from_utf8_lossy(&data[..data.len().min(512)]);
    let offset = |name: &str| -> Option<usize> {
        let line = header.lines().find_map(|l| l.strip_prefix(name))?;
        let value: i64 = line.trim_start_matches(':').trim().parse().ok()?;
        usize::try_from(value).ok()
    };
    let (start, stop) = match (offset("StartHTML"), offset("EndHTML")) {
        (Some(start), Some(stop)) => (start, stop),
        _ => (offset("StartFragment")?, offset("EndFragment")?),
    };
    let stop = stop.min(data.len());
    (start < stop).then(|| String::from_utf8_lossy(&data[start..stop]).into_owned())
}

/// An HTML Format payload around `html`, a fragment or a whole document.
pub(super) fn cf_html_from_html(html: &str) -> Vec<u8> {
    // The header's offsets are written with a fixed width, so its length is
    // known before they are.
    const HEADER: &str = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\n\
        StartFragment:0000000000\r\nEndFragment:0000000000\r\n";
    let (before, fragment, after) = match fragment_of(html) {
        Some((body_start, body_end)) => (
            format!("{}<!--StartFragment-->", &html[..body_start]),
            &html[body_start..body_end],
            format!("<!--EndFragment-->{}", &html[body_end..]),
        ),
        None => (
            "<html><body>\r\n<!--StartFragment-->".to_owned(),
            html,
            "<!--EndFragment-->\r\n</body>\r\n</html>".to_owned(),
        ),
    };
    let start_html = HEADER.len();
    let start_fragment = start_html + before.len();
    let end_fragment = start_fragment + fragment.len();
    let end_html = end_fragment + after.len();
    let mut out = format!(
        "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\n\
         StartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n\
         {before}{fragment}{after}"
    )
    .into_bytes();
    out.push(0);
    out
}

/// Where a whole document's body content starts and ends.
fn fragment_of(html: &str) -> Option<(usize, usize)> {
    let lower = html.to_ascii_lowercase();
    let open = lower.find("<body")?;
    let start = open + lower[open..].find('>')? + 1;
    let end = lower.rfind("</body")?;
    (start <= end).then_some((start, end))
}

/// HTML as a desktop application wrote it: UTF-8, or UTF-16 with a byte
/// order mark, as Firefox once wrote it.
pub(crate) fn html_from_local(data: &[u8]) -> String {
    let utf16 = |bytes: &[u8], from: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| from(*pair))
            .collect();
        String::from_utf16_lossy(&units)
    };
    match data {
        [0xff, 0xfe, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xfe, 0xff, rest @ ..] => utf16(rest, u16::from_be_bytes),
        [0xef, 0xbb, 0xbf, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(data).into_owned(),
    }
}

/// A picture from CF_DIB or CF_DIBV5: a BITMAPINFOHEADER or one of its
/// longer versions, the color masks or palette, then the rows.
pub(super) fn image_from_dib(data: &[u8]) -> Option<Image> {
    let u16_at = |at: usize| Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?));
    let header = u32_at(0)? as usize;
    if header < 40 {
        return None;
    }
    let width = i32::from_le_bytes(data.get(4..8)?.try_into().ok()?);
    let height = i32::from_le_bytes(data.get(8..12)?.try_into().ok()?);
    let bits = u16_at(14)?;
    let compression = u32_at(16)?;
    let colors_used = u32_at(32)?;
    let top_down = height < 0;
    let (width, height) = (width.unsigned_abs(), height.unsigned_abs());
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }

    const BI_RGB: u32 = 0;
    const BI_BITFIELDS: u32 = 3;
    let mut at = header;
    let masks = match (compression, bits) {
        (BI_BITFIELDS, 16 | 32) => {
            // In the header from version 4 on, after it before then.
            let masks = if header >= 56 {
                [u32_at(40)?, u32_at(44)?, u32_at(48)?, u32_at(52)?]
            } else {
                at += 12;
                [u32_at(40)?, u32_at(44)?, u32_at(48)?, 0]
            };
            Some(masks)
        }
        (BI_RGB, 16) => Some([0x7c00, 0x03e0, 0x001f, 0]),
        // The fourth byte is alpha to those that write it, and zero to
        // those that don't, which is caught below.
        (BI_RGB, 32) => Some([0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0xff00_0000]),
        (BI_RGB, 1 | 4 | 8 | 24) => None,
        _ => return None,
    };
    let palette: Vec<[u8; 4]> = if bits <= 8 {
        let count = match colors_used {
            0 => 1usize << bits,
            n => (n as usize).min(1 << bits),
        };
        let table = data.get(at..at + count * 4)?;
        at += count * 4;
        table
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| [c[2], c[1], c[0], 255])
            .collect()
    } else {
        Vec::new()
    };

    let stride = (width as usize * usize::from(bits)).div_ceil(32) * 4;
    let rows = data.get(at..at + stride * height as usize)?;
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    let mut any_alpha = false;
    for y in 0..height as usize {
        let row = if top_down { y } else { height as usize - 1 - y };
        let row = &rows[row * stride..(row + 1) * stride];
        for x in 0..width as usize {
            let pixel = match (bits, masks) {
                (24, _) => [row[x * 3 + 2], row[x * 3 + 1], row[x * 3], 255],
                (32 | 16, Some(masks)) => {
                    let value = if bits == 32 {
                        u32::from_le_bytes(row[x * 4..x * 4 + 4].try_into().ok()?)
                    } else {
                        u32::from(u16::from_le_bytes(row[x * 2..x * 2 + 2].try_into().ok()?))
                    };
                    let alpha = if masks[3] == 0 {
                        255
                    } else {
                        channel(value, masks[3])
                    };
                    [
                        channel(value, masks[0]),
                        channel(value, masks[1]),
                        channel(value, masks[2]),
                        alpha,
                    ]
                }
                (1 | 4 | 8, _) => {
                    let bit = x * usize::from(bits);
                    let byte = row[bit / 8];
                    let shift = 8 - usize::from(bits) - bit % 8;
                    let index = (byte >> shift) & ((1u16 << bits) - 1) as u8;
                    *palette.get(usize::from(index)).unwrap_or(&[0, 0, 0, 255])
                }
                _ => return None,
            };
            any_alpha |= pixel[3] != 0;
            rgba.extend_from_slice(&pixel);
        }
    }
    // A 32-bit bitmap whose alpha is all zero means no alpha at all.
    if !any_alpha {
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
    }
    Some(Image {
        width,
        height,
        rgba,
    })
}

/// A channel picked out by its mask, scaled to eight bits.
fn channel(value: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let max = mask >> mask.trailing_zeros();
    let raw = (value & mask) >> mask.trailing_zeros();
    ((u64::from(raw) * 255 + u64::from(max) / 2) / u64::from(max)) as u8
}

/// CF_DIB from a picture: 32 bits a pixel, bottom row first. Windows reads
/// no alpha from CF_DIB; the bytes are there for those that do.
pub(super) fn dib_from_image(image: &Image) -> Vec<u8> {
    let (width, height) = (image.width as usize, image.height as usize);
    let mut out = Vec::with_capacity(40 + width * height * 4);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(image.width as i32).to_le_bytes());
    out.extend_from_slice(&(image.height as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&((width * height * 4) as u32).to_le_bytes());
    // 96 DPI, in pixels a meter.
    out.extend_from_slice(&3780i32.to_le_bytes());
    out.extend_from_slice(&3780i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for row in image.rgba.chunks_exact(width * 4).rev() {
        for pixel in row.as_chunks::<4>().0 {
            out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_goes_both_ways() {
        let wire = unicode_from_text("one\ntwo ✓");
        assert_eq!(&wire[..8], &[b'o', 0, b'n', 0, b'e', 0, b'\n', 0]);
        assert_eq!(&wire[wire.len() - 2..], &[0, 0]);
        assert_eq!(text_from_unicode(&wire), "one\ntwo ✓");
        assert_eq!(text_from_ansi(b"a\r\nb\0junk"), "a\nb");
    }

    #[test]
    fn html_goes_both_ways() {
        let wire = cf_html_from_html("<b>bold</b>");
        let text = String::from_utf8_lossy(&wire);
        assert!(text.starts_with("Version:0.9\r\nStartHTML:0000000105\r\n"));
        let html = html_from_cf_html(&wire).expect("reads its own");
        assert!(html.contains("<!--StartFragment--><b>bold</b><!--EndFragment-->"));
        let start: usize = text
            .split_once("StartFragment:")
            .and_then(|(_, rest)| rest[..10].parse().ok())
            .expect("a fragment offset");
        assert!(text[start..].starts_with("<b>bold</b>"));

        let page = "<html><head></head><body class=\"x\"><p>hi</p></body></html>";
        let html = html_from_cf_html(&cf_html_from_html(page)).expect("reads a page");
        assert!(
            html.contains(
                "<body class=\"x\"><!--StartFragment--><p>hi</p><!--EndFragment--></body>"
            )
        );
    }

    #[test]
    fn html_is_read_in_its_encoding() {
        let utf16: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain("<i>é</i>".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(html_from_local(&utf16), "<i>é</i>");
        assert_eq!(html_from_local("<i>é</i>".as_bytes()), "<i>é</i>");
    }

    #[test]
    fn a_picture_goes_both_ways() {
        let image = Image {
            width: 2,
            height: 2,
            rgba: vec![
                255, 0, 0, 255, 0, 255, 0, 255, //
                0, 0, 255, 255, 10, 20, 30, 128,
            ],
        };
        let dib = dib_from_image(&image);
        assert_eq!(dib.len(), 40 + 16);
        assert_eq!(image_from_dib(&dib), Some(image));
    }

    #[test]
    fn older_bitmaps_are_read() {
        // 24 bits, 1×2, bottom-up: the blue row is the top one.
        let mut dib = vec![0u8; 40];
        dib[0] = 40;
        dib[4] = 1;
        dib[8] = 2;
        dib[12] = 1;
        dib[14] = 24;
        dib.extend_from_slice(&[0, 0, 255, 0]); // red, padded to 4 bytes
        dib.extend_from_slice(&[255, 0, 0, 0]); // blue
        let image = image_from_dib(&dib).expect("a 24-bit picture");
        assert_eq!(image.rgba, vec![0, 0, 255, 255, 255, 0, 0, 255]);

        // 32 bits with no alpha anywhere: opaque.
        let mut dib = dib_from_image(&Image {
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 0],
        });
        dib[40 + 3] = 0;
        assert_eq!(
            image_from_dib(&dib).expect("a picture").rgba,
            vec![1, 2, 3, 255]
        );
    }
}
