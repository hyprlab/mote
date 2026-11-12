//! A session spread over every monitor: the remote desktop takes one
//! monitor of its own for each here, in the same arrangement, and each
//! monitor but the session window's own gets a fullscreen window showing
//! its part.

use adw::prelude::*;
use gtk::{gdk, glib};
use relm4::prelude::*;

use super::display::Display;
use super::{Page, Session, SessionInput, window_of};
use crate::remote;

/// A monitor here, in GDK's logical pixels, and its scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Here {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) scale: f64,
}

impl Here {
    pub(super) fn of(monitor: &gdk::Monitor) -> Self {
        let area = monitor.geometry();
        Self {
            x: area.x(),
            y: area.y(),
            width: area.width(),
            height: area.height(),
            scale: monitor.scale(),
        }
    }
}

/// A monitor of the remote desktop, and where its part of the desktop's
/// picture starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Placed {
    pub(super) monitor: remote::Monitor,
    pub(super) start: (i32, i32),
}

/// The remote desktop's monitors for these, in the same order: as they
/// are arranged here when they share a scale, side by side in the same
/// order when they don't, as their pixels then don't line up. Positions
/// are relative to the `primary` one, as the protocol has it.
pub(super) fn arrange(monitors: &[Here], primary: usize) -> Vec<Placed> {
    let pixels = |logical: i32, scale: f64| (f64::from(logical) * scale).round() as i32;
    let size = |m: &Here| {
        // Servers want an even width, and nothing smaller than 200.
        let width = (pixels(m.width, m.scale).clamp(200, 8192) & !1) as u32;
        let height = pixels(m.height, m.scale).clamp(200, 8192) as u32;
        (width, height)
    };
    let shared = monitors
        .iter()
        .all(|m| (m.scale - monitors[0].scale).abs() < 0.01);
    let mut origins: Vec<(i32, i32)> = if shared {
        monitors
            .iter()
            .map(|m| (pixels(m.x, m.scale), pixels(m.y, m.scale)))
            .collect()
    } else {
        let mut order: Vec<usize> = (0..monitors.len()).collect();
        order.sort_by_key(|&i| (monitors[i].x, monitors[i].y));
        let mut origins = vec![(0, 0); monitors.len()];
        let mut x = 0;
        for i in order {
            origins[i] = (x, 0);
            x += size(&monitors[i]).0 as i32;
        }
        origins
    };
    let (px, py) = origins[primary];
    for origin in &mut origins {
        *origin = (origin.0 - px, origin.1 - py);
    }
    let left = origins.iter().map(|o| o.0).min().unwrap_or(0);
    let top = origins.iter().map(|o| o.1).min().unwrap_or(0);
    monitors
        .iter()
        .zip(origins)
        .map(|(m, (x, y))| {
            let (width, height) = size(m);
            Placed {
                monitor: remote::Monitor {
                    x,
                    y,
                    width,
                    height,
                    scale: ((m.scale * 100.0).round() as u32).clamp(100, 500),
                },
                start: (x - left, y - top),
            }
        })
        .collect()
}

/// The layout to send: the primary monitor first.
pub(super) fn layout(placed: &[Placed], primary: usize) -> Vec<remote::Monitor> {
    let mut monitors = vec![placed[primary].monitor];
    monitors.extend(
        placed
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != primary)
            .map(|(_, p)| p.monitor),
    );
    monitors
}

/// The windows on the other monitors.
pub(super) struct Spread {
    /// What was sent, to send again after reconnecting.
    pub(super) layout: Vec<remote::Monitor>,
    windows: Vec<(gtk::Window, Display)>,
}

impl Spread {
    /// Open a fullscreen window on each of `others`, showing its part of
    /// the desktop, with input going where `input` sends it.
    pub(super) fn open(
        layout: Vec<remote::Monitor>,
        others: &[(gdk::Monitor, (i32, i32))],
        title: &str,
        main: &Display,
        input: impl Fn(Vec<crate::remote::input::Input>) + Clone + 'static,
        on_closed: impl Fn() + Clone + 'static,
    ) -> Self {
        let windows = others
            .iter()
            .map(|(monitor, start)| {
                let display = Display::default();
                display.set_viewport(Some(*start));
                display.set_input(input.clone());
                display.set_grab(true);
                display.set_view_only(main.view_only());
                if let Some(texture) = main.texture() {
                    display.show_texture(texture);
                }
                let window = gtk::Window::builder().title(title).child(&display).build();
                window.add_css_class("mote-spread");
                let closed = on_closed.clone();
                window.connect_close_request(move |_| {
                    closed();
                    glib::Propagation::Proceed
                });
                window.fullscreen_on_monitor(monitor);
                crate::platform::present(&window);
                (window, display)
            })
            .collect();
        Self { layout, windows }
    }

