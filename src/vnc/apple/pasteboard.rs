//! macOS's clipboard over Screen Sharing: the server says when its
//! clipboard changes, and sends it when asked as an archive of flavors,
//! compressed. Text copied here goes the same way: macOS puts the original
//! cut text where only older apps look.

use flate2::{Compress, Compression, Decompress, FlushCompress};

use super::super::decode::inflate;

/// The server's clipboard, compressed.
pub(in crate::vnc) const CLIPBOARD_SEND: u8 = 0x1f;
/// MiscStatus's news that the server's clipboard changed.
pub(in crate::vnc) const CHANGED: u16 = 2;
/// AutoPasteboard: tell this side when the clipboard changes.
pub(in crate::vnc) const WATCH: [u8; 8] = [0x15, 0, 0, 1, 0, 0, 0, 0];
/// Ask for the server's clipboard, all of it.
pub(in crate::vnc) const FETCH: [u8; 8] = [0x0b, 0, 0, 0, 0, 0, 0, 0];

const UTF8: &str = "public.utf8-plain-text";

/// The text in the clipboard the server sent, `size` bytes once inflated,
/// or none when it holds no text.
pub(in crate::vnc) fn text(compressed: &[u8], size: usize) -> Option<String> {
    if size == 0 {
        return None;
    }
    let archive = inflate(&mut Decompress::new(true), compressed, size).ok()?;
    let flavors = flavors(&archive)?;
    let find = |wanted: &dyn Fn(&str) -> bool| {
        flavors
            .iter()
            .find(|(name, _)| wanted(name))
            .map(|(_, data)| *data)
    };
    if let Some(data) = find(&|name| name == UTF8) {
        return Some(String::from_utf8_lossy(data).into_owned());
    }
    if let Some(data) = find(&|name| name == "public.utf16-external-plain-text") {
        let units: Vec<u16> = data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_be_bytes(*pair))
            .collect();
        return Some(String::from_utf16_lossy(&units));
    }
    find(&|name| name == "com.apple.traditional-mac-plain-text")
        .map(|data| data.iter().map(|&byte| char::from(byte)).collect())
}

/// Each flavor's name and data: a count, then for each a name, a word, a
/// list of other names for it with their values, and its data.
fn flavors(archive: &[u8]) -> Option<Vec<(&str, &[u8])>> {
    let word = |at: usize| {
        let bytes = archive.get(at..at.checked_add(4)?)?;
        Some(u32::from_be_bytes(bytes.try_into().unwrap()) as usize)
    };
    // A length, and that many bytes.
    let field = |at: &mut usize| {
        let length = word(*at)?;
        let data = archive.get(*at + 4..(*at + 4).checked_add(length)?)?;
        *at += 4 + length;
        Some(data)
    };
    let count = word(0)?;
    let mut at = 4;
    let mut flavors = Vec::new();
    for _ in 0..count.min(64) {
        let name = std::str::from_utf8(field(&mut at)?).ok()?;
        let others = word(at + 4)?;
        at += 8;
        for _ in 0..others.min(64) {
            field(&mut at)?;
            field(&mut at)?;
        }
        flavors.push((name, field(&mut at)?));
    }
    Some(flavors)
}

/// The message that puts `text` in the server's clipboard: one flavor,
/// UTF-8, with no other names.
pub(in crate::vnc) fn message(text: &str) -> Vec<u8> {
    let mut archive = 1u32.to_be_bytes().to_vec();
    archive.extend_from_slice(&(UTF8.len() as u32).to_be_bytes());
    archive.extend_from_slice(UTF8.as_bytes());
    archive.extend_from_slice(&[0; 8]);
    archive.extend_from_slice(&(text.len() as u32).to_be_bytes());
    archive.extend_from_slice(text.as_bytes());

    // Flushed as macOS flushes its own, not finished.
    let mut compressed = Vec::with_capacity(archive.len() + archive.len() / 64 + 64);
    let mut stream = Compress::new(Compression::default(), true);
    loop {
        let taken = stream.total_in() as usize;
        if stream
            .compress_vec(&archive[taken..], &mut compressed, FlushCompress::Sync)
            .is_err()
        {
            break;
        }
        if stream.total_in() as usize == archive.len() && compressed.len() < compressed.capacity() {
            break;
        }
        compressed.reserve(4096);
    }
    let mut out = vec![CLIPBOARD_SEND, 0, 0, 0, 0, 0, 0, 0];
    out.extend_from_slice(&(archive.len() as u32).to_be_bytes());
    out.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
    out.extend(compressed);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_copied_here_reads_back_as_the_server_would_send_it() {
        let sent = message("café\nnoir");
        assert_eq!(sent[0], CLIPBOARD_SEND);
        let size = u32::from_be_bytes(sent[8..12].try_into().unwrap()) as usize;
        let length = u32::from_be_bytes(sent[12..16].try_into().unwrap()) as usize;
        assert_eq!(sent.len(), 16 + length);
        assert_eq!(text(&sent[16..], size).as_deref(), Some("café\nnoir"));
    }

    #[test]
    fn reads_text_past_other_flavors_and_their_names() {
        let mut archive = 2u32.to_be_bytes().to_vec();
        let put = |archive: &mut Vec<u8>, bytes: &[u8]| {
            archive.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            archive.extend_from_slice(bytes);
        };
        put(&mut archive, b"public.png");
        archive.extend_from_slice(&[0; 4]);
        archive.extend_from_slice(&1u32.to_be_bytes());
        put(&mut archive, b"public.mime-type");
        put(&mut archive, b"image/png");
        put(&mut archive, &[0x89, b'P']);
        put(&mut archive, b"public.utf16-external-plain-text");
        archive.extend_from_slice(&[0; 8]);
        put(&mut archive, &[0, b'h', 0, b'i']);
        let flavors = flavors(&archive).unwrap();
        assert_eq!(flavors.len(), 2);
        let mut compressed = Vec::with_capacity(archive.len() + 64);
        Compress::new(Compression::default(), true)
            .compress_vec(&archive, &mut compressed, FlushCompress::Finish)
            .unwrap();
        assert_eq!(text(&compressed, archive.len()).as_deref(), Some("hi"));
        // An empty clipboard comes as nothing at all.
        assert_eq!(text(&[], 0), None);
    }
}
