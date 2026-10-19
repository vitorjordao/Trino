//! Localization (issue #59, ADR 0026): string tables with fallback, plural
//! forms and locale formatting, the same on every target.
//!
//! - **Data:** `assets/i18n/<lang>.ron`, one file per language, baked by
//!   every platform's asset bake into a [`StringTable`] blob at
//!   `i18n/<lang>` (`TSTR`, see [`table`]), plus `i18n/languages` (the tags,
//!   the default first). No manifest entry needed.
//! - **Runtime:** a [`Localizer`] holds the tables (`'static` blobs, or the
//!   static code `cargo xtask i18n export --rust` writes) and the current
//!   language. [`Localizer::tr`] looks a key up through the fallback chain,
//!   [`Localizer::tr_fmt`] fills placeholders and picks plural forms into a
//!   [`TextBuf`](crate::TextBuf): no allocation, integers only, so the
//!   output is the same on the N64, the 3DS, the PC and the Web.
//! - **Fonts:** a language can draw a font with another atlas (its own
//!   glyph subset, or a font with its script): [`Localizer::font`]. The UI
//!   (#58) applies it through [`UiStrings::font`](crate::ui::UiStrings).
//! - **System language:** [`Platform::system_language`](crate::Platform)
//!   (PC: the OS locale; Web: `navigator.language`; 3DS: the system
//!   setting; N64: none). [`Localizer::pick`] chooses saved > system >
//!   default.

pub mod format;
pub mod locale;
pub mod table;

pub use format::{Arg, Date, Value};
pub use locale::{DateOrder, Locale, PluralForm, PluralRule};
pub use table::{Entry, PluralForms, StringTable, StringsError};

use crate::asset::{asset_id, asset_id_prefixed};
use crate::platform::{BlobId, Platform};
use crate::render::FontId;
use crate::text::TextBuf;

/// A string key: [`asset_id`] of `"menu.play"`. `const`, so games keep keys
/// as constants.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct StrKey(pub u32);

impl StrKey {
    pub const fn new(key: &str) -> StrKey {
        StrKey(asset_id(key))
    }
}

/// A language: [`asset_id`] of its tag (`"pt-BR"`). Saves store it (4 bytes).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct LangId(pub u32);

impl LangId {
    pub const fn new(tag: &str) -> LangId {
        LangId(asset_id(tag))
    }

    /// The blob a language bakes into (`i18n/pt-BR`).
    pub const fn blob(tag: &str) -> BlobId {
        BlobId(asset_id_prefixed("i18n/", tag))
    }
}

/// The blob that lists the baked languages, one tag per line, the default
/// first.
pub const LANGUAGES_BLOB: BlobId = BlobId(asset_id("i18n/languages"));

/// A language tag without an allocator (BCP 47, at most 15 bytes): what a
/// platform reports as the system language.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LangTag {
    bytes: [u8; 15],
    len: u8,
}

impl LangTag {
    /// A tag from a system string: `pt_BR.UTF-8` and `pt-BR@euro` become
    /// `pt-BR`; `C` and `POSIX` (no language) are `None`.
    pub fn parse(s: &str) -> Option<LangTag> {
        let s = s.trim();
        let end = s.find(['.', '@', ':']).unwrap_or(s.len());
        let s = &s[..end];
        if s.is_empty() || s == "C" || s == "POSIX" || s.len() > 15 {
            return None;
        }
        let mut bytes = [0u8; 15];
        for (i, b) in s.bytes().enumerate() {
            if !(b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
                return None;
            }
            bytes[i] = if b == b'_' { b'-' } else { b };
        }
        Some(LangTag {
            bytes,
            len: s.len() as u8,
        })
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }
}

impl core::fmt::Debug for LangTag {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self.as_str(), f)
    }
}

/// No room for another language in a [`Localizer`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LocalizerFull;

/// The game's languages and the current one (up to `N` tables).
#[derive(Clone, Copy)]
pub struct Localizer<'a, const N: usize = 8> {
    tables: [Option<StringTable<'a>>; N],
    current: usize,
    revision: u32,
}

