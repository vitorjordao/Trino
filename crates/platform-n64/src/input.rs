//! Joypad input: the shim packs the buttons (bits 0..11 at their
//! `trino_core::input::Button` discriminants, the raw C buttons at 16..19)
//! and the stick; [`crate::pad::decode`] maps them. The four controller
//! ports are four players ([`Input::poll_players`], issue #67:
//! [`crate::pad::poll_ports`]).

use trino_core::{Input, InputState, PlayersInput, Seats};

use crate::ffi;

pub struct N64Input {
    seats: Seats,
}

impl N64Input {
    pub const fn new() -> N64Input {
        N64Input {
            seats: Seats::new(),
        }
    }

    /// The ports as devices, with their seats (for the platform's
    /// `HostedPlayers::publish`).
    pub fn seats(&self) -> &Seats {
        &self.seats
    }

    pub fn seats_mut(&mut self) -> &mut Seats {
        &mut self.seats
    }
}

impl Default for N64Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Input for N64Input {
    fn poll(&mut self) -> InputState {
        let bits = unsafe { ffi::trino_joypad_buttons() };
        let stick = unsafe { ffi::trino_joypad_stick() };
        crate::pad::decode(bits, stick)
    }

    fn poll_players(&mut self) -> PlayersInput {
        let mask = unsafe { ffi::trino_joypad_poll() };
        crate::pad::poll_ports(&mut self.seats, mask, |port| unsafe {
            (
                ffi::trino_joypad_port_buttons(port),
                ffi::trino_joypad_port_stick(port),
            )
        })
    }
}
