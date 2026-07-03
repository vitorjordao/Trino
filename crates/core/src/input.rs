//! The input contract.
//!
//! The button set is the common denominator of the three targets, mapped
//! per platform:
//!
//! | Trino    | N64        | 3DS    | PC (default)     |
//! |----------|------------|--------|------------------|
//! | A / B    | A / B      | A / B  | Z / X            |
//! | X / Y    | C-down/left| X / Y  | A / S            |
//! | L / R    | L / R      | L / R  | Q / W            |
//! | Start    | Start      | Start  | Enter            |
//! | Select   | Z          | Select | Shift            |
//! | D-pad    | D-pad      | D-pad  | Arrows           |
//! | Stick    | Stick      | Circle | WASD / gamepad   |

use crate::math::Vec2;

/// A digital button. Discriminants are bit positions in [`InputState`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u16)]
pub enum Button {
    A = 0,
    B = 1,
    X = 2,
    Y = 3,
    L = 4,
    R = 5,
    Start = 6,
    Select = 7,
    DpadUp = 8,
    DpadDown = 9,
    DpadLeft = 10,
    DpadRight = 11,
}

/// Snapshot of the controller for one frame. `Copy` so games can keep the
/// previous frame's state for edge detection.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct InputState {
    buttons: u16,
    /// Analog stick, each axis in -1.0..=1.0. Y-up positive.
    pub stick: Vec2,
}

impl InputState {
    #[inline]
    pub fn is_down(&self, button: Button) -> bool {
        self.buttons & (1 << button as u16) != 0
    }

    /// Pressed this frame (down now, up in `prev`).
    #[inline]
    pub fn just_pressed(&self, prev: &InputState, button: Button) -> bool {
        self.is_down(button) && !prev.is_down(button)
    }

    /// Released this frame (up now, down in `prev`).
    #[inline]
    pub fn just_released(&self, prev: &InputState, button: Button) -> bool {
        !self.is_down(button) && prev.is_down(button)
    }

    /// Backends call this while polling the native controller.
    #[inline]
    pub fn set(&mut self, button: Button, down: bool) {
        let bit = 1 << button as u16;
        if down {
            self.buttons |= bit;
        } else {
            self.buttons &= !bit;
        }
    }
}

/// What every platform backend implements: poll the controller once per
/// frame and hand the snapshot to the game.
pub trait Input {
    fn poll(&mut self) -> InputState;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_read_buttons() {
        let mut s = InputState::default();
        s.set(Button::A, true);
        s.set(Button::DpadRight, true);
        assert!(s.is_down(Button::A));
        assert!(s.is_down(Button::DpadRight));
        assert!(!s.is_down(Button::B));
        s.set(Button::A, false);
        assert!(!s.is_down(Button::A));
    }

    #[test]
    fn edge_detection() {
        let mut prev = InputState::default();
        let mut now = InputState::default();
        now.set(Button::Start, true);
        assert!(now.just_pressed(&prev, Button::Start));
        assert!(!now.just_released(&prev, Button::Start));

        prev.set(Button::Start, true);
        assert!(!now.just_pressed(&prev, Button::Start));

        now.set(Button::Start, false);
        assert!(now.just_released(&prev, Button::Start));
    }
}
