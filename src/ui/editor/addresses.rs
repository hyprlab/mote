//! The connection's other addresses in the editor: a row each under an
//! expander, put in order by dragging their handles or with Alt+Up and
//! Alt+Down.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use relm4::prelude::*;

use super::{Editor, EditorMsg, entry};
use crate::i18n::i18n;

/// The row the other addresses go under, with the button that adds one.
pub(super) fn other_addresses(row: &adw::ExpanderRow, sender: &ComponentSender<Editor>) {
    row.set_title(&i18n("Other Addresses"));
    row.set_subtitle(&i18n("Tried in order when the address can’t be reached"));
    let label = i18n("Add Address");
    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text(&label)
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    add.update_property(&[gtk::accessible::Property::Label(&label)]);
    let input = sender.input_sender().clone();
    add.connect_clicked(move |_| input.emit(EditorMsg::AddAddress));
    row.add_suffix(&add);
}

pub(super) fn other_row(address: &str, sender: &ComponentSender<Editor>) -> adw::EntryRow {
    let row = entry(&i18n("Address"), address);
    let input = sender.input_sender().clone();
    row.connect_changed(move |_| input.emit(EditorMsg::Changed));
    let remove = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text(i18n("Remove"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    remove.update_property(&[gtk::accessible::Property::Label(&i18n("Remove Address"))]);
    let (input, weak) = (sender.input_sender().clone(), row.downgrade());
    remove.connect_clicked(move |_| {
        if let Some(row) = weak.upgrade() {
            input.emit(EditorMsg::RemoveAddress(row));
        }
    });
    row.add_suffix(&remove);
    row.add_css_class("other-address");
    drag_to_reorder(&row, sender);
    row
}

/// The address's handle drags it to another place among them, and Alt+Up
/// and Alt+Down do the same from the keyboard. The handle alone, as a drag
/// on the rest of the row selects its text.
fn drag_to_reorder(row: &adw::EntryRow, sender: &ComponentSender<Editor>) {
    use gtk::gdk;

    let handle = gtk::Image::from_icon_name("list-drag-handle-symbolic");
    handle.add_css_class("dim-label");
    handle.set_cursor_from_name(Some("grab"));
    row.add_prefix(&handle);

    let source = gtk::DragSource::new();
    source.set_actions(gdk::DragAction::MOVE);
    // Where on the handle it was grabbed, so the card stays under the pointer.
    let grabbed_at = Rc::new(Cell::new((0.0, 0.0)));
    let (weak, at) = (row.downgrade(), grabbed_at.clone());
    source.connect_prepare(move |_, x, y| {
        at.set((x, y));
        let row = weak.upgrade()?;
        Some(gdk::ContentProvider::for_value(&row.to_value()))
    });
    let (weak, grabbed) = (row.downgrade(), handle.downgrade());
    source.connect_drag_begin(move |_, drag| {
        let (Some(row), Some(grabbed)) = (weak.upgrade(), grabbed.upgrade()) else {
            return;
        };
        drag_card(&row, drag);
        let (x, y) = grabbed_at.get();
        let at = grabbed
            .compute_point(&row, &gtk::graphene::Point::new(x as f32, y as f32))
            .unwrap_or_else(gtk::graphene::Point::zero);
        drag.set_hotspot(at.x() as i32, at.y() as i32);
        row.add_css_class("dragged");
    });
    let weak = row.downgrade();
    source.connect_drag_end(move |_, _, _| {
        if let Some(row) = weak.upgrade() {
            row.remove_css_class("dragged");
        }
    });
    handle.add_controller(source);

    let target = gtk::DropTarget::new(adw::EntryRow::static_type(), gdk::DragAction::MOVE);
    let weak = row.downgrade();
    target.connect_motion(move |_, _, y| {
        if let Some(row) = weak.upgrade() {
            let after = y > f64::from(row.height()) / 2.0;
            mark_drop(&row, Some(after));
        }
        gdk::DragAction::MOVE
    });
    let weak = row.downgrade();
    target.connect_leave(move |_| {
        if let Some(row) = weak.upgrade() {
            mark_drop(&row, None);
        }
    });
    let (input, weak) = (sender.input_sender().clone(), row.downgrade());
    target.connect_drop(move |_, value, _, y| {
        let (Ok(dragged), Some(beside)) = (value.get::<adw::EntryRow>(), weak.upgrade()) else {
            return false;
        };
        mark_drop(&beside, None);
        let after = y > f64::from(beside.height()) / 2.0;
        input.emit(EditorMsg::MoveAddress {
            row: dragged,
            beside,
            after,
        });
        true
    });
    row.add_controller(target);

    let keys = gtk::EventControllerKey::new();
    let (input, weak) = (sender.input_sender().clone(), row.downgrade());
    keys.connect_key_pressed(move |_, key, _, mods| {
        let down = match key {
            gdk::Key::Up => false,
            gdk::Key::Down => true,
            _ => return gtk::glib::Propagation::Proceed,
        };
        match weak
            .upgrade()
            .filter(|_| mods == gdk::ModifierType::ALT_MASK)
        {
            Some(row) => {
                input.emit(EditorMsg::NudgeAddress { row, down });
                gtk::glib::Propagation::Stop
            }
            None => gtk::glib::Propagation::Proceed,
        }
    });
    row.add_controller(keys);
}

/// Mark where a dragged address would land, as a line along the row's edge:
/// `Some(true)` for after this row, `Some(false)` for before it.
fn mark_drop(row: &adw::EntryRow, after: Option<bool>) {
    for (class, on) in [
        ("drop-above", after == Some(false)),
        ("drop-below", after == Some(true)),
    ] {
        if on {
            row.add_css_class(class);
        } else {
            row.remove_css_class(class);
        }
    }
}

/// What follows the pointer during a drag: a row with the address on it,
/// as wide as the one it stands for.
fn drag_card(row: &adw::EntryRow, drag: &gtk::gdk::Drag) {
    let copy = adw::ActionRow::new();
    copy.set_use_markup(false);
    copy.set_title(&row.text());
    copy.add_prefix(&gtk::Image::from_icon_name("list-drag-handle-symbolic"));
    let card = gtk::ListBox::new();
    card.add_css_class("boxed-list");
    card.set_size_request(row.width(), row.height());
    card.append(&copy);
    gtk::DragIcon::for_drag(drag).set_child(Some(&card));
}

/// Move `item` to just before or after `beside`, both being in `list`.
pub(super) fn moved<T: PartialEq>(list: &mut Vec<T>, item: &T, beside: &T, after: bool) {
    if item == beside || !list.contains(beside) {
        return;
    }
    let Some(from) = list.iter().position(|other| other == item) else {
        return;
    };
    let item = list.remove(from);
    let to = list
        .iter()
        .position(|other| other == beside)
        .unwrap_or_default();
    list.insert(to + usize::from(after), item);
}

#[cfg(test)]
mod tests {
    use super::moved;

    #[test]
    fn an_address_lands_beside_the_one_it_is_dropped_on() {
        let after = |item, beside, after| {
            let mut list = vec![1, 2, 3, 4];
            moved(&mut list, &item, &beside, after);
            list
        };
        assert_eq!(after(1, 3, true), [2, 3, 1, 4]);
        assert_eq!(after(1, 3, false), [2, 1, 3, 4]);
        assert_eq!(after(4, 1, false), [4, 1, 2, 3]);
        assert_eq!(after(4, 2, true), [1, 2, 4, 3]);
        assert_eq!(after(2, 2, true), [1, 2, 3, 4]);
        assert_eq!(after(9, 2, true), [1, 2, 3, 4]);
    }
}
