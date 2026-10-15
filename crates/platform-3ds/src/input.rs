//! HID input. Button bit positions are packed by the shim to match
//! `trino_core::input::Button` discriminants directly.
//!
//! The touch screen is the pointer, in its own pixels (320x240) with
//! `pointer_screen = 1` (the renderer's second screen). A touch holds
//! `PointerButton::Primary`.

use trino_core::{Button, Input, InputState, PointerButton, Vec2};

use crate::ffi;

pub struct N3dsInput;

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

/// Circle pad reaches roughly +/-156 on real hardware.
const STICK_RANGE: f32 = 156.0;

impl Input for N3dsInput {
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
        let touch = unsafe { ffi::trino_touch() };
        if let Some(p) = touch_point(touch) {
            state.pointer = Some(p);
            state.pointer_screen = 1;
            state.set_pointer_button(PointerButton::Primary, true);
        }
        state
    }
}

/// The packed touch (bit 31 = touching) → touch-screen pixels.
fn touch_point(packed: u32) -> Option<Vec2> {
    if packed & (1 << 31) == 0 {
        return None;
    }
    let x = (packed & 0x3ff) as f32;
    let y = ((packed >> 16) & 0xff) as f32;
    Some(Vec2::new(x, y))
}
