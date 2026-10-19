//! Text by key: what a node's `key` resolves through.
//!
//! [`UiStrings`] is the lookup a screen is drawn with. Games with their own
//! tables implement it; the engine's own is [`StringTable`], a baked
//! `TSTR` blob made from `assets/i18n/<lang>.ron` (the plain-string part of
//! the localization format of issue #59: plurals and locale formats come
//! with it, in a later TSTR version).
//!
//! TSTR (little-endian), version 1: `magic "TSTR"`, `u16 version`,
//! `u16 count`, the language and its fallback as string refs (`u32 offset,
//! u32 length` into the pool each), then `count` records of 12 bytes sorted
//! by key hash ([`asset_id`](crate::asset_id) of the key): `u32 hash, u32
//! offset, u32 length`, then the UTF-8 pool. The bake refuses two keys with
//! one hash.

use crate::asset::asset_id;

/// A key → text lookup (the current language).
pub trait UiStrings {
    fn get(&self, key: &str) -> Option<&str>;

    /// Changes whenever the texts do (a language switch): the UI lays out
    /// again when it moves.
    fn revision(&self) -> u32 {
        0
    }
}

/// No table: every node shows its own `text`.
pub struct NoStrings;

impl UiStrings for NoStrings {
    fn get(&self, _key: &str) -> Option<&str> {
        None
    }
}

pub const TSTR_MAGIC: &[u8; 4] = b"TSTR";
pub const TSTR_VERSION: u16 = 1;
const HEADER: usize = 24;
const ENTRY: usize = 12;

/// Why a TSTR blob was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StringsError {
    BadMagic,
    UnsupportedVersion(u16),
    Truncated,
    BadUtf8,
}

/// A baked string table (zero-copy).
#[derive(Clone, Copy, Debug)]
pub struct StringTable<'a> {
    records: &'a [u8],
    pool: &'a str,
    lang: &'a str,
    fallback: &'a str,
}

impl<'a> StringTable<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<StringTable<'a>, StringsError> {
        if bytes.len() < HEADER {
            return Err(StringsError::Truncated);
        }
        if &bytes[0..4] != TSTR_MAGIC {
            return Err(StringsError::BadMagic);
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != TSTR_VERSION {
            return Err(StringsError::UnsupportedVersion(version));
        }
        let count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        let records = bytes
            .get(HEADER..HEADER + count * ENTRY)
            .ok_or(StringsError::Truncated)?;
        let pool = core::str::from_utf8(&bytes[HEADER + count * ENTRY..])
            .map_err(|_| StringsError::BadUtf8)?;
        let pick = |at: usize| {
            let off = rd32(bytes, at) as usize;
            let len = rd32(bytes, at + 4) as usize;
            pool.get(off..off + len).ok_or(StringsError::BadUtf8)
        };
        let table = StringTable {
            records,
            pool,
            lang: pick(8)?,
            fallback: pick(16)?,
        };
        for i in 0..count {
            let (off, len) = table.span(i);
            if pool.get(off..off + len).is_none() {
                return Err(StringsError::BadUtf8);
            }
        }
        Ok(table)
    }

    /// The language tag (`en-US`, `pt-BR`).
    pub fn lang(&self) -> &'a str {
        self.lang
    }

    /// The language its missing keys come from (empty: none).
    pub fn fallback(&self) -> &'a str {
        self.fallback
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

    fn span(&self, i: usize) -> (usize, usize) {
        (
            rd32(self.records, i * ENTRY + 4) as usize,
            rd32(self.records, i * ENTRY + 8) as usize,
        )
    }

    /// The text for `key` (a binary search on its hash).
    pub fn lookup(&self, key: &str) -> Option<&'a str> {
        let h = asset_id(key);
        let (mut lo, mut hi) = (0, self.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.hash(mid).cmp(&h) {
                core::cmp::Ordering::Less => lo = mid + 1,
                core::cmp::Ordering::Greater => hi = mid,
                core::cmp::Ordering::Equal => {
                    let (off, len) = self.span(mid);
                    return self.pool.get(off..off + len);
                }
            }
        }
        None
    }
}

impl UiStrings for StringTable<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.lookup(key)
    }

    fn revision(&self) -> u32 {
        asset_id(self.lang)
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
}

/// Bake a table: `entries` as `(key, text)` in any order. `Err` names the
/// first key whose hash another key already has. Host tools only (it needs
/// a buffer the size of the result).
pub fn encode_tstr(
    lang: &str,
    fallback: &str,
    entries: &mut [(&str, &str)],
    out: &mut dyn FnMut(&[u8]),
) -> Result<(), (u32, usize)> {
    entries.sort_unstable_by_key(|(k, _)| asset_id(k));
    for (i, w) in entries.windows(2).enumerate() {
        if asset_id(w[0].0) == asset_id(w[1].0) {
            return Err((asset_id(w[1].0), i + 1));
        }
    }
    let mut header = [0u8; HEADER];
    header[0..4].copy_from_slice(TSTR_MAGIC);
    header[4..6].copy_from_slice(&TSTR_VERSION.to_le_bytes());
    header[6..8].copy_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut at = 0u32;
    let mut put = |b: &mut [u8], pos: usize, len: usize| {
        b[pos..pos + 4].copy_from_slice(&at.to_le_bytes());
        b[pos + 4..pos + 8].copy_from_slice(&(len as u32).to_le_bytes());
        at += len as u32;
    };
    put(&mut header, 8, lang.len());
    put(&mut header, 16, fallback.len());
    out(&header);
    for (k, text) in entries.iter() {
        let mut r = [0u8; ENTRY];
        r[0..4].copy_from_slice(&asset_id(k).to_le_bytes());
        r[4..8].copy_from_slice(&at.to_le_bytes());
        r[8..12].copy_from_slice(&(text.len() as u32).to_le_bytes());
        at += text.len() as u32;
        out(&r);
    }
    out(lang.as_bytes());
    out(fallback.as_bytes());
    for (_, text) in entries.iter() {
        out(text.as_bytes());
    }
    Ok(())
}

fn rd32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
