//! Updates, through Sparkle (https://sparkle-project.org), which Mote.app
//! carries in Contents/Frameworks. Sparkle reads the appcast named by
//! SUFeedURL in Info.plist, which Mote's site serves, checks each update's
//! signature against SUPublicEDKey, installs it and starts the new version.
//! It asks once, at the second launch, whether to check on its own, and
//! "Check for Updates…" in the app menu checks at once.
//!
//! The framework is loaded when the app starts rather than linked, so a
//! bare binary run from target/, which has no Sparkle beside it, runs too.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{class, msg_send, sel};
use objc2_foundation::{NSBundle, NSString};

use crate::i18n::i18n;

/// Start Sparkle and add its menu item. Once, on the main thread, after
/// GTK has made the app menu.
pub(crate) fn start() {
    let Some(controller) = controller() else {
        return;
    };
    add_menu_item(&controller);
    // Sparkle's menu item and its own timers hold it weakly; it lives for
    // as long as the app.
    std::mem::forget(controller);
}

fn controller() -> Option<Retained<AnyObject>> {
    let frameworks = NSBundle::mainBundle().privateFrameworksPath()?;
    let path = NSString::from_str(&format!("{frameworks}/Sparkle.framework"));
    let sparkle = NSBundle::bundleWithPath(&path)?;
    // SAFETY: loading Sparkle runs its initializers, which only set
    // themselves up.
    if !unsafe { sparkle.load() } {
        tracing::warn!("Sparkle is in the app but would not load");
        return None;
    }
    let class = AnyClass::get(c"SPUStandardUpdaterController")?;
    let none: Option<&AnyObject> = None;
    // SAFETY: SPUStandardUpdaterController's designated initializer, with
    // no delegates.
    let controller: Option<Retained<AnyObject>> = unsafe {
        msg_send![
            msg_send![class, alloc],
            initWithStartingUpdater: Bool::YES,
            updaterDelegate: none,
            userDriverDelegate: none
        ]
    };
    if controller.is_none() {
        tracing::warn!("Sparkle would not start");
    }
    controller
}

/// "Check for Updates…" in the app menu, after About, as Mac apps have it.
fn add_menu_item(controller: &AnyObject) {
    // SAFETY: AppKit's menus, on the main thread, as GTK made them.
    unsafe {
        let app: Option<&AnyObject> = msg_send![class!(NSApplication), sharedApplication];
        let Some(app) = app else { return };
        let menu: Option<&AnyObject> = msg_send![app, mainMenu];
        let Some(menu) = menu else {
            tracing::warn!("no menu bar to add Check for Updates to");
            return;
        };
        let first: Option<&AnyObject> = msg_send![menu, itemAtIndex: 0isize];
        let app_menu: Option<&AnyObject> = match first {
            Some(item) => msg_send![item, submenu],
            None => None,
        };
        let Some(app_menu) = app_menu else { return };
        // About is first, then a separator before Settings.
        let count: isize = msg_send![app_menu, numberOfItems];
        let mut at = count.min(1);
        for index in 0..count {
            let item: &AnyObject = msg_send![app_menu, itemAtIndex: index];
            let separator: bool = msg_send![item, isSeparatorItem];
            if separator {
                at = index;
                break;
            }
        }
        let title = NSString::from_str(&i18n("Check for Updates…"));
        let none = NSString::from_str("");
        let item: Retained<AnyObject> = msg_send![
            msg_send![class!(NSMenuItem), alloc],
            initWithTitle: &*title,
            action: sel!(checkForUpdates:),
            keyEquivalent: &*none
        ];
        let _: () = msg_send![&item, setTarget: controller];
        let _: () = msg_send![app_menu, insertItem: &*item, atIndex: at];
        tracing::debug!("Check for Updates is item {at} of the app menu's {count}");
    }
}
