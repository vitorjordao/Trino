//! `TACC`: a project's accessibility data, baked into the `access/data`
//! blob on every target (no manifest entry): the semantic colors of
//! `assets/palettes.ron` with an alternative per [`CvdMode`], the captions
//! of the manifest's sounds, the sprites' second-channel icons and the
//! high-contrast UI theme (`ui/themes/high_contrast.ron`). Zero-copy:
//! [`AccessData::parse`] checks it once, lookups binary-search it.
//!
//! Layout (little-endian), version 1:
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 4 | magic `TACC` |
//! | 4 | 2 | version (1) |
//! | 6 | 2 | flags: bit 0 has a high-contrast theme |
//! | 8 | 2 | semantic colors |
//! | 10 | 2 | captions |
//! | 12 | 2 | icons |
//! | 14 | 2 | reserved |
//! | 16 | 44 | theme: font (`FontId`), text scale (u8) + 3 reserved, 9 colors (RGBA) as in `TUIL` |
//! | 60 | 24 × colors | name id (`asset_id(name)`), then RGBA for off, protanopia, deuteranopia, tritanopia, grayscale; sorted by id |
//! | … | 12 × captions | sound id (`asset_id("sounds/x")`), key (u16 offset, u16 length into the pool), priority (u8), flags (u8: bit 0 important, bit 1 direction), duration ms (u16, 0 = the default 3 s); sorted by id |
//! | … | 8 × icons | sprite id, icon sprite id; sorted by sprite id |
//! | … | rest | string pool (UTF-8) |

use super::CvdMode;
use crate::asset::asset_id;
use crate::math::Color;
use crate::platform::{BlobId, Platform};
use crate::render::{FontId, Palette, SpriteId};
use crate::ui::Theme;

pub const MAGIC: &[u8; 4] = b"TACC";
pub const VERSION: u16 = 1;
/// The blob's logical path.
pub const LOGICAL: &str = "access/data";
/// Header bytes (the theme included).
pub const HEADER: usize = 60;
/// Bytes per semantic color.
pub const COLOR_RECORD: usize = 24;
/// Bytes per caption.
pub const CAPTION_RECORD: usize = 12;
/// Bytes per icon.
pub const ICON_RECORD: usize = 8;

/// Why a blob was refused, or why one could not be written.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AccessError {
    BadMagic,
    UnsupportedVersion(u16),
    Truncated,
    /// A caption key outside the pool, or not UTF-8.
    BadString,
    /// Records out of order (or the same id twice).
    Unsorted,
    /// The output buffer is too small.
    NoRoom,
}

/// A sound's caption ([`AccessData::caption`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CaptionInfo<'a> {
    /// A string-table key (`cap.whistle`): the text in the player's
    /// language; the key itself where no table has it.
    pub key: &'a str,
    /// Higher stays longer when the queue is full.
    pub priority: u8,
    /// Shown in [`CaptionMode::Important`](super::CaptionMode::Important).
    pub important: bool,
    /// Shows an arrow toward the sound (from its pan).
    pub direction: bool,
    /// How long it stays, ms (0 = [`DEFAULT_SECONDS`](super::captions::DEFAULT_SECONDS)).
    pub duration_ms: u16,
}

/// A caption to bake ([`encode`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CaptionRecord<'a> {
    pub sound: u32,
    pub info: CaptionInfo<'a>,
}

/// A parsed `access/data` blob.
#[derive(Clone, Copy, Debug)]
pub struct AccessData<'a> {
    colors: &'a [u8],
    captions: &'a [u8],
    icons: &'a [u8],
    pool: &'a str,
    theme: Option<Theme>,
}

