//! The Settings categories' pages. Every row reports its change at once;
//! the main window applies and saves it.
//!
//! Combo and spin rows are set once, here, and never bound to a watched
//! value: Hylki's Settings window hung GTK's list item manager that way.

use adw::prelude::*;
use relm4::prelude::*;

use super::{PrefChange, Preferences};
use crate::config::{ColorScheme, Settings};
use crate::connection::{Quality, Scaling};
use crate::i18n::i18n;
use crate::ui::editor::{quality_cost, quality_label};
use crate::ui::session::keyboard::LAYOUTS;
use crate::ui::session::scaling_label;

pub(super) fn build(
    settings: &Settings,
    sender: &ComponentSender<Preferences>,
) -> Vec<(&'static str, adw::PreferencesPage)> {
    vec![
        ("defaults", defaults(settings, sender)),
        ("appearance", appearance(settings, sender)),
        ("sessions", sessions(settings, sender)),
        ("sidebar", sidebar(settings, sender)),
        ("video", super::video::page(settings, sender)),
    ]
}

fn page_with(groups: &[&adw::PreferencesGroup]) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    for group in groups {
        page.add(*group);
    }
    page
}

fn defaults(settings: &Settings, sender: &ComponentSender<Preferences>) -> adw::PreferencesPage {
    let group = adw::PreferencesGroup::new();

    let port = adw::SpinRow::with_range(1.0, 65535.0, 1.0);
    port.set_title(&i18n("Port"));
    port.set_value(f64::from(settings.default_port));
    let out = sender.output_sender().clone();
    port.connect_value_notify(move |r| out.emit(PrefChange::DefaultPort(r.value() as u16)));
    group.add(&port);

    let user = text_row(&i18n("User Name"), &settings.default_username);
    let out = sender.output_sender().clone();
    user.connect_changed(move |r| {
        out.emit(PrefChange::DefaultUsername(r.text().trim().to_owned()))
    });
    group.add(&user);

    let domain = text_row(&i18n("Domain"), &settings.default_domain);
    let out = sender.output_sender().clone();
    domain
        .connect_changed(move |r| out.emit(PrefChange::DefaultDomain(r.text().trim().to_owned())));
    group.add(&domain);

    let display = adw::PreferencesGroup::new();
    display.set_title(&i18n("Display"));
    let labels: Vec<String> = Scaling::ALL.into_iter().map(scaling_label).collect();
    let scaling = combo(&i18n("Scaling"), &labels);
    scaling.set_selected(position(&Scaling::ALL, &settings.default_scaling));
    let out = sender.output_sender().clone();
    scaling.connect_selected_notify(move |r| {
        if let Some(scaling) = Scaling::ALL.get(r.selected() as usize) {
            out.emit(PrefChange::DefaultScaling(*scaling));
        }
    });
    display.add(&scaling);
    let labels: Vec<String> = Quality::ALL.into_iter().map(quality_label).collect();
    let codec = combo(&i18n("Codec"), &labels);
    let out = sender.output_sender().clone();
    codec.connect_selected_notify(move |r| {
        if let Some(quality) = Quality::ALL.get(r.selected() as usize) {
            r.set_subtitle(&quality_cost(*quality));
            out.emit(PrefChange::DefaultQuality(*quality));
        }
    });
    codec.set_selected(position(&Quality::ALL, &settings.default_quality));
    codec.set_subtitle(&quality_cost(settings.default_quality));
    display.add(&codec);
    // The same rows as in the connection editor.
    for (title, on, change) in [
        (
            i18n("Open Fullscreen"),
            settings.default_fullscreen,
            PrefChange::DefaultFullscreen as fn(bool) -> PrefChange,
        ),
        (
            i18n("Open in New Window"),
            settings.default_new_window,
            PrefChange::DefaultNewWindow,
        ),
    ] {
        let row = adw::SwitchRow::new();
        row.set_title(&title);
        row.set_active(on);
        let out = sender.output_sender().clone();
        row.connect_active_notify(move |r| out.emit(change(r.is_active())));
        display.add(&row);
    }

    let sharing = adw::PreferencesGroup::new();
    sharing.set_title(&i18n("Sharing"));
    // The same rows, and the same limits, as in the connection editor.
    let audio_limit = i18n("Not with KRdp; xrdp needs its PipeWire module");
    for (title, subtitle, on, change) in [
        (
            i18n("Clipboard"),
            i18n("Text only with KRdp; xrdp leaves out folders"),
            settings.default_clipboard,
            PrefChange::DefaultClipboard as fn(bool) -> PrefChange,
        ),
        (
            i18n("Sound"),
            audio_limit.clone(),
            settings.default_sound,
            PrefChange::DefaultSound,
        ),
        (
            i18n("Microphone"),
            audio_limit,
            settings.default_microphone,
            PrefChange::DefaultMicrophone,
        ),
    ] {
        let row = adw::SwitchRow::new();
        row.set_title(&title);
        row.set_subtitle(&subtitle);
        row.set_active(on);
        let out = sender.output_sender().clone();
        row.connect_active_notify(move |r| out.emit(change(r.is_active())));
        sharing.add(&row);
    }

    page_with(&[&group, &display, &sharing])
}

