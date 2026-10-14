//! Keyboard + mouse input backend. Gamepad support (gilrs) is planned
//! alongside the editor's input settings.
//!
//! The mouse is the [`InputState::pointer`]: the host converts window
//! pixels to game pixels ([`crate::PcRenderer::window_to_game`]) and feeds
//! [`PcInput::handle_pointer`], [`PcInput::handle_mouse_button`] and
//! [`PcInput::handle_wheel`]. Left = `Primary`, right = `Secondary`,
//! middle = `Middle`.
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

use trino_core::{Button, Input, InputState, PointerButton, Vec2};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

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

impl PcInput {
    /// Feed any winit window event: keyboard (no repeats), cursor motion
    /// (mapped to game pixels through `renderer`), mouse buttons and wheel.
    /// Returns true when the event was input.
    pub fn handle_window_event(
        &mut self,
        event: &WindowEvent,
        renderer: &crate::PcRenderer,
    ) -> bool {
        match event {
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state,
                        repeat: false,
                        ..
                    },
                ..
            } => self.handle_key(*code, *state == ElementState::Pressed),
            WindowEvent::CursorMoved { position, .. } => {
                self.handle_pointer(renderer.window_to_game(position.x, position.y));
            }
            WindowEvent::CursorLeft { .. } => self.handle_pointer(None),
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_button(*button, *state == ElementState::Pressed);
            }
            WindowEvent::MouseWheel { delta, .. } => self.handle_wheel(match delta {
                MouseScrollDelta::LineDelta(_, y) => *y,
                // Touchpads report pixels; ~40 px per notch.
                MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
            }),
            _ => return false,
        }
        true
    }

    /// Mouse moved: `pos` in game pixels, `None` outside the picture.
    pub fn handle_pointer(&mut self, pos: Option<Vec2>) {
        self.state.pointer = pos;
    }

    /// Feed a winit mouse button event.
    pub fn handle_mouse_button(&mut self, button: MouseButton, down: bool) {
        let b = match button {
            MouseButton::Left => PointerButton::Primary,
            MouseButton::Right => PointerButton::Secondary,
            MouseButton::Middle => PointerButton::Middle,
            _ => return,
        };
        self.state.set_pointer_button(b, down);
    }

    /// Wheel motion in notches (positive = away from the user). Accumulates
    /// until the next [`Input::poll`].
    pub fn handle_wheel(&mut self, notches: f32) {
        self.state.wheel += notches;
    }
}

impl Input for PcInput {
    fn poll(&mut self) -> InputState {
        let state = self.state;
        // The wheel is a per-frame delta; everything else is held state.
        self.state.wheel = 0.0;
        state
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
    fn mouse_buttons_pointer_and_wheel() {
        let mut input = PcInput::new();
        input.handle_pointer(Some(Vec2::new(12.0, 34.0)));
        input.handle_mouse_button(MouseButton::Left, true);
        input.handle_mouse_button(MouseButton::Right, true);
        input.handle_wheel(1.0);
        input.handle_wheel(0.5);
        let s = input.poll();
        assert_eq!(s.pointer, Some(Vec2::new(12.0, 34.0)));
        assert!(s.pointer_down(PointerButton::Primary));
        assert!(s.pointer_down(PointerButton::Secondary));
        assert_eq!(s.wheel, 1.5);
        // The wheel resets per poll; buttons and position are held.
        let s = input.poll();
        assert_eq!(s.wheel, 0.0);
        assert!(s.pointer_down(PointerButton::Primary));
        input.handle_mouse_button(MouseButton::Left, false);
        input.handle_pointer(None);
        let s = input.poll();
        assert!(!s.pointer_down(PointerButton::Primary));
        assert_eq!(s.pointer, None);
    }

    #[test]
    fn opposite_keys_cancel() {
        let mut input = PcInput::new();
        input.handle_key(KeyCode::KeyA, true);
        input.handle_key(KeyCode::KeyD, true);
        assert_eq!(input.poll().stick.x, 0.0);
    }
}
