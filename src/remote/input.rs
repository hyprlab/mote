//! Keys and the pointer as the interface sends them to a session, whatever
//! its protocol. RDP sends a key as its scancode; VNC as the keysym it
//! typed here, or both where the server takes QEMU's extended key events.

/// A key: where it is on the keyboard, and what it types here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Key {
    /// The PC scancode, with `0xE0` in the high byte for the extended keys.
    pub(crate) scancode: u16,
    /// The X keysym it types with this computer's layout; 0 when there is
    /// none to go by, as for keys sent from the menu.
    pub(crate) keysym: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Button {
    Left,
    Middle,
    Right,
    Back,
    Forward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Input {
    Key {
        key: Key,
        down: bool,
    },
    /// The pointer, in desktop pixels.
    Pointer {
        x: u16,
        y: u16,
    },
    Button {
        button: Button,
        down: bool,
    },
    /// Wheel units, 120 to a notch: up and right are positive.
    Wheel {
        vertical: bool,
        units: i16,
    },
    /// This keyboard's lock keys, for the server to match.
    Locks {
        caps: bool,
        num: bool,
        scroll: bool,
    },
}

/// What is held down on the server's side, so a key or button still down
/// when the focus leaves can be let go of, and a key is let go of with the
/// keysym it was pressed with, whatever the modifiers are by then.
#[derive(Default)]
pub(crate) struct Held {
    keys: Vec<Key>,
    buttons: Vec<Button>,
    pointer: Option<(u16, u16)>,
}

impl Held {
    pub(crate) fn is_key_down(&self, scancode: u16) -> bool {
        self.keys.iter().any(|k| k.scancode == scancode)
    }

    /// The input to send for `input`, if any: a key let go of that isn't
    /// down, a button already in the state asked for and the pointer where
    /// it already is send nothing.
    pub(crate) fn apply(&mut self, input: Input) -> Option<Input> {
        match input {
            Input::Key { key, down: true } => {
                if !self.is_key_down(key.scancode) {
                    self.keys.push(key);
                }
                Some(input)
            }
            Input::Key { key, down: false } => {
                let at = self.keys.iter().position(|k| k.scancode == key.scancode)?;
                let pressed = self.keys.remove(at);
                Some(Input::Key {
                    key: pressed,
                    down: false,
                })
            }
            Input::Button { button, down } => {
                let at = self.buttons.iter().position(|&b| b == button);
                match (down, at) {
                    (true, None) => self.buttons.push(button),
                    (false, Some(at)) => {
                        self.buttons.remove(at);
                    }
                    _ => return None,
                }
                Some(input)
            }
            Input::Pointer { x, y } => {
                (self.pointer.replace((x, y)) != Some((x, y))).then_some(input)
            }
            Input::Wheel { .. } | Input::Locks { .. } => Some(input),
        }
    }

    /// Let go of every button and key still down.
    pub(crate) fn release_all(&mut self) -> Vec<Input> {
        let buttons = self.buttons.drain(..).map(|button| Input::Button {
            button,
            down: false,
        });
        let keys = self
            .keys
            .drain(..)
            .map(|key| Input::Key { key, down: false });
        buttons.chain(keys).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Key = Key {
        scancode: 0x1E,
        keysym: 0x61,
    };

    #[test]
    fn a_key_is_let_go_of_as_it_was_pressed() {
        let mut held = Held::default();
        held.apply(Input::Key { key: A, down: true });
        // Shift went down meanwhile: GDK now calls the key "A".
        let release = held.apply(Input::Key {
            key: Key { keysym: 0x41, ..A },
            down: false,
        });
        assert_eq!(
            release,
            Some(Input::Key {
                key: A,
                down: false
            })
        );
        assert!(!held.is_key_down(A.scancode));
    }

    #[test]
    fn nothing_is_let_go_of_twice() {
        let mut held = Held::default();
        assert_eq!(
            held.apply(Input::Key {
                key: A,
                down: false
            }),
            None
        );
        held.apply(Input::Button {
            button: Button::Left,
            down: true,
        });
        let up = Input::Button {
            button: Button::Left,
            down: false,
        };
        assert_eq!(held.apply(up), Some(up));
        assert_eq!(held.apply(up), None);
    }

    #[test]
    fn releasing_all_lets_go_of_buttons_then_keys() {
        let mut held = Held::default();
        held.apply(Input::Key { key: A, down: true });
        held.apply(Input::Button {
            button: Button::Right,
            down: true,
        });
        assert_eq!(
            held.release_all(),
            [
                Input::Button {
                    button: Button::Right,
                    down: false
                },
                Input::Key {
                    key: A,
                    down: false
                },
            ]
        );
        assert!(held.release_all().is_empty());
    }

    #[test]
    fn the_pointer_moves_only_when_it_moves() {
        let mut held = Held::default();
        let at = Input::Pointer { x: 3, y: 4 };
        assert_eq!(held.apply(at), Some(at));
        assert_eq!(held.apply(at), None);
    }
}
