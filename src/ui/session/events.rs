//! What the session does with what the engine tells it, and with how the
//! session ended.

use adw::prelude::*;
use relm4::prelude::*;

use super::*;

impl Session {
    pub(super) fn event(
        &mut self,
        event: Event,
        sender: &ComponentSender<Self>,
        root: &adw::ToolbarView,
    ) {
        match event {
            Event::Certificate {
                host,
                port,
                fingerprint,
                of,
                also,
                reply,
            } => {
                let known = also
                    .iter()
                    .chain([&(host.clone(), port)])
                    .filter_map(|(host, port)| config::trusted_fingerprint(host, *port))
                    .collect();
                self.check_certificate((&host, port), fingerprint, of, known, reply, root);
            }
            Event::Trying(address) => {
                self.connecting
                    .set_description(Some(&gtk::glib::markup_escape_text(&address)));
                self.address = address;
            }
            Event::Waking => self.connecting.set_title(&i18n_f(
                "Waking {name}…",
                &[("name", &self.connection.title())],
            )),
            Event::Reconnecting => {
                self.connecting.set_title(&i18n_f(
                    "Reconnecting to {name}…",
                    &[("name", &self.connection.title())],
                ));
                self.page = Page::Connecting;
            }
            Event::Connected { width, height } => {
                tracing::info!("connected, {width}×{height}");
                // A reactivation reports the desktop again; only a new
                // connection counts.
                if self.page != Page::Desktop {
                    let _ = sender.output(SessionOutput::Connected(self.connection.id.clone()));
                    self.tell_clipboard(sender);
                    // The window may have changed size while the session
                    // was away, and a layout of several monitors is gone.
                    sender.input(SessionInput::Resized);
                    if let (Some(spread), Some(handle)) = (&self.spread, &self.handle) {
                        handle.send(remote::Command::Layout(spread.layout.clone()));
                    }
                }
                self.page = Page::Desktop;
                self.display.grab_focus();
                if std::mem::take(&mut self.remember) {
                    let _ = sender.output(SessionOutput::Remember {
                        id: self.connection.id.clone(),
                        username: self.username.clone(),
                        domain: self.domain.clone(),
                        password: self.password.clone(),
                    });
                }
            }
            Event::Frame(frame) => {
                self.display.show_frame(frame);
                self.check_size_kept(sender);
                if let (Some(spread), Some(texture)) = (&self.spread, self.display.texture()) {
                    for display in spread.displays() {
                        display.show_texture(texture.clone());
                    }
                }
                // The next frame can come now, and replace this one if it
                // isn't painted yet: what is painted is the newest there is.
                if let Some(handle) = &self.handle {
                    handle.send(remote::Command::FrameDone);
                }
            }
            Event::Bell => self.display.error_bell(),
            Event::Cursor(cursor) => {
                for display in self.spread.iter().flat_map(|s| s.displays()) {
                    display.set_remote_cursor(cursor.clone());
                }
                self.display.set_remote_cursor(cursor);
            }
            Event::KeepsSize => {
                self.display.set_keeps_size();
                self.check_size_kept(sender);
            }
            event @ (Event::Server(_)
            | Event::System(_)
            | Event::Graphics { .. }
            | Event::NoH264) => {
                if self.findings.learn(event) && self.h264_warning {
                    let toast =
                        adw::Toast::new(&i18n("The remote computer is not sending H.264 video"));
                    toast.set_button_label(Some(&i18n("How to Fix")));
                    toast.set_timeout(10);
                    let input = sender.input_sender().clone();
                    toast.connect_button_clicked(move |_| input.emit(SessionInput::H264Help));
                    self.toasts.add_toast(toast);
                }
            }
            Event::OneMonitor => {
                if self.spread.is_some() {
                    self.gather(sender);
                    self.toasts.add_toast(adw::Toast::new(&i18n(
                        "The remote desktop can only use one monitor",
                    )));
                }
            }
            Event::Clipboard(event) => {
                if let Some(clipboard) = &self.clipboard {
                    clipboard.event(event);
                }
            }
            Event::Ended(ending) => {
                self.finished();
                if let Some(spread) = self.spread.take() {
                    spread.close();
                    self.display.set_viewport(None);
                }
                self.display.report();
                self.clipboard = None;
                self.handle = None;
                self.ended(ending, sender);
            }
        }
    }

    fn ended(&mut self, ending: Ending, sender: &ComponentSender<Self>) {
        let was_connected = self.page == Page::Desktop;
        if !matches!(ending, Ending::Closed) {
            let _ = sender.output(SessionOutput::Stopped(self.connection.id.clone()));
        }
        match endings::shown(
            ending,
            &self.connection,
            was_connected,
            self.findings.offered,
        ) {
            endings::Shown::Nothing => {}
            endings::Shown::SignIn(message) => {
                self.page = Page::SignIn;
                self.sign_in.refused(&message);
            }
            endings::Shown::Page {
                title,
                description,
                help,
            } => {
                self.help.set_visible(help);
                self.ended.set_title(&title);
                self.ended
                    .set_description(Some(&gtk::glib::markup_escape_text(&description)));
                self.page = Page::Ended;
            }
        }
    }
}
