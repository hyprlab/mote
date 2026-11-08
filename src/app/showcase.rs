//! A development hook, as Hylki has: with `MOTE_SHOWCASE=<file.png>` set,
//! the app stages a view and writes a picture of it, so a change to the
//! interface can be checked without anyone at the keyboard.
//!
//! - `MOTE_SHOWCASE_OPEN`: comma-separated steps before the capture:
//!   `first` selects the first connection, `editor` opens it in the editor
//!   (or a new one), `settings` or `settings:<page>` opens Settings and
//!   captures that window instead, `about` opens About and captures its
//!   window (`MOTE_SHOWCASE_SCROLL=<0..1>` scrolls it, and
//!   `MOTE_SHOWCASE_NOTES` opens Release Notes), `shortcuts` the
//!   keyboard shortcuts, `search:<text>`
//!   opens the search with that text in it, and `connect:<name>` connects
//!   to the connection of that name; `detach` and `fullscreen` then do that
//!   to its session, and `scaling:<resize|fit|actual>` sets its scaling;
//!   `keys:<name>` sends a combination from its Send Keys menu,
//!   `screenshot` takes a screenshot, `view-only:<1|0>` sets that, `menu`
//!   opens its menu, `info` its Connection Info window, captured as
//!   `<file>-info.png`, and
//!   `all-monitors` spreads it over every monitor, whose windows are
//!   captured too, as `<file>-2.png` and on.
//!   `delay:<seconds>` makes the steps after it wait that long; `bottom`
//!   scrolls an open dialog to its end, `picker` opens the editor's
//!   avatar picker, captured as `<file>-popover.png`, and `scan` opens Scan
//!   Network, which scans the network the rig's computer is on and those
//!   of its settings; `networks:<a+b>` then shows its networks page, with
//!   those entered, as many as there are.
//! - `MOTE_SHOWCASE_DELAY`: seconds before the capture, 3 by default.

use std::time::Duration;

use adw::prelude::*;
use relm4::prelude::*;

use super::{AppModel, AppMsg};
use crate::connection::Scaling;
use crate::ui::preferences::PrefInput;
use crate::ui::session::SessionInput;

