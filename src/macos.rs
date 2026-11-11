//! What the macOS app does around GTK that the GNOME app has no need of:
//! finding the libraries and data Mote.app carries, the language macOS is
//! set to, and updates through Sparkle.

mod bundle;
mod language;
pub(crate) mod updates;

pub(crate) use bundle::rebind_toolkit_translations;

/// Point GTK and gettext at the bundle and the user's language. First thing
/// in `main`.
///
/// SAFETY: no other thread may exist yet, as it sets environment variables.
pub(crate) unsafe fn prepare() {
    unsafe {
        bundle::prepare();
        language::prepare();
    }
}
