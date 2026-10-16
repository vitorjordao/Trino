//! The shim's packed HID words → [`InputState`] (pure, so host tests check
//! the mapping without a console). The touch is added by
//! [`crate::N3dsInput`].
//!
//! New 3DS extras: ZL/ZR are L2/R2 (and `triggers` 0 or 1), the C-stick is
//! `right_stick`. On an Old 3DS they stay off. L3/R3 do not exist here.

use trino_core::{Button, InputState, Vec2};

/// The buttons the shim reports (bits 0..13, at their discriminants).
const BUTTONS: [Button; 14] = [
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
    Button::L2,
    Button::R2,
];

/// Circle pad (and C-stick) reach roughly +/-156 on real hardware.
const STICK_RANGE: f32 = 156.0;

/// A packed stick (x in the high 16 bits, y in the low, signed) → -1..=1.
fn unpack_stick(packed: u32) -> Vec2 {
    let x = (packed >> 16) as i16 as f32 / STICK_RANGE;
    let y = packed as u16 as i16 as f32 / STICK_RANGE;
    Vec2::new(x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0))
}

/// `trino_joypad_buttons`, `trino_joypad_stick` and `trino_cstick` → the
/// input state (no pointer).
pub fn decode(bits: u32, stick: u32, cstick: u32) -> InputState {
    let mut state = InputState::default();
    for button in BUTTONS {
        if bits & (1 << button as u32) != 0 {
            state.set(button, true);
        }
    }
    state.stick = unpack_stick(stick);
    state.right_stick = unpack_stick(cstick);
    state.triggers = Vec2::new(
        state.is_down(Button::L2) as u8 as f32,
        state.is_down(Button::R2) as u8 as f32,
    );
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(x: i16, y: i16) -> u32 {
        ((x as u16 as u32) << 16) | y as u16 as u32
    }

    #[test]
    fn zl_zr_and_the_c_stick() {
        assert_eq!(decode(0, 0, 0), InputState::default());
        let s = decode(1 << 12, pack(156, -156), pack(-78, 300));
        assert!(s.is_down(Button::L2) && !s.is_down(Button::R2));
        assert_eq!(s.triggers, Vec2::new(1.0, 0.0));
        assert_eq!(s.stick, Vec2::new(1.0, -1.0));
        assert_eq!(s.right_stick, Vec2::new(-0.5, 1.0));
        let zr = decode(1 << 13, 0, 0);
        assert!(zr.is_down(Button::R2) && zr.triggers == Vec2::new(0.0, 1.0));
        // Bits past ZR never reach L3/R3.
        let all = decode(0xffff_ffff, 0, 0);
        assert!(!all.is_down(Button::L3) && !all.is_down(Button::R3));
        assert!(Button::ALL[..14].iter().all(|b| all.is_down(*b)));
    }
}
