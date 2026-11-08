//! The session menu and what it does: where the session is, how the
//! desktop is scaled, whether keys and the pointer go to it, the key
//! combinations this computer would otherwise keep, and screenshots.

use std::path::PathBuf;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use relm4::prelude::*;

use super::{Session, SessionInput, pages::scaling_label};
use crate::connection::Scaling;
use crate::i18n::{i18n, i18n_f};

const CTRL: u16 = 0x1D;
const ALT: u16 = 0x38;
const TAB: u16 = 0x0F;
const DELETE: u16 = 0xE053;
const SUPER: u16 = 0xE05B;
const PRINT: u16 = 0xE037;

/// The scancodes of a combination the menu sends, by its action target.
pub(super) fn keys(name: &str) -> Option<Vec<u16>> {
    Some(match name {
        "ctrl-alt-delete" => vec![CTRL, ALT, DELETE],
        "super" => vec![SUPER],
        "alt-tab" => vec![ALT, TAB],
        "print" => vec![PRINT],
        _ => {
            let number: u16 = name.strip_prefix("ctrl-alt-f")?.parse().ok()?;
            let f = match number {
                1..=10 => 0x3A + number,
                11 => 0x57,
                12 => 0x58,
                _ => return None,
            };
            vec![CTRL, ALT, f]
        }
    })
}

/// The session menu. Detached, it offers the way back.
pub(super) fn menu(detached: bool) -> gio::Menu {
    let menu = gio::Menu::new();
    let about = gio::Menu::new();
    about.append(Some(&i18n("Connection Info")), Some("session.info"));
    menu.append_section(None, &about);
    let place = gio::Menu::new();
    place.append(
        Some(&if detached {
            i18n("Return to Main Window")
        } else {
            i18n("Open in New Window")
        }),
        Some("session.move"),
    );
    place.append(
        Some(&i18n("Fullscreen on All Monitors")),
        Some("session.all-monitors"),
    );
    menu.append_section(None, &place);

    let scaling = gio::Menu::new();
    for mode in Scaling::ALL {
        let item = gio::MenuItem::new(Some(&scaling_label(mode)), None);
        item.set_action_and_target_value(Some("session.scaling"), Some(&mode.key().to_variant()));
        scaling.append_item(&item);
    }
    menu.append_section(None, &scaling);

    let input = gio::Menu::new();
    input.append(Some(&i18n("Grab Keyboard")), Some("session.grab"));
    input.append(Some(&i18n("View Only")), Some("session.view-only"));
    let send = gio::Menu::new();
    let common = gio::Menu::new();
    for (label, name) in [
        (i18n("Ctrl+Alt+Delete"), "ctrl-alt-delete"),
        (i18n("Super"), "super"),
        (i18n("Alt+Tab"), "alt-tab"),
        (i18n("Print Screen"), "print"),
    ] {
        common.append_item(&key_item(&label, name));
    }
    send.append_section(None, &common);
    let consoles = gio::Menu::new();
    for number in 1..=12 {
        let label = i18n_f("Ctrl+Alt+F{number}", &[("number", &number.to_string())]);
        consoles.append_item(&key_item(&label, &format!("ctrl-alt-f{number}")));
    }
    send.append_section(None, &consoles);
    input.append_submenu(Some(&i18n("Send Keys")), &send);
    input.append(Some(&i18n("Take Screenshot")), Some("session.screenshot"));
    menu.append_section(None, &input);

    let quit = gio::Menu::new();
    let item = gio::MenuItem::new(
        Some(&i18n_f("Quit {name}", &[("name", &crate::APP_NAME)])),
        Some("app.quit"),
    );
    item.set_attribute_value("accel", Some(&"<Control>q".to_variant()));
    quit.append_item(&item);
    menu.append_section(None, &quit);
    menu
}

fn key_item(label: &str, name: &str) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), None);
    item.set_action_and_target_value(Some("session.keys"), Some(&name.to_variant()));
    item
}