    pub(super) fn displays(&self) -> impl Iterator<Item = &Display> {
        self.windows.iter().map(|(_, display)| display)
    }

    pub(super) fn close(self) {
        for (window, _) in self.windows {
            window.destroy();
        }
    }
}

impl Session {
    /// Take every monitor: the session window fullscreen on its own, a
    /// window on each of the others, and the remote desktop given a
    /// monitor for each.
    pub(super) fn spread_out(&mut self, sender: &ComponentSender<Self>, root: &adw::ToolbarView) {
        let (Some(window), Some(handle)) = (window_of(root), self.handle.clone()) else {
            return;
        };
        if self.spread.is_some() || self.page != Page::Desktop {
            return;
        }
        let display = gtk::prelude::WidgetExt::display(&window);
        let list = display.monitors();
        let all: Vec<gdk::Monitor> = (0..list.n_items())
            .filter_map(|i| list.item(i).and_downcast())
            .collect();
        let here = window
            .surface()
            .and_then(|surface| display.monitor_at_surface(&surface));
        let primary = here
            .and_then(|here| all.iter().position(|m| *m == here))
            .unwrap_or(0);
        if all.len() < 2 {
            window.fullscreen();
            return;
        }
        let heres: Vec<Here> = all.iter().map(Here::of).collect();
        let placed = arrange(&heres, primary);
        let layout = layout(&placed, primary);
        tracing::info!("spreading over {} monitors: {layout:?}", layout.len());
        let others: Vec<(gdk::Monitor, (i32, i32))> = all
            .iter()
            .zip(&placed)
            .enumerate()
            .filter(|(i, _)| *i != primary)
            .map(|(_, (monitor, place))| (monitor.clone(), place.start))
            .collect();
        let input = handle.clone();
        let back = sender.input_sender().clone();
        self.spread = Some(Spread::open(
            layout.clone(),
            &others,
            self.connection.title(),
            &self.display,
            move |events| input.send(remote::Command::Input(events)),
            move || back.emit(SessionInput::OneMonitor),
        ));
        self.display.set_viewport(Some(placed[primary].start));
        window.fullscreen_on_monitor(&all[primary]);
        handle.send(remote::Command::Layout(layout));
    }

    /// Back to one monitor: the other windows close, and the remote desktop
    /// takes the session window's size again.
    pub(super) fn gather(&mut self, sender: &ComponentSender<Self>) {
        let Some(spread) = self.spread.take() else {
            return;
        };
        spread.close();
        self.display.set_viewport(None);
        sender.input(SessionInput::Resized);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn here(x: i32, y: i32, width: i32, height: i32, scale: f64) -> Here {
        Here {
            x,
            y,
            width,
            height,
            scale,
        }
    }

    #[test]
    fn monitors_keep_their_arrangement() {
        // A laptop with a larger monitor to its left, at 2x.
        let monitors = [here(1920, 0, 1440, 900, 2.0), here(0, 0, 1920, 1080, 2.0)];
        let placed = arrange(&monitors, 0);
        assert_eq!(
            placed[0].monitor,
            remote::Monitor {
                x: 0,
                y: 0,
                width: 2880,
                height: 1800,
                scale: 200
            }
        );
        assert_eq!((placed[1].monitor.x, placed[1].monitor.y), (-3840, 0));
        // The picture starts at the left monitor.
        assert_eq!((placed[0].start, placed[1].start), ((3840, 0), (0, 0)));
        let layout = layout(&placed, 0);
        assert_eq!(layout[0], placed[0].monitor);
    }

    #[test]
    fn mixed_scales_go_side_by_side() {
        let monitors = [here(0, 0, 1280, 800, 2.0), here(1280, 200, 1920, 1080, 1.0)];
        let placed = arrange(&monitors, 1);
        assert_eq!((placed[1].monitor.x, placed[1].monitor.y), (0, 0));
        assert_eq!((placed[0].monitor.x, placed[0].monitor.y), (-2560, 0));
        assert_eq!(placed[0].monitor.scale, 200);
        assert_eq!((placed[0].start, placed[1].start), ((0, 0), (2560, 0)));
    }
}
