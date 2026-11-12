//! Fullscreen as macOS has it. On GNOME, fullscreen is for a session, and
//! the main window's sidebar steps aside for it. On macOS the green button
//! makes any window fullscreen, as a bigger window: the main window keeps
//! its sidebar then, and a session in it stays as it was, until the session
//! itself asks for fullscreen.

use std::cell::RefCell;

use gtk::glib;
use gtk::prelude::*;

#[derive(Default)]
struct Windows {
    /// Windows whose fullscreen is a bigger window unless a session asks.
    own: Vec<glib::WeakRef<gtk::Window>>,
    /// Windows a session made fullscreen, and whether each already was.
    for_session: Vec<(glib::WeakRef<gtk::Window>, bool)>,
}

thread_local! {
    static WINDOWS: RefCell<Windows> = RefCell::default();
}

fn same(weak: &glib::WeakRef<gtk::Window>, window: &gtk::Window) -> bool {
    weak.upgrade().is_some_and(|w| &w == window)
}

/// The main window: its fullscreen is the user's, not a session's.
pub(crate) fn keep_own(window: &gtk::Window) {
    WINDOWS.with_borrow_mut(|w| w.own.push(window.downgrade()));
}

/// Fullscreen for a session in the window.
pub(crate) fn enter(window: &gtk::Window) {
    let already = window.is_fullscreen();
    WINDOWS.with_borrow_mut(|w| {
        w.for_session.retain(|(weak, _)| !same(weak, window));
        w.for_session.push((window.downgrade(), already));
    });
    if already {
        // Nothing changes for GTK to tell of, so this tells instead.
        window.notify("fullscreened");
    } else {
        window.fullscreen();
    }
}

/// Leave a session's fullscreen: back to the window as it was before.
pub(crate) fn leave(window: &gtk::Window) {
    let was = WINDOWS.with_borrow_mut(|w| {
        let at = w
            .for_session
            .iter()
            .position(|(weak, _)| same(weak, window))?;
        Some(w.for_session.remove(at).1)
    });
    if was == Some(true) {
        window.notify("fullscreened");
    } else {
        window.unfullscreen();
    }
}

/// Whether the window is fullscreen for a session.
pub(crate) fn for_session(window: &gtk::Window) -> bool {
    WINDOWS.with_borrow_mut(|w| {
        w.own.retain(|weak| weak.upgrade().is_some());
        w.for_session.retain(|(weak, _)| weak.upgrade().is_some());
        if !window.is_fullscreen() {
            // Left by the green button or the menu: the session's is over.
            w.for_session.retain(|(weak, _)| !same(weak, window));
            return false;
        }
        !w.own.iter().any(|weak| same(weak, window))
            || w.for_session.iter().any(|(weak, _)| same(weak, window))
    })
}