impl AppModel {
    pub(super) fn arm_showcase(&self, sender: &ComponentSender<Self>) {
        let Ok(path) = std::env::var("MOTE_SHOWCASE") else {
            return;
        };
        let steps = std::env::var("MOTE_SHOWCASE_OPEN").unwrap_or_default();
        let input = sender.input_sender().clone();
        // Seconds the steps after a `delay:<seconds>` wait.
        let mut wait = 0;
        for step in steps.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if let Some(seconds) = step.strip_prefix("delay:") {
                wait += seconds.parse::<u64>().unwrap_or(1);
                continue;
            }
            let send = |message: AppMsg| {
                if wait == 0 {
                    input.emit(message);
                } else {
                    let input = input.clone();
                    gtk::glib::timeout_add_local_once(Duration::from_secs(wait), move || {
                        input.emit(message);
                    });
                }
            };
            match step.split_once(':').unwrap_or((step, "")) {
                ("first", _) => {
                    if let Some(id) = super::sidebar::first_visible(&self.list) {
                        send(AppMsg::Select(id));
                    }
                }
                ("editor", _) => match super::sidebar::first_visible(&self.list) {
                    Some(_) if steps.contains("first") => send(AppMsg::EditSelected),
                    _ => send(AppMsg::NewConnection(None)),
                },
                ("settings", page) => {
                    send(AppMsg::OpenSettings);
                    send(AppMsg::ShowcaseSettingsPage(page.to_owned()));
                }
                ("about", _) => send(AppMsg::About),
                ("scan", _) => send(AppMsg::ScanNetwork),
                ("networks", text) => send(AppMsg::ShowcaseScanNetworks(text.to_owned())),
                ("shortcuts", _) => send(AppMsg::Shortcuts),
                ("connect", name) => {
                    if let Some(id) = self
                        .connections
                        .iter()
                        .find(|c| c.title() == name)
                        .map(|c| c.id.clone())
                    {
                        send(AppMsg::Select(id));
                        send(AppMsg::ConnectSelected);
                    }
                }
                (
                    name @ ("detach" | "fullscreen" | "scaling" | "keys" | "screenshot"
                    | "view-only" | "all-monitors" | "info" | "menu"),
                    argument,
                ) => {
                    send(AppMsg::ShowcaseSession(format!("{name}:{argument}")));
                }
                // Open the editor's avatar picker, after the steps before.
                ("picker", _) => {
                    gtk::glib::timeout_add_local_once(Duration::from_secs(wait + 1), open_picker);
                }
                // Scroll the open dialog to its end, after the steps before.
                ("bottom", _) => {
                    gtk::glib::timeout_add_local_once(
                        Duration::from_secs(wait + 1),
                        scroll_dialog_to_end,
                    );
                }
                ("search", text) => {
                    send(AppMsg::FocusSearch);
                    self.search.set_text(text);
                }
                (other, _) => tracing::warn!("showcase: unknown step {other}"),
            }
        }
        let delay = std::env::var("MOTE_SHOWCASE_DELAY")
            .ok()
            .and_then(|d| d.parse().ok())
            .unwrap_or(3);
        gtk::glib::timeout_add_local_once(Duration::from_secs(delay), move || {
            input.emit(AppMsg::ShowcaseCapture(path));
        });
    }

    pub(super) fn showcase_session(&self, step: &str) {
        let Some(session) = self.sessions.values().next() else {
            return;
        };
        session.emit(match step.split_once(':') {
            Some(("scaling", key)) => match Scaling::from_key(key) {
                Some(scaling) => SessionInput::SetScaling(scaling),
                None => return,
            },
            Some(("detach", _)) => SessionInput::ToggleDetached,
            Some(("keys", name)) => SessionInput::SendKeys(name.to_owned()),
            Some(("screenshot", _)) => SessionInput::Screenshot,
            Some(("view-only", on)) => SessionInput::SetViewOnly(on != "0"),
            Some(("all-monitors", _)) => SessionInput::AllMonitors,
            Some(("info", _)) => SessionInput::ShowInfo,
            Some(("menu", _)) => SessionInput::OpenMenu,
            _ => SessionInput::ToggleFullscreen,
        });
    }

    pub(super) fn showcase_settings_page(&self, page: &str) {
        if let Some(prefs) = &self.prefs
            && !page.is_empty()
        {
            prefs.emit(PrefInput::SelectPage(page.to_owned()));
        }
    }

    pub(super) fn showcase_capture(&self, path: &str, root: &adw::ApplicationWindow) {
        let detached = self.detached.values().next().cloned();
        let about = gtk::Window::list_toplevels()
            .into_iter()
            .find(|w| w.is_visible() && w.has_css_class("about-window"));
        let window: gtk::Widget = match (&self.prefs, detached) {
            _ if about.is_some() => about.expect("checked").upcast(),
            (_, Some(window)) => window.upcast(),
            (Some(prefs), _) if prefs.widget().is_visible() => prefs.widget().clone().upcast(),
            _ => root.clone().upcast(),
        };
        match capture(&window, path) {
            Ok(()) => tracing::info!("showcase: wrote {path}"),
            Err(e) => tracing::error!("showcase: {e}"),
        }
        // A popover open in it, which is a surface of its own.
        if let Some(popover) = descendants(&window)
            .into_iter()
            .find(|w| w.is::<gtk::Popover>() && w.is_visible())
        {
            let path = path.replace(".png", "-popover.png");
            match capture(&popover, &path) {
                Ok(()) => tracing::info!("showcase: wrote {path}"),
                Err(e) => tracing::error!("showcase: {e}"),
            }
        }
        // A session's windows on the other monitors.
        let spread = gtk::Window::list_toplevels()
            .into_iter()
            .filter(|w| w.is_visible() && w.has_css_class("mote-spread"));
        for (i, other) in spread.enumerate() {
            let path = path.replace(".png", &format!("-{}.png", i + 2));
            match capture(other.upcast_ref(), &path) {
                Ok(()) => tracing::info!("showcase: wrote {path}"),
                Err(e) => tracing::error!("showcase: {e}"),
            }
        }
        // A session's Connection Info window.
        if let Some(info) = gtk::Window::list_toplevels()
            .into_iter()
            .find(|w| w.is_visible() && w.has_css_class("session-info"))
        {
            let path = path.replace(".png", "-info.png");
            match capture(info.upcast_ref(), &path) {
                Ok(()) => tracing::info!("showcase: wrote {path}"),
                Err(e) => tracing::error!("showcase: {e}"),
            }
        }
        relm4::main_adw_application().quit();
    }
}

fn capture(widget: &gtk::Widget, path: &str) -> Result<(), String> {
    let scale = f64::from(widget.scale_factor());
    let (w, h) = (f64::from(widget.width()), f64::from(widget.height()));
    let paintable = gtk::WidgetPaintable::new(Some(widget));
    let snapshot = gtk::Snapshot::new();
    snapshot.scale(scale as f32, scale as f32);
    paintable.snapshot(&snapshot, w, h);
    let node = snapshot
        .to_node()
        .ok_or("nothing to render (window suspended?)")?;
    let renderer = widget
        .native()
        .and_then(|n| n.renderer())
        .ok_or("no renderer")?;
    let bounds = gtk::graphene::Rect::new(0.0, 0.0, (w * scale) as f32, (h * scale) as f32);
    renderer
        .render_texture(&node, Some(&bounds))
        .save_to_png(path)
        .map_err(|e| e.to_string())
}

/// Every widget under `root`, popovers included.
fn descendants(root: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut all = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(widget) = pending.pop() {
        let mut child = widget.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            pending.push(c);
        }
        all.push(widget);
    }
    all
}

/// Pop open the avatar picker of the editor that is open.
fn open_picker() {
    let windows = gtk::Window::list_toplevels();
    for window in windows {
        for widget in descendants(&window) {
            if let Some(button) = widget.downcast_ref::<gtk::MenuButton>()
                && button.has_css_class("avatar-edit")
            {
                button.popup();
                return;
            }
        }
    }
}

/// The open dialog's scrolled content, scrolled to its end.
fn scroll_dialog_to_end() {
    let dialog = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<adw::ApplicationWindow>().ok())
        .find_map(|w| w.visible_dialog());
    let mut pending: Vec<gtk::Widget> = dialog.into_iter().map(|d| d.upcast()).collect();
    while let Some(widget) = pending.pop() {
        if let Some(scrolled) = widget.downcast_ref::<gtk::ScrolledWindow>() {
            let adjustment = scrolled.vadjustment();
            adjustment.set_value(adjustment.upper());
            return;
        }
        let mut child = widget.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            pending.push(c);
        }
    }
}
