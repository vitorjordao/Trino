//! The input contract.
//!
//! The button set is the common denominator of the three targets, mapped
//! per platform:
//!
//! | Trino     | N64              | 3DS                | PC keys (default) | PC gamepad        |
//! |-----------|------------------|--------------------|-------------------|-------------------|
//! | A / B     | A / B            | A / B              | Z / X             | A / B (S/E)       |
//! | X / Y     | C-left / C-down  | X / Y              | C / V             | X / Y (W/N)       |
//! | L / R     | L / R            | L / R              | Q / E             | LB+LT / RB+RT     |
//! | L2 / R2   | —                | ZL / ZR (New 3DS)  | — (bindable)      | LT / RT           |
//! | L3 / R3   | —                | —                  | — (bindable)      | LS / RS (clicks)  |
//! | Start     | Start            | Start              | Enter             | Start, Guide      |
//! | Select    | Z                | Select             | Right Shift       | Back, LS          |
//! | D-pad     | D-pad            | D-pad              | Arrows            | D-pad             |
//! | `stick`   | Stick            | Circle pad         | WASD              | Left stick        |
//! | `right_stick` | C buttons (digital) | C-stick (New 3DS) | —          | Right stick       |
//! | `triggers`| —                | ZL / ZR (0 or 1)   | L2 / R2 keys (0 or 1) | LT / RT (analog) |
//!
//! The **keyboard** ([`Key`], [`InputState::keys`]) is PC-only: every key
//! sets its bit, on top of the button mapping above. Consoles never set it.
//!
//! The **pointer** is optional: a mouse on PC, the touch screen on 3DS
//! (on the second screen, [`InputState::pointer_on`]), absent on the N64
//! (no mouse support yet). A game that wants pointer control must also work
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
    /// Second shoulder: a gamepad's left trigger, the New 3DS's ZL.
    L2 = 12,
    /// Second shoulder: a gamepad's right trigger, the New 3DS's ZR.
    R2 = 13,
    /// Left stick click (PC gamepads).
    L3 = 14,
    /// Right stick click (PC gamepads).
    R3 = 15,
}

impl Button {
    /// Every button, in discriminant order.
    pub const ALL: [Button; 16] = [
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
        Button::L3,
        Button::R3,
    ];
}

/// A PC keyboard key (bits of [`InputState::keys`]). Consoles never set
/// them; PC backends set them in addition to the button mapping, so a game
/// can offer PC shortcuts (Esc, Space, F-keys…) and text-like controls
/// while console-style play keeps working.
///
/// Discriminants are bit positions in [`KeySet`]. The first 30 keep the
/// positions they had before the full keyboard was added, so code that
/// compared `keys` with a `u32` mask still means the same keys.
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
    /// `=`/`+` on the main row or `+` on the numpad (the `=` key also sets
    /// [`Key::Equal`]).
    Plus,
    /// `-` on the main row or on the numpad.
    Minus,
    /// Either Shift.
    Shift,
    /// Either Control.
    Ctrl,
    /// A letter for a game's view toggle (e.g. roofs on and off).
    T,
    // Added with the full keyboard: letters, digits, F9–F12, navigation,
    // modifiers and punctuation.
    B,
    C,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    U,
    V,
    X,
    Y,
    Z,
    Digit0,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F9,
    F10,
    F11,
    F12,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    End,
    Insert,
    /// Either Alt (Option on macOS).
    Alt,
    /// Either Super (Windows / Command).
    Super,
    /// The main row's `=` key (also sets [`Key::Plus`]).
    Equal,
    BracketLeft,
    BracketRight,
    Semicolon,
    Quote,
    Comma,
    Period,
    Slash,
    Backslash,
    /// `` ` `` / `~`.
    Backquote,
}

impl Key {
    /// How many keys there are.
    pub const COUNT: usize = Key::ALL.len();

