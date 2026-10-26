//! A player's input as it crosses a network (issue #67, ADR 0036): the
//! console buttons, both sticks and the triggers in 9 bytes.
//!
//! A network session (rollback or lockstep) sends each player's input every
//! frame, so every peer must simulate with exactly the same values —
//! including the local player's own: the host quantizes its input with
//! [`NetPad::from_input`] and feeds the game [`NetPad::to_input`] of it, as
//! the remote peers do. Sticks and triggers keep 1/127 and 1/255 steps; the
//! PC keyboard ([`InputState::keys`]), the pointer and the wheel don't cross
//! (a netplayed game reads the console buttons and sticks, the input every
//! target has).

use crate::input::InputState;
use crate::math::Vec2;

/// One player's networked input.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct NetPad {
    /// [`InputState::button_bits`] (the 16 console buttons).
    pub buttons: u16,
    /// The left stick, -127..=127 per axis.
    pub stick: [i8; 2],
    /// The right stick.
    pub right: [i8; 2],
    /// The triggers, 0..=255.
    pub triggers: [u8; 2],
    /// Bit 0: a gamepad was used ([`InputState::pad`]).
    pub flags: u8,
}

/// Bytes of [`NetPad::to_bytes`].
pub const NET_PAD_LEN: usize = 9;

fn axis(v: f32) -> i8 {
    let q = v.clamp(-1.0, 1.0) * 127.0;
    // Round half away from zero without `round` (core has none).
    (if q >= 0.0 { q + 0.5 } else { q - 0.5 }) as i8
}

fn trigger(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

impl NetPad {
    /// Nothing held.
    pub const IDLE: NetPad = NetPad {
        buttons: 0,
        stick: [0; 2],
        right: [0; 2],
        triggers: [0; 2],
        flags: 0,
    };

    /// Quantize `s` (what crosses the network).
    pub fn from_input(s: &InputState) -> NetPad {
        NetPad {
            buttons: s.button_bits() as u16,
            stick: [axis(s.stick.x), axis(s.stick.y)],
            right: [axis(s.right_stick.x), axis(s.right_stick.y)],
            triggers: [trigger(s.triggers.x), trigger(s.triggers.y)],
            flags: s.pad as u8,
        }
    }

    /// The input every peer feeds the game.
    pub fn to_input(&self) -> InputState {
        let mut s = InputState::IDLE;
        s.set_button_bits(self.buttons as u32);
        let a = |v: i8| v as f32 / 127.0;
        s.stick = Vec2::new(a(self.stick[0]), a(self.stick[1]));
        s.right_stick = Vec2::new(a(self.right[0]), a(self.right[1]));
        s.triggers = Vec2::new(
            self.triggers[0] as f32 / 255.0,
            self.triggers[1] as f32 / 255.0,
        );
        s.pad = self.flags & 1 != 0;
        s
    }

    pub fn to_bytes(&self) -> [u8; NET_PAD_LEN] {
        let b = self.buttons.to_le_bytes();
        [
            b[0],
            b[1],
            self.stick[0] as u8,
            self.stick[1] as u8,
            self.right[0] as u8,
            self.right[1] as u8,
            self.triggers[0],
            self.triggers[1],
            self.flags,
        ]
    }

    pub fn from_bytes(b: [u8; NET_PAD_LEN]) -> NetPad {
        NetPad {
            buttons: u16::from_le_bytes([b[0], b[1]]),
            stick: [b[2] as i8, b[3] as i8],
            right: [b[4] as i8, b[5] as i8],
            triggers: [b[6], b[7]],
            flags: b[8],
        }
    }

    /// `s` as every peer will see it (the local player's input after the
    /// round trip through the network format).
    pub fn quantize(s: &InputState) -> InputState {
        NetPad::from_input(s).to_input()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Button, Key};

    #[test]
    fn a_pad_round_trips_and_quantizes_stably() {
        let mut s = InputState::IDLE;
        s.set(Button::A, true);
        s.set(Button::R3, true);
        s.stick = Vec2::new(0.5, -1.0);
        s.right_stick = Vec2::new(-0.33, 0.0);
        s.triggers = Vec2::new(0.25, 1.0);
        s.pad = true;
        s.set_key(Key::Space, true);
        s.pointer = Some(Vec2::new(3.0, 4.0));
        let n = NetPad::from_input(&s);
        assert_eq!(NetPad::from_bytes(n.to_bytes()), n);
        let q = n.to_input();
        assert!(q.is_down(Button::A) && q.is_down(Button::R3) && q.pad);
        assert_eq!(q.stick.y, -1.0);
        assert!((q.stick.x - 0.5).abs() < 0.005);
        assert!((q.right_stick.x + 0.33).abs() < 0.005);
        assert_eq!(q.triggers.y, 1.0);
        // The keyboard and the pointer don't cross.
        assert!(q.keys.is_empty() && q.pointer.is_none());
        // Quantizing twice changes nothing: both peers see one value.
        assert_eq!(NetPad::quantize(&q), q);
        assert_eq!(NetPad::from_input(&InputState::IDLE), NetPad::IDLE);
        assert_eq!(NetPad::IDLE.to_input(), InputState::IDLE);
    }
}
