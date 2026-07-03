//! Keyboard input backend. Gamepad support (gilrs) is planned alongside the
//! editor's input settings; keyboard covers Fase 1.
//!
//! Default mapping (chosen to avoid key overlaps):
//!
//! | Trino    | Key         |
//! |----------|-------------|
//! | A / B    | Z / X       |
//! | X / Y    | C / V       |
//! | L / R    | Q / E       |
//! | Start    | Enter       |
//! | Select   | Right Shift |
//! | D-pad    | Arrows      |
//! | Stick    | WASD        |

use trino_core::{Button, Input, InputState, Vec2};
use winit::keyboard::KeyCode;

#[derive(Default)]
pub struct PcInput {
    state: InputState,
    // W, A, S, D held state for the digital stick.
    wasd: [bool; 4],
}

impl PcInput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a winit keyboard event. `down` is true for press, false for
    /// release; key repeats should be filtered out by the caller.
    pub fn handle_key(&mut self, code: KeyCode, down: bool) {
        let button = match code {
            KeyCode::KeyZ => Some(Button::A),
            KeyCode::KeyX => Some(Button::B),
            KeyCode::KeyC => Some(Button::X),
            KeyCode::KeyV => Some(Button::Y),
            KeyCode::KeyQ => Some(Button::L),
            KeyCode::KeyE => Some(Button::R),
            KeyCode::Enter => Some(Button::Start),
            KeyCode::ShiftRight => Some(Button::Select),
            KeyCode::ArrowUp => Some(Button::DpadUp),
            KeyCode::ArrowDown => Some(Button::DpadDown),
            KeyCode::ArrowLeft => Some(Button::DpadLeft),
            KeyCode::ArrowRight => Some(Button::DpadRight),
            _ => None,
        };
        if let Some(button) = button {
            self.state.set(button, down);
        }
        match code {
            KeyCode::KeyW => self.wasd[0] = down,
            KeyCode::KeyA => self.wasd[1] = down,
            KeyCode::KeyS => self.wasd[2] = down,
            KeyCode::KeyD => self.wasd[3] = down,
            _ => {}
        }
        // Digital stick: Y-up positive, matching real console sticks.
        self.state.stick = Vec2::new(
            (self.wasd[3] as i8 - self.wasd[1] as i8) as f32,
            (self.wasd[0] as i8 - self.wasd[2] as i8) as f32,
        );
    }
}

impl Input for PcInput {
    fn poll(&mut self) -> InputState {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_map() {
        let mut input = PcInput::new();
        input.handle_key(KeyCode::KeyZ, true);
        input.handle_key(KeyCode::Enter, true);
        input.handle_key(KeyCode::ArrowLeft, true);
        let s = input.poll();
        assert!(s.is_down(Button::A));
        assert!(s.is_down(Button::Start));
        assert!(s.is_down(Button::DpadLeft));
        assert!(!s.is_down(Button::B));

        input.handle_key(KeyCode::KeyZ, false);
        assert!(!input.poll().is_down(Button::A));
    }

    #[test]
    fn wasd_builds_stick_vector() {
        let mut input = PcInput::new();
        input.handle_key(KeyCode::KeyD, true);
        input.handle_key(KeyCode::KeyW, true);
        assert_eq!(input.poll().stick, Vec2::new(1.0, 1.0));

        input.handle_key(KeyCode::KeyD, false);
        input.handle_key(KeyCode::KeyA, true);
        input.handle_key(KeyCode::KeyW, false);
        input.handle_key(KeyCode::KeyS, true);
        assert_eq!(input.poll().stick, Vec2::new(-1.0, -1.0));

        input.handle_key(KeyCode::KeyA, false);
        input.handle_key(KeyCode::KeyS, false);
        assert_eq!(input.poll().stick, Vec2::ZERO);
    }

    #[test]
    fn opposite_keys_cancel() {
        let mut input = PcInput::new();
        input.handle_key(KeyCode::KeyA, true);
        input.handle_key(KeyCode::KeyD, true);
        assert_eq!(input.poll().stick.x, 0.0);
    }
}
