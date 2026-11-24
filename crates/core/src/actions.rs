//! A game's own actions bound to keys and pad buttons, rebindable by the
//! player on every target (Bongo-Kart #183, ADR 0045 §3).
//!
//! [`Platform::rebind_player`](crate::Platform::rebind_player) remaps the
//! console buttons a key or a pad button presses, on the PC. A game that reads
//! its controls as actions ("accelerate", "drift", "use item") — several
//! keys each, and keys no console button has — keeps an [`ActionMap`]
//! instead: the defaults are the game's, the player's changes are the map's,
//! [`InputState`] is the only input, so the same remapping screen works on the
//! PC, in a browser and with a console's pad, with no backend code.
//!
//! - **Reading:** [`ActionMap::down`], [`pressed`](ActionMap::pressed),
//!   [`released`](ActionMap::released).
//! - **Remapping:** [`pressed_key`] / [`pressed_button`] catch the next key
//!   or button the player presses (skipping the ones the game reserves, such
//!   as Escape for "cancel"); [`ActionMap::rebind_key`] /
//!   [`rebind_button`](ActionMap::rebind_button) put it in a slot. A key
//!   belongs to one action: taking it from another action gives that action
//!   this slot's old key (a swap), so nothing is left unbound by accident.
//! - **Saving:** [`ActionMap::write_save`] / [`read_save`](ActionMap::read_save),
//!   `2 + 4 × actions` bytes for the game's save (a damaged or foreign block
//!   is refused and the map keeps what it had).
//!
//! The map's methods are thin over slice functions, so every map size shares
//! one copy of the code (the `Ui<N>` lesson on the N64).

use crate::input::{Button, InputState, Key};

/// Keys an action can have.
pub const ACTION_KEYS: usize = 2;
/// Pad buttons an action can have.
pub const ACTION_BUTTONS: usize = 2;
/// The save block's version byte.
pub const ACTIONS_SAVE_VERSION: u8 = 1;
/// A save block's byte for "no key / no button".
const UNBOUND: u8 = 0xff;

/// What triggers one action: up to [`ACTION_KEYS`] keys and
/// [`ACTION_BUTTONS`] pad buttons.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ActionBinding {
    pub keys: [Option<Key>; ACTION_KEYS],
    pub buttons: [Option<Button>; ACTION_BUTTONS],
}

impl ActionBinding {
    pub const NONE: ActionBinding = ActionBinding {
        keys: [None; ACTION_KEYS],
        buttons: [None; ACTION_BUTTONS],
    };

    /// One key.
    pub const fn key(key: Key) -> ActionBinding {
        ActionBinding::NONE.or_key(key)
    }

    /// One pad button.
    pub const fn button(button: Button) -> ActionBinding {
        ActionBinding::NONE.or_button(button)
    }

    /// The same, `key` in the first free key slot (ignored when both are
    /// taken).
    pub const fn or_key(mut self, key: Key) -> ActionBinding {
        if self.keys[0].is_none() {
            self.keys[0] = Some(key);
        } else if self.keys[1].is_none() {
            self.keys[1] = Some(key);
        }
        self
    }

    /// The same, `button` in the first free button slot.
    pub const fn or_button(mut self, button: Button) -> ActionBinding {
        if self.buttons[0].is_none() {
            self.buttons[0] = Some(button);
        } else if self.buttons[1].is_none() {
            self.buttons[1] = Some(button);
        }
        self
    }

    /// Any of its keys or buttons held in `input`. On the PC and the Web a
    /// bound key also presses its console button (Z presses A): a game that
    /// tells the keyboard from a pad reads [`keys_down`](Self::keys_down) and
    /// [`buttons_down`](Self::buttons_down) by [`InputState::pad`].
    pub fn down(&self, input: &InputState) -> bool {
        self.keys_down(input) || self.buttons_down(input)
    }

    /// Any of its keys held.
    pub fn keys_down(&self, input: &InputState) -> bool {
        self.keys.iter().flatten().any(|k| input.key(*k))
    }

