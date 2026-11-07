//! The interface's input as RFB messages. A key goes as the keysym it
//! typed here; to a server that takes QEMU's extended key events it goes
//! with its scancode too, and the remote layout decides what it types, as
//! it does over RDP. The wheel is buttons 4 to 7, one press a notch.

use crate::remote::input::{Button, Input};

/// One notch of the wheel, in the interface's units.
const NOTCH: i32 = 120;

/// What the server has said it takes, and the state the pointer messages
/// carry: each one sends every button's state, not a change.
#[derive(Default)]
pub(super) struct Keys {
    pub(super) qemu: bool,
    pub(super) extended_buttons: bool,
    buttons: u8,
    back: bool,
    forward: bool,
    x: u16,
    y: u16,
    /// Wheel units short of a notch, vertical and horizontal.
    wheel: (i32, i32),
}

impl Keys {
    /// A new connection: the server has said nothing yet.
    pub(super) fn connected(&mut self) {
        self.qemu = false;
        self.extended_buttons = false;
    }

    /// The bytes to send for `input`.
    pub(super) fn message(&mut self, input: Input, out: &mut Vec<u8>) {
        match input {
            Input::Key { key, down } => {
                let keysym = match key.keysym {
                    // GDK's VoidSymbol: a key it has no symbol for.
                    0 | 0xFF_FFFF => keysym(key.scancode).unwrap_or(0),
                    keysym => keysym,
                };
                match xt(key.scancode).filter(|_| self.qemu) {
                    Some(code) => {
                        out.extend_from_slice(&[255, 0, 0, u8::from(down)]);
                        out.extend_from_slice(&keysym.to_be_bytes());
                        out.extend_from_slice(&code.to_be_bytes());
                    }
                    None if keysym != 0 => {
                        out.extend_from_slice(&[4, u8::from(down), 0, 0]);
                        out.extend_from_slice(&keysym.to_be_bytes());
                    }
                    None => tracing::debug!("no keysym for scancode {:#x}", key.scancode),
                }
            }
            Input::Pointer { x, y } => {
                (self.x, self.y) = (x, y);
                self.pointer(out);
            }
            Input::Button { button, down } => {
                let bit = |keys: &mut Self, bit: u8| {
                    if down {
                        keys.buttons |= bit;
                    } else {
                        keys.buttons &= !bit;
                    }
                };
                match button {
                    Button::Left => bit(self, 1),
                    Button::Middle => bit(self, 1 << 1),
                    Button::Right => bit(self, 1 << 2),
                    Button::Back => self.back = down,
                    Button::Forward => self.forward = down,
                }
                self.pointer(out);
            }
            Input::Wheel { vertical, units } => {
                let pending = if vertical {
                    &mut self.wheel.0
                } else {
                    &mut self.wheel.1
                };
                *pending += i32::from(units);
                let notches = *pending / NOTCH;
                *pending %= NOTCH;
                // Up and right are positive: buttons 4 and 7.
                let bit = match (vertical, notches > 0) {
                    (true, true) => 1 << 3,
                    (true, false) => 1 << 4,
                    (false, false) => 1 << 5,
                    (false, true) => 1 << 6,
                };
                for _ in 0..notches.unsigned_abs() {
                    self.buttons |= bit;
                    self.pointer(out);
                    self.buttons &= !bit;
                    self.pointer(out);
                }
            }
            // RFB has no message for them.
            Input::Locks { .. } => {}
        }
    }

    fn pointer(&self, out: &mut Vec<u8>) {
        let (x, y) = (self.x.to_be_bytes(), self.y.to_be_bytes());
        if self.extended_buttons {
            // The marker bit, and back and forward in a byte of their own.
            let extra = u8::from(self.back) | u8::from(self.forward) << 1;
            out.extend_from_slice(&[5, self.buttons | 0x80, x[0], x[1], y[0], y[1], extra]);
        } else {
            // Button 8 is back; forward has no place.
            let mask = self.buttons | u8::from(self.back) << 7;
            out.extend_from_slice(&[5, mask, x[0], x[1], y[0], y[1]]);
        }
    }
}

/// The XT keycode QEMU's extended key events carry: the scancode, with
/// the 0xE0 prefix folded into the high bit.
fn xt(scancode: u16) -> Option<u32> {
    // Print Screen goes as 0x54, whatever the modifiers.
    if scancode == 0xE037 {
        return Some(0x54);
    }
    let low = u32::from(scancode & 0x7F);
    match scancode & 0xFF80 {
        0 => Some(low),
        0xE000 => Some(low | 0x80),
        _ => None,
    }
}

