//! Keys go to the server as PC scancodes, so the server's own keyboard
//! layout decides what they type. GTK reports the hardware keycode, which
//! on Linux is the evdev code plus 8 and on macOS is Apple's virtual key
//! code; this maps either to scancodes.

use crate::remote::input::Key;

#[cfg(target_os = "macos")]
mod mac;

/// The scancode for a key code from GTK, with `0xE0` in the high byte for
/// the extended keys.
#[cfg(not(target_os = "macos"))]
pub(crate) fn from_gtk(keycode: u32) -> Option<u16> {
    scancode(keycode.checked_sub(8)?)
}

#[cfg(target_os = "macos")]
pub(crate) use mac::from_gtk;

/// The key that, pressed and let go alone, lets go of the keyboard: Right
/// Ctrl, or on macOS Right Command, as a MacBook has no Right Control. Its
/// key code from GTK, and the key it is on the server when it comes with
/// another (Right Super for Right Command).
#[cfg(not(target_os = "macos"))]
pub(crate) const HOST_KEY: (u32, Key) = (
    97 + 8,
    Key {
        scancode: 0xE01D,
        keysym: 0xFFE4, // Control_R
    },
);
#[cfg(target_os = "macos")]
pub(crate) const HOST_KEY: (u32, Key) = (
    0x36,
    Key {
        scancode: 0xE05C,
        keysym: 0xFFEC, // Super_R
    },
);

/// Whether the key toggles a lock that macOS reports as pressed when it
/// turns on and released when it turns off: each of those is a whole tap.
#[cfg(target_os = "macos")]
pub(crate) fn taps_on_change(keycode: u32) -> bool {
    keycode == mac::CAPS_LOCK
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn taps_on_change(_keycode: u32) -> bool {
    false
}

/// The scancode for an evdev key code, with `0xE0` in the high byte for the
/// extended keys.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn scancode(evdev: u32) -> Option<u16> {
    let code = match evdev {
        // Escape to keypad period: evdev numbered these after the scancodes.
        1..=83 => evdev as u16,
        85 => 0x76,    // Zenkaku/Hankaku
        86 => 0x56,    // the ISO key between left Shift and Z
        87 => 0x57,    // F11
        88 => 0x58,    // F12
        89 => 0x73,    // Ro
        90 => 0x78,    // Katakana
        91 => 0x77,    // Hiragana
        92 => 0x79,    // Henkan
        93 => 0x70,    // Katakana/Hiragana
        94 => 0x7B,    // Muhenkan
        95 => 0x5C,    // keypad comma (Japanese)
        96 => 0xE01C,  // keypad Enter
        97 => 0xE01D,  // right Ctrl
        98 => 0xE035,  // keypad /
        99 => 0xE037,  // Print Screen
        100 => 0xE038, // right Alt
        102 => 0xE047, // Home
        103 => 0xE048, // Up
        104 => 0xE049, // Page Up
        105 => 0xE04B, // Left
        106 => 0xE04D, // Right
        107 => 0xE04F, // End
        108 => 0xE050, // Down
        109 => 0xE051, // Page Down
        110 => 0xE052, // Insert
        111 => 0xE053, // Delete
        113 => 0xE020, // Mute
        114 => 0xE02E, // Volume Down
        115 => 0xE030, // Volume Up
        116 => 0xE05E, // Power
        117 => 0x59,   // keypad =
        121 => 0x7E,   // keypad comma
        122 => 0x72,   // Hangul
        123 => 0x71,   // Hanja
        124 => 0x7D,   // Yen
        125 => 0xE05B, // left Super
        126 => 0xE05C, // right Super
        127 => 0xE05D, // Menu
        142 => 0xE05F, // Sleep
        143 => 0xE063, // Wake
        158 => 0xE06A, // browser Back
        159 => 0xE069, // browser Forward
        163 => 0xE019, // next track
        164 => 0xE022, // play/pause
        165 => 0xE010, // previous track
        166 => 0xE024, // stop
        172 => 0xE032, // browser Home
        173 => 0xE067, // browser Refresh
        // F13 to F23.
        183..=193 => 0x64 + (evdev - 183) as u16,
        194 => 0x76,   // F24
        217 => 0xE065, // browser Search
        _ => return None,
    };
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::scancode;

    #[test]
    fn maps_the_common_keys() {
        assert_eq!(scancode(1), Some(0x01)); // Escape
        assert_eq!(scancode(30), Some(0x1E)); // A
        assert_eq!(scancode(57), Some(0x39)); // Space
        assert_eq!(scancode(103), Some(0xE048)); // Up
        assert_eq!(scancode(125), Some(0xE05B)); // Super
        assert_eq!(scancode(240), None);
    }
}
