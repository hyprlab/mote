//! The session's details: what it is connected to, how, and how much is
//! coming in, in a window opened from the session menu that keeps itself
//! up to date while it is open.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::glib;

use crate::connection::Connection;
use crate::i18n::{i18n, i18n_f};
use crate::remote::Protection;

/// How far back the frame rate looks.
const FRAMES_OVER: Duration = Duration::from_secs(2);

/// What the session has learned of its connection.
#[derive(Default)]
pub(super) struct Facts {
    pub(super) protocol: String,
    pub(super) address: String,
    /// The way there: straight, or through a tunnel or a gateway.
    pub(super) route: String,
    /// Which server it is, where its certificate says.
    pub(super) server: Option<String>,
    /// How it signed in, in the protocol's own terms.
    pub(super) security: Option<String>,
    /// What the server calls its desktop.
    pub(super) desktop: Option<String>,
    pub(super) protection: Option<Protection>,
    pub(super) size: Option<(u16, u16)>,
    pub(super) codec: Option<String>,
    since: Option<Instant>,
    /// The last count of bytes read and when it came, and the rate since
    /// the one before.
    received: Option<(Instant, u64)>,
    rate: Option<f64>,
    frames: VecDeque<Instant>,
}

impl Facts {
    pub(super) fn new(connection: &Connection, address: &str) -> Self {
        let route = if let Some((host, _)) = connection.ssh_server() {
            i18n_f("SSH tunnel through {host}", &[("host", &host)])
        } else if let Some((host, _)) = connection.gateway_server() {
            i18n_f("RD Gateway at {host}", &[("host", &host)])
        } else {
            i18n("Direct")
        };
        Self {
            protocol: connection.kind().name().to_owned(),
            address: address.to_owned(),
            route,
            // RDP is always encrypted; a VNC server says how.
            protection: (connection.protocol == crate::connection::Protocol::Rdp)
                .then_some(Protection::Verified),
            ..Self::default()
        }
    }

    /// The session is up; it has been since the first time.
    pub(super) fn connected(&mut self, width: u16, height: u16) {
        self.size = Some((width, height));
        self.since.get_or_insert_with(Instant::now);
    }

    pub(super) fn frame(&mut self) {
        let now = Instant::now();
        self.frames.push_back(now);
        while self
            .frames
            .front()
            .is_some_and(|at| now.duration_since(*at) > FRAMES_OVER)
        {
            self.frames.pop_front();
        }
    }

    pub(super) fn received(&mut self, total: u64) {
        let now = Instant::now();
        if let Some((at, before)) = self.received {
            let seconds = now.duration_since(at).as_secs_f64();
            if seconds > 0.0 {
                self.rate = Some(total.saturating_sub(before) as f64 / seconds);
            }
        }
        self.received = Some((now, total));
    }

    /// What is known, as rows: a title and its value.
    fn rows(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            (i18n("Protocol"), self.protocol.clone()),
            (i18n("Address"), self.address.clone()),
            (i18n("Route"), self.route.clone()),
        ];
        let mut add = |title: String, value: Option<String>| {
            if let Some(value) = value.filter(|v| !v.is_empty()) {
                rows.push((title, value));
            }
        };
        add(i18n("Server"), self.server.clone());
        add(i18n("Desktop Name"), self.desktop.clone());
        add(i18n("Security"), self.security.clone());
        add(
            i18n("Encryption"),
            self.protection.map(|protection| match protection {
                Protection::Verified => i18n("Encrypted, computer verified"),
                Protection::Unverified => i18n("Encrypted, computer not verified"),
                Protection::PasswordOnly => i18n("Password only"),
                Protection::Unencrypted => i18n("None"),
            }),
        );
        add(
            i18n("Desktop Size"),
            self.size.map(|(w, h)| format!("{w} × {h}")),
        );
        add(i18n("Codec"), self.codec.clone());
        if self.since.is_some() {
            let now = Instant::now();
            let frames = self
                .frames
                .iter()
                .filter(|at| now.duration_since(**at) <= FRAMES_OVER)
                .count();
            let rate = frames as f64 / FRAMES_OVER.as_secs_f64();
            add(
                i18n("Frame Rate"),
                Some(i18n_f("{rate} fps", &[("rate", &format!("{rate:.0}"))])),
            );
        }
        add(
            i18n("Data Rate"),
            self.rate.map(|rate| {
                i18n_f(
                    "{size}/s",
                    &[("size", &glib::format_size(rate as u64).to_string())],
                )
            }),
        );
        add(
            i18n("Data Received"),
            self.received
                .map(|(_, total)| glib::format_size(total).to_string()),
        );
        add(
            i18n("Connected For"),
            self.since.map(|since| {
                let seconds = since.elapsed().as_secs();
                format!(
                    "{}:{:02}:{:02}",
                    seconds / 3600,
                    seconds / 60 % 60,
                    seconds % 60
                )
            }),
        );
        rows
    }
}

