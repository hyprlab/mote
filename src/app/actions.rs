//! The window's actions, the menus that name them, and their shortcuts.

use adw::prelude::*;
use gtk::{gio, glib};
use relm4::prelude::*;

use super::{AppModel, AppMsg};
use crate::config::SortOrder;
use crate::i18n::i18n;
use crate::ui::preferences::PrefChange;

/// `win.<name>` actions and the message each sends, with their shortcuts.
const ACTIONS: &[(&str, &[&str])] = &[
    ("new-connection", &["<Control>n"]),
    ("import-rdp", &["<Control>o"]),
    ("import-remmina", &[]),
    ("scan-network", &[]),
    ("connect", &["<Control>Return"]),
    ("edit", &["<Control>e"]),
    ("duplicate", &[]),
    ("export-rdp", &[]),
    ("delete", &[]),
    ("search", &["<Control>f"]),
    ("settings", &["<Control>comma"]),
    ("shortcuts", &["<Control>question"]),
    ("about", &[]),
];

fn message(action: &str) -> AppMsg {
    match action {
        "new-connection" => AppMsg::NewConnection(None),
        "import-rdp" => AppMsg::ImportRdp,
        "import-remmina" => AppMsg::ImportRemmina,
        "scan-network" => AppMsg::ScanNetwork,
        "connect" => AppMsg::ConnectSelected,
        "edit" => AppMsg::EditSelected,
        "duplicate" => AppMsg::DuplicateSelected,
        "export-rdp" => AppMsg::ExportSelected,
        "delete" => AppMsg::DeleteSelected,
        "search" => AppMsg::FocusSearch,
        "settings" => AppMsg::OpenSettings,
        "shortcuts" => AppMsg::Shortcuts,
        _ => AppMsg::About,
    }
}

pub(super) fn install(
    window: &adw::ApplicationWindow,
    sender: &ComponentSender<AppModel>,
    sort: SortOrder,
) {
    let app = relm4::main_adw_application();
    // The shortcuts belong to this window and run after the focused widget
    // has had the key, never as the application's: GTK tries those first,
    // so they would take keys from a session's desktop, which keeps every
    // key it gets.
    let shortcuts = gtk::ShortcutController::new();
    let add = |accel: &str, action: &str| {
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string(accel),
            Some(gtk::NamedAction::new(action)),
        ));
    };
    for (name, accels) in ACTIONS {
        let input = sender.input_sender().clone();
        let action = gio::ActionEntry::builder(name)
            .activate(move |_: &adw::ApplicationWindow, _, _| input.emit(message(name)))
            .build();
        window.add_action_entries([action]);
        for accel in *accels {
            add(accel, &format!("win.{name}"));
        }
    }
    // A sidebar row's buttons and menu, which name their connection.
    for (name, message) in [
        ("connect-to", AppMsg::Connect as fn(String) -> AppMsg),
        ("disconnect-from", AppMsg::CloseSession),
        ("edit-connection", AppMsg::Edit),
        ("duplicate-connection", AppMsg::Duplicate),
        ("export-connection", AppMsg::Export),
        ("delete-connection", AppMsg::ConfirmDelete),
        ("new-group", AppMsg::NewGroup),
        ("rename-group", AppMsg::RenameGroup),
        ("ungroup", |old| AppMsg::Regroup {
            old,
            new: String::new(),
        }),
    ] {
        let input = sender.input_sender().clone();
        let action = gio::ActionEntry::builder(name)
            .parameter_type(Some(glib::VariantTy::STRING))
            .activate(move |_: &adw::ApplicationWindow, _, value| {
                if let Some(id) = value.and_then(|v| v.str()) {
                    input.emit(message(id.to_owned()));
                }
            })
            .build();
        window.add_action_entries([action]);
    }
    // A connection and the group to put it in.
    let input = sender.input_sender().clone();
    let action = gio::ActionEntry::builder("set-group")
        .parameter_type(Some(
            glib::VariantTy::new("(ss)").expect("a tuple of two strings"),
        ))
        .activate(move |_: &adw::ApplicationWindow, _, value| {
            if let Some((id, group)) = value.and_then(|v| v.get::<(String, String)>()) {
                input.emit(AppMsg::SetGroup { id, group });
            }
        })
        .build();
    window.add_action_entries([action]);
    // The sort menu's choice, a radio group.
    let input = sender.input_sender().clone();
    let action = gio::ActionEntry::builder("sort")
        .parameter_type(Some(glib::VariantTy::STRING))
        .state(sort.key().to_variant())
        .activate(move |_: &adw::ApplicationWindow, action, value| {
            if let Some(order) = value.and_then(|v| v.str()).and_then(SortOrder::from_key) {
                action.set_state(&order.key().to_variant());
                input.emit(AppMsg::Pref(PrefChange::Sort(order)));
            }
        })
        .build();
    window.add_action_entries([action]);
    add("<Control>q", "app.quit");
    add("<Control>w", "window.close");
    window.add_controller(shortcuts);

    let quit = gio::ActionEntry::builder("quit")
        .activate(|app: &adw::Application, _, _| {
            // Closing the windows, not app.quit(): the main window saves
            // its size as it closes.
            for window in app.windows() {
                window.close();
            }
        })
        .build();
    app.add_action_entries([quit]);
}

/// A menu item with its shortcut shown beside it. The menu can't find the
/// shortcut itself, since it isn't the application's.
fn item(label: &str, action: &str, accel: Option<&str>) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), Some(action));
    if let Some(accel) = accel {
        item.set_attribute_value("accel", Some(&accel.to_variant()));
    }
    item
}

