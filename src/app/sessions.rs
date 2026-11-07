//! The open sessions: each a page of the content pane, or a window of its
//! own once detached, and what they report back.

use adw::prelude::*;
use relm4::prelude::*;

use super::{AppCmd, AppModel, AppMsg};
use crate::config;
use crate::secrets;
use crate::ui::session::{Session, SessionInit, SessionInput, SessionOutput};

/// The content pane's page for a connection's session.
pub(super) fn page_name(id: &str) -> String {
    format!("session:{id}")
}

impl AppModel {
    /// Open a session to the connection, or show the one already open.
    pub(super) fn connect(&mut self, id: &str, sender: &ComponentSender<Self>) {
        if let Some(window) = self.detached.get(id) {
            window.present();
            return;
        }
        if !self.sessions.contains_key(id) {
            let Some(connection) = self.connections.iter().find(|c| c.id == id).cloned() else {
                return;
            };
            let session = Session::builder()
                .launch(SessionInit {
                    connection,
                    grab: self.settings.grab_keyboard,
                    reconnect: self.settings.reconnect,
                    keyboard_layout: self.settings.keyboard_layout,
                    grab_notice: self.settings.grab_notice,
                    h264_warning: self.settings.h264_warning,
                    size_notice: self.settings.size_notice,
                    clipboard_notice: self.settings.clipboard_notice,
                })
                .forward(sender.input_sender(), AppMsg::Session);
            let new_window = connection_new_window(&self.connections, id);
            self.panes.add_named(session.widget(), Some(&page_name(id)));
            self.sessions.insert(id.to_owned(), session);
            self.rebuild_list();
            // Moved before it is ever shown here, so the first window it
            // maps in, and goes fullscreen in, is its own.
            if new_window {
                self.detach(id, sender);
            }
        }
        self.select(Some(id.to_owned()));
    }

    pub(super) fn session_output(&mut self, output: SessionOutput, sender: &ComponentSender<Self>) {
        match output {
            SessionOutput::Connected(id) => self.session_connected(&id),
            SessionOutput::Remember {
                id,
                username,
                domain,
                password,
            } => self.remember(&id, username, domain, password, sender),
            SessionOutput::Stopped(id) => {
                self.live.remove(&id);
                self.rebuild_list();
            }
            SessionOutput::Closed(id) => self.close_session(&id),
            SessionOutput::ToggleSidebar => {
                self.split.set_show_sidebar(!self.split.shows_sidebar())
            }
            SessionOutput::Scaling(id, scaling) => {
                if let Some(connection) = self.connections.iter_mut().find(|c| c.id == id) {
                    connection.scaling = scaling;
                    config::save_connections(&self.connections);
                }
            }
            SessionOutput::System {
                id,
                server,
                distro,
                graphics,
            } => {
                if let Some(connection) = self.connections.iter_mut().find(|c| c.id == id) {
                    (connection.server, connection.distro, connection.graphics) =
                        (server, distro, graphics);
                    config::save_connections(&self.connections);
                }
            }
            SessionOutput::ViewOnly(id, view_only) => {
                if let Some(connection) = self.connections.iter_mut().find(|c| c.id == id) {
                    connection.view_only = view_only;
                    config::save_connections(&self.connections);
                }
            }
            SessionOutput::ClipboardTold(id) => {
                if let Some(connection) = self.connections.iter_mut().find(|c| c.id == id) {
                    connection.clipboard_told = true;
                    config::save_connections(&self.connections);
                }
            }
            SessionOutput::AllowUnencrypted(id) => {
                if let Some(connection) = self.connections.iter_mut().find(|c| c.id == id) {
                    connection.allow_unencrypted = true;
                    config::save_connections(&self.connections);
                }
            }
            SessionOutput::ProtectionTold(id) => {
                if let Some(connection) = self.connections.iter_mut().find(|c| c.id == id) {
                    connection.protection_told = true;
                    config::save_connections(&self.connections);
                }
            }
            SessionOutput::Detach(id) => self.detach(&id, sender),
            SessionOutput::Attach(id) => self.attach(&id),
        }
    }

