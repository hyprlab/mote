//! Running from Mote.app, which carries its own GTK and the data GTK reads
//! (tools/macos-bundle.py puts them there). GLib, GTK, gdk-pixbuf and
//! fontconfig look where Homebrew installed them unless told otherwise, and
//! a Mac that runs the app may have no Homebrew, so they are told. A bare
//! binary run from target/ has no bundle around it and uses Homebrew's.

use std::ffi::{CString, c_char};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The bundle's Contents directory, when Mote runs from one that carries
/// its own GTK.
fn contents() -> Option<&'static Path> {
    static CONTENTS: OnceLock<Option<PathBuf>> = OnceLock::new();
    CONTENTS
        .get_or_init(|| {
            let exe = std::env::current_exe().ok()?;
            let contents = exe.parent()?.parent()?;
            contents
                .join("Resources/share")
                .is_dir()
                .then(|| contents.to_path_buf())
        })
        .as_deref()
}

/// SAFETY: as [`super::prepare`].
pub(super) unsafe fn prepare() {
    let Some(contents) = contents() else {
        return;
    };
    let resources = contents.join("Resources");
    let share = resources.join("share");
    let vars = [
        // Icons and anything else GTK finds under a data directory.
        ("XDG_DATA_DIRS", share.clone()),
        ("GSETTINGS_SCHEMA_DIR", share.join("glib-2.0/schemas")),
        // GIO's modules: none, rather than Homebrew's.
        ("GIO_MODULE_DIR", contents.join("PlugIns/gio")),
        ("FONTCONFIG_FILE", resources.join("etc/fonts/fonts.conf")),
        ("MOTE_LOCALEDIR", share.join("locale")),
    ];
    for (name, value) in vars {
        // SAFETY: the caller's.
        unsafe { std::env::set_var(name, value) };
    }
    if let Some(cache) = loader_cache(contents, &resources) {
        // SAFETY: the caller's.
        unsafe { std::env::set_var("GDK_PIXBUF_MODULE_FILE", cache) };
    }
}

/// gdk-pixbuf's list of loaders names each by its whole path, which
/// depends on where the app is, so the bundle's list has @CONTENTS@ in its
/// place and is written out with the real path among the app's caches.
fn loader_cache(contents: &Path, resources: &Path) -> Option<PathBuf> {
    let template =
        std::fs::read_to_string(resources.join("lib/gdk-pixbuf-2.0/2.10.0/loaders.cache")).ok()?;
    let text = template.replace("@CONTENTS@", &contents.to_string_lossy());
    let dir = PathBuf::from(std::env::var_os("HOME")?)
        .join("Library/Caches")
        .join(crate::APP_ID);
    let cache = dir.join("loaders.cache");
    if std::fs::read_to_string(&cache).ok().as_deref() != Some(text.as_str()) {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&cache, text).ok()?;
    }
    Some(cache)
}

/// GTK and libadwaita bind their translations to the directory they were
/// built for, Homebrew's, as they start. Once they have, this binds them to
/// the bundle's instead. They use Homebrew's libintl, the one in the
/// bundle, rather than the one gettext-rs builds into Mote, so its
/// `bindtextdomain` is looked up by name.
pub(crate) fn rebind_toolkit_translations() {
    let Some(contents) = contents() else {
        return;
    };
    type Bind = unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_char;
    // SAFETY: looking up a symbol in what is already loaded.
    let symbol = unsafe { libc_dlsym(c"libintl_bindtextdomain".as_ptr()) };
    if symbol.is_null() {
        return;
    }
    // SAFETY: libintl's bindtextdomain has this signature.
    let bind: Bind = unsafe { std::mem::transmute(symbol) };
    let Ok(dir) = CString::new(
        contents
            .join("Resources/share/locale")
            .to_string_lossy()
            .as_bytes(),
    ) else {
        return;
    };
    for domain in [c"gtk40", c"libadwaita", c"glib20"] {
        // SAFETY: both are NUL-terminated strings that outlive the call.
        unsafe { bind(domain.as_ptr(), dir.as_ptr()) };
    }
}

/// `dlsym(RTLD_DEFAULT, name)`: the first image loaded that exports it.
unsafe fn libc_dlsym(name: *const c_char) -> *mut std::ffi::c_void {
    // SAFETY: the caller's.
    unsafe { nix::libc::dlsym(nix::libc::RTLD_DEFAULT, name) }
}