    /// Every key, in discriminant order (`Key::ALL[k as usize] == k`).
    pub const ALL: [Key; 79] = [
        Key::Escape,
        Key::Enter,
        Key::Space,
        Key::Tab,
        Key::Delete,
        Key::Backspace,
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::Digit1,
        Key::Digit2,
        Key::Digit3,
        Key::PageUp,
        Key::PageDown,
        Key::Home,
        Key::R,
        Key::W,
        Key::A,
        Key::S,
        Key::D,
        Key::Plus,
        Key::Minus,
        Key::Shift,
        Key::Ctrl,
        Key::T,
        Key::B,
        Key::C,
        Key::E,
        Key::F,
        Key::G,
        Key::H,
        Key::I,
        Key::J,
        Key::K,
        Key::L,
        Key::M,
        Key::N,
        Key::O,
        Key::P,
        Key::Q,
        Key::U,
        Key::V,
        Key::X,
        Key::Y,
        Key::Z,
        Key::Digit0,
        Key::Digit4,
        Key::Digit5,
        Key::Digit6,
        Key::Digit7,
        Key::Digit8,
        Key::Digit9,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
        Key::ArrowUp,
        Key::ArrowDown,
        Key::ArrowLeft,
        Key::ArrowRight,
        Key::End,
        Key::Insert,
        Key::Alt,
        Key::Super,
        Key::Equal,
        Key::BracketLeft,
        Key::BracketRight,
        Key::Semicolon,
        Key::Quote,
        Key::Comma,
        Key::Period,
        Key::Slash,
        Key::Backslash,
        Key::Backquote,
    ];

    /// The key with this discriminant.
    #[inline]
    pub fn from_index(i: usize) -> Option<Key> {
        Key::ALL.get(i).copied()
    }
}

/// A set of [`Key`]s: 128 bits, one per key ([`InputState::keys`]).
///
/// Besides the methods it has the set operators (`&`, `|`, `^`, `!`) and
/// compares with a `u32` mask of the first 32 key bits, so code written
/// when `keys` was a `u32` (`now.keys & !prev.keys != 0`, `keys == 0`)
/// keeps compiling and meaning the same.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct KeySet([u64; 2]);

const _: () = assert!(Key::COUNT <= 128, "a KeySet holds 128 keys");

impl KeySet {
    /// No key.
    pub const EMPTY: KeySet = KeySet([0; 2]);
    /// Every [`Key`].
    pub const ALL: KeySet = {
        let n = Key::COUNT;
        let lo = if n >= 64 { u64::MAX } else { (1u64 << n) - 1 };
        let hi = if n <= 64 {
            0
        } else if n >= 128 {
            u64::MAX
        } else {
            (1u64 << (n - 64)) - 1
        };
        KeySet([lo, hi])
    };

    /// The set as 16 little-endian bytes (a recording's field,
    /// [`crate::replay`]).
    pub fn to_bytes(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&self.0[0].to_le_bytes());
        b[8..].copy_from_slice(&self.0[1].to_le_bytes());
        b
    }

    /// [`to_bytes`](Self::to_bytes) back (bits past [`Key::COUNT`] are
    /// dropped).
    pub fn from_bytes(b: [u8; 16]) -> KeySet {
        let mut lo = [0u8; 8];
        let mut hi = [0u8; 8];
        lo.copy_from_slice(&b[..8]);
        hi.copy_from_slice(&b[8..]);
        let all = KeySet::ALL;
        KeySet([
            u64::from_le_bytes(lo) & all.0[0],
            u64::from_le_bytes(hi) & all.0[1],
        ])
    }

    #[inline]
    fn bit(key: Key) -> (usize, u64) {
        let i = key as usize;
        (i >> 6, 1u64 << (i & 63))
    }

    #[inline]
    pub fn contains(&self, key: Key) -> bool {
        let (w, b) = Self::bit(key);
        self.0[w] & b != 0
    }

    #[inline]
    pub fn insert(&mut self, key: Key) {
        let (w, b) = Self::bit(key);
        self.0[w] |= b;
    }

    #[inline]
    pub fn remove(&mut self, key: Key) {
        let (w, b) = Self::bit(key);
        self.0[w] &= !b;
    }

    /// Insert (`down`) or remove `key`.
    #[inline]
    pub fn set(&mut self, key: Key, down: bool) {
        if down {
            self.insert(key)
        } else {
            self.remove(key)
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0 == [0, 0]
    }

    /// How many keys are in the set.
    #[inline]
    pub fn len(&self) -> usize {
        (self.0[0].count_ones() + self.0[1].count_ones()) as usize
    }

    /// The keys in the set, in discriminant order.
    pub fn iter(&self) -> impl Iterator<Item = Key> + '_ {
        Key::ALL.into_iter().filter(|k| self.contains(*k))
    }

    /// The raw bits (bit `k as usize` = key `k`): word 0 holds keys 0..64.
    #[inline]
    pub const fn bits(&self) -> [u64; 2] {
        self.0
    }

    /// From raw bits; bits past the last [`Key`] are dropped.
    #[inline]
    pub const fn from_bits(bits: [u64; 2]) -> KeySet {
        KeySet([bits[0] & KeySet::ALL.0[0], bits[1] & KeySet::ALL.0[1]])
    }
}

