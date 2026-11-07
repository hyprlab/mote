//! The session window's pages around the desktop, and the certificate
//! question.

use adw::prelude::*;
use relm4::prelude::*;

use super::{Session, SessionInput};
use crate::connection::{Connection, Scaling};
use crate::i18n::{i18n, i18n_f};
use crate::remote::Identity;

/// What the sign-in page holds.
pub(super) struct Account {
    pub(super) username: String,
    pub(super) domain: String,
    pub(super) password: crate::secrets::Password,
    pub(super) remember: bool,
}

/// Asks for the account a connection has no saved password for, or for
/// another after the server refused one.
pub(super) struct SignIn {
    pub(super) page: adw::StatusPage,
    username: adw::EntryRow,
    password: adw::PasswordEntryRow,
    domain: adw::EntryRow,
    remember: adw::SwitchRow,
}

impl SignIn {
    pub(super) fn new(connection: &Connection, sender: &ComponentSender<Session>) -> Self {
        let page = adw::StatusPage::new();
        page.set_icon_name(Some("dialog-password-symbolic"));
        page.set_title(&i18n("Sign In"));

        let username = adw::EntryRow::new();
        username.set_title(&i18n("User Name"));
        username.set_text(&connection.username);
        let password = adw::PasswordEntryRow::new();
        password.set_title(&i18n("Password"));
        let domain = adw::EntryRow::new();
        domain.set_title(&i18n("Domain"));
        domain.set_text(&connection.domain);
        let remember = adw::SwitchRow::new();
        remember.set_title(&i18n("Remember Password"));
        remember.set_active(true);

        let group = adw::PreferencesGroup::new();
        for row in [
            username.upcast_ref::<gtk::Widget>(),
            password.upcast_ref(),
            domain.upcast_ref(),
            remember.upcast_ref(),
        ] {
            group.add(row);
        }
        for row in [&username, password.upcast_ref(), &domain] {
            let input = sender.input_sender().clone();
            row.connect_entry_activated(move |_| input.emit(SessionInput::SignIn));
        }

        let connect = gtk::Button::with_label(&i18n("Connect"));
        connect.add_css_class("pill");
        connect.add_css_class("suggested-action");
        connect.set_halign(gtk::Align::Center);
        let input = sender.input_sender().clone();
        connect.connect_clicked(move |_| input.emit(SessionInput::SignIn));

        let content = gtk::Box::new(gtk::Orientation::Vertical, 24);
        content.append(&group);
        content.append(&connect);
        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(400);
        clamp.set_child(Some(&content));
        page.set_child(Some(&clamp));

        Self {
            page,
            username,
            password,
            domain,
            remember,
        }
    }

    pub(super) fn account(&self) -> Account {
        Account {
            username: self.username.text().trim().to_owned(),
            domain: self.domain.text().trim().to_owned(),
            password: crate::secrets::Password::new(self.password.text().into()),
            remember: self.remember.is_active(),
        }
    }

    /// Put the cursor where typing should start.
    pub(super) fn focus(&self) {
        if self.username.text().trim().is_empty() {
            self.username.grab_focus();
        } else {
            self.password.grab_focus();
        }
    }

    /// The server turned the account down: say so, and ask again.
    pub(super) fn refused(&self, message: &str) {
        self.page
            .set_description(Some(&gtk::glib::markup_escape_text(message)));
        self.password.set_text("");
        self.focus();
    }
}

/// The scaling modes as the menu and the editor name them.
pub(crate) fn scaling_label(mode: Scaling) -> String {
    match mode {
        Scaling::Resize => i18n("Resize to Window"),
        Scaling::Fit => i18n("Scale to Fit"),
        Scaling::Actual => i18n("Actual Size"),
    }
}

/// The page shown while connecting; its title says to what, and how.
pub(super) fn connecting(page: &adw::StatusPage) {
    page.set_paintable(Some(&adw::SpinnerPaintable::new(Some(page))));
}

