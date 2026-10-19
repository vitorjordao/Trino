//! `TSTR`: a baked string table (one language), zero-copy.
//!
//! Little-endian. Version 2 (issue #59):
//!
//! | at | field |
//! |---|---|
//! | 0 | magic `TSTR` |
//! | 4 | `u16` version (2) |
//! | 6 | `u16` record count |
//! | 8 | refs (`u32 offset, u32 length` into the pool): language, fallback, display name, decimal separator, thousands separator, currency symbol, date separator |
//! | 64 | `u8` plural rule, `u8` date order, `u8` currency decimals, `u8` flags (1 = currency after, 2 = space) |
//! | 68 | `u8` min group, `u8` 0, `u16` font remap count |
//! | 72 | `u32` the language's asset group hash (0 = none), `u32` 0 |
//! | 80 | font remaps: `u32` font, `u32` the font this language draws it with |
//! | … | records sorted by key hash: `u32` hash, `u32` offset, `u32` length (bit 31: a plural entry) |
//! | … | the UTF-8 pool |
//!
//! A plural entry's text is its six forms (zero, one, two, few, many,
//! other) joined by U+001F; an absent form is empty. Version 1 (#58: header
//! of 24 bytes, language and fallback, plain records) still parses; its
//! locale is [`Locale::for_tag`] of its language.

use super::locale::{DateOrder, Locale, PluralForm, PluralRule};
use crate::asset::asset_id;

pub const TSTR_MAGIC: &[u8; 4] = b"TSTR";
/// The version [`encode_tstr`] writes.
pub const TSTR_VERSION: u16 = 2;
const HEADER_V1: usize = 24;
const HEADER: usize = 80;
const ENTRY: usize = 12;
const PLURAL: u32 = 1 << 31;
/// Separates a plural entry's forms in the pool.
pub const FORM_SEP: char = '\u{1f}';

/// Why a TSTR blob was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StringsError {
    BadMagic,
    UnsupportedVersion(u16),
    Truncated,
    BadUtf8,
}

/// One entry of a table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Entry<'a> {
    Text(&'a str),
    Plural(PluralForms<'a>),
}

impl<'a> Entry<'a> {
    /// The text (a plural entry's `other` form).
    pub fn text(&self) -> &'a str {
        match self {
            Entry::Text(t) => t,
            Entry::Plural(p) => p.other(),
        }
    }
}

/// A plural entry's forms (zero, one, two, few, many, other).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PluralForms<'a> {
    joined: &'a str,
}

impl<'a> PluralForms<'a> {
    /// The text of `form` (empty when the table doesn't give it).
    pub fn get(&self, form: PluralForm) -> &'a str {
        self.joined.split(FORM_SEP).nth(form as usize).unwrap_or("")
    }

    pub fn other(&self) -> &'a str {
        self.get(PluralForm::Other)
    }

    /// `form`'s text, or `other`'s when the table leaves it out.
    pub fn pick(&self, form: PluralForm) -> &'a str {
        match self.get(form) {
            "" => self.other(),
            t => t,
        }
    }
}

/// A baked string table (zero-copy).
#[derive(Clone, Copy, Debug)]
pub struct StringTable<'a> {
    header: &'a [u8],
    fonts: &'a [u8],
    records: &'a [u8],
    pool: &'a str,
}