impl<'a> AccessData<'a> {
    /// An empty table (no blob baked): every lookup answers `None`.
    pub const EMPTY: AccessData<'static> = AccessData {
        colors: &[],
        captions: &[],
        icons: &[],
        pool: "",
        theme: None,
    };

    pub fn parse(bytes: &'a [u8]) -> Result<AccessData<'a>, AccessError> {
        if bytes.len() < HEADER {
            return Err(AccessError::Truncated);
        }
        if &bytes[0..4] != MAGIC {
            return Err(AccessError::BadMagic);
        }
        let version = rd16(bytes, 4);
        if version != VERSION {
            return Err(AccessError::UnsupportedVersion(version));
        }
        let flags = rd16(bytes, 6);
        let n = |at| rd16(bytes, at) as usize;
        let (nc, nk, ni) = (n(8), n(10), n(12));
        let mut at = HEADER;
        let mut take = |len: usize| -> Result<&'a [u8], AccessError> {
            let s = bytes.get(at..at + len).ok_or(AccessError::Truncated)?;
            at += len;
            Ok(s)
        };
        let colors = take(nc * COLOR_RECORD)?;
        let captions = take(nk * CAPTION_RECORD)?;
        let icons = take(ni * ICON_RECORD)?;
        let pool = core::str::from_utf8(&bytes[at..]).map_err(|_| AccessError::BadString)?;
        sorted(colors, COLOR_RECORD)?;
        sorted(captions, CAPTION_RECORD)?;
        sorted(icons, ICON_RECORD)?;
        for r in captions.as_chunks::<CAPTION_RECORD>().0 {
            let (off, len) = (rd16(r, 4) as usize, rd16(r, 6) as usize);
            if pool.get(off..off + len).is_none() {
                return Err(AccessError::BadString);
            }
        }
        let theme = (flags & 1 != 0).then(|| {
            let c = |k: usize| rd_color(bytes, 24 + 4 * k);
            Theme {
                font: FontId(rd32(bytes, 16)),
                text_scale: bytes[20].max(1),
                text: c(0),
                dim: c(1),
                panel: c(2),
                button: c(3),
                button_text: c(4),
                focus: c(5),
                focus_text: c(6),
                accent: c(7),
                pressed: c(8),
            }
        });
        Ok(AccessData {
            colors,
            captions,
            icons,
            pool,
            theme,
        })
    }

    /// A semantic color as `mode` wants it (its base color for `Off`), or
    /// `None` when the project has no such name.
    pub fn color(&self, name: &str, mode: CvdMode) -> Option<Color> {
        self.color_id(asset_id(name), mode)
    }

    /// [`color`](Self::color) by the name's id (`asset_id(name)`, a const).
    pub fn color_id(&self, id: u32, mode: CvdMode) -> Option<Color> {
        let r = find(self.colors, COLOR_RECORD, id)?;
        Some(rd_color(r, 4 + 4 * mode as usize))
    }

    /// Four semantic colors as a [`Palette`] for a slotted mesh: one lookup
    /// per draw (or per mode change), then the draw the game already makes.
    /// A missing name (or `""`) keeps that slot white, i.e. its baked color.
    pub fn palette(&self, names: [&str; 4], mode: CvdMode) -> Palette {
        names.map(|n| {
            if n.is_empty() {
                Color::WHITE
            } else {
                self.color(n, mode).unwrap_or(Color::WHITE)
            }
        })
    }

    /// The caption of a sound (or a music track: `asset_id("music/x")`).
    pub fn caption(&self, sound: u32) -> Option<CaptionInfo<'a>> {
        let r = find(self.captions, CAPTION_RECORD, sound)?;
        let (off, len) = (rd16(r, 4) as usize, rd16(r, 6) as usize);
        Some(CaptionInfo {
            key: self.pool.get(off..off + len).unwrap_or(""),
            priority: r[8],
            important: r[9] & 1 != 0,
            direction: r[9] & 2 != 0,
            duration_ms: rd16(r, 10),
        })
    }

    /// The icon a sprite shows next to (or over) its color while a
    /// color-vision mode is on: a pattern, a shape, a letter.
    pub fn icon(&self, sprite: SpriteId) -> Option<SpriteId> {
        find(self.icons, ICON_RECORD, sprite.0).map(|r| SpriteId(rd32(r, 4)))
    }

    /// The project's high-contrast UI theme (`ui/themes/high_contrast.ron`),
    /// if it has one; [`Theme::high_contrast`] derives one otherwise.
    pub fn contrast_theme(&self) -> Option<Theme> {
        self.theme
    }

    pub fn color_count(&self) -> usize {
        self.colors.len() / COLOR_RECORD
    }

    pub fn caption_count(&self) -> usize {
        self.captions.len() / CAPTION_RECORD
    }

    pub fn icon_count(&self) -> usize {
        self.icons.len() / ICON_RECORD
    }
}

impl Default for AccessData<'static> {
    fn default() -> Self {
        Self::EMPTY
    }
}

fn sorted(records: &[u8], size: usize) -> Result<(), AccessError> {
    let mut last = None;
    for r in records.chunks_exact(size) {
        let id = rd32(r, 0);
        if last.is_some_and(|l| l >= id) {
            return Err(AccessError::Unsorted);
        }
        last = Some(id);
    }
    Ok(())
}

fn ascending(mut ids: impl Iterator<Item = u32>) -> bool {
    let mut last = None;
    ids.all(|id| {
        let ok = last.is_none_or(|l| l < id);
        last = Some(id);
        ok
    })
}

fn find(records: &[u8], size: usize, id: u32) -> Option<&[u8]> {
    let (mut lo, mut hi) = (0usize, records.len() / size);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let r = &records[mid * size..(mid + 1) * size];
        match rd32(r, 0).cmp(&id) {
            core::cmp::Ordering::Equal => return Some(r),
            core::cmp::Ordering::Less => lo = mid + 1,
            core::cmp::Ordering::Greater => hi = mid,
        }
    }
    None
}