/// The window, and the facts it shows.
pub(super) struct Info {
    pub(super) facts: Rc<RefCell<Facts>>,
    window: adw::Window,
    list: gtk::ListBox,
    rows: Rc<RefCell<Vec<adw::ActionRow>>>,
}

impl Info {
    /// `name` is the connection's, under the window's title.
    pub(super) fn new(facts: Facts, name: &str) -> Self {
        let title = adw::WindowTitle::new(&i18n("Connection Info"), name);
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        list.set_valign(gtk::Align::Start);
        list.set_margin_top(6);
        list.set_margin_bottom(18);
        list.set_margin_start(12);
        list.set_margin_end(12);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&list)
            .build();
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&scroller));
        let window = adw::Window::builder()
            .title(i18n("Connection Info"))
            .default_width(400)
            .hide_on_close(true)
            .content(&view)
            .build();
        window.add_css_class("session-info");
        // Escape closes it, as it closed the popover it was.
        let keys = gtk::ShortcutController::new();
        keys.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("Escape"),
            Some(gtk::NamedAction::new("window.close")),
        ));
        window.add_controller(keys);
        Self {
            facts: Rc::new(RefCell::new(facts)),
            window,
            list,
            rows: Rc::default(),
        }
    }

    /// Show the window over `parent`, and keep it current while it is
    /// open.
    pub(super) fn show(&self, parent: Option<&gtk::Window>) {
        let open = self.window.is_visible();
        // The session moves between windows; this stays with it.
        self.window.set_transient_for(parent);
        let refresh = {
            let (facts, list, rows) = (self.facts.clone(), self.list.clone(), self.rows.clone());
            move || fill(&list, &mut rows.borrow_mut(), &facts.borrow().rows())
        };
        refresh();
        crate::platform::present(&self.window);
        if open {
            return;
        }
        let window = self.window.downgrade();
        glib::timeout_add_local(Duration::from_secs(1), move || match window.upgrade() {
            Some(window) if window.is_visible() => {
                refresh();
                glib::ControlFlow::Continue
            }
            _ => glib::ControlFlow::Break,
        });
    }

    /// The session is over: so is its window.
    pub(super) fn close(&self) {
        self.window.destroy();
    }
}

/// Show `values` in `list`: the rows as they are when only the values
/// changed, so a value being selected stays selected.
fn fill(list: &gtk::ListBox, rows: &mut Vec<adw::ActionRow>, values: &[(String, String)]) {
    let same = rows.len() == values.len()
        && rows
            .iter()
            .zip(values)
            .all(|(row, (title, _))| row.title() == *title);
    if !same {
        for row in rows.drain(..) {
            list.remove(&row);
        }
        for (title, _) in values {
            let row = adw::ActionRow::new();
            row.set_title(title);
            row.set_subtitle_selectable(true);
            row.add_css_class("property");
            list.append(&row);
            rows.push(row);
        }
    }
    for (row, (_, value)) in rows.iter().zip(values) {
        if row.subtitle().as_deref() != Some(value.as_str()) {
            row.set_subtitle(value);
        }
    }
}
