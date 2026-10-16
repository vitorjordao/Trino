//! HID input. Button bit positions are packed by the shim to match
//! `trino_core::input::Button` discriminants directly; [`crate::pad`] maps
//! them, the circle pad and the New 3DS's C-stick.
//!
//! The touch screen is the pointer, in its own pixels (320x240) with
//! `pointer_screen = 1` (the renderer's second screen). A touch holds
//! `PointerButton::Primary`.

use trino_core::{Input, InputState, PointerButton, Vec2};

use crate::ffi;

pub struct N3dsInput;

impl Input for N3dsInput {
    fn poll(&mut self) -> InputState {
        let bits = unsafe { ffi::trino_joypad_buttons() };
        let stick = unsafe { ffi::trino_joypad_stick() };
        let cstick = unsafe { ffi::trino_cstick() };
        let mut state = crate::pad::decode(bits, stick, cstick);
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