/// The keysym a key types on a US layout, for keys sent with none: the
/// combinations sent from the session's menu.
fn keysym(scancode: u16) -> Option<u32> {
    const ROW_1: &[u8] = b"1234567890-=";
    const ROW_Q: &[u8] = b"qwertyuiop[]";
    const ROW_A: &[u8] = b"asdfghjkl;'`";
    const ROW_Z: &[u8] = b"\\zxcvbnm,./";
    let ascii = |row: &[u8], at: u16| row.get(usize::from(at)).map(|&c| u32::from(c));
    Some(match scancode {
        0x01 => 0xFF1B, // Escape
        0x02..=0x0D => ascii(ROW_1, scancode - 0x02)?,
        0x0E => 0xFF08, // BackSpace
        0x0F => 0xFF09, // Tab
        0x10..=0x1B => ascii(ROW_Q, scancode - 0x10)?,
        0x1C => 0xFF0D, // Return
        0x1D => 0xFFE3, // Control_L
        0x1E..=0x29 => ascii(ROW_A, scancode - 0x1E)?,
        0x2A => 0xFFE1, // Shift_L
        0x2B..=0x35 => ascii(ROW_Z, scancode - 0x2B)?,
        0x36 => 0xFFE2,                                     // Shift_R
        0x38 => 0xFFE9,                                     // Alt_L
        0x39 => 0x20,                                       // space
        0x3A => 0xFFE5,                                     // Caps_Lock
        0x3B..=0x44 => 0xFFBE + u32::from(scancode - 0x3B), // F1 to F10
        0x57 => 0xFFC8,                                     // F11
        0x58 => 0xFFC9,                                     // F12
        0xE01C => 0xFF8D,                                   // KP_Enter
        0xE01D => 0xFFE4,                                   // Control_R
        0xE037 => 0xFF61,                                   // Print
        0xE038 => 0xFFEA,                                   // Alt_R
        0xE047 => 0xFF50,                                   // Home
        0xE048 => 0xFF52,                                   // Up
        0xE049 => 0xFF55,                                   // Prior
        0xE04B => 0xFF51,                                   // Left
        0xE04D => 0xFF53,                                   // Right
        0xE04F => 0xFF57,                                   // End
        0xE050 => 0xFF54,                                   // Down
        0xE051 => 0xFF56,                                   // Next
        0xE052 => 0xFF63,                                   // Insert
        0xE053 => 0xFFFF,                                   // Delete
        0xE05B => 0xFFEB,                                   // Super_L
        0xE05C => 0xFFEC,                                   // Super_R
        0xE05D => 0xFF67,                                   // Menu
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::input::Key;

    fn sent(keys: &mut Keys, input: Input) -> Vec<u8> {
        let mut out = Vec::new();
        keys.message(input, &mut out);
        out
    }

    #[test]
    fn folds_the_extended_prefix_into_the_keycode() {
        assert_eq!(xt(0x1E), Some(0x1E));
        assert_eq!(xt(0xE04D), Some(0xCD));
        assert_eq!(xt(0xE037), Some(0x54));
    }

    #[test]
    fn a_key_goes_with_its_scancode_only_to_a_server_that_takes_one() {
        let a = Input::Key {
            key: Key {
                scancode: 0x1E,
                keysym: 0x61,
            },
            down: true,
        };
        let mut keys = Keys::default();
        assert_eq!(sent(&mut keys, a), [4, 1, 0, 0, 0, 0, 0, 0x61]);
        keys.qemu = true;
        assert_eq!(
            sent(&mut keys, a),
            [255, 0, 0, 1, 0, 0, 0, 0x61, 0, 0, 0, 0x1E]
        );
    }

    #[test]
    fn a_menu_key_takes_its_keysym_from_the_scancode() {
        let delete = Input::Key {
            key: Key {
                scancode: 0xE053,
                keysym: 0,
            },
            down: true,
        };
        assert_eq!(
            sent(&mut Keys::default(), delete),
            [4, 1, 0, 0, 0, 0, 0xFF, 0xFF]
        );
        assert_eq!(keysym(0x10), Some(u32::from(b'q')));
        assert_eq!(keysym(0x3B), Some(0xFFBE));
    }

    #[test]
    fn a_notch_of_the_wheel_presses_and_lets_go() {
        let mut keys = Keys::default();
        let half = Input::Wheel {
            vertical: true,
            units: -60,
        };
        assert!(sent(&mut keys, half).is_empty());
        // Down: button 5.
        assert_eq!(
            sent(&mut keys, half),
            [5, 1 << 4, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn back_and_forward_go_in_their_own_byte_once_the_server_takes_it() {
        let back = Input::Button {
            button: Button::Back,
            down: true,
        };
        let mut keys = Keys::default();
        assert_eq!(sent(&mut keys, back), [5, 0x80, 0, 0, 0, 0]);
        keys.extended_buttons = true;
        assert_eq!(sent(&mut keys, back), [5, 0x80, 0, 0, 0, 0, 1]);
    }
}
