//! The sidebar's connection list, rebuilt whole when the connections, the
//! search or the settings change. A list of a few dozen rows rebuilds faster
//! than it can be kept in step.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use relm4::prelude::*;

use super::{AppModel, AppMsg};
use crate::config::{self, Settings, SortOrder};
use crate::connection::{Connection, split_address};
use crate::i18n::{i18n, i18n_f};
use crate::ui::preferences::PrefChange;

mod row_menu;

const ROW_PREFIX: &str = "conn:";
const GROUP_PREFIX: &str = "group:";

/// Selecting a row shows its connection, a double click connects to it,
/// and Delete asks to delete it.
pub(super) fn connect_list(
    list: &gtk::ListBox,
    line: &gtk::Box,
    groups: &super::groups::SharedGroups,
    quiet: &Rc<Cell<bool>>,
    sender: &ComponentSender<AppModel>,
) {
    let (input, quiet) = (sender.input_sender().clone(), quiet.clone());
    list.connect_row_selected(move |_, row| {
        if quiet.get() {
            return;
        }
        if let Some(id) = row.and_then(row_id) {
            input.emit(AppMsg::Select(id));
        }
    });
    // A double click connects, as it opens a file in Files.
    let clicks = gtk::GestureClick::new();
    let input = sender.input_sender().clone();
    clicks.connect_pressed(move |_, n_press, _, _| {
        if n_press == 2 {
            input.emit(AppMsg::ConnectSelected);
        }
    });
    list.add_controller(clicks);
    row_menu::connect(list, groups);
    let keys = gtk::EventControllerKey::new();
    let input = sender.input_sender().clone();
    keys.connect_key_pressed(glib::clone!(
        #[weak]
        list,
        #[upgrade_or]
        gtk::glib::Propagation::Proceed,
        move |_, key, _, mods| {
            use gtk::gdk::{Key, ModifierType};
            if key == Key::Delete && mods.is_empty() {
                input.emit(AppMsg::DeleteSelected);
                return gtk::glib::Propagation::Stop;
            }
            // Alt+Up and Alt+Down move the selected connection past its
            // neighbor, as dragging it would.
            if mods == ModifierType::ALT_MASK && (key == Key::Up || key == Key::Down) {
                let down = key == Key::Down;
                let neighbor = list.selected_row().and_then(|row| {
                    let id = row_id(&row)?;
                    let step = if down { 1 } else { -1 };
                    let mut i = row.index() + step;
                    while let Some(other) = list.row_at_index(i) {
                        if let Some(target) = row_id(&other) {
                            return Some((id, target));
                        }
                        i += step;
                    }
                    None
                });
                if let Some((id, target)) = neighbor {
                    input.emit(AppMsg::Move {
                        id,
                        target,
                        after: down,
                    });
                }
                return gtk::glib::Propagation::Stop;
            }
            gtk::glib::Propagation::Proceed
        }
    ));
    list.add_controller(keys);
    connect_reorder(list, line, sender);
}

/// The space between two connection cards (`row.connection-card` in
/// styles.css), where the line showing a drop's place is drawn.
const GAP: f32 = 8.0;

/// The line showing where a dragged connection would land: a dot and a
/// rule, laid over the list and moved to the gap between two cards.
pub(super) fn drop_line() -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    line.add_css_class("drop-line");
    line.set_valign(gtk::Align::Start);
    line.set_can_target(false);
    line.set_visible(false);
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("drop-dot");
    dot.set_valign(gtk::Align::Center);
    let rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    rule.add_css_class("drop-rule");
    rule.set_valign(gtk::Align::Center);
    rule.set_hexpand(true);
    line.append(&dot);
    line.append(&rule);
    line
}

