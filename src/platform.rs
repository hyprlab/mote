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

/// A window beside the main one, such as About or Settings: on macOS it
/// stays a window over a fullscreen one rather than going fullscreen with
/// it (see `macos::windows`). GNOME has nothing to change.
pub(crate) fn keep_windowed(window: &impl IsA<gtk::Window>) {
    #[cfg(target_os = "macos")]
    crate::macos::windows::keep_windowed(window.as_ref());
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

/// The main window: on macOS, making it fullscreen with the green button
/// gives a bigger window, sidebar and all, not a session's fullscreen (see
/// `macos::fullscreen`). GNOME's fullscreen is always a session's.
pub(crate) fn keep_own_fullscreen(window: &impl IsA<gtk::Window>) {
    #[cfg(target_os = "macos")]
    crate::macos::fullscreen::keep_own(window.as_ref());
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

/// Fullscreen for the session in the window.
pub(crate) fn fullscreen_for_session(window: &impl IsA<gtk::Window>) {
    #[cfg(target_os = "macos")]
    crate::macos::fullscreen::enter(window.as_ref());
    #[cfg(not(target_os = "macos"))]
    window.fullscreen();
}

/// Leave the session's fullscreen.
pub(crate) fn leave_fullscreen_for_session(window: &impl IsA<gtk::Window>) {
    #[cfg(target_os = "macos")]
    crate::macos::fullscreen::leave(window.as_ref());
    #[cfg(not(target_os = "macos"))]
    window.unfullscreen();
}

/// Whether the window is fullscreen for the session in it.
pub(crate) fn is_fullscreen_for_session(window: &impl IsA<gtk::Window>) -> bool {
    #[cfg(target_os = "macos")]
    return crate::macos::fullscreen::for_session(window.as_ref());
    #[cfg(not(target_os = "macos"))]
    window.is_fullscreen()
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
