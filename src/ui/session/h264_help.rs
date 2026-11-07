//! The advice shown when a server sends its picture without H.264: what the
//! computer runs, found or chosen, and what to install there for it to.

mod advice;

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::connection::Connection;
use crate::i18n::{i18n, i18n_f};
use crate::rdp::server::{Distro, Graphics, Server, System};
use crate::remote::Event;
use advice::{Restart, Step};

/// What a session learns of the computer, which the advice starts from.
#[derive(Debug, Default)]
pub(super) struct Findings {
    server: Option<Server>,
    system: System,
    /// The graphics channel offered H.264.
    pub(super) offered: bool,
    /// The toast pointing to the advice was shown.
    told: bool,
}

impl Findings {
    /// Learn from `event`, one of those that tell of the computer; `true`
    /// when it is time to point to the advice.
    pub(super) fn learn(&mut self, event: Event) -> bool {
        match event {
            // Remote Login hands the session to a user's, whose own
            // certificate says Desktop Sharing.
            Event::Server(server) if self.server != Some(Server::GnomeLogin) => {
                self.server = Some(server);
            }
            Event::System(system) => self.system = system,
            Event::Graphics { h264 } => self.offered = h264,
            Event::NoH264 => return !std::mem::replace(&mut self.told, true),
            _ => {}
        }
        false
    }

    /// Where the advice starts: what was found, before what was chosen
    /// last time; but a server chosen by hand over one guessed from its
    /// certificate.
    pub(super) fn choice(&self, connection: &Connection) -> Choice {
        Choice {
            server: match self.server {
                Some(Server::GnomeLogin) => self.server,
                found => connection.server.or(found),
            },
            distro: self.system.distro.or(connection.distro),
            graphics: self.system.graphics.or(connection.graphics),
        }
    }
}

/// What the computer runs, as far as is known.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Choice {
    pub(crate) server: Option<Server>,
    pub(crate) distro: Option<Distro>,
    pub(crate) graphics: Option<Graphics>,
}

/// Show the advice for the computer called `name`, which `ended` the session for want of H.264
/// or sends something else, starting from `choice`; `done` gets what was
/// chosen when the dialog closes.
pub(super) fn present(
    parent: &impl IsA<gtk::Widget>,
    name: &str,
    ended: bool,
    choice: Choice,
    done: impl Fn(Choice) + 'static,
) {
    let chosen = Rc::new(Cell::new(choice));
    let page = adw::PreferencesPage::new();

    let intro = adw::PreferencesGroup::new();
    intro.set_description(Some(&if ended {
        i18n_f(
            "{name} ended the session because it could not send its picture as H.264 video. Installing what it is missing fixes that.",
            &[("name", &name)],
        )
    } else {
        i18n_f(
            "{name} is not sending its picture as H.264 video, which is smoother and uses less of the network. Installing what it is missing fixes that.",
            &[("name", &name)],
        )
    }));
    page.add(&intro);

    let computer = adw::PreferencesGroup::new();
    computer.set_title(&i18n("Remote Computer"));
    let names: Vec<String> = Server::ALL.into_iter().map(advice::server_name).collect();
    let server = combo(
        &i18n("Server"),
        &names,
        choice.server.map(index_of(&Server::ALL)),
    );
    let names: Vec<String> = Distro::ALL
        .into_iter()
        .map(|d| advice::distro_name(d).to_owned())
        .collect();
    let distro = combo(
        &i18n("Distribution"),
        &names,
        choice.distro.map(index_of(&Distro::ALL)),
    );
    let names: Vec<String> = Graphics::ALL
        .into_iter()
        .map(advice::graphics_name)
        .collect();
    let graphics = combo(
        &i18n("Graphics Card"),
        &names,
        choice.graphics.map(index_of(&Graphics::ALL)),
    );
    computer.add(&server);
    computer.add(&distro);
    computer.add(&graphics);
    page.add(&computer);

    let steps = adw::PreferencesGroup::new();
    steps.set_title(&i18n("On the Remote Computer"));
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    steps.add(&body);
    page.add(&steps);

    let refresh = {
        let chosen = chosen.clone();
        let (server, distro, graphics) = (server.clone(), distro.clone(), graphics.clone());
        move || {
            let choice = Choice {
                server: picked(&server, &Server::ALL),
                distro: picked(&distro, &Distro::ALL),
                graphics: picked(&graphics, &Graphics::ALL),
            };
            chosen.set(choice);
            let gnome = matches!(
                choice.server,
                Some(Server::GnomeDesktop | Server::GnomeLogin)
            );
            graphics.set_visible(gnome);
            show_steps(&body, choice, gnome);
        }
    };
    refresh();
    let refresh = Rc::new(refresh);
    for row in [&server, &distro, &graphics] {
        let refresh = refresh.clone();
        row.connect_selected_notify(move |_| refresh());
    }

    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title(i18n("Get H.264 Video"))
        .content_width(560)
        .content_height(640)
        .child(&view)
        .build();
    dialog.connect_closed(move |_| done(chosen.get()));
    dialog.present(Some(parent));
}