/// Rows drag to another place in the list, the connection's id their
/// content, its picture following the pointer. `line` shows where one
/// would land.
fn connect_reorder(list: &gtk::ListBox, line: &gtk::Box, sender: &ComponentSender<AppModel>) {
    use gtk::gdk;

    let dragged: Rc<RefCell<Option<gtk::ListBoxRow>>> = Rc::default();
    let source = gtk::DragSource::new();
    source.set_actions(gdk::DragAction::MOVE);
    source.connect_prepare(glib::clone!(
        #[weak]
        list,
        #[strong]
        dragged,
        #[upgrade_or]
        None,
        move |source, _, y| {
            let row = list.row_at_y(y as i32)?;
            let id = row_id(&row)?;
            if let Some(picture) = row.child().and_then(|line| line.first_child()) {
                let size = picture.width() / 2;
                source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&picture))), size, size);
            }
            *dragged.borrow_mut() = Some(row);
            Some(gdk::ContentProvider::for_value(&id.to_value()))
        }
    ));
    // The card being moved fades while it is away.
    source.connect_drag_begin(glib::clone!(
        #[strong]
        dragged,
        move |_, _| {
            if let Some(row) = dragged.borrow().as_ref() {
                row.add_css_class("dragged");
            }
        }
    ));
    source.connect_drag_end(glib::clone!(
        #[strong]
        dragged,
        move |_, _, _| {
            if let Some(row) = dragged.borrow_mut().take() {
                row.remove_css_class("dragged");
            }
        }
    ));
    list.add_controller(source);

    let target = gtk::DropTarget::new(String::static_type(), gdk::DragAction::MOVE);
    target.connect_motion(glib::clone!(
        #[weak]
        list,
        #[weak]
        line,
        #[upgrade_or]
        gdk::DragAction::empty(),
        move |_, _, y| {
            let place = landing(&list, y).and_then(|(row, after)| {
                let overlay = line.parent()?;
                let bounds = row.compute_bounds(&overlay)?;
                Some(if after {
                    bounds.y() + bounds.height() + GAP / 2.0
                } else {
                    bounds.y() - GAP / 2.0
                })
            });
            let Some(at) = place else {
                line.set_visible(false);
                return gdk::DragAction::empty();
            };
            // The line's middle on the gap's.
            line.set_margin_top((at - line.height().max(8) as f32 / 2.0).round() as i32);
            line.set_visible(true);
            gdk::DragAction::MOVE
        }
    ));
    target.connect_leave(glib::clone!(
        #[weak]
        line,
        move |_| line.set_visible(false)
    ));
    let input = sender.input_sender().clone();
    target.connect_drop(glib::clone!(
        #[weak]
        list,
        #[weak]
        line,
        #[upgrade_or]
        false,
        move |_, value, _, y| {
            line.set_visible(false);
            let (Ok(id), Some((row, after))) = (value.get::<String>(), landing(&list, y)) else {
                return false;
            };
            let Some(target) = row_id(&row) else {
                return false;
            };
            input.emit(AppMsg::Move { id, target, after });
            true
        }
    ));
    list.add_controller(target);
}

/// The connection row a drop at `y` lands beside, and whether after it:
/// the card under it, or beside the gap it is in, by which half; above a
/// group's heading, before that group's first connection; past the end,
/// after the last.
fn landing(list: &gtk::ListBox, y: f64) -> Option<(gtk::ListBoxRow, bool)> {
    let y = y as f32;
    let mut last = None;
    let mut i = 0;
    while let Some(row) = list.row_at_index(i) {
        i += 1;
        if row_id(&row).is_none() {
            continue;
        }
        let bounds = row.compute_bounds(list)?;
        let (top, bottom) = (bounds.y(), bounds.y() + bounds.height());
        if y < top - GAP / 2.0 {
            return Some((row, false));
        }
        if y <= bottom + GAP / 2.0 {
            return Some((row, y > top + bounds.height() / 2.0));
        }
        last = Some(row);
    }
    last.map(|row| (row, true))
}

/// The group a heading row names.
fn heading_group(row: &gtk::ListBoxRow) -> Option<String> {
    row.widget_name()
        .strip_prefix(GROUP_PREFIX)
        .map(str::to_owned)
}

fn row_id(row: &gtk::ListBoxRow) -> Option<String> {
    row.widget_name()
        .strip_prefix(ROW_PREFIX)
        .map(str::to_owned)
}

