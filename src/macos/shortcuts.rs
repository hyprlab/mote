//! Keeping macOS's own shortcuts for the session while it has the keyboard.
//!
//! GNOME holds its shortcuts back when asked (the shortcuts-inhibit
//! protocol), which GTK's macOS backend doesn't do: macOS acts on
//! Command-Tab, Command-Space, Control and the arrows and the rest before
//! an app sees them, and the app menu takes Command-Q and Command-H before
//! GTK does. An event tap sees keys before all of them. While a session has
//! the keyboard grabbed, one takes every key pressed with Command or
//! Control, and the letting go of each, and gives it to the session in
//! their place. The modifiers alone, Right Command among them, still reach
//! GTK, so letting go of the keyboard works as before.
//!
//! An event tap that changes events needs the Accessibility permission
//! (System Settings > Privacy & Security > Accessibility). macOS asks for
//! it the first time; until it is given, the session gets every key but
//! the ones macOS keeps.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::{CFString, CFStringRef};

type Ref = *mut c_void;

type TapCallback = extern "C" fn(Ref, u32, Ref, *mut c_void) -> Ref;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events: u64,
        callback: TapCallback,
        refcon: *mut c_void,
    ) -> Ref;
    fn CGEventTapEnable(tap: Ref, enable: bool);
    fn CGEventGetIntegerValueField(event: Ref, field: u32) -> i64;
    fn CGEventGetFlags(event: Ref) -> u64;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    static kAXTrustedCheckOptionPrompt: CFStringRef;
    fn AXIsProcessTrustedWithOptions(options: Ref) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopCommonModes: CFStringRef;
    fn CFRunLoopGetMain() -> Ref;
    fn CFRunLoopAddSource(run_loop: Ref, source: Ref, mode: CFStringRef);
    fn CFRunLoopRemoveSource(run_loop: Ref, source: Ref, mode: CFStringRef);
    fn CFMachPortCreateRunLoopSource(allocator: Ref, port: Ref, order: isize) -> Ref;
    fn CFMachPortInvalidate(port: Ref);
    fn CFRelease(object: Ref);
}

/// kCGSessionEventTap, kCGHeadInsertEventTap, kCGEventTapOptionDefault.
const SESSION: u32 = 1;
const FIRST: u32 = 0;
const CHANGES_EVENTS: u32 = 0;
const KEY_DOWN: u32 = 10;
const KEY_UP: u32 = 11;
/// macOS turned the tap off: it took too long, or the user's input did.
const DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const DISABLED_BY_INPUT: u32 = 0xFFFF_FFFF;
/// kCGKeyboardEventKeycode: the key's code, as GDK passes it on macOS.
const KEYCODE: u32 = 9;
const COMMAND: u64 = 0x0010_0000;
const CONTROL: u64 = 0x0004_0000;

/// A key for the session: its code and whether it went down. True when
/// the session took it.
type Handler = Box<dyn Fn(u32, bool) -> bool>;

struct State {
    handler: Handler,
    port: Cell<Ref>,
    /// Keys taken as they went down, whose letting go is taken too, with
    /// or without the modifier still held.
    taken: RefCell<HashSet<i64>>,
}

struct Tap {
    port: Ref,
    source: Ref,
    // Boxed, so the tap's pointer to it holds; dropped after the tap.
    _state: Box<State>,
}

thread_local! {
    static HELD: RefCell<Option<Tap>> = const { RefCell::new(None) };
}

/// Asked for the permission once per run; macOS shows its own prompt.
static ASKED: AtomicBool = AtomicBool::new(false);

/// Take the shortcuts macOS keeps for `handler`, until [`release`]. False
/// without the Accessibility permission, which macOS is asked for.
pub(crate) fn hold(handler: impl Fn(u32, bool) -> bool + 'static) -> bool {
    release();
    if !trusted() {
        tracing::info!("macOS's own shortcuts stay with it: Mote has no Accessibility permission");
        return false;
    }
    let state = Box::new(State {
        handler: Box::new(handler),
        port: Cell::new(std::ptr::null_mut()),
        taken: RefCell::default(),
    });
    let mask = (1 << KEY_DOWN) | (1 << KEY_UP);
    // SAFETY: the callback's refcon is the boxed state, which lives until
    // the tap is invalidated in `release`.
    let port = unsafe {
        CGEventTapCreate(
            SESSION,
            FIRST,
            CHANGES_EVENTS,
            mask,
            tapped,
            std::ptr::from_ref::<State>(&state).cast_mut().cast(),
        )
    };
    if port.is_null() {
        tracing::warn!("macOS would not let Mote watch the keyboard");
        return false;
    }
    state.port.set(port);
    // SAFETY: the port was just made; its source runs on the main run
    // loop, the thread GTK and this run on.
    let source = unsafe {
        let source = CFMachPortCreateRunLoopSource(std::ptr::null_mut(), port, 0);
        CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopCommonModes);
        CGEventTapEnable(port, true);
        source
    };
    HELD.with(|held| {
        held.replace(Some(Tap {
            port,
            source,
            _state: state,
        }))
    });
    tracing::info!("this computer's shortcuts are held back");
    true
}

/// Give macOS its shortcuts back.
pub(crate) fn release() {
    let Some(tap) = HELD.with(RefCell::take) else {
        return;
    };
    // SAFETY: both were made by `hold` and are let go of once, the tap
    // turned off first so the callback runs no more.
    unsafe {
        CGEventTapEnable(tap.port, false);
        CFRunLoopRemoveSource(CFRunLoopGetMain(), tap.source, kCFRunLoopCommonModes);
        CFMachPortInvalidate(tap.port);
        CFRelease(tap.source);
        CFRelease(tap.port);
    }
    tracing::info!("this computer's shortcuts work again");
}

/// Whether Mote has the Accessibility permission, asking for it once.
fn trusted() -> bool {
    let ask = !ASKED.swap(true, Ordering::Relaxed);
    // SAFETY: ApplicationServices' constant key.
    let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::from(ask))]);
    // SAFETY: the dictionary is alive for the call.
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef().cast_mut().cast()) }
}

extern "C" fn tapped(_: Ref, kind: u32, event: Ref, refcon: *mut c_void) -> Ref {
    // SAFETY: the refcon is the tap's state, alive while the tap is.
    let state = unsafe { &*refcon.cast::<State>() };
    match kind {
        DISABLED_BY_TIMEOUT | DISABLED_BY_INPUT => {
            // SAFETY: the tap is alive while its callback runs.
            unsafe { CGEventTapEnable(state.port.get(), true) };
            event
        }
        KEY_DOWN | KEY_UP => {
            // SAFETY: a keyboard event, alive for the callback.
            let (keycode, flags) = unsafe {
                (
                    CGEventGetIntegerValueField(event, KEYCODE),
                    CGEventGetFlags(event),
                )
            };
            let down = kind == KEY_DOWN;
            let shortcut = flags & (COMMAND | CONTROL) != 0;
            let take = {
                let mut taken = state.taken.borrow_mut();
                if down && shortcut {
                    taken.insert(keycode);
                }
                if down {
                    shortcut
                } else {
                    taken.remove(&keycode)
                }
            };
            let Ok(code) = u32::try_from(keycode) else {
                return event;
            };
            if take && (state.handler)(code, down) {
                std::ptr::null_mut()
            } else {
                event
            }
        }
        _ => event,
    }
}