impl From<u32> for KeySet {
    /// The keys of a `u32` mask (the layout `keys` had before it grew).
    fn from(mask: u32) -> KeySet {
        KeySet::from_bits([mask as u64, 0])
    }
}

impl FromIterator<Key> for KeySet {
    fn from_iter<I: IntoIterator<Item = Key>>(iter: I) -> KeySet {
        let mut s = KeySet::EMPTY;
        for k in iter {
            s.insert(k);
        }
        s
    }
}

impl PartialEq<u32> for KeySet {
    /// Equal to a `u32` mask of the first 32 keys: `keys == 0` is "no key".
    fn eq(&self, mask: &u32) -> bool {
        *self == KeySet::from(*mask)
    }
}

impl core::ops::BitAnd for KeySet {
    type Output = KeySet;
    #[inline]
    fn bitand(self, o: KeySet) -> KeySet {
        KeySet([self.0[0] & o.0[0], self.0[1] & o.0[1]])
    }
}

impl core::ops::BitOr for KeySet {
    type Output = KeySet;
    #[inline]
    fn bitor(self, o: KeySet) -> KeySet {
        KeySet([self.0[0] | o.0[0], self.0[1] | o.0[1]])
    }
}

impl core::ops::BitXor for KeySet {
    type Output = KeySet;
    #[inline]
    fn bitxor(self, o: KeySet) -> KeySet {
        KeySet([self.0[0] ^ o.0[0], self.0[1] ^ o.0[1]])
    }
}

impl core::ops::Not for KeySet {
    type Output = KeySet;
    /// Every other [`Key`] (never bits past the last key).
    #[inline]
    fn not(self) -> KeySet {
        KeySet([!self.0[0] & KeySet::ALL.0[0], !self.0[1] & KeySet::ALL.0[1]])
    }
}

impl core::ops::BitAndAssign for KeySet {
    #[inline]
    fn bitand_assign(&mut self, o: KeySet) {
        *self = *self & o;
    }
}

impl core::ops::BitOrAssign for KeySet {
    #[inline]
    fn bitor_assign(&mut self, o: KeySet) {
        *self = *self | o;
    }
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
    buttons: u32,
    pointer_buttons: u8,
    /// Analog stick, each axis in -1.0..=1.0. Y-up positive.
    pub stick: Vec2,
    /// Second stick, each axis in -1.0..=1.0, Y-up positive like `stick`:
    /// a gamepad's right stick (PC), the New 3DS's C-stick, the N64's four
    /// C buttons as digital directions (C-right = +x, C-up = +y). Zero
    /// where there is none.
    pub right_stick: Vec2,
    /// Analog triggers, 0.0..=1.0 each: `x` = left (L2), `y` = right (R2).
    /// Digital triggers (the New 3DS's ZL/ZR, keys bound to L2/R2) read 0
    /// or 1. Zero on the N64.
    pub triggers: Vec2,
    /// Pointer position in **game screen pixels** (the internal resolution
    /// the game was created with: X-right, Y-down, origin top-left).
    /// `None` when the platform has no pointer, the mouse is outside the
    /// picture, or (3DS) nothing touches the screen.
    pub pointer: Option<Vec2>,
    /// Scroll wheel notches **this frame** (positive = away from the user).
    /// Always 0 without a wheel.
    pub wheel: f32,
    /// PC keyboard keys held. Empty on consoles. Read it through
    /// [`key`](Self::key) / [`key_pressed`](Self::key_pressed).
    pub keys: KeySet,
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
    /// Nothing pressed, no pointer (`Default`, in const contexts).
    pub const IDLE: InputState = InputState {
        buttons: 0,
        pointer_buttons: 0,
        stick: Vec2::ZERO,
        right_stick: Vec2::ZERO,
        triggers: Vec2::ZERO,
        pointer: None,
        wheel: 0.0,
        keys: KeySet::EMPTY,
        pointer_screen: 0,
        pad: false,
    };