/// The connection of the first row the search left, if any.
pub(super) fn first_visible(list: &gtk::ListBox) -> Option<String> {
    let mut i = 0;
    while let Some(row) = list.row_at_index(i) {
        if let Some(id) = row_id(&row) {
            return Some(id);
        }
        i += 1;
    }
    None
}

impl AppModel {
    /// Mark the selected connection's row, without that counting as the
    /// user picking it.
    pub(super) fn highlight_selected(&self) {
        let wanted = self.selected.as_deref();
        let mut i = 0;
        while let Some(row) = self.list.row_at_index(i) {
            if wanted.is_some() && row_id(&row).as_deref() == wanted {
                if !row.is_selected() {
                    self.quiet.set(true);
                    self.list.select_row(Some(&row));
                    self.quiet.set(false);
                }
                return;
            }
            i += 1;
        }
        self.quiet.set(true);
        self.list.unselect_all();
        self.quiet.set(false);
    }

    /// Every connection, in the sidebar's order.
    fn ordered(&self) -> Vec<&Connection> {
        let mut shown: Vec<&Connection> = self.connections.iter().collect();
        let by_name = |a: &&Connection, b: &&Connection| {
            a.title().to_lowercase().cmp(&b.title().to_lowercase())
        };
        match self.settings.sort {
            SortOrder::Name => shown.sort_by(by_name),
            SortOrder::RecentlyUsed => {
                shown.sort_by(|a, b| b.last_used.cmp(&a.last_used).then_with(|| by_name(a, b)))
            }
            SortOrder::Custom => {}
        }
        if self.settings.show_groups {
            // Ungrouped first, then each group under its name, in order.
            // The sort is stable, so each group keeps the order above.
            shown.sort_by(|a, b| {
                (!a.group.is_empty(), a.group.to_lowercase())
                    .cmp(&(!b.group.is_empty(), b.group.to_lowercase()))
            });
        }
        shown
    }

    /// Put connection `id` just before or after `target`, in `target`'s
    /// group while groups show. Any other order becomes the custom one,
    /// starting from the order on screen.
    pub(super) fn move_connection(
        &mut self,
        id: &str,
        target: &str,
        after: bool,
        root: &adw::ApplicationWindow,
    ) {
        let position =
            |connections: &[Connection], id: &str| connections.iter().position(|c| c.id == id);
        if id == target
            || position(&self.connections, id).is_none()
            || position(&self.connections, target).is_none()
        {
            return;
        }
        if self.settings.sort != SortOrder::Custom {
            let order: Vec<String> = self.ordered().iter().map(|c| c.id.clone()).collect();
            self.connections
                .sort_by_key(|c| order.iter().position(|o| *o == c.id));
            self.apply_pref(PrefChange::Sort(SortOrder::Custom));
            super::actions::show_sort(root, SortOrder::Custom);
        }
        let Some(from) = position(&self.connections, id) else {
            return;
        };
        let mut conn = self.connections.remove(from);
        let Some(to) = position(&self.connections, target) else {
            return;
        };
        if self.settings.show_groups {
            conn.group = self.connections[to].group.clone();
        }
        self.connections.insert(to + usize::from(after), conn);
        config::save_connections(&self.connections);
        self.rebuild_list();
        // Keep the keyboard on a row moved by Alt+Up or Alt+Down.
        if self.selected.as_deref() == Some(id)
            && let Some(row) = self.list.selected_row()
        {
            row.grab_focus();
        }
    }

    pub(super) fn rebuild_list(&self) {
        self.quiet.set(true);
        self.list.remove_all();
        self.index_groups();

        let shown: Vec<&Connection> = self
            .ordered()
            .into_iter()
            .filter(|c| self.query.is_empty() || c.matches(&self.query))
            .collect();

        let mut group: Option<&str> = None;
        for conn in &shown {
            if self.settings.show_groups && !conn.group.is_empty() && group != Some(&conn.group) {
                group = Some(&conn.group);
                self.list.append(&heading(&conn.group));
            }
            let open = self.live.contains(&conn.id);
            let row = connection_row(conn, &self.settings, open);
            self.list.append(&row);
            if self.selected.as_deref() == Some(conn.id.as_str()) {
                self.list.select_row(Some(&row));
            }
        }

        let hint = if !shown.is_empty() || self.query.is_empty() {
            None
        } else if let Some((host, _)) = split_address(&self.query) {
            Some(i18n_f(
                "Press Enter to add {address}",
                &[("address", &host)],
            ))
        } else {
            Some(i18n("No Results"))
        };
        self.hint.set_visible(hint.is_some());
        self.hint.set_label(hint.as_deref().unwrap_or_default());
        self.quiet.set(false);
    }
}