/// A row choosing one of `names`, or none until it is known: a combo row
/// always has a choice, so the first is Choose….
fn combo(title: &str, names: &[String], selected: Option<usize>) -> adw::ComboRow {
    let choose = i18n("Choose…");
    let names: Vec<&str> = std::iter::once(choose.as_str())
        .chain(names.iter().map(String::as_str))
        .collect();
    let row = adw::ComboRow::new();
    row.set_title(title);
    row.set_model(Some(&gtk::StringList::new(&names)));
    row.set_selected(selected.map_or(0, |i| i as u32 + 1));
    row
}

fn index_of<T: PartialEq + Copy>(all: &[T]) -> impl Fn(T) -> usize + '_ {
    move |item| all.iter().position(|a| *a == item).unwrap_or(0)
}

fn picked<T: Copy>(row: &adw::ComboRow, all: &[T]) -> Option<T> {
    (row.selected() as usize)
        .checked_sub(1)
        .and_then(|i| all.get(i))
        .copied()
}

fn show_steps(body: &gtk::Box, choice: Choice, gnome: bool) {
    while let Some(child) = body.first_child() {
        body.remove(&child);
    }
    let (Some(server), Some(distro)) = (choice.server, choice.distro) else {
        body.append(&paragraph(&i18n(
            "Choose the server and its distribution to see what to install.",
        )));
        return;
    };
    let Some(graphics) = choice.graphics.or((!gnome).then_some(Graphics::Intel)) else {
        body.append(&paragraph(&i18n(
            "Choose the graphics card to see what to install.",
        )));
        return;
    };
    let (steps, restart) = advice::advice(server, distro, graphics);
    for Step { text, commands } in steps {
        body.append(&paragraph(&text));
        for command in commands {
            body.append(&command_box(command));
        }
    }
    match restart {
        Restart::Command(command) => {
            body.append(&paragraph(&i18n(
                "Then restart the server, and connect again:",
            )));
            body.append(&command_box(command));
        }
        Restart::Computer => body.append(&paragraph(&i18n(
            "Then restart the computer, and connect again.",
        ))),
        Restart::Nothing => {}
    }
}

pub(super) fn paragraph(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_wrap(true);
    label.set_xalign(0.0);
    label
}

/// A command to type on the server, with a button that copies it.
pub(super) fn command_box(command: &'static str) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    line.add_css_class("card");
    line.add_css_class("h264-command");
    let text = gtk::Label::new(Some(command));
    text.add_css_class("monospace");
    text.set_wrap(true);
    text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    text.set_selectable(true);
    text.set_xalign(0.0);
    text.set_hexpand(true);
    line.append(&text);
    let copy = gtk::Button::from_icon_name("edit-copy-symbolic");
    copy.add_css_class("flat");
    copy.set_valign(gtk::Align::Center);
    copy.set_tooltip_text(Some(&i18n("Copy")));
    copy.update_property(&[gtk::accessible::Property::Label(&i18n("Copy"))]);
    copy.connect_clicked(move |button| {
        button.clipboard().set_text(command);
        button.set_icon_name("object-select-symbolic");
        let button = button.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
            if let Some(button) = button.upgrade() {
                button.set_icon_name("edit-copy-symbolic");
            }
        });
    });
    line.append(&copy);
    line
}