fn sessions(settings: &Settings, sender: &ComponentSender<Preferences>) -> adw::PreferencesPage {
    let keyboard = adw::PreferencesGroup::new();
    keyboard.set_title(&i18n("Keyboard"));
    let mut labels = vec![i18n("Automatic")];
    labels.extend(LAYOUTS.iter().map(|l| i18n(l.name)));
    let layout = combo(&i18n("Keyboard Layout"), &labels);
    layout.set_subtitle(&i18n("Only supported by xrdp"));
    layout.set_enable_search(true);
    layout.set_expression(Some(gtk::PropertyExpression::new(
        gtk::StringObject::static_type(),
        None::<gtk::Expression>,
        "string",
    )));
    let chosen = LAYOUTS
        .iter()
        .position(|l| l.id == settings.keyboard_layout)
        .map_or(0, |i| i + 1);
    layout.set_selected(chosen as u32);
    let out = sender.output_sender().clone();
    layout.connect_selected_notify(move |r| {
        let id = match r.selected() as usize {
            0 => 0,
            i => LAYOUTS.get(i - 1).map_or(0, |l| l.id),
        };
        out.emit(PrefChange::KeyboardLayout(id));
    });
    keyboard.add(&layout);

    let grab = adw::SwitchRow::new();
    grab.set_title(&i18n("Grab Keyboard"));
    grab.set_subtitle(&if cfg!(target_os = "macos") {
        i18n("Right Command lets go of the keyboard")
    } else {
        i18n("Right Ctrl lets go of the keyboard")
    });
    grab.set_active(settings.grab_keyboard);
    let out = sender.output_sender().clone();
    grab.connect_active_notify(move |r| out.emit(PrefChange::GrabKeyboard(r.is_active())));
    keyboard.add(&grab);

    let network = adw::PreferencesGroup::new();
    network.set_title(&i18n("Network"));
    let reconnect = adw::SwitchRow::new();
    reconnect.set_title(&i18n("Reconnect Automatically"));
    reconnect.set_subtitle(&i18n("For two minutes after the network drops"));
    reconnect.set_active(settings.reconnect);
    let out = sender.output_sender().clone();
    reconnect.connect_active_notify(move |r| out.emit(PrefChange::Reconnect(r.is_active())));
    network.add(&reconnect);

    let notices = adw::PreferencesGroup::new();
    notices.set_title(&i18n("Notifications"));
    let once = i18n("Once a session");
    for (title, subtitle, on, change) in [
        (
            i18n("Keyboard Grab Notice"),
            once.clone(),
            settings.grab_notice,
            PrefChange::GrabNotice as fn(bool) -> PrefChange,
        ),
        (
            i18n("Missing H.264 Warning"),
            once.clone(),
            settings.h264_warning,
            PrefChange::H264Warning,
        ),
        (
            i18n("Desktop Sharing Size Notice"),
            once,
            settings.size_notice,
            PrefChange::SizeNotice,
        ),
        (
            i18n("Clipboard Sharing Notice"),
            i18n("Once a connection"),
            settings.clipboard_notice,
            PrefChange::ClipboardNotice,
        ),
    ] {
        let row = adw::SwitchRow::new();
        row.set_title(&title);
        row.set_subtitle(&subtitle);
        row.set_active(on);
        let out = sender.output_sender().clone();
        row.connect_active_notify(move |r| out.emit(change(r.is_active())));
        notices.add(&row);
    }

    page_with(&[&keyboard, &network, &notices])
}