/// The burger menu above the sidebar, in Hylki's order: Settings first,
/// then what the app does, then About, and Quit on its own.
pub(super) fn main_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let settings = gio::Menu::new();
    settings.append_item(&item(
        &i18n("Settings"),
        "win.settings",
        Some("<Control>comma"),
    ));
    menu.append_section(None, &settings);
    menu.append_section(None, &import_menu());
    let help = gio::Menu::new();
    help.append_item(&item(
        &i18n("Keyboard Shortcuts"),
        "win.shortcuts",
        Some("<Control>question"),
    ));
    help.append(Some(&i18n("About Mote")), Some("win.about"));
    menu.append_section(None, &help);
    let quit = gio::Menu::new();
    quit.append_item(&item(&i18n("Quit"), "app.quit", Some("<Control>q")));
    menu.append_section(None, &quit);
    menu
}

/// The sort button's menu.
pub(super) fn sort_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    for (label, order) in [
        (i18n("Name"), SortOrder::Name),
        (i18n("Recently Used"), SortOrder::RecentlyUsed),
        (i18n("Custom"), SortOrder::Custom),
    ] {
        let item = gio::MenuItem::new(Some(&label), None);
        item.set_action_and_target_value(Some("win.sort"), Some(&order.key().to_variant()));
        menu.append_item(&item);
    }
    let section = gio::Menu::new();
    section.append_section(Some(&i18n("Sort By")), &menu);
    section
}

/// Mark `order` in the sort menu, when something other than the menu
/// changed it.
pub(super) fn show_sort(window: &adw::ApplicationWindow, order: SortOrder) {
    if let Some(action) = window.lookup_action("sort") {
        action.change_state(&order.key().to_variant());
    }
}

/// The other ways to get connections, in the main menu.
pub(super) fn import_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    menu.append(Some(&i18n("Scan Network…")), Some("win.scan-network"));
    menu.append_item(&item(
        &i18n("Import .rdp and .vnc Files…"),
        "win.import-rdp",
        Some("<Control>o"),
    ));
    menu.append(
        Some(&i18n("Import from Remmina")),
        Some("win.import-remmina"),
    );
    menu
}

/// The menu at the end of a connection's header.
pub(super) fn connection_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let items = gio::Menu::new();
    items.append(Some(&i18n("Duplicate")), Some("win.duplicate"));
    items.append(Some(&i18n("Export to File…")), Some("win.export-rdp"));
    menu.append_section(None, &items);
    let delete = gio::Menu::new();
    delete.append(Some(&i18n("Delete…")), Some("win.delete"));
    menu.append_section(None, &delete);
    menu
}

/// Every shortcut. Those of a session are the ones that work while the
/// remote desktop has the keyboard. Built from rows rather than with
/// libadwaita 1.8's shortcuts dialog, which Debian 13 doesn't have.
pub(super) fn present_shortcuts(parent: &adw::ApplicationWindow) {
    let sections: [(String, Vec<(String, &str)>); 3] = [
        (
            i18n("Connections"),
            vec![
                (i18n("New Connection"), "<Control>n"),
                (i18n("Connect"), "<Control>Return"),
                (i18n("Edit Connection"), "<Control>e"),
                (i18n("Search"), "<Control>f"),
                (i18n("Import .rdp and .vnc Files…"), "<Control>o"),
                (i18n("Move Connection Up"), "<Alt>Up"),
                (i18n("Move Connection Down"), "<Alt>Down"),
                (i18n("Connection Menu"), "<Shift>F10"),
            ],
        ),
        (
            i18n("Sessions"),
            vec![
                (
                    i18n("Let Go of the Keyboard and Show the Header"),
                    if cfg!(target_os = "macos") {
                        "Meta_R"
                    } else {
                        "Control_R"
                    },
                ),
                (i18n("Give the Shortcuts Back"), "<Super>Escape"),
            ],
        ),
        (
            i18n("General"),
            vec![
                (i18n("Settings"), "<Control>comma"),
                (i18n("Keyboard Shortcuts"), "<Control>question"),
                (i18n("Close Window"), "<Control>w"),
                (i18n("Quit"), "<Control>q"),
            ],
        ),
    ];
    let page = adw::PreferencesPage::new();
    for (title, items) in sections {
        let group = adw::PreferencesGroup::new();
        group.set_title(&title);
        for (label, accel) in items {
            let row = adw::ActionRow::new();
            row.set_title(&label);
            // Deprecated for libadwaita 1.8's AdwShortcutLabel, which
            // Debian 13 doesn't have yet.
            #[allow(deprecated)]
            let keys = gtk::ShortcutLabel::new(accel);
            keys.set_valign(gtk::Align::Center);
            #[cfg(target_os = "macos")]
            if accel == "Meta_R" {
                command_key_label(&keys);
            }
            row.add_suffix(&keys);
            group.add(&row);
        }
        page.add(&group);
    }
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    let dialog = adw::Dialog::new();
    dialog.set_title(&i18n("Keyboard Shortcuts"));
    dialog.set_content_width(480);
    dialog.set_content_height(640);
    dialog.set_child(Some(&view));
    dialog.present(Some(parent));
}

/// GTK draws Command as ⌘ beside another key but as "Meta" on its own, the
/// way Right Command is shown: ⌘ there too, before GTK's side marker.
#[cfg(target_os = "macos")]
fn command_key_label(keys: &impl IsA<gtk::Widget>) {
    let mut child = keys.as_ref().first_child();
    while let Some(widget) = child {
        // GTK puts a narrow space before the marker.
        if let Some(label) = widget.downcast_ref::<gtk::Label>()
            && let Some(side) = label.label().find(char::is_whitespace)
        {
            label.set_label(&format!("⌘{}", &label.label()[side..]));
        }
        child = widget.next_sibling();
    }
}
