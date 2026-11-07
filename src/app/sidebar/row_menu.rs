//! The menu a connection's row opens on a right click, a long press, the
//! Menu key or Shift+F10, and the one a group's heading opens. Its items
//! name the row's connection or group, so it works on that one without
//! selecting it, and so without leaving a session.

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use super::super::groups::SharedGroups;
use super::{heading_group, row_id};
use crate::i18n::i18n;

pub(super) fn connect(list: &gtk::ListBox, groups: &SharedGroups) {
    let popover = gtk::PopoverMenu::builder()
        .has_arrow(false)
        .halign(gtk::Align::Start)
        .build();
    // Held by the handlers below: it has no parent until it first opens.

    let clicks = gtk::GestureClick::new();
    clicks.set_button(gdk::BUTTON_SECONDARY);
    clicks.connect_pressed(glib::clone!(
        #[weak]
        list,
        #[strong]
        popover,
        #[strong]
        groups,
        move |gesture, _, x, y| {
            if open_at(&list, &popover, &groups, x, y) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        }
    ));
    list.add_controller(clicks);

    let press = gtk::GestureLongPress::new();
    press.set_touch_only(true);
    press.connect_pressed(glib::clone!(
        #[weak]
        list,
        #[strong]
        popover,
        #[strong]
        groups,
        move |gesture, x, y| {
            if open_at(&list, &popover, &groups, x, y) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        }
    ));
    list.add_controller(press);

    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(glib::clone!(
        #[weak]
        list,
        #[strong]
        popover,
        #[strong]
        groups,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, mods| {
            let menu_key = (key == gdk::Key::Menu && mods.is_empty())
                || (key == gdk::Key::F10 && mods == gdk::ModifierType::SHIFT_MASK);
            // The focused row, under the keyboard.
            let row = list
                .focus_child()
                .and_downcast::<gtk::ListBoxRow>()
                .or_else(|| list.selected_row());
            match row.and_then(|row| Some((row.compute_bounds(&list)?, row))) {
                Some((bounds, row)) if menu_key => {
                    let x = f64::from(bounds.x() + bounds.width() / 2.0);
                    let y = f64::from(bounds.y() + bounds.height());
                    open(&list, &popover, &groups, &row, x, y);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        }
    ));
    list.add_controller(keys);
}

/// Open the menu of the row at (`x`, `y`), if it has one.
fn open_at(
    list: &gtk::ListBox,
    popover: &gtk::PopoverMenu,
    groups: &SharedGroups,
    x: f64,
    y: f64,
) -> bool {
    match list.row_at_y(y as i32) {
        Some(row) => open(list, popover, groups, &row, x, y),
        None => false,
    }
}

/// `x` and `y` are in the list's coordinates.
fn open(
    list: &gtk::ListBox,
    popover: &gtk::PopoverMenu,
    groups: &SharedGroups,
    row: &gtk::ListBoxRow,
    x: f64,
    y: f64,
) -> bool {
    let model = match (row_id(row), heading_group(row)) {
        (Some(id), _) => {
            let groups = groups.borrow();
            let current = groups.of.get(&id).map_or("", String::as_str);
            menu(&id, row.has_css_class("connected"), current, &groups.names)
        }
        (None, Some(group)) => group_menu(&group),
        (None, None) => return false,
    };
    // Set on the list's container, not the list: GtkListBox empties itself
    // by its first child, and loops on one that isn't a row.
    let Some(parent) = list.parent() else {
        return false;
    };
    if popover.parent().is_none() {
        popover.set_parent(&parent);
    }
    let Some(at) = list.compute_point(&parent, &gtk::graphene::Point::new(x as f32, y as f32))
    else {
        return false;
    };
    let (x, y) = (at.x(), at.y());
    popover.set_menu_model(Some(&model));
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.popup();
    true
}

/// A connection's menu: what the details page's header offers, for `id`,
/// and the groups it can move to from `current`.
fn menu(id: &str, connected: bool, current: &str, groups: &[String]) -> gio::Menu {
    let item = |label: String, action: &str| {
        let item = gio::MenuItem::new(Some(&label), None);
        item.set_action_and_target_value(Some(action), Some(&id.to_variant()));
        item
    };
    let menu = gio::Menu::new();
    let session = gio::Menu::new();
    session.append_item(&if connected {
        item(i18n("Disconnect"), "win.disconnect-from")
    } else {
        item(i18n("Connect"), "win.connect-to")
    });
    session.append_item(&item(i18n("Edit Connection"), "win.edit-connection"));
    menu.append_section(None, &session);
    let into = gio::Menu::new();
    let names = gio::Menu::new();
    let to = |label: &str, group: &str| {
        let item = gio::MenuItem::new(Some(label), None);
        item.set_action_and_target_value(
            Some("win.set-group"),
            Some(&(id.to_owned(), group.to_owned()).to_variant()),
        );
        item
    };
    if !current.is_empty() {
        names.append_item(&to(&i18n("No Group"), ""));
    }
    for group in groups.iter().filter(|g| *g != current) {
        names.append_item(&to(group, group));
    }
    into.append_section(None, &names);
    let new = gio::Menu::new();
    new.append_item(&item(i18n("New Group…"), "win.new-group"));
    into.append_section(None, &new);
    let grouping = gio::Menu::new();
    grouping.append_submenu(Some(&i18n("Move to Group")), &into);
    menu.append_section(None, &grouping);
    let copies = gio::Menu::new();
    copies.append_item(&item(i18n("Duplicate"), "win.duplicate-connection"));
    copies.append_item(&item(i18n("Export to File…"), "win.export-connection"));
    menu.append_section(None, &copies);
    let delete = gio::Menu::new();
    delete.append_item(&item(i18n("Delete…"), "win.delete-connection"));
    menu.append_section(None, &delete);
    menu
}

/// A group heading's menu.
fn group_menu(group: &str) -> gio::Menu {
    let item = |label: String, action: &str| {
        let item = gio::MenuItem::new(Some(&label), None);
        item.set_action_and_target_value(Some(action), Some(&group.to_variant()));
        item
    };
    let menu = gio::Menu::new();
    menu.append_item(&item(i18n("Rename Group…"), "win.rename-group"));
    menu.append_item(&item(i18n("Ungroup"), "win.ungroup"));
    menu
}