    /// Any of its pad buttons held.
    pub fn buttons_down(&self, input: &InputState) -> bool {
        self.buttons.iter().flatten().any(|b| input.is_down(*b))
    }
}

/// A game's actions (an index each: the game's own enum `as usize`), their
/// defaults and the player's bindings.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ActionMap<const N: usize> {
    defaults: [ActionBinding; N],
    binds: [ActionBinding; N],
}

impl<const N: usize> ActionMap<N> {
    /// The map at its defaults.
    pub const fn new(defaults: [ActionBinding; N]) -> ActionMap<N> {
        ActionMap {
            defaults,
            binds: defaults,
        }
    }

    /// Every action's binding now.
    pub fn bindings(&self) -> &[ActionBinding] {
        &self.binds
    }

    /// Action `action`'s binding ([`ActionBinding::NONE`] past the end).
    pub fn binding(&self, action: usize) -> ActionBinding {
        self.binds
            .get(action)
            .copied()
            .unwrap_or(ActionBinding::NONE)
    }

    /// Action `action` is held.
    pub fn down(&self, action: usize, input: &InputState) -> bool {
        self.binding(action).down(input)
    }

    /// Action `action` went down between `prev` and `now`.
    pub fn pressed(&self, action: usize, now: &InputState, prev: &InputState) -> bool {
        let b = self.binding(action);
        b.down(now) && !b.down(prev)
    }

    /// Action `action` went up between `prev` and `now`.
    pub fn released(&self, action: usize, now: &InputState, prev: &InputState) -> bool {
        let b = self.binding(action);
        !b.down(now) && b.down(prev)
    }

    /// `key` in action `action`'s key slot `slot`. Another action that had
    /// it gets this slot's old key instead (a swap) and is returned; the
    /// same action's other slot just lets it go. Out-of-range indexes change
    /// nothing.
    pub fn rebind_key(&mut self, action: usize, slot: usize, key: Key) -> Option<usize> {
        rebind_key(&mut self.binds, action, slot, key)
    }

    /// `button` in action `action`'s button slot `slot`, the same way as
    /// [`rebind_key`](Self::rebind_key).
    pub fn rebind_button(&mut self, action: usize, slot: usize, button: Button) -> Option<usize> {
        rebind_button(&mut self.binds, action, slot, button)
    }

    /// Action `action`'s key slot `slot` unbound.
    pub fn clear_key(&mut self, action: usize, slot: usize) {
        if let Some(k) = self
            .binds
            .get_mut(action)
            .and_then(|b| b.keys.get_mut(slot))
        {
            *k = None;
        }
    }

    /// Action `action`'s button slot `slot` unbound.
    pub fn clear_button(&mut self, action: usize, slot: usize) {
        if let Some(b) = self
            .binds
            .get_mut(action)
            .and_then(|b| b.buttons.get_mut(slot))
        {
            *b = None;
        }
    }

    /// Every action back to the game's defaults.
    pub fn reset(&mut self) {
        self.binds = self.defaults;
    }

    /// The player changed nothing.
    pub fn is_default(&self) -> bool {
        self.binds == self.defaults
    }

    /// Bytes [`write_save`](Self::write_save) writes.
    pub const fn save_len() -> usize {
        2 + 4 * N
    }

    /// The bindings into `out` (at least [`save_len`](Self::save_len)
    /// bytes): the version, the action count, then each action's two keys
    /// and two buttons. `None` when `out` is short or the map has more than
    /// 255 actions.
    pub fn write_save(&self, out: &mut [u8]) -> Option<usize> {
        write_save(&self.binds, out)
    }

    /// Bindings from [`write_save`](Self::write_save)'s bytes. `false` (the
    /// map unchanged) for another version, another action count or a byte
    /// that names no key or button.
    pub fn read_save(&mut self, bytes: &[u8]) -> bool {
        read_save(&mut self.binds, bytes)
    }
}

