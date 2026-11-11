//! The Video page: where H.264 comes from, and Cisco's OpenH264 to download
//! when the system has no decoder of its own.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use relm4::prelude::*;

use super::{PrefChange, Preferences};
use crate::config::Settings;
use crate::i18n::{i18n, i18n_f};
use crate::rdp::h264::{self, Source, cisco};

pub(super) fn page(
    settings: &Settings,
    sender: &ComponentSender<Preferences>,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::new();
    group.set_title("H.264");
    group.set_description(Some(&i18n("Changes apply to the next connection.")));

    let decoder = adw::ComboRow::new();
    decoder.set_title(&i18n("Preferred Decoder"));
    group.add(&decoder);

    let download = adw::ActionRow::new();
    download.set_title(&i18n("Cisco’s OpenH264"));
    let button = gtk::Button::new();
    button.set_valign(gtk::Align::Center);
    download.add_suffix(&button);
    download.set_visible(cisco::offered());
    group.add(&download);

    let rows = Rows {
        decoder: decoder.clone(),
        download,
        button: button.clone(),
        offered: Rc::default(),
        chosen: Rc::new(Cell::new(settings.decoder)),
        quiet: Rc::new(Cell::new(false)),
        out: sender.output_sender().clone(),
    };
    button.connect_clicked(glib::clone!(
        #[strong]
        rows,
        move |_| rows.act()
    ));
    decoder.connect_selected_notify(glib::clone!(
        #[strong]
        rows,
        move |_| rows.picked()
    ));
    rows.refresh();

    page.add(&group);
    page
}

fn label(source: Source) -> String {
    match source {
        Source::Ffmpeg => i18n("FFmpeg"),
        Source::SystemOpenH264 => i18n("System OpenH264"),
        Source::CiscoOpenH264 => i18n("Cisco’s OpenH264"),
        // A name, so never translated.
        #[cfg(target_os = "macos")]
        Source::VideoToolbox => "VideoToolbox".to_owned(),
    }
}

#[derive(Clone)]
struct Rows {
    decoder: adw::ComboRow,
    download: adw::ActionRow,
    button: gtk::Button,
    /// The sources the decoder row offers, in the order they are tried
    /// when none is chosen.
    offered: Rc<RefCell<Vec<Source>>>,
    chosen: Rc<Cell<Option<Source>>>,
    /// Set while the row's choices are replaced, which changes its selection.
    quiet: Rc<Cell<bool>>,
    out: relm4::Sender<PrefChange>,
}

impl Rows {
    /// Find the decoders again, off the main thread: finding one loads it.
    fn refresh(&self) {
        self.decoder.set_sensitive(false);
        self.decoder.set_subtitle(&i18n("Checking…"));
        let downloaded = cisco::downloaded();
        self.download.set_subtitle(&if downloaded {
            i18n("Downloaded from Cisco, under Cisco’s patent license")
        } else {
            i18n("Downloaded from Cisco on request, under Cisco’s patent license")
        });
        self.button.set_label(&if downloaded {
            i18n("Remove")
        } else {
            i18n("Download")
        });
        self.button.set_sensitive(true);
        self.button.remove_css_class("suggested-action");
        let rows = self.clone();
        glib::spawn_future_local(async move {
            let have = gio::spawn_blocking(h264::available)
                .await
                .unwrap_or_default();
            // The download is the way forward only without a decoder.
            if have.is_empty() && !downloaded {
                rows.button.add_css_class("suggested-action");
            }
            let labels: Vec<String> = have.iter().map(|s| label(*s)).collect();
            let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
            // None chosen, or the one chosen gone: the first, which is
            // what sessions then use.
            let chosen = rows.chosen.get();
            let selected = have.iter().position(|s| Some(*s) == chosen).unwrap_or(0);
            rows.quiet.set(true);
            rows.decoder.set_model(Some(&gtk::StringList::new(&labels)));
            rows.decoder.set_selected(selected as u32);
            rows.quiet.set(false);
            *rows.offered.borrow_mut() = have;
            rows.decoder.set_sensitive(rows.offered.borrow().len() > 1);
            rows.describe();
        });
    }

    /// Without a decoder, what that costs.
    fn describe(&self) {
        let subtitle = if self.offered.borrow().is_empty() {
            i18n("None: servers send RemoteFX instead, which blurs small text")
        } else {
            String::new()
        };
        self.decoder.set_subtitle(&subtitle);
    }

    fn picked(&self) {
        if self.quiet.get() {
            return;
        }
        let source = self
            .offered
            .borrow()
            .get(self.decoder.selected() as usize)
            .copied();
        self.choose(source);
        self.describe();
    }

    fn choose(&self, source: Option<Source>) {
        self.chosen.set(source);
        self.out.emit(PrefChange::Decoder(source));
    }

    fn act(&self) {
        self.button.set_sensitive(false);
        let rows = self.clone();
        if cisco::downloaded() {
            if let Err(e) = cisco::remove() {
                tracing::warn!("removing OpenH264: {e}");
            }
            if self.chosen.get() == Some(Source::CiscoOpenH264) {
                self.choose(None);
            }
            return rows.refresh();
        }
        self.download.set_subtitle(&i18n("Downloading…"));
        glib::spawn_future_local(async move {
            match gio::spawn_blocking(cisco::download).await {
                Ok(Ok(())) => rows.refresh(),
                Ok(Err(e)) => {
                    tracing::warn!("downloading OpenH264: {e}");
                    rows.refresh();
                    rows.download
                        .set_subtitle(&i18n_f("The download failed: {error}", &[("error", &e)]));
                }
                Err(_) => rows.refresh(),
            }
        });
    }
}
