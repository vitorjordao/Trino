//! The shim's packed joypad words → [`InputState`] (pure, so host tests
//! check the mapping without a console).
//!
//! C-left and C-down are X and Y (as always), and the four C buttons are
//! also the digital `right_stick`: C-left/C-right = x -1/+1, C-up/C-down =
//! y +1/-1. Z is Select. L2, R2, L3, R3 and `triggers` stay off: the N64
//! pad has nothing left for them.

use trino_core::{Button, InputState, Vec2};

/// The buttons the shim reports (bits 0..11, at their discriminants).
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

/// The shim's raw C-button bits.
pub const C_UP: u32 = 1 << 16;
pub const C_DOWN: u32 = 1 << 17;
pub const C_LEFT: u32 = 1 << 18;
pub const C_RIGHT: u32 = 1 << 19;

/// N64 stick reaches roughly +/-85 on real hardware.
const STICK_RANGE: f32 = 85.0;

/// `trino_joypad_buttons` and `trino_joypad_stick` → the input state.
pub fn decode(bits: u32, stick: u32) -> InputState {
    let mut state = InputState::default();
    for button in BUTTONS {
        if bits & (1 << button as u32) != 0 {
            state.set(button, true);
        }
    }
    let axis = |pos: u32, neg: u32| (bits & pos != 0) as i8 as f32 - (bits & neg != 0) as i8 as f32;
    state.right_stick = Vec2::new(axis(C_RIGHT, C_LEFT), axis(C_UP, C_DOWN));
    let x = (stick >> 16) as i16 as f32 / STICK_RANGE;
    let y = stick as u16 as i16 as f32 / STICK_RANGE;
    state.stick = Vec2::new(x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0));
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the shim packs: the C buttons keep X/Y, add the right stick.
    #[test]
    fn c_buttons_are_x_y_and_the_right_stick() {
        let idle = decode(0, 0);
        assert_eq!(idle, InputState::default());
        let c_left_down = decode(C_LEFT | C_DOWN | 1 << 2 | 1 << 3, 0);
        assert!(c_left_down.is_down(Button::X) && c_left_down.is_down(Button::Y));
        assert_eq!(c_left_down.right_stick, Vec2::new(-1.0, -1.0));
        let c_up_right = decode(C_UP | C_RIGHT, 0);
        assert_eq!(c_up_right.right_stick, Vec2::new(1.0, 1.0));
        assert!(!c_up_right.any_button(), "C-up/C-right press no button");
        assert_eq!(decode(C_LEFT | C_RIGHT, 0).right_stick, Vec2::ZERO);
        // Z (bit 7) stays Select; nothing reaches L2..R3 or the triggers.
        let all = decode(0xffff_ffff, 0);
        assert!(all.is_down(Button::Select));
        for b in [Button::L2, Button::R2, Button::L3, Button::R3] {
            assert!(!all.is_down(b), "{b:?}");
        }
        assert_eq!(all.triggers, Vec2::ZERO);
        // The stick: x high, y low, signed, clamped.
        let s = decode(0, (85u32 << 16) | (-170i16 as u16 as u32));
        assert_eq!(s.stick, Vec2::new(1.0, -1.0));
    }
}