fn heading(text: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    row.set_can_focus(false);
    row.add_css_class("sidebar-heading");
    row.set_widget_name(&format!("{GROUP_PREFIX}{text}"));
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.add_css_class("caption-heading");
    label.add_css_class("dim-label");
    row.set_child(Some(&label));
    row.update_property(&[gtk::accessible::Property::Label(text)]);
    row
}

/// A connection's row; `open` when a session to it is connected.
fn connection_row(conn: &Connection, settings: &Settings, open: bool) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("connection-card");
    // For the row's menu, which offers Disconnect then.
    if open {
        row.add_css_class("connected");
    }
    row.set_widget_name(&format!("{ROW_PREFIX}{}", conn.id));
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.add_css_class("nav-row");
    if settings.show_avatars {
        let avatar = crate::ui::avatar::Avatar::new(32);
        avatar.show_connection(conn);
        line.append(&avatar);
    }

    let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    text.set_valign(gtk::Align::Center);
    text.set_hexpand(true);
    let name = gtk::Label::new(Some(conn.title()));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.add_css_class("row-name");
    text.append(&name);
    // A VNC connection says so; RDP, the usual, goes unsaid.
    let vnc = conn.protocol == crate::connection::Protocol::Vnc;
    let named = !conn.name.trim().is_empty();
    let caption = match (settings.show_hosts && named, vnc) {
        (true, true) => Some(format!("VNC · {}", conn.address())),
        (true, false) => Some(conn.address()),
        (false, true) => Some("VNC".to_owned()),
        (false, false) => None,
    };
    if let Some(caption) = caption {
        let host = gtk::Label::new(Some(&caption));
        host.set_xalign(0.0);
        host.set_ellipsize(gtk::pango::EllipsizeMode::End);
        host.add_css_class("caption");
        host.add_css_class("dim-label");
        text.append(&host);
    }
    line.append(&text);
    if open {
        let dot = gtk::Image::from_icon_name("media-record-symbolic");
        dot.set_pixel_size(8);
        dot.add_css_class("session-dot");
        dot.set_tooltip_text(Some(&i18n("Connected")));
        line.append(&dot);
    }
    // Edit and Connect for this row's connection, close together so the
    // name keeps its room.
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    actions.set_valign(gtk::Align::Center);
    // Connect, or Disconnect while a session to it is up.
    let session = if open {
        (
            "media-playback-stop-symbolic",
            i18n("Disconnect"),
            "win.disconnect-from",
        )
    } else {
        (
            "media-playback-start-symbolic",
            i18n("Connect"),
            "win.connect-to",
        )
    };
    for (icon, label, action) in [
        (
            "document-edit-symbolic",
            i18n("Edit Connection"),
            "win.edit-connection",
        ),
        session,
    ] {
        let button = gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(&label)
            .valign(gtk::Align::Center)
            .css_classes(["flat", "row-action"])
            .build();
        button.update_property(&[gtk::accessible::Property::Label(&label)]);
        button.set_action_name(Some(action));
        button.set_action_target_value(Some(&conn.id.to_variant()));
        actions.append(&button);
    }
    line.append(&actions);
    row.set_child(Some(&line));
    row.set_tooltip_text(Some(&conn.address()));
    // Named for the connection, not by the tooltip, which GTK would take.
    let description = if open {
        i18n_f("{address}, connected", &[("address", &conn.address())])
    } else {
        conn.address()
    };
    row.update_property(&[
        gtk::accessible::Property::Label(conn.title()),
        gtk::accessible::Property::Description(&description),
    ]);
    row
}
