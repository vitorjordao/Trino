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

    /// Any 9 bytes decode. An axis of -128 (which [`from_input`](Self::from_input)
    /// never writes, but a hostile peer may send) reads -127, so a stick
    /// stays within -1..=1 on every peer (found by the `netpad` fuzz
    /// target, issue #117).
    pub fn from_bytes(b: [u8; NET_PAD_LEN]) -> NetPad {
        let ax = |v: u8| (v as i8).max(-127);
        NetPad {
            buttons: u16::from_le_bytes([b[0], b[1]]),
            stick: [ax(b[2]), ax(b[3])],
            right: [ax(b[4]), ax(b[5])],
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

/// Bytes of [`NetInput::to_bytes`]: a [`NetPad`], then the keyboard (16),
/// the pointer (x, y as `u16` fractions, a flags byte, its buttons) and the
/// wheel (eighths of a notch).
pub const NET_INPUT_LEN: usize = 32;

/// What a sender puts on the wire beyond the pad (`[net] keys`,
/// `[net] pointer`). Receivers decode whatever arrives, so two peers that
/// disagree here don't desync: one only never sends its keyboard.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct NetExtras {
    pub keys: bool,
    pub pointer: bool,
}

impl NetExtras {
    /// The pad alone (the default: every target has one).
    pub const PAD: NetExtras = NetExtras {
        keys: false,
        pointer: false,
    };
}

/// One player's networked input with the PC's extras (issue #117).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct NetInput {
    pub pad: NetPad,
    /// [`KeySet::bits`](crate::KeySet::bits); zero unless the sender opted in.
    pub keys: [u64; 2],
    /// The pointer as a fraction of the sender's screen, 0..=65535 per axis.
    pub pointer: Option<[u16; 2]>,
    /// [`InputState::pointer_screen`].
    pub pointer_screen: u8,
    /// [`InputState::pointer_bits`].
    pub pointer_buttons: u8,
    /// The wheel in eighths of a notch.
    pub wheel: i8,
}

fn fraction(v: f32, size: u16) -> u16 {
    let f = (v / size.max(1) as f32).clamp(0.0, 1.0) * 65535.0;
    (f + 0.5) as u16
}

impl NetInput {
    pub const IDLE: NetInput = NetInput {
        pad: NetPad::IDLE,
        keys: [0; 2],
        pointer: None,
        pointer_screen: 0,
        pointer_buttons: 0,
        wheel: 0,
    };

    /// Quantize `s`; `screen` is the sender's game screen in pixels (what
    /// its pointer is measured in), `extras` what it shares.
    pub fn from_input(s: &InputState, extras: NetExtras, screen: (u16, u16)) -> NetInput {
        let mut n = NetInput {
            pad: NetPad::from_input(s),
            ..NetInput::IDLE
        };
        if extras.keys {
            n.keys = s.keys.bits();
        }
        if extras.pointer {
            n.pointer = s
                .pointer
                .map(|p| [fraction(p.x, screen.0), fraction(p.y, screen.1)]);
            n.pointer_screen = s.pointer_screen & 1;
            n.pointer_buttons = s.pointer_bits();
            let w = s.wheel.clamp(-15.0, 15.0) * 8.0;
            n.wheel = (if w >= 0.0 { w + 0.5 } else { w - 0.5 }) as i8;
        }
        n
    }

    /// The input every peer feeds the game, the pointer in `space` (the
    /// project's `pointer_space`, the same on every peer).
    pub fn to_input(&self, space: (u16, u16)) -> InputState {
        let mut s = self.pad.to_input();
        s.keys = crate::KeySet::from_bits(self.keys);
        if let Some([x, y]) = self.pointer {
            s.pointer = Some(Vec2::new(
                x as f32 * space.0 as f32 / 65535.0,
                y as f32 * space.1 as f32 / 65535.0,
            ));
            s.pointer_screen = self.pointer_screen & 1;
        }
        s.set_pointer_bits(self.pointer_buttons);
        s.wheel = self.wheel as f32 / 8.0;
        s
    }

