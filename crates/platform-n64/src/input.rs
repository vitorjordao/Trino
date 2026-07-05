//! Joypad input. Button bit positions are packed by the shim to match
//! `trino_core::input::Button` discriminants directly.

use trino_core::{Button, Input, InputState, Vec2};

use crate::ffi;

pub struct N64Input;

const BUTTONS: [Button; 12] = [
    Button::A,
    Button::B,
    Button::X,
    Button::Y,
    Button::L,
    Button::R,
    Button::Start,
    Button::Select,
    Button::DpadUp,
    Button::DpadDown,
    Button::DpadLeft,
    Button::DpadRight,
];

/// N64 stick reaches roughly +/-85 on real hardware.
const STICK_RANGE: f32 = 85.0;

impl Input for N64Input {
    fn poll(&mut self) -> InputState {
        let bits = unsafe { ffi::trino_joypad_buttons() };
        let stick = unsafe { ffi::trino_joypad_stick() };

        let mut state = InputState::default();
        for button in BUTTONS {
            if bits & (1 << button as u16) != 0 {
                state.set(button, true);
            }
        }
        let x = (stick >> 16) as i16 as f32 / STICK_RANGE;
        let y = stick as u16 as i16 as f32 / STICK_RANGE;
        state.stick = Vec2::new(x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0));
        state
    }
}
