//! What the app changes about GTK on macOS.

/// GTK on macOS sets text at the system's 12 points on a 72 dpi screen, 12
/// pixels, where GNOME's 11 points on 96 dpi come to nearly 15, the size
/// libadwaita's spacing is drawn around. This keeps the system's typeface
/// at GNOME's size.
pub(super) fn apply_text_size() {
    let Some(settings) = gtk::Settings::default() else {
        return;
    };
    let font = settings.gtk_font_name().unwrap_or_default();
    // The name is the family and then the size.
    let family = font
        .rsplit_once(' ')
        .filter(|(_, size)| size.parse::<f64>().is_ok())
        .map_or(font.as_str(), |(family, _)| family);
    settings.set_gtk_xft_dpi(96 * 1024);
    settings.set_gtk_font_name(Some(&format!("{family} 11")));
}