impl<'a> StringTable<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<StringTable<'a>, StringsError> {
        if bytes.len() < HEADER_V1 {
            return Err(StringsError::Truncated);
        }
        if &bytes[0..4] != TSTR_MAGIC {
            return Err(StringsError::BadMagic);
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        let (head, fonts) = match version {
            1 => (HEADER_V1, 0),
            2 => {
                if bytes.len() < HEADER {
                    return Err(StringsError::Truncated);
                }
                (HEADER, u16::from_le_bytes([bytes[70], bytes[71]]) as usize)
            }
            v => return Err(StringsError::UnsupportedVersion(v)),
        };
        let count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        let records_at = head + fonts * 8;
        let pool_at = records_at + count * ENTRY;
        if bytes.len() < pool_at {
            return Err(StringsError::Truncated);
        }
        let pool = core::str::from_utf8(&bytes[pool_at..]).map_err(|_| StringsError::BadUtf8)?;
        let table = StringTable {
            header: &bytes[..head],
            fonts: &bytes[head..records_at],
            records: &bytes[records_at..pool_at],
            pool,
        };
        let refs = if version == 1 { 2 } else { 7 };
        for r in 0..refs {
            table.ref_at(8 + r * 8).ok_or(StringsError::BadUtf8)?;
        }
        for i in 0..count {
            let (off, len, _) = table.span(i);
            if pool.get(off..off + len).is_none() {
                return Err(StringsError::BadUtf8);
            }
        }
        Ok(table)
    }

    /// A table from parts already checked (generated code: `cargo xtask
    /// i18n export --rust`, whose test parses the same bytes). Lookups on
    /// a malformed table return `None` or empty text, never panic.
    pub const fn from_parts(
        header: &'a [u8],
        fonts: &'a [u8],
        records: &'a [u8],
        pool: &'a str,
    ) -> StringTable<'a> {
        StringTable {
            header,
            fonts,
            records,
            pool,
        }
    }

    /// The parts [`from_parts`](Self::from_parts) takes back.
    pub fn parts(&self) -> (&'a [u8], &'a [u8], &'a [u8], &'a str) {
        (self.header, self.fonts, self.records, self.pool)
    }

    pub fn version(&self) -> u16 {
        u16::from_le_bytes([self.header[4], self.header[5]])
    }

    fn ref_at(&self, at: usize) -> Option<&'a str> {
        if at + 8 > self.header.len() {
            return None;
        }
        let off = rd32(self.header, at) as usize;
        let len = rd32(self.header, at + 4) as usize;
        self.pool.get(off..off.checked_add(len)?)
    }

    fn v2_ref(&self, at: usize) -> Option<&'a str> {
        if self.header.len() >= HEADER {
            self.ref_at(at)
        } else {
            None
        }
    }

    /// The language tag (`en-US`, `pt-BR`).
    pub fn lang(&self) -> &'a str {
        self.ref_at(8).unwrap_or("")
    }

    /// The language's id ([`asset_id`] of its tag).
    pub fn id(&self) -> super::LangId {
        super::LangId::new(self.lang())
    }

    /// The language its missing keys come from (empty: none).
    pub fn fallback(&self) -> &'a str {
        self.ref_at(16).unwrap_or("")
    }

    /// The language's own name for a language menu (`Português`); the tag
    /// when the table gives none.
    pub fn name(&self) -> &'a str {
        match self.v2_ref(24) {
            Some(n) if !n.is_empty() => n,
            _ => self.lang(),
        }
    }

    /// How the language writes numbers, money and dates.
    pub fn locale(&self) -> Locale<'a> {
        if self.header.len() < HEADER {
            return Locale::for_tag(self.lang());
        }
        let h = self.header;
        Locale {
            decimal: self.ref_at(32).unwrap_or("."),
            thousands: self.ref_at(40).unwrap_or(","),
            currency: self.ref_at(48).unwrap_or(""),
            date_sep: self.ref_at(56).unwrap_or("/"),
            plural: PluralRule::from_u8(h[64]),
            date_order: DateOrder::from_u8(h[65]),
            currency_decimals: h[66].min(18),
            currency_after: h[67] & 1 != 0,
            currency_space: h[67] & 2 != 0,
            min_group: h[68],
        }
    }

    /// The asset group the language's fonts are in (`None`: boot).
    pub fn group(&self) -> Option<crate::GroupId> {
        if self.header.len() < HEADER {
            return None;
        }
        match rd32(self.header, 72) {
            0 => None,
            h => Some(crate::GroupId(h)),
        }
    }

    /// The font this language draws `font` with: a per-language atlas or
    /// the manifest's `[i18n.fonts]` override, else `font` itself.
    pub fn font(&self, font: crate::FontId) -> crate::FontId {
        let n = self.fonts.len() / 8;
        for i in 0..n {
            if rd32(self.fonts, i * 8) == font.0 {
                return crate::FontId(rd32(self.fonts, i * 8 + 4));
            }
        }
        font
    }

    /// Every font remap (`(font, replacement)`).
    pub fn font_remaps(&self) -> impl Iterator<Item = (crate::FontId, crate::FontId)> + 'a {
        let f = self.fonts;
        (0..f.len() / 8).map(move |i| {
            (
                crate::FontId(rd32(f, i * 8)),
                crate::FontId(rd32(f, i * 8 + 4)),
            )
        })
    }

    pub fn len(&self) -> usize {
        self.records.len() / ENTRY
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    fn hash(&self, i: usize) -> u32 {
        rd32(self.records, i * ENTRY)
    }

    fn span(&self, i: usize) -> (usize, usize, bool) {
        let len = rd32(self.records, i * ENTRY + 8);
        (
            rd32(self.records, i * ENTRY + 4) as usize,
            (len & !PLURAL) as usize,
            len & PLURAL != 0,
        )
    }

    /// The entry whose key hashes to `hash` (a binary search).
    pub fn entry_hash(&self, hash: u32) -> Option<Entry<'a>> {
        let (mut lo, mut hi) = (0, self.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.hash(mid).cmp(&hash) {
                core::cmp::Ordering::Less => lo = mid + 1,
                core::cmp::Ordering::Greater => hi = mid,
                core::cmp::Ordering::Equal => {
                    let (off, len, plural) = self.span(mid);
                    let text = self.pool.get(off..off + len)?;
                    return Some(if plural {
                        Entry::Plural(PluralForms { joined: text })
                    } else {
                        Entry::Text(text)
                    });
                }
            }
        }
        None
    }

    /// The entry for `key`.
    pub fn entry(&self, key: super::StrKey) -> Option<Entry<'a>> {
        self.entry_hash(key.0)
    }

    /// The text for `key` (a plural entry's `other` form).
    pub fn lookup(&self, key: &str) -> Option<&'a str> {
        self.entry_hash(asset_id(key)).map(|e| e.text())
    }

    /// Every entry's key hash, in table order.
    pub fn hashes(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.len()).map(|i| self.hash(i))
    }
}

