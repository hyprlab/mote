//! A window that comes up whole. With GTK's GL renderer, a new macOS
//! window takes about 130ms to draw its first frame (measured on an M-series
//! MacBook, GTK 4.24), and macOS puts the window on the screen as it is
//! shown: its shadow and frame appeared first, the content after. GNOME's
//! compositor waits for the first frame. Here the window stays transparent
//! until GTK has drawn it.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::glib::translate::ToGlibPtr;
use gtk::prelude::*;
use objc2::msg_send;
use objc2::runtime::AnyObject;

unsafe extern "C" {
    /// GTK's own, public since 4.8: the NSWindow behind a surface.
    fn gdk_macos_surface_get_native_window(
        surface: *mut gtk::gdk::ffi::GdkSurface,
    ) -> *mut AnyObject;
}

/// A first frame that never comes (a window shown minimized, say) mustn't
/// leave the window invisible.
const REVEAL_ANYWAY: Duration = Duration::from_millis(600);

pub(crate) fn present(window: &gtk::Window) {
    if window.is_visible() {
        window.present();
        return;
    }
    // The surface, and the NSWindow with it, exist once the window is
    // realized; showing it would realize it anyway.
    WidgetExt::realize(window);
    let Some(clock) = window.frame_clock() else {
        window.present();
        return;
    };
    set_alpha(window, 0.0);

    let revealed = Rc::new(Cell::new(false));
    let handler = Rc::new(Cell::new(None));
    let reveal = {
        let (window, revealed, handler) = (window.downgrade(), revealed.clone(), handler.clone());
        let clock = clock.clone();
        move || {
            if revealed.replace(true) {
                return;
            }
            if let Some(id) = handler.take() {
                clock.disconnect(id);
            }
            if let Some(window) = window.upgrade() {
                set_alpha(&window, 1.0);
            }
        }
    };
    handler.set(Some(clock.connect_after_paint({
        let reveal = reveal.clone();
        move |_| reveal()
    })));
    glib::timeout_add_local_once(REVEAL_ANYWAY, reveal);
    window.present();
}

fn set_alpha(window: &gtk::Window, alpha: f64) {
    let Some(surface) = window.surface() else {
        return;
    };
    // SAFETY: the surface is a live GdkMacosSurface (the only kind GTK
    // makes on macOS), and its NSWindow lives as long as it does.
    unsafe {
        let ns = gdk_macos_surface_get_native_window(surface.to_glib_none().0);
        if let Some(ns) = ns.as_ref() {
            let _: () = msg_send![ns, setAlphaValue: alpha];
        }
    }
}
