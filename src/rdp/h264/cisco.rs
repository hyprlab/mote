//! Cisco's own build of OpenH264, fetched from Cisco on the user's request,
//! for systems that have no H.264 decoder of their own (an AppImage on a
//! system without OpenH264, say). Cisco's patent license covers the copies
//! it hands out itself, which is why it is downloaded rather than shipped.
//! The download is checked against Cisco's published hash before it is
//! kept, and again whenever it is loaded.

use std::io::Read as _;
use std::path::PathBuf;

const VERSION: &str = "2.6.0";

/// Cisco's builds, by machine, with their SHA-256 once unpacked.
fn build() -> Option<(&'static str, &'static str)> {
    // Cisco's builds here are Linux's; the macOS app has VideoToolbox.
    if cfg!(target_os = "macos") {
        return None;
    }
    match std::env::consts::ARCH {
        "x86_64" => Some((
            "linux64",
            "2f0cde7c6a6abcf5cae76942894ea42897fa677bce4ed6c91a24dd1b041d5f04",
        )),
        "aarch64" => Some((
            "linux-arm64",
            "12e7b33623667cdab0e575170c147b1b36eadb77d0d2aa7ceb5afd3e58902140",
        )),
        _ => None,
    }
}

/// Whether Cisco builds OpenH264 for this machine.
pub(crate) fn offered() -> bool {
    build().is_some()
}

/// Where the download is kept.
pub(crate) fn path() -> PathBuf {
    let name = build().map_or("libopenh264.so".to_owned(), |(platform, _)| {
        format!("libopenh264-{VERSION}-{platform}.8.so")
    });
    crate::paths::data_dir().join("openh264").join(name)
}

pub(crate) fn downloaded() -> bool {
    path().is_file()
}

/// Fetch, unpack, check and keep Cisco's library. Blocks: run it off the
/// main thread.
pub(crate) fn download() -> Result<(), String> {
    let (platform, expected) = build().ok_or("Cisco has no build for this computer")?;
    let url = format!("https://ciscobinary.openh264.org/libopenh264-{VERSION}-{platform}.8.so.bz2");
    let packed = ureq::get(&url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    let mut library = Vec::new();
    bzip2::read::BzDecoder::new(packed.as_slice())
        .read_to_end(&mut library)
        .map_err(|e| e.to_string())?;
    let hash = gtk::glib::compute_checksum_for_data(gtk::glib::ChecksumType::Sha256, &library)
        .map(|s| s.to_string())
        .unwrap_or_default();
    if hash != expected {
        return Err(format!("the download doesn't match Cisco's build ({hash})"));
    }
    let path = path();
    let dir = path.parent().ok_or("no data directory")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let partial = path.with_extension("part");
    std::fs::write(&partial, &library).map_err(|e| e.to_string())?;
    std::fs::rename(&partial, &path).map_err(|e| e.to_string())?;
    tracing::info!("downloaded Cisco's OpenH264 to {}", path.display());
    Ok(())
}

pub(crate) fn remove() -> Result<(), String> {
    match std::fs::remove_file(path()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