    fn close_session(&mut self, id: &str) {
        let Some(session) = self.sessions.remove(id) else {
            return;
        };
        self.live.remove(id);
        match self.detached.remove(id) {
            Some(window) => window.destroy(),
            None => self.panes.remove(session.widget()),
        }
        self.rebuild_list();
        if self.selected.as_deref() == Some(id) {
            self.select(Some(id.to_owned()));
        }
    }

    /// Move a session into a window of its own.
    fn detach(&mut self, id: &str, sender: &ComponentSender<Self>) {
        let Some(session) = self.sessions.get(id) else {
            return;
        };
        let widget = session.widget().clone();
        let size = (self.panes.width().max(800), self.panes.height().max(600));
        self.panes.remove(&widget);
        let window = adw::ApplicationWindow::builder()
            .application(&relm4::main_adw_application())
            .title(widget_title(&self.connections, id))
            .default_width(size.0)
            .default_height(size.1)
            .content(&widget)
            .build();
        // Closing the window ends its session.
        let input = sender.input_sender().clone();
        let closing = id.to_owned();
        window.connect_close_request(move |_| {
            input.emit(AppMsg::CloseSession(closing.clone()));
            gtk::glib::Propagation::Stop
        });
        session.emit(SessionInput::SetDetached(true));
        window.present();
        self.detached.insert(id.to_owned(), window);
        if self.selected.as_deref() == Some(id) {
            self.select(Some(id.to_owned()));
        }
    }

    /// Bring a detached session back into the main window.
    fn attach(&mut self, id: &str) {
        let (Some(session), Some(window)) = (self.sessions.get(id), self.detached.remove(id))
        else {
            return;
        };
        window.set_content(None::<&gtk::Widget>);
        window.destroy();
        self.panes.add_named(session.widget(), Some(&page_name(id)));
        session.emit(SessionInput::SetDetached(false));
        self.select(Some(id.to_owned()));
    }

    /// The session's page, when it is in the main window.
    pub(super) fn session_page(&self, id: &str) -> Option<String> {
        (self.sessions.contains_key(id) && !self.detached.contains_key(id)).then(|| page_name(id))
    }

    pub(super) fn end_session(&self, id: &str) {
        if let Some(session) = self.sessions.get(id) {
            session.emit(SessionInput::Disconnect);
        }
    }

    fn session_connected(&mut self, id: &str) {
        self.live.insert(id.to_owned());
        let Some(connection) = self.connections.iter_mut().find(|c| c.id == id) else {
            return;
        };
        connection.last_used = gtk::glib::real_time() / 1_000_000;
        config::save_connections(&self.connections);
        self.rebuild_list();
    }

    /// Keep the account typed at sign-in: the password in the keyring, the
    /// user name and domain in the profile.
    fn remember(
        &mut self,
        id: &str,
        username: String,
        domain: String,
        password: secrets::Password,
        sender: &ComponentSender<Self>,
    ) {
        let Some(connection) = self.connections.iter_mut().find(|c| c.id == id) else {
            return;
        };
        connection.username = username;
        connection.domain = domain;
        let title = connection.title().to_owned();
        config::save_connections(&self.connections);
        let saved = id.to_owned();
        sender.oneshot_command(async move {
            AppCmd::Keyring(
                secrets::set_password(saved, secrets::Secret::Account, title, password).await,
            )
        });
        self.rebuild_list();
        self.refresh_details();
    }
}

fn connection_new_window(connections: &[crate::connection::Connection], id: &str) -> bool {
    connections.iter().any(|c| c.id == id && c.new_window)
}

fn widget_title(connections: &[crate::connection::Connection], id: &str) -> String {
    connections
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.title().to_owned())
        .unwrap_or_default()
}
