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
use crate::render::Screen;

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

/// Extra PC keyboard keys, beyond the console buttons (bits of
/// [`InputState::keys`]). Consoles never set them; PC backends set them in
/// addition to the button mapping, so a game can offer PC shortcuts (Esc,
/// Space, F-keys…) while console-style play keeps working.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub enum Key {
    Escape = 0,
    Enter,
    Space,
    Tab,
    Delete,
    Backspace,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    Digit1,
    Digit2,
    Digit3,
    PageUp,
    PageDown,
    Home,
    R,
    W,
    A,
    S,
    D,
    Plus,
    Minus,
    Shift,
    Ctrl,
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
    /// PC keyboard keys held ([`Key`] bits). 0 on consoles.
    pub keys: u32,
    /// Which screen `pointer` is on: 0 = the main screen, 1 = the second
    /// screen (3DS touch screen, `Renderer::second_screen`), in that
    /// screen's own pixels ([`Screen::index`]). Read it through
    /// [`pointer_on`](Self::pointer_on).
    pub pointer_screen: u8,
    /// A gamepad button or stick was active this frame (PC): games can show
    /// pad button names instead of keyboard keys.
    pub pad: bool,
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

    /// The pointer, only while it is on `screen` (in that screen's
    /// pixels). World picking on the top screen asks for
    /// `pointer_on(Screen::Top)`, so a touch on the 3DS bottom screen (or
    /// the PC's simulated one) never picks the world; bottom-screen widgets
    /// ask for `Screen::Bottom`, which is always `None` on single-screen
    /// targets.
    #[inline]
    pub fn pointer_on(&self, screen: Screen) -> Option<Vec2> {
        self.pointer
            .filter(|_| self.pointer_screen == screen.index())
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

    /// PC keyboard key held.
    #[inline]
    pub fn key(&self, key: Key) -> bool {
        self.keys & (1 << key as u32) != 0
    }

    /// PC keyboard key pressed this frame (down now, up in `prev`).
    #[inline]
    pub fn key_pressed(&self, prev: &InputState, key: Key) -> bool {
        self.key(key) && !prev.key(key)
    }

    /// Backends call this while polling the keyboard.
    #[inline]
    pub fn set_key(&mut self, key: Key, down: bool) {
        let bit = 1 << key as u32;
        if down {
            self.keys |= bit;
        } else {
            self.keys &= !bit;
        }
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
    fn pointer_on_reports_the_pointer_only_on_its_screen() {
        let mut s = InputState::default();
        assert_eq!(s.pointer_on(Screen::Top), None);
        assert_eq!(s.pointer_on(Screen::Bottom), None);
        // A mouse on the main picture.
        s.pointer = Some(Vec2::new(10.0, 20.0));
        assert_eq!(s.pointer_on(Screen::Top), Some(Vec2::new(10.0, 20.0)));
        assert_eq!(s.pointer_on(Screen::Bottom), None);
        // A 3DS touch (bottom screen): world picking never sees it.
        s.pointer_screen = Screen::Bottom.index();
        assert_eq!(s.pointer_on(Screen::Top), None);
        assert_eq!(s.pointer_on(Screen::Bottom), Some(Vec2::new(10.0, 20.0)));
        // An index no screen has reports nowhere.
        s.pointer_screen = 7;
        assert_eq!(s.pointer_on(Screen::Top), None);
        assert_eq!(s.pointer_on(Screen::Bottom), None);
        assert_eq!(Screen::from_index(7), None);
        for screen in [Screen::Top, Screen::Bottom] {
            assert_eq!(Screen::from_index(screen.index()), Some(screen));
        }
        assert_eq!(Screen::default(), Screen::Top);
    }

    #[test]
    fn pc_keys_are_separate_bits() {
        let prev = InputState::default();
        let mut now = InputState::default();
        now.set_key(Key::Escape, true);
        now.set_key(Key::Ctrl, true);
        assert!(now.key(Key::Escape) && now.key(Key::Ctrl) && !now.key(Key::Space));
        assert!(now.key_pressed(&prev, Key::Escape));
        assert!(!now.is_down(Button::B), "keys never alias buttons");
        now.set_key(Key::Escape, false);
        assert!(!now.key(Key::Escape));
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