/// The session menu's actions, under `session.`.
pub(super) fn install_actions(
    root: &adw::ToolbarView,
    sender: &ComponentSender<Session>,
    scaling: Scaling,
    grab: bool,
    view_only: bool,
) {
    let group = gio::SimpleActionGroup::new();
    let scale = gio::SimpleAction::new_stateful(
        "scaling",
        Some(glib::VariantTy::STRING),
        &scaling.key().to_variant(),
    );
    let input = sender.input_sender().clone();
    scale.connect_activate(move |action, value| {
        let Some(scaling) = value.and_then(|v| v.str()).and_then(Scaling::from_key) else {
            return;
        };
        action.set_state(&scaling.key().to_variant());
        input.emit(SessionInput::SetScaling(scaling));
    });
    group.add_action(&scale);

    for (name, initial, message) in [
        (
            "grab",
            grab,
            SessionInput::SetGrab as fn(bool) -> SessionInput,
        ),
        ("view-only", view_only, SessionInput::SetViewOnly),
    ] {
        let action = gio::SimpleAction::new_stateful(name, None, &initial.to_variant());
        let input = sender.input_sender().clone();
        action.connect_activate(move |action, _| {
            let on = !action
                .state()
                .and_then(|s| s.get::<bool>())
                .unwrap_or(false);
            action.set_state(&on.to_variant());
            input.emit(message(on));
        });
        group.add_action(&action);
    }

    let keys = gio::SimpleAction::new("keys", Some(glib::VariantTy::STRING));
    let input = sender.input_sender().clone();
    keys.connect_activate(move |_, value| {
        if let Some(name) = value.and_then(|v| v.str()) {
            input.emit(SessionInput::SendKeys(name.to_owned()));
        }
    });
    group.add_action(&keys);

    for (name, message) in [
        (
            "disconnect",
            (|| SessionInput::Disconnect) as fn() -> SessionInput,
        ),
        ("move", || SessionInput::ToggleDetached),
        ("screenshot", || SessionInput::Screenshot),
        ("info", || SessionInput::ShowInfo),
        ("all-monitors", || SessionInput::AllMonitors),
    ] {
        let action = gio::SimpleAction::new(name, None);
        let input = sender.input_sender().clone();
        action.connect_activate(move |_, _| input.emit(message()));
        group.add_action(&action);
        if name == "all-monitors"
            && let Some(display) = gdk::Display::default()
        {
            // Only with more than one monitor to take.
            let monitors = display.monitors();
            action.set_enabled(monitors.n_items() > 1);
            monitors.connect_items_changed(glib::clone!(
                #[weak]
                action,
                move |list, _, _, _| action.set_enabled(list.n_items() > 1)
            ));
        }
    }
    root.insert_action_group("session", Some(&group));
}

/// Where a screenshot of the desktop called `title` goes: the Screenshots
/// folder GNOME's own screenshots go to, named for the connection and the
/// time.
pub(super) fn screenshot_path(title: &str) -> PathBuf {
    let pictures =
        glib::user_special_dir(glib::UserDirectory::Pictures).unwrap_or_else(glib::home_dir);
    let stamp = glib::DateTime::now_local()
        .and_then(|now| now.format("%Y-%m-%d %H-%M-%S"))
        .map(|s| s.to_string())
        .unwrap_or_default();
    let name: String = title
        .chars()
        .map(|c| if c == '/' { '-' } else { c })
        .collect();
    pictures
        .join(i18n("Screenshots"))
        .join(format!("{name} {stamp}.png"))
}

/// Write the desktop as a PNG; slow enough at 4K to keep off the main
/// thread.
pub(super) async fn save_screenshot(
    texture: gdk::Texture,
    path: PathBuf,
) -> Result<PathBuf, String> {
    gio::spawn_blocking(move || {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        texture.save_to_png(&path).map_err(|e| e.to_string())?;
        Ok(path)
    })
    .await
    .map_err(|_| "the screenshot thread failed".to_owned())?
}
