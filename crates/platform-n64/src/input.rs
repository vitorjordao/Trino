//! Joypad input: the shim packs the buttons (bits 0..11 at their
//! `trino_core::input::Button` discriminants, the raw C buttons at 16..19)
//! and the stick; [`crate::pad::decode`] maps them.

use trino_core::{Input, InputState};

use crate::ffi;

pub struct N64Input;

impl Input for N64Input {
    fn poll(&mut self) -> InputState {
        let bits = unsafe { ffi::trino_joypad_buttons() };
        let stick = unsafe { ffi::trino_joypad_stick() };
        crate::pad::decode(bits, stick)
    }
}
