//! Keyboard + mouse + gamepad input backend. Gamepads come through gilrs
//! with the `gamepad` feature (Xbox-style layout: A/B/X/Y = South/East/
//! West/North, L/R = bumpers or triggers, the left stick and the D-pad);
//! every connected pad is merged with the keyboard.
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
    #[cfg(feature = "gamepad")]
    pads: Option<gilrs::Gilrs>,
}

impl PcInput {
    pub fn new() -> Self {
        #[cfg(feature = "gamepad")]
        {
            // No pad subsystem (headless CI, missing permissions): keyboard
            // and mouse still work.
            Self {
                pads: gilrs::Gilrs::new().ok(),
                ..Self::default()
            }
        }
        #[cfg(not(feature = "gamepad"))]
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

#[cfg(feature = "gamepad")]
impl PcInput {
    /// Fold every connected gamepad into `state` (buttons OR'ed, the
    /// strongest stick wins over the keyboard's digital stick).
    fn merge_pads(&mut self, state: &mut InputState) {
        let Some(g) = self.pads.as_mut() else { return };
        while g.next_event().is_some() {}
        use gilrs::{Axis, Button as B};
        const MAP: [(B, Button); 16] = [
            (B::South, Button::A),
            (B::East, Button::B),
            (B::West, Button::X),
            (B::North, Button::Y),
            (B::LeftTrigger, Button::L),
            (B::LeftTrigger2, Button::L),
            (B::RightTrigger, Button::R),
            (B::RightTrigger2, Button::R),
            (B::Start, Button::Start),
            (B::Select, Button::Select),
            (B::DPadUp, Button::DpadUp),
            (B::DPadDown, Button::DpadDown),
            (B::DPadLeft, Button::DpadLeft),
            (B::DPadRight, Button::DpadRight),
            (B::Mode, Button::Start),
            (B::LeftThumb, Button::Select),
        ];
        for (_, pad) in g.gamepads() {
            for (from, to) in MAP {
                if pad.is_pressed(from) {
                    state.set(to, true);
                }
            }
            // gilrs sticks are Y-up, like InputState.
            let s = Vec2::new(pad.value(Axis::LeftStickX), pad.value(Axis::LeftStickY));
            if s.x * s.x + s.y * s.y > state.stick.x * state.stick.x + state.stick.y * state.stick.y
            {
                state.stick = s;
            }
        }
    }
}

impl Input for PcInput {
    fn poll(&mut self) -> InputState {
        #[allow(unused_mut)]
        let mut state = self.state;
        #[cfg(feature = "gamepad")]
        self.merge_pads(&mut state);
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