fn combo(title: &str, labels: &[String]) -> adw::ComboRow {
    let row = adw::ComboRow::new();
    row.set_title(title);
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    row.set_model(Some(&gtk::StringList::new(&labels)));
    row
}

fn position<T: PartialEq>(all: &[T], value: &T) -> u32 {
    all.iter().position(|v| v == value).unwrap_or(0) as u32
}

fn appearance(settings: &Settings, sender: &ComponentSender<Preferences>) -> adw::PreferencesPage {
    let group = adw::PreferencesGroup::new();
    let row = adw::ActionRow::new();
    row.set_title(&i18n("Style"));

    let toggles = adw::ToggleGroup::new();
    toggles.set_valign(gtk::Align::Center);
    let choices = [
        ("system", i18n("System"), ColorScheme::System),
        ("light", i18n("Light"), ColorScheme::Light),
        ("dark", i18n("Dark"), ColorScheme::Dark),
    ];
    for (name, label, _) in &choices {
        toggles.add(adw::Toggle::builder().name(*name).label(label).build());
    }
    let current = choices
        .iter()
        .find(|(_, _, s)| *s == settings.color_scheme)
        .map_or("system", |(n, _, _)| *n);
    toggles.set_active_name(Some(current));
    let out = sender.output_sender().clone();
    toggles.connect_active_name_notify(move |t| {
        let picked = t.active_name();
        if let Some((_, _, scheme)) = choices
            .iter()
            .find(|(n, _, _)| Some(*n) == picked.as_deref())
        {
            out.emit(PrefChange::ColorScheme(*scheme));
        }
    });
    row.add_suffix(&toggles);
    group.add(&row);

    let pictures = adw::SwitchRow::new();
    pictures.set_title(&i18n("Show Large Account Avatars"));
    pictures.set_subtitle(&i18n("Still shown when editing a connection"));
    pictures.set_active(settings.show_large_avatars);
    let out = sender.output_sender().clone();
    pictures.connect_active_notify(move |r| out.emit(PrefChange::ShowLargeAvatars(r.is_active())));
    group.add(&pictures);
    page_with(&[&group])
}

fn sidebar(settings: &Settings, sender: &ComponentSender<Preferences>) -> adw::PreferencesPage {
    let group = adw::PreferencesGroup::new();
    group.set_title(&i18n("Connections"));

    let groups = adw::SwitchRow::new();
    groups.set_title(&i18n("Show Groups"));
    groups.set_active(settings.show_groups);
    let out = sender.output_sender().clone();
    groups.connect_active_notify(move |r| out.emit(PrefChange::ShowGroups(r.is_active())));
    group.add(&groups);

    let hosts = adw::SwitchRow::new();
    hosts.set_title(&i18n("Show Addresses"));
    hosts.set_active(settings.show_hosts);
    let out = sender.output_sender().clone();
    hosts.connect_active_notify(move |r| out.emit(PrefChange::ShowHosts(r.is_active())));
    group.add(&hosts);

    let avatars = adw::SwitchRow::new();
    avatars.set_title(&i18n("Show Account Avatars"));
    avatars.set_active(settings.show_avatars);
    let out = sender.output_sender().clone();
    avatars.connect_active_notify(move |r| out.emit(PrefChange::ShowAvatars(r.is_active())));
    group.add(&avatars);

    page_with(&[&group])
}

fn text_row(title: &str, text: &str) -> adw::EntryRow {
    let row = adw::EntryRow::new();
    row.set_title(title);
    row.set_text(text);
    row
}
