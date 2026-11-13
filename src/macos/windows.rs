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

/// NSWindowCollectionBehaviorFullScreenAuxiliary.
const FULL_SCREEN_AUXILIARY: usize = 1 << 8;
/// NSWindowTabbingModeDisallowed.
const TABBING_DISALLOWED: isize = 2;

/// A window beside the main one, such as About or Settings, that stays a
/// window when the main window is fullscreen. GTK makes every window one
/// that can be fullscreen by itself, and attaches a transient window to its
/// parent as a child window as it realizes it. AppKit gives a child of a
/// fullscreen window that can be fullscreen the fullscreen style, so About
/// opened over a fullscreen session filled a space of its own, with no way
/// to close it. Made an auxiliary window before it is attached, it floats
/// over the fullscreen space instead, never joins a window as a tab, and
/// its green button zooms.
pub(crate) fn keep_windowed(window: &gtk::Window) {
    let parent = window.transient_for();
    window.set_transient_for(None::<&gtk::Window>);
    WidgetExt::realize(window);
    // SAFETY: plain property setters on the window's own NSWindow, on the
    // main thread.
    with_native(window, |ns| unsafe {
        let _: () = msg_send![ns, setCollectionBehavior: FULL_SCREEN_AUXILIARY];
        let _: () = msg_send![ns, setTabbingMode: TABBING_DISALLOWED];
    });
    window.set_transient_for(parent.as_ref());
}

fn set_alpha(window: &gtk::Window, alpha: f64) {
    // SAFETY: as above.
    with_native(window, |ns| unsafe {
        let _: () = msg_send![ns, setAlphaValue: alpha];
    });
}

/// Run `f` on the NSWindow behind a realized window.
fn with_native(window: &gtk::Window, f: impl FnOnce(&AnyObject)) {
    let Some(surface) = window.surface() else {
        return;
    };
    // SAFETY: the surface is a live GdkMacosSurface (the only kind GTK
    // makes on macOS), and its NSWindow lives as long as it does.
    let ns = unsafe { gdk_macos_surface_get_native_window(surface.to_glib_none().0).as_ref() };
    if let Some(ns) = ns {
        f(ns);
    }
}
