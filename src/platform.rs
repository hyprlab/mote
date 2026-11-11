//! The small differences between the GNOME and macOS builds that shared
//! code has to ask about. Whole features that differ live in their own
//! modules instead, chosen by `#[cfg(target_os = "macos")]`.

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