    pub fn to_bytes(&self) -> [u8; NET_INPUT_LEN] {
        let mut b = [0u8; NET_INPUT_LEN];
        b[..NET_PAD_LEN].copy_from_slice(&self.pad.to_bytes());
        b[9..17].copy_from_slice(&self.keys[0].to_le_bytes());
        b[17..25].copy_from_slice(&self.keys[1].to_le_bytes());
        let [x, y] = self.pointer.unwrap_or([0, 0]);
        b[25..27].copy_from_slice(&x.to_le_bytes());
        b[27..29].copy_from_slice(&y.to_le_bytes());
        b[29] = self.pointer.is_some() as u8 | (self.pointer_screen & 1) << 1;
        b[30] = self.pointer_buttons;
        b[31] = self.wheel as u8;
        b
    }

    /// Any 32 bytes decode (a hostile peer can send anything): unknown
    /// flag bits are dropped, the wheel is held to ±15 notches.
    pub fn from_bytes(b: [u8; NET_INPUT_LEN]) -> NetInput {
        let mut pad = [0u8; NET_PAD_LEN];
        pad.copy_from_slice(&b[..NET_PAD_LEN]);
        let u64_at = |i: usize| {
            let mut w = [0u8; 8];
            w.copy_from_slice(&b[i..i + 8]);
            u64::from_le_bytes(w)
        };
        let present = b[29] & 1 != 0;
        NetInput {
            pad: NetPad::from_bytes(pad),
            keys: [u64_at(9), u64_at(17)],
            pointer: present.then(|| {
                [
                    u16::from_le_bytes([b[25], b[26]]),
                    u16::from_le_bytes([b[27], b[28]]),
                ]
            }),
            pointer_screen: if present { (b[29] >> 1) & 1 } else { 0 },
            pointer_buttons: b[30],
            // ±15 notches is what a sender writes; a hostile byte past it
            // reads as the limit (the `netpad` fuzz target, CI).
            wheel: (b[31] as i8).clamp(-120, 120),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Button, Key, PointerButton};

    #[test]
    fn keys_pointer_and_wheel_cross_in_the_shared_space() {
        let mut s = InputState::IDLE;
        s.set(Button::B, true);
        s.set_key(Key::W, true);
        s.set_key(Key::F12, true);
        s.pointer = Some(Vec2::new(960.0, 270.0));
        s.set_pointer_button(PointerButton::Primary, true);
        s.wheel = -1.5;
        let all = NetExtras {
            keys: true,
            pointer: true,
        };
        // A 1920x1080 sender and a 640x360 one pointing at the same spot
        // both arrive at (160, 60) in a 320x240 space.
        let big = NetInput::from_input(&s, all, (1920, 1080));
        let mut small = s;
        small.pointer = Some(Vec2::new(320.0, 90.0));
        let small = NetInput::from_input(&small, all, (640, 360));
        assert_eq!(big, small);
        assert_eq!(NetInput::from_bytes(big.to_bytes()), big);
        let q = big.to_input((320, 240));
        let p = q.pointer.unwrap();
        assert!(
            (p.x - 160.0).abs() < 0.01 && (p.y - 60.0).abs() < 0.01,
            "{p:?}"
        );
        assert!(q.key(Key::W) && q.key(Key::F12) && q.is_down(Button::B));
        assert_eq!(q.pointer_bits(), s.pointer_bits());
        assert_eq!(q.wheel, -1.5);
        // Decoding is idempotent: every peer feeds the same value.
        let again = NetInput::from_input(&q, all, (320, 240)).to_input((320, 240));
        assert_eq!(again, q);
        // Pad-only senders leave the extras zero (and 9 bytes of a NetPad).
        let pad = NetInput::from_input(&s, NetExtras::PAD, (1920, 1080));
        assert_eq!(pad.to_bytes()[9..], [0u8; 23]);
        assert!(pad.to_input((320, 240)).keys.is_empty());
        assert_eq!(pad.pad, NetPad::from_input(&s));
        // A hostile -128 axis stays a full stick (the fuzz target's find).
        let edge = NetPad::from_bytes([0, 0, 0x80, 0x80, 0x80, 0x80, 0, 0, 0]);
        assert_eq!(edge.to_input().stick, Vec2::new(-1.0, -1.0));
        assert_eq!(NetPad::quantize(&edge.to_input()), edge.to_input());
        // Anything decodes.
        let mut w = [0u8; NET_INPUT_LEN];
        w[31] = 0x83; // -125 eighths: past what a sender writes
        assert_eq!(NetInput::from_bytes(w).to_input((320, 240)).wheel, -15.0);
        let junk = NetInput::from_bytes([0xff; NET_INPUT_LEN]);
        assert!(junk.pointer.is_some());
        let _ = junk.to_input((320, 240));
    }

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
