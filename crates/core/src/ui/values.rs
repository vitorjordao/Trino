//! The values a screen shows and edits, by key: what `bind`, `visible_if`
//! and `enabled_if` read. The game writes them (`ui.set("money", 1200)`),
//! toggles and sliders write them back. Fixed capacity, no allocation.
//!
//! A game with more to show than [`MAX_VALUES`](super::MAX_VALUES) (a
//! squad, a league table, a market) answers the screen's keys itself
//! instead: a [`UiData`] passed with
//! [`UiEnv::with_data`](super::UiEnv::with_data) (issue #95). A value the
//! [`Ui`](super::Ui) holds wins over the game's.

use crate::asset::asset_id;
use crate::text::TextBuf;

/// Longest text value, bytes (64 since issue #95: a game's own texts, in
/// Portuguese with two-byte accents, outgrew 32).
pub const TEXT_LEN: usize = 64;

/// One value.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum UiValue {
    Int(i32),
    Bool(bool),
    Text(TextBuf<TEXT_LEN>),
}

impl UiValue {
    /// As an int (a bool is 0 or 1; a text has none).
    pub fn int(&self) -> Option<i32> {
        match self {
            UiValue::Int(v) => Some(*v),
            UiValue::Bool(b) => Some(i32::from(*b)),
            UiValue::Text(_) => None,
        }
    }

    /// As a bool (an int is true when non-zero; a text when non-empty).
    pub fn truthy(&self) -> bool {
        match self {
            UiValue::Int(v) => *v != 0,
            UiValue::Bool(b) => *b,
            UiValue::Text(t) => !t.as_str().is_empty(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Entry {
    hash: u32,
    /// The list item it belongs to (`NO_ITEM`: the screen).
    item: u16,
    value: UiValue,
}

/// "Not in a list."
pub const NO_ITEM: u16 = u16::MAX;

/// `V` values by key (and list item).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct UiValues<const V: usize> {
    entries: [Option<Entry>; V],
    revision: u32,
    /// Values a full store refused.
    pub dropped: u32,
}

impl<const V: usize> Default for UiValues<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const V: usize> UiValues<V> {
    pub const fn new() -> Self {
        UiValues {
            entries: [None; V],
            revision: 0,
            dropped: 0,
        }
    }

    /// Moves on every change (the layout depends on values).
    pub fn revision(&self) -> u32 {
        self.revision
    }

    fn slot(&self, hash: u32, item: u16) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| matches!(e, Some(e) if e.hash == hash && e.item == item))
    }

    /// Set `key` (of list item `item`, or [`NO_ITEM`]); `false` when the
    /// store is full.
    pub fn put(&mut self, key: &str, item: u16, value: UiValue) -> bool {
        let hash = asset_id(key);
        let at = match self.slot(hash, item) {
            Some(i) => i,
            None => match self.entries.iter().position(|e| e.is_none()) {
                Some(i) => i,
                None => {
                    self.dropped += 1;
                    return false;
                }
            },
        };
        let entry = Some(Entry { hash, item, value });
        if self.entries[at] != entry {
            self.entries[at] = entry;
            self.revision = self.revision.wrapping_add(1);
        }
        true
    }

    pub fn get(&self, key: &str, item: u16) -> Option<UiValue> {
        self.slot(asset_id(key), item)
            .and_then(|i| self.entries[i])
            .map(|e| e.value)
            .or_else(|| {
                // A list item's key falls back to the screen's value.
                (item != NO_ITEM).then(|| self.get(key, NO_ITEM)).flatten()
            })
    }

    pub fn remove(&mut self, key: &str, item: u16) {
        if let Some(i) = self.slot(asset_id(key), item) {
            self.entries[i] = None;
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn clear(&mut self) {
        self.entries = [None; V];
        self.revision = self.revision.wrapping_add(1);
    }

    /// How many values are set.
    pub fn len(&self) -> usize {
        self.entries.iter().filter(|e| e.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Values a game gives a screen when it asks for them, by key and list
/// item ([`NO_ITEM`] outside a list): nothing stored, no capacity.
pub trait UiData {
    fn value(&self, key: &str, item: u16) -> Option<UiValue>;

    /// `Some`: changes whenever any value does, and the screen lays out
    /// again only then. `None` (the default): the screen lays out on every
    /// draw, so a value can never go stale (a few float operations per
    /// node: menus afford it on every target).
    fn revision(&self) -> Option<u32> {
        None
    }
}

/// No game data: only the [`Ui`](super::Ui)'s own values.
pub struct NoData;

impl UiData for NoData {
    fn value(&self, _key: &str, _item: u16) -> Option<UiValue> {
        None
    }

    fn revision(&self) -> Option<u32> {
        Some(0)
    }
}

/// `key`'s value for list item `item`: the store's (its item, else its
/// screen value), else the game's (the item, else the screen's).
pub(crate) fn lookup<const V: usize>(
    values: &UiValues<V>,
    data: &dyn UiData,
    key: &str,
    item: u16,
) -> Option<UiValue> {
    values.get(key, item).or_else(|| {
        data.value(key, item).or_else(|| {
            (item != NO_ITEM)
                .then(|| data.value(key, NO_ITEM))
                .flatten()
        })
    })
}

/// A text value from `s` (cut to [`TEXT_LEN`] bytes on a character).
pub fn text_value(s: &str) -> UiValue {
    let mut b = TextBuf::new();
    for ch in s.chars() {
        if core::fmt::Write::write_char(&mut b, ch).is_err() {
            break;
        }
    }
    UiValue::Text(b)
}
