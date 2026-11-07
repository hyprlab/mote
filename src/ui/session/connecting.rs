//! Starting a session: the desktop asked for, the account, and the way
//! to the computer.

use adw::prelude::*;
use gtk::glib;
use relm4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use tokio::sync::oneshot;

use super::{Page, Session, SessionCmd, SessionInput, clipboard, keyboard, pages, window_of};
use crate::config;
use crate::i18n::i18n_f;
use crate::remote::{self, Identity, Trust};

impl Session {
    /// Not waiting for the window to go fullscreen: it isn't to, or the page
    /// has the monitor's size already. The window says it is fullscreen
    /// before the page has grown, and in the main window the sidebar is
    /// still sliding away then.
    fn fullscreen_settled(&mut self) -> bool {
        if self.fullscreen_pending
            && self.fullscreen.get()
            && let Some(surface) = window_of(&self.stack).and_then(|w| w.surface())
            && let Some(monitor) = surface.display().monitor_at_surface(&surface)
        {
            let room = monitor.geometry();
            self.fullscreen_pending =
                (self.stack.width(), self.stack.height()) != (room.width(), room.height());
        }
        !self.fullscreen_pending
    }

    pub(super) fn connect(&mut self, sender: &ComponentSender<Self>) {
        self.clipboard = None;
        if let Some(handle) = self.handle.take() {
            handle.send(remote::Command::Close);
            self.finished();
        }
        self.page = Page::Connecting;
        self.connecting.set_title(&i18n_f(
            "Connecting to {name}…",
            &[("name", &self.connection.title())],
        ));
        self.connecting.set_description(None);
        // The desktop is asked for at the page's size, so wait for there to
        // be one, and for the window to have gone fullscreen when it opens so.
        let size = self.desktop_size().filter(|_| self.fullscreen_settled());
        let Some((width, height, scale)) = size else {
            // Polled rather than tied to the page's frame clock, which
            // doesn't run while another connection is showing.
            let input = sender.input_sender().clone();
            let stack = self.stack.downgrade();
            glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
                let Some(stack) = stack.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let sized = stack.width() > 0 || window_of(&stack).is_some_and(|w| w.width() > 0);
                if !sized {
                    return glib::ControlFlow::Continue;
                }
                input.emit(SessionInput::Connect);
                glib::ControlFlow::Break
            });
            return;
        };
        self.display.set_asked(width, height);
        let target = remote::Target {
            protocol: self.connection.protocol,
            allow_unencrypted: self.connection.allow_unencrypted,
            vnc_codec: self.connection.vnc_codec,
            resize: self.scaling == crate::connection::Scaling::Resize,
            host: self.connection.host.clone(),
            port: self.connection.port,
            addresses: self.connection.addresses(),
            username: self.username.clone(),
            password: self.password.to_string(),
            domain: self.domain.clone(),
            width,
            height,
            scale,
            quality: self.connection.quality,
            network: self.connection.network,
            clipboard: self.connection.clipboard,
            clipboard_files: self.connection.clipboard_files,
            sound: self.connection.sound,
            microphone: self.connection.microphone,
            folder: self.connection.shared_folder(),
            via: self.via(),
            wake: self.connection.wake_address(),
            reconnect: self.reconnect,
            keyboard_layout: keyboard::layout_id(self.keyboard_layout, &self.display),
        };
        let (handle, mut events) = remote::start(target);
        self.begun = true;
        let input = handle.clone();
        self.display
            .set_input(move |events| input.send(remote::Command::Input(events)));
        if self.connection.clipboard {
            self.clipboard = Some(clipboard::Sharing::new(
                &self.display,
                handle.clone(),
                self.connection.clipboard_files,
            ));
        }
        self.handle = Some(handle);
        self.attempt += 1;
        let attempt = self.attempt;
        sender.command(move |out, shutdown| {
            shutdown
                .register(async move {
                    while let Some(event) = events.recv().await {
                        if out.send(SessionCmd::Event(attempt, event)).is_err() {
                            break;
                        }
                    }
                })
                .drop_on_shutdown()
        });
    }

    /// `known`: what is trusted for this computer, under any of its
    /// addresses: all of them lead to one computer, so one question will do.
    pub(super) fn check_certificate(
        &self,
        (host, port): (&str, u16),
        fingerprint: String,
        of: Identity,
        known: Vec<String>,
        reply: oneshot::Sender<Trust>,
        root: &adw::ToolbarView,
    ) {
        if known.contains(&fingerprint) {
            let _ = reply.send(Trust::Known);
            return;
        }
        let reply = Rc::new(RefCell::new(Some(reply)));
        let name = match of {
            Identity::Server | Identity::ServerKey => self.connection.title(),
            Identity::Ssh | Identity::Gateway => host,
        };
        let dialog = pages::certificate_dialog(name, of, &fingerprint, !known.is_empty());
        let (host, port) = (host.to_owned(), port);
        dialog.connect_response(None, move |_, response| {
            let Some(reply) = reply.borrow_mut().take() else {
                return;
            };
            if response == "trust" {
                config::trust_certificate(&host, port, of == Identity::ServerKey, &fingerprint);
                let _ = reply.send(Trust::Accepted);
            } else {
                let _ = reply.send(Trust::Rejected);
            }
        });
        dialog.present(Some(root));
    }

    /// The way to the computer: through the SSH server or the gateway, if
    /// the connection has one.
    fn via(&self) -> remote::Via {
        let c = &self.connection;
        if let Some((host, port)) = c.ssh_server() {
            let username = if c.ssh_username.is_empty() {
                glib::user_name().to_string_lossy().into_owned()
            } else {
                c.ssh_username.clone()
            };
            return remote::Via::Ssh(remote::Hop {
                host,
                port,
                username,
                password: self.ssh_password.to_string(),
            });
        }
        if let Some((host, port)) = c.gateway_server() {
            // Without an account of its own, the gateway takes the one
            // signing in to the computer, as Windows' client does.
            let (username, password) = if c.gateway_username.is_empty() {
                let account = if self.domain.is_empty() {
                    self.username.clone()
                } else {
                    format!("{}\\{}", self.domain, self.username)
                };
                (account, self.password.to_string())
            } else {
                (
                    c.gateway_username.clone(),
                    self.gateway_password.to_string(),
                )
            };
            return remote::Via::Gateway(remote::Hop {
                host,
                port,
                username,
                password,
            });
        }
        remote::Via::Direct
    }
}