/// An entry to bake.
#[derive(Clone, Copy, Debug)]
pub enum EntrySpec<'s> {
    Text(&'s str),
    /// zero, one, two, few, many, other.
    Plural([&'s str; 6]),
}

/// The language-wide part of a table to bake.
#[derive(Clone, Copy, Debug)]
pub struct TableSpec<'s> {
    pub lang: &'s str,
    pub fallback: &'s str,
    pub name: &'s str,
    pub locale: Locale<'s>,
    /// `(font, replacement)` hashes.
    pub fonts: &'s [(u32, u32)],
    /// The asset group of the language's fonts (0 = none).
    pub group: u32,
}

impl<'s> TableSpec<'s> {
    /// No name, the tag's default locale, no fonts or group.
    pub fn plain(lang: &'s str, fallback: &'s str) -> TableSpec<'s> {
        TableSpec {
            lang,
            fallback,
            name: "",
            locale: Locale::for_tag(lang),
            fonts: &[],
            group: 0,
        }
    }
}

/// Bake a plain table (v2, the tag's default locale): `entries` as `(key,
/// text)` in any order. `Err` names the first key whose hash another key
/// already has. Host tools only.
pub fn encode_tstr<'e>(
    lang: &str,
    fallback: &str,
    entries: &mut [(&'e str, &'e str)],
    out: &mut dyn FnMut(&[u8]),
) -> Result<(), (u32, usize)> {
    entries.sort_unstable_by_key(|(k, _)| asset_id(k));
    let entries = &*entries;
    encode_with(
        &TableSpec::plain(lang, fallback),
        entries.len(),
        &mut |i| (entries[i].0, EntrySpec::Text(entries[i].1)),
        out,
    )
}