/// Shown when a session ends without the window closing; the reason goes
/// in its title and description. With it, How to Fix, shown when the
/// server ended it for want of H.264.
pub(super) fn ended(sender: &ComponentSender<Session>) -> (adw::StatusPage, gtk::Button) {
    let page = adw::StatusPage::new();
    page.set_icon_name(Some("network-offline-symbolic"));
    let buttons = gtk::Box::new(gtk::Orientation::Vertical, 12);
    buttons.set_halign(gtk::Align::Center);
    let reconnect = gtk::Button::with_label(&i18n("Reconnect"));
    reconnect.add_css_class("pill");
    reconnect.add_css_class("suggested-action");
    let input = sender.input_sender().clone();
    reconnect.connect_clicked(move |_| input.emit(SessionInput::Reconnect));
    buttons.append(&reconnect);
    let help = gtk::Button::with_label(&i18n("How to Fix"));
    help.add_css_class("pill");
    help.set_visible(false);
    let input = sender.input_sender().clone();
    help.connect_clicked(move |_| input.emit(SessionInput::H264Help));
    buttons.append(&help);
    page.set_child(Some(&buttons));
    (page, help)
}

/// Ask whether to trust a server's certificate, an SSH server's key or a
/// gateway's certificate: one never seen before, or one that differs from
/// the one trusted last time. The answer is the response "trust" or
/// "cancel".
pub(super) fn certificate_dialog(
    name: &str,
    of: Identity,
    fingerprint: &str,
    changed: bool,
) -> adw::AlertDialog {
    let named = |text: &str| i18n_f(text, &[("name", &name)]);
    let (heading, body) = match (of, changed) {
        (Identity::Server, true) => (
            i18n("The Computer’s Identity Has Changed"),
            named(
                "{name} presented a different certificate from the one trusted before. Someone may be intercepting the connection, or the computer was set up again.",
            ),
        ),
        (Identity::Server, false) => (
            named("Trust {name}?"),
            i18n(
                "Check that this fingerprint matches the one in the computer’s Remote Desktop settings.",
            ),
        ),
        (Identity::Ssh, true) => (
            i18n("The SSH Server’s Identity Has Changed"),
            named(
                "{name} presented a different key from the one trusted before. Someone may be intercepting the connection, or the server was set up again.",
            ),
        ),
        (Identity::Ssh, false) => (
            named("Trust {name}?"),
            i18n(
                "This SSH server is new to this computer. Check that this fingerprint matches its host key.",
            ),
        ),
        (Identity::Gateway, true) => (
            i18n("The Gateway’s Identity Has Changed"),
            named(
                "{name} presented a different certificate from the one trusted before. Someone may be intercepting the connection, or the gateway was set up again.",
            ),
        ),
        (Identity::Gateway, false) => (
            named("Trust {name}?"),
            i18n(
                "No authority this computer knows vouches for this gateway’s certificate. Check that this fingerprint matches it.",
            ),
        ),
    };
    let dialog = adw::AlertDialog::new(Some(&heading), None);
    dialog.set_body_use_markup(true);
    dialog.set_body(&format!(
        "{}\n\n<tt>{}</tt>",
        gtk::glib::markup_escape_text(&body),
        gtk::glib::markup_escape_text(&in_lines(fingerprint)),
    ));
    dialog.add_response("cancel", &i18n("Cancel"));
    if changed {
        dialog.add_response("trust", &i18n("Connect Anyway"));
        dialog.set_response_appearance("trust", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
    } else {
        dialog.add_response("trust", &i18n("Trust and Connect"));
        dialog.set_response_appearance("trust", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("trust"));
    }
    dialog.set_close_response("cancel");
    dialog
}

/// A SHA-256 fingerprint is 32 pairs; 8 to a line fits a dialog.
fn in_lines(fingerprint: &str) -> String {
    let pairs: Vec<&str> = fingerprint.split(':').collect();
    pairs
        .chunks(8)
        .map(|line| line.join(":"))
        .collect::<Vec<_>>()
        .join("\n")
}
