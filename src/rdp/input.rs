//! The interface's input as RDP's fast-path input events. IronRDP's input
//! database turns each into the event the server expects; the interface
//! has already dropped what wouldn't change anything.

use ironrdp_input::{Database, MouseButton, MousePosition, Operation, Scancode, WheelRotations};
use ironrdp_pdu::input::fast_path::FastPathInputEvent;

use crate::remote::input::{Button, Input};

pub(super) fn events(database: &mut Database, input: Vec<Input>) -> Vec<FastPathInputEvent> {
    let mut events = Vec::new();
    for input in input {
        tracing::trace!(?input, "input");
        let operation = match input {
            Input::Key { key, down } => {
                let code = Scancode::from_u16(key.scancode);
                if down {
                    Operation::KeyPressed(code)
                } else {
                    Operation::KeyReleased(code)
                }
            }
            Input::Pointer { x, y } => Operation::MouseMove(MousePosition { x, y }),
            Input::Button { button, down } => {
                let button = match button {
                    Button::Left => MouseButton::Left,
                    Button::Middle => MouseButton::Middle,
                    Button::Right => MouseButton::Right,
                    Button::Back => MouseButton::X1,
                    Button::Forward => MouseButton::X2,
                };
                if down {
                    Operation::MouseButtonPressed(button)
                } else {
                    Operation::MouseButtonReleased(button)
                }
            }
            Input::Wheel { vertical, units } => Operation::WheelRotations(WheelRotations {
                is_vertical: vertical,
                rotation_units: units,
            }),
            Input::Locks { caps, num, scroll } => {
                events.push(ironrdp_input::synchronize_event(scroll, num, caps, false));
                continue;
            }
        };
        events.extend(database.apply([operation]));
    }
    events
}