/// Bake a table with its locale, plural entries and font remaps.
pub fn encode_tstr_with<'e>(
    spec: &TableSpec<'_>,
    entries: &mut [(&'e str, EntrySpec<'e>)],
    out: &mut dyn FnMut(&[u8]),
) -> Result<(), (u32, usize)> {
    entries.sort_unstable_by_key(|(k, _)| asset_id(k));
    let entries = &*entries;
    encode_with(spec, entries.len(), &mut |i| entries[i], out)
}

fn encode_with<'e>(
    spec: &TableSpec<'_>,
    count: usize,
    entry: &mut dyn FnMut(usize) -> (&'e str, EntrySpec<'e>),
    out: &mut dyn FnMut(&[u8]),
) -> Result<(), (u32, usize)> {
    for i in 1..count {
        if asset_id(entry(i - 1).0) == asset_id(entry(i).0) {
            return Err((asset_id(entry(i).0), i));
        }
    }
    let l = &spec.locale;
    let mut header = [0u8; HEADER];
    header[0..4].copy_from_slice(TSTR_MAGIC);
    header[4..6].copy_from_slice(&TSTR_VERSION.to_le_bytes());
    header[6..8].copy_from_slice(&(count as u16).to_le_bytes());
    let refs = [
        spec.lang,
        spec.fallback,
        spec.name,
        l.decimal,
        l.thousands,
        l.currency,
        l.date_sep,
    ];
    let mut at = 0u32;
    for (r, s) in refs.iter().enumerate() {
        header[8 + r * 8..12 + r * 8].copy_from_slice(&at.to_le_bytes());
        header[12 + r * 8..16 + r * 8].copy_from_slice(&(s.len() as u32).to_le_bytes());
        at += s.len() as u32;
    }
    header[64] = l.plural as u8;
    header[65] = l.date_order as u8;
    header[66] = l.currency_decimals;
    header[67] = (l.currency_after as u8) | ((l.currency_space as u8) << 1);
    header[68] = l.min_group;
    header[70..72].copy_from_slice(&(spec.fonts.len() as u16).to_le_bytes());
    header[72..76].copy_from_slice(&spec.group.to_le_bytes());
    out(&header);
    for (from, to) in spec.fonts {
        out(&from.to_le_bytes());
        out(&to.to_le_bytes());
    }
    let text_len = |e: &EntrySpec<'_>| match e {
        EntrySpec::Text(t) => t.len(),
        EntrySpec::Plural(f) => f.iter().map(|s| s.len()).sum::<usize>() + 5,
    };
    for i in 0..count {
        let (k, e) = entry(i);
        let len = text_len(&e) as u32;
        let flag = if matches!(e, EntrySpec::Plural(_)) {
            PLURAL
        } else {
            0
        };
        let mut r = [0u8; ENTRY];
        r[0..4].copy_from_slice(&asset_id(k).to_le_bytes());
        r[4..8].copy_from_slice(&at.to_le_bytes());
        r[8..12].copy_from_slice(&(len | flag).to_le_bytes());
        at += len;
        out(&r);
    }
    for s in refs {
        out(s.as_bytes());
    }
    let mut sep = [0u8; 1];
    FORM_SEP.encode_utf8(&mut sep);
    for i in 0..count {
        match entry(i).1 {
            EntrySpec::Text(t) => out(t.as_bytes()),
            EntrySpec::Plural(forms) => {
                for (f, s) in forms.iter().enumerate() {
                    if f > 0 {
                        out(&sep);
                    }
                    out(s.as_bytes());
                }
            }
        }
    }
    Ok(())
}

fn rd32(b: &[u8], at: usize) -> u32 {
    match b.get(at..at + 4) {
        Some(w) => u32::from_le_bytes([w[0], w[1], w[2], w[3]]),
        None => 0,
    }
}