/// Bytes [`encode`] writes for these tables.
pub fn encoded_len(colors: usize, captions: &[CaptionRecord<'_>], icons: usize) -> usize {
    HEADER
        + colors * COLOR_RECORD
        + captions.len() * CAPTION_RECORD
        + icons * ICON_RECORD
        + captions.iter().map(|c| c.info.key.len()).sum::<usize>()
}

/// Write a `TACC` blob (the asset bake): every table sorted by id.
/// Returns the bytes written.
pub fn encode(
    colors: &[(u32, [Color; 5])],
    captions: &[CaptionRecord<'_>],
    icons: &[(u32, u32)],
    theme: Option<&Theme>,
    out: &mut [u8],
) -> Result<usize, AccessError> {
    let len = encoded_len(colors.len(), captions, icons.len());
    if out.len() < len {
        return Err(AccessError::NoRoom);
    }
    if !ascending(colors.iter().map(|c| c.0))
        || !ascending(captions.iter().map(|c| c.sound))
        || !ascending(icons.iter().map(|c| c.0))
    {
        return Err(AccessError::Unsorted);
    }
    let out = &mut out[..len];
    out.fill(0);
    out[0..4].copy_from_slice(MAGIC);
    wr16(out, 4, VERSION);
    wr16(out, 6, theme.is_some() as u16);
    wr16(out, 8, colors.len() as u16);
    wr16(out, 10, captions.len() as u16);
    wr16(out, 12, icons.len() as u16);
    if let Some(t) = theme {
        wr32(out, 16, t.font.0);
        out[20] = t.text_scale;
        let cs = [
            t.text,
            t.dim,
            t.panel,
            t.button,
            t.button_text,
            t.focus,
            t.focus_text,
            t.accent,
            t.pressed,
        ];
        for (k, c) in cs.into_iter().enumerate() {
            wr_color(out, 24 + 4 * k, c);
        }
    }
    let mut at = HEADER;
    for (id, cs) in colors {
        wr32(out, at, *id);
        for (k, c) in cs.iter().enumerate() {
            wr_color(out, at + 4 + 4 * k, *c);
        }
        at += COLOR_RECORD;
    }
    let pool_at = at + captions.len() * CAPTION_RECORD + icons.len() * ICON_RECORD;
    let mut pool = 0usize;
    for c in captions {
        let key = c.info.key.as_bytes();
        if pool + key.len() > u16::MAX as usize {
            return Err(AccessError::NoRoom);
        }
        wr32(out, at, c.sound);
        wr16(out, at + 4, pool as u16);
        wr16(out, at + 6, key.len() as u16);
        out[at + 8] = c.info.priority;
        out[at + 9] = c.info.important as u8 | (c.info.direction as u8) << 1;
        wr16(out, at + 10, c.info.duration_ms);
        out[pool_at + pool..pool_at + pool + key.len()].copy_from_slice(key);
        pool += key.len();
        at += CAPTION_RECORD;
    }
    for (sprite, icon) in icons {
        wr32(out, at, *sprite);
        wr32(out, at + 4, *icon);
        at += ICON_RECORD;
    }
    Ok(len)
}

/// The project's `access/data`, kept parsed across frames like
/// [`ScreenBlob`](crate::ui::ScreenBlob) (re-parsed when the PC's live
/// reload hands out new bytes). Fetch it in `update_with`.
#[derive(Clone, Copy, Debug)]
pub struct AccessBlob {
    bytes: Option<&'static [u8]>,
    data: Option<AccessData<'static>>,
    /// Why the last bytes did not parse.
    pub error: Option<AccessError>,
}

impl AccessBlob {
    pub const fn new() -> AccessBlob {
        AccessBlob {
            bytes: None,
            data: None,
            error: None,
        }
    }

    /// The data, re-parsed when the blob changed; `None` when the project
    /// baked none (no palettes, captions, icons or theme) or it is broken.
    pub fn get(&mut self, platform: &dyn Platform) -> Option<&AccessData<'static>> {
        let bytes = platform.blob(BlobId::from_path(LOGICAL));
        if bytes.map(<[u8]>::as_ptr) != self.bytes.map(<[u8]>::as_ptr) {
            self.bytes = bytes;
            self.data = None;
            self.error = None;
            if let Some(b) = bytes {
                match AccessData::parse(b) {
                    Ok(d) => self.data = Some(d),
                    Err(e) => self.error = Some(e),
                }
            }
        }
        self.data.as_ref()
    }

    /// The data as last fetched.
    pub fn data(&self) -> Option<&AccessData<'static>> {
        self.data.as_ref()
    }

    /// The data as last fetched, or the empty table.
    pub fn or_empty(&self) -> AccessData<'static> {
        self.data.unwrap_or(AccessData::EMPTY)
    }
}

impl Default for AccessBlob {
    fn default() -> Self {
        Self::new()
    }
}

fn rd16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn rd32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn rd_color(b: &[u8], at: usize) -> Color {
    Color::rgba(b[at], b[at + 1], b[at + 2], b[at + 3])
}

fn wr16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn wr32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn wr_color(b: &mut [u8], at: usize, c: Color) {
    b[at..at + 4].copy_from_slice(&[c.r, c.g, c.b, c.a]);
}