    #[inline]
    pub fn is_down(&self, button: Button) -> bool {
        self.buttons & (1 << button as u32) != 0
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
        let bit = 1 << button as u32;
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

    /// Any button held.
    #[inline]
    pub fn any_button(&self) -> bool {
        self.buttons != 0
    }

    /// PC keyboard key held.
    #[inline]
    pub fn key(&self, key: Key) -> bool {
        self.keys.contains(key)
    }

    /// PC keyboard key pressed this frame (down now, up in `prev`).
    #[inline]
    pub fn key_pressed(&self, prev: &InputState, key: Key) -> bool {
        self.key(key) && !prev.key(key)
    }

    /// PC keyboard key released this frame (up now, down in `prev`).
    #[inline]
    pub fn key_released(&self, prev: &InputState, key: Key) -> bool {
        !self.key(key) && prev.key(key)
    }

    /// Backends call this while polling the keyboard.
    #[inline]
    pub fn set_key(&mut self, key: Key, down: bool) {
        self.keys.set(key, down);
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

    /// Every held button as its bit mask (bit `b as u32` for each
    /// [`Button`]): what an input recording stores ([`crate::replay`]).
    #[inline]
    pub const fn button_bits(&self) -> u32 {
        self.buttons
    }

    /// Replace every held button at once (a decoded recording).
    #[inline]
    pub fn set_button_bits(&mut self, bits: u32) {
        self.buttons = bits;
    }

    /// The held pointer buttons as a bit mask (bit `b as u8`).
    #[inline]
    pub const fn pointer_bits(&self) -> u8 {
        self.pointer_buttons
    }

    /// Replace the held pointer buttons at once.
    #[inline]
    pub fn set_pointer_bits(&mut self, bits: u8) {
        self.pointer_buttons = bits;
    }

    /// Fold another device's input into this one, as one player holding
    /// both (a keyboard and a pad on the same seat, [`crate::players`]):
    /// buttons, keys and pointer buttons are OR'ed, the stronger of each
    /// stick wins (a tie keeps this one's), the triggers take the larger
    /// value, the wheel adds up, the pointer is this one's unless it has
    /// none, and `pad` is set when either used a pad.
    pub fn merge(&mut self, other: &InputState) {
        let len2 = |v: Vec2| v.x * v.x + v.y * v.y;
        self.buttons |= other.buttons;
        self.pointer_buttons |= other.pointer_buttons;
        if len2(other.stick) > len2(self.stick) {
            self.stick = other.stick;
        }
        if len2(other.right_stick) > len2(self.right_stick) {
            self.right_stick = other.right_stick;
        }
        self.triggers = Vec2::new(
            self.triggers.x.max(other.triggers.x),
            self.triggers.y.max(other.triggers.y),
        );
        if self.pointer.is_none() && other.pointer.is_some() {
            self.pointer = other.pointer;
            self.pointer_screen = other.pointer_screen;
        }
        self.wheel += other.wheel;
        self.keys |= other.keys;
        self.pad |= other.pad;
    }
}

/// What every platform backend implements: poll the controller once per
/// frame and hand the snapshot to the game.
pub trait Input {
    fn poll(&mut self) -> InputState;

    /// Every local player's input this frame ([`crate::players`], issue
    /// #67): the N64's four controller ports, the PC's and the Web's
    /// keyboard and pads by seat. Player 0 is what [`poll`](Self::poll)
    /// returns. The default — a backend with one controller — is that one
    /// player, connected. Call this *or* `poll` once per frame, not both.
    fn poll_players(&mut self) -> crate::PlayersInput {
        crate::PlayersInput::single(self.poll())
    }
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
        now.set_key(Key::Z, true);
        now.set_key(Key::Backquote, true);
        assert!(now.key(Key::Z) && now.key(Key::Backquote) && !now.key(Key::Y));
        now.set_key(Key::Escape, false);
        assert!(!now.key(Key::Escape));
    }

    #[test]
    fn every_key_is_its_own_bit() {
        assert_eq!(Key::COUNT, 79);
        let mut all = KeySet::EMPTY;
        for (i, k) in Key::ALL.iter().enumerate() {
            assert_eq!(*k as usize, i, "Key::ALL is in discriminant order");
            assert_eq!(Key::from_index(i), Some(*k));
            let mut one = KeySet::EMPTY;
            one.insert(*k);
            assert_eq!(one.len(), 1);
            assert_eq!(one.iter().collect::<Vec<_>>(), [*k]);
            assert!(!all.contains(*k), "{k:?} shares a bit");
            all.insert(*k);
        }
        assert_eq!(all, KeySet::ALL);
        assert_eq!(all.len(), Key::COUNT);
        assert_eq!(Key::from_index(Key::COUNT), None);
        // Every key round-trips through a whole InputState, alone.
        for k in Key::ALL {
            let mut s = InputState::default();
            s.set_key(k, true);
            assert!(Key::ALL.iter().all(|o| s.key(*o) == (*o == k)), "{k:?}");
            assert!(!s.any_button(), "keys never alias buttons");
            s.set_key(k, false);
            assert!(s.keys.is_empty());
        }
    }

    #[test]
    fn the_first_keys_keep_their_old_bits() {
        // `keys` was a u32 with these bit positions; masks written then
        // still name the same keys.
        let old = [
            (Key::Escape, 0),
            (Key::Enter, 1),
            (Key::Space, 2),
            (Key::F1, 6),
            (Key::F8, 13),
            (Key::Digit1, 14),
            (Key::Home, 19),
            (Key::R, 20),
            (Key::D, 24),
            (Key::Plus, 25),
            (Key::Minus, 26),
            (Key::Shift, 27),
            (Key::Ctrl, 28),
            (Key::T, 29),
        ];
        for (k, bit) in old {
            assert_eq!(k as u32, bit, "{k:?}");
            let mut s = KeySet::EMPTY;
            s.insert(k);
            assert_eq!(s, 1u32 << bit);
            assert_eq!(KeySet::from(1u32 << bit), s);
        }
    }

    #[test]
    fn key_set_operators_keep_u32_era_code_working() {
        let mut prev = InputState::default();
        let mut now = InputState::default();
        // Expressions existing projects wrote against a `u32`.
        assert!(now.keys == 0);
        now.set_key(Key::Backquote, true);
        assert!(now.keys != 0, "a key past bit 64 is still 'some key'");
        let keys_used = now.keys & !prev.keys != 0;
        assert!(keys_used);
        prev.set_key(Key::Backquote, true);
        assert!(now.keys & !prev.keys == 0, "held, not new");
        assert!(!now.key_released(&InputState::default(), Key::Backquote));
        assert!(InputState::default().key_released(&now, Key::Backquote));
        // Set algebra.
        let a: KeySet = [Key::A, Key::Z, Key::F12].into_iter().collect();
        let b: KeySet = [Key::Z, Key::Comma].into_iter().collect();
        assert_eq!((a & b).iter().collect::<Vec<_>>(), [Key::Z]);
        assert_eq!((a | b).len(), 4);
        assert_eq!((a ^ b).len(), 3);
        assert_eq!(!KeySet::EMPTY, KeySet::ALL);
        assert_eq!(!KeySet::ALL, KeySet::EMPTY);
        assert_eq!(!!a, a);
        let mut c = a;
        c &= b;
        c |= KeySet::from(1);
        assert_eq!(c.iter().collect::<Vec<_>>(), [Key::Escape, Key::Z]);
        let mut d = a;
        d.remove(Key::A);
        d.set(Key::F12, false);
        d.set(Key::Super, true);
        assert_eq!(d.iter().collect::<Vec<_>>(), [Key::Z, Key::Super]);
        assert_eq!(KeySet::from_bits([u64::MAX; 2]), KeySet::ALL);
        assert_eq!(KeySet::from_bits(a.bits()), a);
    }

    #[test]
    fn new_buttons_sticks_and_triggers() {
        assert_eq!(Button::ALL.len(), 16);
        for (i, b) in Button::ALL.iter().enumerate() {
            assert_eq!(*b as usize, i);
            let mut s = InputState::default();
            s.set(*b, true);
            assert!(s.any_button());
            assert!(
                Button::ALL.iter().all(|o| s.is_down(*o) == (o == b)),
                "{b:?}"
            );
            assert!(s.keys.is_empty(), "buttons never alias keys");
        }
        let prev = InputState::default();
        let mut now = InputState::default();
        assert_eq!((now.right_stick, now.triggers), (Vec2::ZERO, Vec2::ZERO));
        now.set(Button::L2, true);
        now.set(Button::R3, true);
        now.right_stick = Vec2::new(-1.0, 0.5);
        now.triggers = Vec2::new(1.0, 0.25);
        assert!(now.just_pressed(&prev, Button::L2) && now.just_pressed(&prev, Button::R3));
        assert!(!now.is_down(Button::R2) && !now.is_down(Button::L3));
        assert!(prev.just_released(&now, Button::L2));
        assert_ne!(now, prev, "the new fields take part in equality");
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
