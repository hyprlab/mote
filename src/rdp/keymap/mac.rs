//! Apple's virtual key codes (`kVK_*` in Carbon's `Events.h`), which GTK
//! passes on as the hardware keycode on macOS, mapped to PC scancodes by
//! the key's place on the keyboard.

/// `kVK_CapsLock`.
pub(super) const CAPS_LOCK: u32 = 0x39;

/// The scancode for a key code from GTK, with `0xE0` in the high byte for
/// the extended keys.
pub(crate) fn from_gtk(keycode: u32) -> Option<u16> {
    let code = match keycode {
        0x00 => 0x1E, // A
        0x01 => 0x1F, // S
        0x02 => 0x20, // D
        0x03 => 0x21, // F
        0x04 => 0x23, // H
        0x05 => 0x22, // G
        0x06 => 0x2C, // Z
        0x07 => 0x2D, // X
        0x08 => 0x2E, // C
        0x09 => 0x2F, // V
        0x0A => 0x56, // the ISO key, § on Apple's ISO keyboards
        0x0B => 0x30, // B
        0x0C => 0x10, // Q
        0x0D => 0x11, // W
        0x0E => 0x12, // E
        0x0F => 0x13, // R
        0x10 => 0x15, // Y
        0x11 => 0x14, // T
        0x12 => 0x02, // 1
        0x13 => 0x03, // 2
        0x14 => 0x04, // 3
        0x15 => 0x05, // 4
        0x16 => 0x07, // 6
        0x17 => 0x06, // 5
        0x18 => 0x0D, // =
        0x19 => 0x0A, // 9
        0x1A => 0x08, // 7
        0x1B => 0x0C, // -
        0x1C => 0x09, // 8
        0x1D => 0x0B, // 0
        0x1E => 0x1B, // ]
        0x1F => 0x18, // O
        0x20 => 0x16, // U
        0x21 => 0x1A, // [
        0x22 => 0x17, // I
        0x23 => 0x19, // P
        0x24 => 0x1C, // Return
        0x25 => 0x26, // L
        0x26 => 0x24, // J
        0x27 => 0x28, // '
        0x28 => 0x25, // K
        0x29 => 0x27, // ;
        0x2A => 0x2B, // backslash
        0x2B => 0x33, // ,
        0x2C => 0x35, // /
        0x2D => 0x31, // N
        0x2E => 0x32, // M
        0x2F => 0x34, // .
        0x30 => 0x0F, // Tab
        0x31 => 0x39, // Space
        0x32 => 0x29, // `
        0x33 => 0x0E, // Delete, which is Backspace
        0x35 => 0x01, // Escape
        // Command is where a PC keyboard has its Super keys.
        0x36 => 0xE05C, // right Command
        0x37 => 0xE05B, // Command
        0x38 => 0x2A,   // Shift
        0x39 => 0x3A,   // Caps Lock
        0x3A => 0x38,   // Option, which is Alt
        0x3B => 0x1D,   // Control
        0x3C => 0x36,   // right Shift
        0x3D => 0xE038, // right Option
        0x3E => 0xE01D, // right Control
        0x40 => 0x68,   // F17
        0x41 => 0x53,   // keypad .
        0x43 => 0x37,   // keypad *
        0x45 => 0x4E,   // keypad +
        0x47 => 0x45,   // keypad Clear, where Num Lock is
        0x48 => 0xE030, // Volume Up
        0x49 => 0xE02E, // Volume Down
        0x4A => 0xE020, // Mute
        0x4B => 0xE035, // keypad /
        0x4C => 0xE01C, // keypad Enter
        0x4E => 0x4A,   // keypad -
        0x4F => 0x69,   // F18
        0x50 => 0x6A,   // F19
        0x51 => 0x59,   // keypad =
        0x52 => 0x52,   // keypad 0
        0x53 => 0x4F,   // keypad 1
        0x54 => 0x50,   // keypad 2
        0x55 => 0x51,   // keypad 3
        0x56 => 0x4B,   // keypad 4
        0x57 => 0x4C,   // keypad 5
        0x58 => 0x4D,   // keypad 6
        0x59 => 0x47,   // keypad 7
        0x5A => 0x6B,   // F20
        0x5B => 0x48,   // keypad 8
        0x5C => 0x49,   // keypad 9
        0x5D => 0x7D,   // Yen
        0x5E => 0x73,   // Ro
        0x5F => 0x5C,   // keypad comma (Japanese)
        0x60 => 0x3F,   // F5
        0x61 => 0x40,   // F6
        0x62 => 0x41,   // F7
        0x63 => 0x3D,   // F3
        0x64 => 0x42,   // F8
        0x65 => 0x43,   // F9
        0x66 => 0x7B,   // Eisu, as Muhenkan
        0x67 => 0x57,   // F11
        0x68 => 0x79,   // Kana, as Henkan
        0x69 => 0x64,   // F13
        0x6A => 0x67,   // F16
        0x6B => 0x65,   // F14
        0x6D => 0x44,   // F10
        0x6E => 0xE05D, // Menu
        0x6F => 0x58,   // F12
        0x71 => 0x66,   // F15
        0x72 => 0xE052, // Help, where Insert is
        0x73 => 0xE047, // Home
        0x74 => 0xE049, // Page Up
        0x75 => 0xE053, // Forward Delete
        0x76 => 0x3E,   // F4
        0x77 => 0xE04F, // End
        0x78 => 0x3C,   // F2
        0x79 => 0xE051, // Page Down
        0x7A => 0x3B,   // F1
        0x7B => 0xE04B, // Left
        0x7C => 0xE04D, // Right
        0x7D => 0xE050, // Down
        0x7E => 0xE048, // Up
        _ => return None,
    };
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::from_gtk;

    #[test]
    fn maps_the_common_keys() {
        assert_eq!(from_gtk(0x35), Some(0x01)); // Escape
        assert_eq!(from_gtk(0x00), Some(0x1E)); // A
        assert_eq!(from_gtk(0x0B), Some(0x30)); // B
        assert_eq!(from_gtk(0x2C), Some(0x35)); // /
        assert_eq!(from_gtk(0x31), Some(0x39)); // Space
        assert_eq!(from_gtk(0x7E), Some(0xE048)); // Up
        assert_eq!(from_gtk(0x37), Some(0xE05B)); // Command
        assert_eq!(from_gtk(0x3F), None); // Fn
    }

    #[test]
    fn no_two_keys_share_a_scancode() {
        let mut codes: Vec<u16> = (0..0x7F).filter_map(from_gtk).collect();
        let all = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), all);
    }
}