impl<'a, const N: usize> Default for Localizer<'a, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a, const N: usize> Localizer<'a, N> {
    pub const fn new() -> Self {
        Localizer {
            tables: [None; N],
            current: 0,
            revision: 0,
        }
    }

    /// Tables known up front (static code); the first is current.
    pub const fn from_tables(tables: [Option<StringTable<'a>>; N]) -> Self {
        Localizer {
            tables,
            current: 0,
            revision: 0,
        }
    }

    /// Adds a table, or replaces the one with its language (a live
    /// reload). The first table added is current.
    pub fn add(&mut self, table: StringTable<'a>) -> Result<LangId, LocalizerFull> {
        let id = table.id();
        let slot = self
            .tables
            .iter()
            .position(|t| t.is_some_and(|t| t.id() == id))
            .or_else(|| self.tables.iter().position(Option::is_none))
            .ok_or(LocalizerFull)?;
        self.tables[slot] = Some(table);
        self.revision = self.revision.wrapping_add(1);
        Ok(id)
    }

    /// Every language the platform baked (`i18n/languages`), the default
    /// first and current. Returns how many loaded.
    pub fn load(&mut self, platform: &dyn Platform) -> usize
    where
        'a: 'static,
    {
        let Some(list) = platform
            .blob(LANGUAGES_BLOB)
            .and_then(|b| core::str::from_utf8(b).ok())
        else {
            return 0;
        };
        let mut n = 0;
        for tag in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if let Some(t) = platform
                .blob(BlobId(asset_id_prefixed("i18n/", tag)))
                .and_then(|b| StringTable::parse(b).ok())
                && self.add(t).is_ok()
            {
                n += 1;
            }
        }
        n
    }

    /// The loaded languages, in load order.
    pub fn languages(&self) -> impl Iterator<Item = &StringTable<'a>> {
        self.tables.iter().flatten()
    }

    pub fn table(&self, lang: LangId) -> Option<&StringTable<'a>> {
        self.languages().find(|t| t.id() == lang)
    }

    fn current_table(&self) -> Option<&StringTable<'a>> {
        self.tables.get(self.current).and_then(Option::as_ref)
    }

    /// Switch language; `false` (nothing changes) when it isn't loaded.
    pub fn set_language(&mut self, lang: LangId) -> bool {
        match self
            .tables
            .iter()
            .position(|t| t.is_some_and(|t| t.id() == lang))
        {
            Some(i) => {
                if i != self.current {
                    self.current = i;
                    self.revision = self.revision.wrapping_add(1);
                }
                true
            }
            None => false,
        }
    }

    /// The current language (`LangId(0)` with no table).
    pub fn language(&self) -> LangId {
        self.current_table().map_or(LangId(0), |t| t.id())
    }

    /// The current language's tag (`""` with no table).
    pub fn tag(&self) -> &'a str {
        self.current_table().map_or("", |t| t.lang())
    }

    /// Changes on every language switch or table reload.
    pub fn revision(&self) -> u32 {
        self.revision
    }

    /// The loaded language that best fits `tag`: the same tag, else the
    /// same primary language (`pt` or `pt-PT` → `pt-BR`), ASCII case
    /// ignored.
    pub fn best_match(&self, tag: &str) -> Option<LangId> {
        use locale::{eq_ci, primary};
        self.languages()
            .find(|t| eq_ci(t.lang(), tag))
            .or_else(|| {
                self.languages()
                    .find(|t| eq_ci(primary(t.lang()), primary(tag)))
            })
            .map(|t| t.id())
    }

    /// The language to start in: a saved choice still loaded, else the
    /// system's best match, else the first table (the default). Sets it
    /// and returns it.
    pub fn pick(&mut self, saved: Option<LangId>, system: Option<LangTag>) -> LangId {
        let lang = saved
            .filter(|l| self.table(*l).is_some())
            .or_else(|| system.and_then(|s| self.best_match(s.as_str())))
            .or_else(|| self.languages().next().map(|t| t.id()));
        if let Some(l) = lang {
            self.set_language(l);
        }
        self.language()
    }

    /// The entry for `key` in `lang` or its fallback chain.
    pub fn entry_in(&self, lang: LangId, key: StrKey) -> Option<Entry<'a>> {
        let mut at = self.table(lang).copied();
        // At most N hops: a cycle in the fallbacks ends.
        for _ in 0..N {
            let t = at?;
            if let Some(e) = t.entry(key) {
                return Some(e);
            }
            let next = t.fallback();
            if next.is_empty() {
                return None;
            }
            at = self.table(LangId::new(next)).copied();
        }
        None
    }

    /// The text for `key` in `lang` or its fallback chain.
    pub fn get_in(&self, lang: LangId, key: StrKey) -> Option<&'a str> {
        self.entry_in(lang, key).map(|e| e.text())
    }

    /// The entry for `key` in the current language or its fallback chain.
    pub fn entry(&self, key: StrKey) -> Option<Entry<'a>> {
        self.entry_in(self.language(), key)
    }

    pub fn get(&self, key: StrKey) -> Option<&'a str> {
        self.entry(key).map(|e| e.text())
    }

    /// The text for `key` (empty when no table in the chain has it; `cargo
    /// xtask i18n check` reports such keys).
    pub fn tr(&self, key: StrKey) -> &'a str {
        self.get(key).unwrap_or("")
    }

    /// The plural form of `key` for the count `n`.
    pub fn plural(&self, key: StrKey, n: i64) -> &'a str {
        match self.entry(key) {
            Some(Entry::Plural(p)) => p.pick(self.locale().plural.select(n, 0)),
            Some(Entry::Text(t)) => t,
            None => "",
        }
    }

    /// The current language's locale (en-US's without a table).
    pub fn locale(&self) -> Locale<'a> {
        self.current_table().map_or(Locale::EN_US, |t| t.locale())
    }

    /// `key` with its placeholders filled and its plural form picked by the
    /// count argument (`n` or `count`, else the first number), written into
    /// `out` (cleared first). Text past `out`'s capacity is cut.
    pub fn tr_fmt<'b, const M: usize>(
        &self,
        key: StrKey,
        args: &[Arg<'_>],
        out: &'b mut TextBuf<M>,
    ) -> &'b str {
        out.clear();
        let _ = self.write_fmt(key, args, out);
        out.as_str()
    }

    /// [`tr_fmt`](Self::tr_fmt) into any writer.
    pub fn write_fmt(
        &self,
        key: StrKey,
        args: &[Arg<'_>],
        out: &mut dyn core::fmt::Write,
    ) -> core::fmt::Result {
        self.write_fmt_in(self.language(), key, args, out)
    }

    /// [`tr_fmt`](Self::tr_fmt) in `lang` rather than the current language
    /// (a game that keeps the language itself, or shows two at once).
    pub fn tr_fmt_in<'b, const M: usize>(
        &self,
        lang: LangId,
        key: StrKey,
        args: &[Arg<'_>],
        out: &'b mut TextBuf<M>,
    ) -> &'b str {
        out.clear();
        let _ = self.write_fmt_in(lang, key, args, out);
        out.as_str()
    }

    /// [`write_fmt`](Self::write_fmt) in `lang`.
    pub fn write_fmt_in(
        &self,
        lang: LangId,
        key: StrKey,
        args: &[Arg<'_>],
        out: &mut dyn core::fmt::Write,
    ) -> core::fmt::Result {
        let locale = self.table(lang).map_or(Locale::EN_US, |t| t.locale());
        let pattern = match self.entry_in(lang, key) {
            Some(Entry::Text(t)) => t,
            Some(Entry::Plural(p)) => {
                let form = format::count_arg(args)
                    .and_then(|a| a.value.plural_operands(&locale))
                    .map_or(PluralForm::Other, |(i, v)| locale.plural.select(i, v));
                p.pick(form)
            }
            None => return Ok(()),
        };
        format::format(pattern, args, &locale, out)
    }

    /// A pattern that is not in a table (a game's own text) formatted by
    /// the current locale.
    pub fn format<'b, const M: usize>(
        &self,
        pattern: &str,
        args: &[Arg<'_>],
        out: &'b mut TextBuf<M>,
    ) -> &'b str {
        out.clear();
        let _ = format::format(pattern, args, &self.locale(), out);
        out.as_str()
    }

    /// The font the current language draws `font` with.
    pub fn font(&self, font: FontId) -> FontId {
        self.current_table().map_or(font, |t| t.font(font))
    }

    /// The asset group the current language's fonts are in (`None`: boot);
    /// load it before switching on a console that streams them.
    pub fn group(&self) -> Option<crate::GroupId> {
        self.current_table().and_then(|t| t.group())
    }
}

impl<const N: usize> crate::ui::UiStrings for Localizer<'_, N> {
    fn get(&self, key: &str) -> Option<&str> {
        Localizer::get(self, StrKey::new(key))
    }

    fn revision(&self) -> u32 {
        // Never 0: 0 is "no table" to the UI.
        self.revision.wrapping_add(self.language().0) | 1
    }

    fn font(&self, font: FontId) -> FontId {
        Localizer::font(self, font)
    }
}
