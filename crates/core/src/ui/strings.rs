//! Text by key: what a node's `key` resolves through.
//!
//! [`UiStrings`] is the lookup a screen is drawn with. Games with their own
//! tables implement it; the engine's own is [`StringTable`], a baked
//! `TSTR` blob made from `assets/i18n/<lang>.ron` (the plain-string part of
//! the localization format: [`crate::i18n`], issue #59, whose
//! [`Localizer`](crate::i18n::Localizer) is a `UiStrings` too). The format
//! is documented in [`crate::i18n::table`].

use crate::asset::asset_id;
use crate::render::FontId;

/// A key → text lookup (the current language).
pub trait UiStrings {
    fn get(&self, key: &str) -> Option<&str>;

    /// Changes whenever the texts do (a language switch): the UI lays out
    /// again when it moves.
    fn revision(&self) -> u32 {
        0
    }

    /// The font this language draws `font` with (a per-language atlas or
    /// override, issue #59); the UI measures and draws through it.
    fn font(&self, font: FontId) -> FontId {
        font
    }
}

/// No table: every node shows its own `text`.
pub struct NoStrings;

impl UiStrings for NoStrings {
    fn get(&self, _key: &str) -> Option<&str> {
        None
    }
}

pub use crate::i18n::table::{StringTable, StringsError, TSTR_MAGIC, TSTR_VERSION, encode_tstr};

impl UiStrings for StringTable<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.lookup(key)
    }

    fn revision(&self) -> u32 {
        asset_id(self.lang())
    }

    fn font(&self, font: FontId) -> FontId {
        StringTable::font(self, font)
    }
}

/// A language and the one it falls back to.
#[derive(Clone, Copy, Debug)]
pub struct Fallback<'a> {
    pub first: StringTable<'a>,
    pub then: Option<StringTable<'a>>,
}

impl UiStrings for Fallback<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.first
            .lookup(key)
            .or_else(|| self.then.and_then(|t| t.lookup(key)))
    }

    fn revision(&self) -> u32 {
        self.first.revision()
    }

    fn font(&self, font: FontId) -> FontId {
        self.first.font(font)
    }
}
