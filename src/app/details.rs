//! The chosen connection, beside the sidebar.

use adw::prelude::*;

use crate::connection::Connection;
use crate::i18n::i18n;

/// `picture`: whether the header shows the connection's picture.
pub(super) fn page(conn: &Connection, picture: bool) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();

    let header = gtk::Box::new(gtk::Orientation::Vertical, 6);
    header.add_css_class("details-header");
    if picture {
        let avatar = crate::ui::avatar::Avatar::new(96);
        avatar.show_connection(conn);
        header.append(&avatar);
    }
    let title = gtk::Label::new(Some(conn.title()));
    title.add_css_class("title-1");
    title.set_wrap(true);
    title.set_justify(gtk::Justification::Center);
    title.set_margin_top(6);
    header.append(&title);
    if !conn.name.trim().is_empty() {
        let address = gtk::Label::new(Some(&conn.address()));
        address.add_css_class("dim-label");
        address.set_selectable(true);
        header.append(&address);
    }
    let connect = gtk::Button::with_label(&i18n("Connect"));
    connect.add_css_class("pill");
    connect.add_css_class("suggested-action");
    connect.set_halign(gtk::Align::Center);
    connect.set_margin_top(12);
    connect.set_action_name(Some("win.connect"));
    header.append(&connect);
    let top = adw::PreferencesGroup::new();
    top.add(&header);
    page.add(&top);

    let info = adw::PreferencesGroup::new();
    info.set_title(&i18n("Connection"));
    info.add(&property(&i18n("Protocol"), conn.protocol.name()));
    info.add(&property(&i18n("Address"), &conn.address()));
    if !conn.other_addresses.is_empty() {
        info.add(&property(
            &i18n("Other Addresses"),
            &conn.other_addresses.join(", "),
        ));
    }
    if conn.takes_username() {
        let account = conn
            .account()
            .unwrap_or_else(|| i18n("Asked when connecting"));
        info.add(&property(&i18n("User Name"), &account));
    }
    if !conn.group.is_empty() {
        info.add(&property(&i18n("Group"), &conn.group));
    }
    if conn.last_used > 0
        && let Ok(when) = gtk::glib::DateTime::from_unix_local(conn.last_used)
        && let Ok(text) = when.format("%x %X")
    {
        info.add(&property(&i18n("Last Connected"), &text));
    }
    page.add(&info);
    page
}

/// A label and its value, the value set off as the thing to read.
fn property(title: &str, value: &str) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_title(title);
    row.set_subtitle(value);
    row.set_subtitle_selectable(true);
    row.add_css_class("property");
    row
}
