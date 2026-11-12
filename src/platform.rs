//! The small differences between the GNOME and macOS builds that shared
//! code has to ask about. Whole features that differ live in their own
//! modules instead, chosen by `#[cfg(target_os = "macos")]`.

use gtk::prelude::*;

/// Shows a window, or brings it forward when it is already showing. On
/// macOS a new window stays out of sight until its first frame is drawn
/// (see `macos::windows`), as GNOME's compositor already does.
pub(crate) fn present(window: &impl IsA<gtk::Window>) {
    #[cfg(target_os = "macos")]
    crate::macos::windows::present(window.as_ref());
    #[cfg(not(target_os = "macos"))]
    window.present();
}

/// A shortcut as the platform writes it: `<Control>` is Command on macOS.
/// GTK's `<Primary>` would be Ctrl there too.
pub(crate) fn accel(accel: &str) -> String {
    if cfg!(target_os = "macos") {
        accel.replace("<Control>", "<Meta>")
    } else {
        accel.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::accel;

    #[test]
    fn control_is_command_on_macos_only() {
        let expected = if cfg!(target_os = "macos") {
            "<Meta>q"
        } else {
            "<Control>q"
        };
        assert_eq!(accel("<Control>q"), expected);
        assert_eq!(accel("<Alt>Up"), "<Alt>Up");
    }
}
