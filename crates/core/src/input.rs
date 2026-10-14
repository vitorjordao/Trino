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
//!
//! The **pointer** is optional: a mouse on PC, the touch screen on 3DS
//! (mapped onto the top screen's coordinates), absent on the N64 (no
//! mouse support yet). A game that wants pointer control must also work
//! with the stick and D-pad — a virtual cursor is the portable pattern.

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

/// A pointer button. Discriminants are bit positions in [`InputState`].
/// On 3DS a touch is `Primary`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub enum PointerButton {
    Primary = 0,
    Secondary = 1,
    Middle = 2,
}

/// Snapshot of the controller for one frame. `Copy` so games can keep the
/// previous frame's state for edge detection.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct InputState {
    buttons: u16,
    pointer_buttons: u8,
    /// Analog stick, each axis in -1.0..=1.0. Y-up positive.
    pub stick: Vec2,
    /// Pointer position in **game screen pixels** (the internal resolution
    /// the game was created with: X-right, Y-down, origin top-left).
    /// `None` when the platform has no pointer, the mouse is outside the
    /// picture, or (3DS) nothing touches the screen.
    pub pointer: Option<Vec2>,
    /// Scroll wheel notches **this frame** (positive = away from the user).
    /// Always 0 without a wheel.
    pub wheel: f32,
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

    #[inline]
    pub fn pointer_down(&self, button: PointerButton) -> bool {
        self.pointer_buttons & (1 << button as u8) != 0
    }

    /// Pointer button pressed this frame (down now, up in `prev`).
    #[inline]
    pub fn pointer_pressed(&self, prev: &InputState, button: PointerButton) -> bool {
        self.pointer_down(button) && !prev.pointer_down(button)
    }

    /// Pointer button released this frame (up now, down in `prev`).
    #[inline]
    pub fn pointer_released(&self, prev: &InputState, button: PointerButton) -> bool {
        !self.pointer_down(button) && prev.pointer_down(button)
    }

    /// Backends call this while polling the native pointer.
    #[inline]
    pub fn set_pointer_button(&mut self, button: PointerButton, down: bool) {
        let bit = 1 << button as u8;
        if down {
            self.pointer_buttons |= bit;
        } else {
            self.pointer_buttons &= !bit;
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
    fn pointer_buttons_and_edges() {
        let prev = InputState::default();
        let mut now = InputState::default();
        assert_eq!(now.pointer, None);
        assert_eq!(now.wheel, 0.0);
        now.pointer = Some(Vec2::new(10.0, 20.0));
        now.set_pointer_button(PointerButton::Primary, true);
        assert!(now.pointer_down(PointerButton::Primary));
        assert!(!now.pointer_down(PointerButton::Secondary));
        assert!(now.pointer_pressed(&prev, PointerButton::Primary));
        // Pointer bits never alias controller buttons.
        assert!(!now.is_down(Button::A));
        let later = InputState::default();
        assert!(later.pointer_released(&now, PointerButton::Primary));
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