fn rebind_key(binds: &mut [ActionBinding], action: usize, slot: usize, key: Key) -> Option<usize> {
    if action >= binds.len() || slot >= ACTION_KEYS {
        return None;
    }
    let old = binds[action].keys[slot];
    let mut other = None;
    for (i, b) in binds.iter_mut().enumerate() {
        for (j, k) in b.keys.iter_mut().enumerate() {
            if *k == Some(key) && (i, j) != (action, slot) {
                if i == action {
                    *k = None;
                } else {
                    *k = old;
                    other = Some(i);
                }
            }
        }
    }
    binds[action].keys[slot] = Some(key);
    other
}

fn rebind_button(
    binds: &mut [ActionBinding],
    action: usize,
    slot: usize,
    button: Button,
) -> Option<usize> {
    if action >= binds.len() || slot >= ACTION_BUTTONS {
        return None;
    }
    let old = binds[action].buttons[slot];
    let mut other = None;
    for (i, b) in binds.iter_mut().enumerate() {
        for (j, p) in b.buttons.iter_mut().enumerate() {
            if *p == Some(button) && (i, j) != (action, slot) {
                if i == action {
                    *p = None;
                } else {
                    *p = old;
                    other = Some(i);
                }
            }
        }
    }
    binds[action].buttons[slot] = Some(button);
    other
}

fn write_save(binds: &[ActionBinding], out: &mut [u8]) -> Option<usize> {
    let len = 2 + 4 * binds.len();
    if binds.len() > 255 || out.len() < len {
        return None;
    }
    out[0] = ACTIONS_SAVE_VERSION;
    out[1] = binds.len() as u8;
    for (b, o) in binds
        .iter()
        .zip(out[2..len].as_chunks_mut::<4>().0.iter_mut())
    {
        let k = |k: Option<Key>| k.map_or(UNBOUND, |k| k as u8);
        let p = |p: Option<Button>| p.map_or(UNBOUND, |p| p as u8);
        o.copy_from_slice(&[k(b.keys[0]), k(b.keys[1]), p(b.buttons[0]), p(b.buttons[1])]);
    }
    Some(len)
}

fn read_save(binds: &mut [ActionBinding], bytes: &[u8]) -> bool {
    let len = 2 + 4 * binds.len();
    if bytes.len() < len || bytes[0] != ACTIONS_SAVE_VERSION || bytes[1] as usize != binds.len() {
        return false;
    }
    let key = |v: u8| match v {
        UNBOUND => Some(None),
        v => Key::from_index(v as usize).map(Some),
    };
    let button = |v: u8| match v {
        UNBOUND => Some(None),
        v => Button::ALL.get(v as usize).copied().map(Some),
    };
    // Validate everything before touching the map.
    for c in bytes[2..len].as_chunks::<4>().0 {
        if key(c[0]).is_none()
            || key(c[1]).is_none()
            || button(c[2]).is_none()
            || button(c[3]).is_none()
        {
            return false;
        }
    }
    for (b, c) in binds.iter_mut().zip(bytes[2..len].as_chunks::<4>().0) {
        *b = ActionBinding {
            keys: [key(c[0]).flatten(), key(c[1]).flatten()],
            buttons: [button(c[2]).flatten(), button(c[3]).flatten()],
        };
    }
    true
}

/// The first key (in [`Key::ALL`] order) down in `now` and not in `prev`,
/// leaving out `reserved` (the game's "cancel" key, keys with fixed roles):
/// what a remapping screen binds.
pub fn pressed_key(now: &InputState, prev: &InputState, reserved: &[Key]) -> Option<Key> {
    Key::ALL
        .iter()
        .copied()
        .find(|k| now.key(*k) && !prev.key(*k) && !reserved.contains(k))
}

/// The first pad button down in `now` and not in `prev`, leaving out
/// `reserved`.
pub fn pressed_button(now: &InputState, prev: &InputState, reserved: &[Button]) -> Option<Button> {
    Button::ALL
        .iter()
        .copied()
        .find(|b| now.is_down(*b) && !prev.is_down(*b) && !reserved.contains(b))
}
