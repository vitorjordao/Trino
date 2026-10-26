//! HID input. Button bit positions are packed by the shim to match
//! `trino_core::input::Button` discriminants directly; [`crate::pad`] maps
//! them, the circle pad and the New 3DS's C-stick.
//!
//! The touch screen is the pointer, in its own pixels (320x240) with
//! `pointer_screen = 1` (the renderer's second screen). A touch holds
//! `PointerButton::Primary`.

use trino_core::{DeviceKind, Input, InputState, PlayersInput, PointerButton, Seats, Vec2};

use crate::ffi;

/// The console's controls: one player ([`Input::poll_players`] reports
/// them as port 1, issue #67; more players need two consoles).
pub struct N3dsInput {
    seats: Seats,
}

impl N3dsInput {
    pub const fn new() -> N3dsInput {
        N3dsInput {
            seats: Seats::new(),
        }
    }

    pub fn seats(&self) -> &Seats {
        &self.seats
    }

    pub fn seats_mut(&mut self) -> &mut Seats {
        &mut self.seats
    }
}

impl Default for N3dsInput {
    fn default() -> Self {
        Self::new()
    }
}

impl Input for N3dsInput {
    fn poll_players(&mut self) -> PlayersInput {
        let state = self.poll();
        self.seats.begin();
        self.seats.report(DeviceKind::Port, 0, None, state);
        *self.seats.finish()
    }

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
