//! Bitmap text: the baked font format and the layout every backend shares.
//!
//! A font is two baked pieces under one handle ([`FontId`]): an **atlas**
//! sprite (registered with the renderer like any sprite, so it gets each
//! console's native texture format — CI4 on the N64) and a **glyph table**
//! blob (`TFNT`, below). [`draw`] lays text out and issues one
//! [`Renderer::draw_sprite`] per glyph with a sprite region, so text works
//! on every backend with no backend code beyond keeping the table.
//!
//! TFNT (little-endian): `magic "TFNT"`, `u16 version (1)`,
//! `u16 line_height`, `u16 ascent`, `u16 glyph_count`, then `glyph_count`
//! records of 12 bytes sorted by codepoint: `u32 codepoint`, `u16 x, u16 y`
//! (atlas pixels), `u8 w, u8 h`, `i8 x_offset, u8 advance`. A glyph draws at
//! `pen + (x_offset, 0)` and moves the pen by `advance`; glyph boxes are
//! line-height tall cells, so there is no per-glyph y offset.
//!
//! Characters missing from the table draw as `?` (or nothing if that is
//! missing too); `\n` starts a new line.

use crate::math::{Rect, Vec2};
use crate::render::{FontId, Renderer, SpriteId, SpriteParams, TextAlign, TextParams};

pub const TFNT_MAGIC: &[u8; 4] = b"TFNT";
pub const TFNT_VERSION: u16 = 1;
const HEADER: usize = 12;
const RECORD: usize = 12;

/// One glyph of a [`BitmapFont`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Glyph {
    pub codepoint: u32,
    pub x: u16,
    pub y: u16,
    pub w: u8,
    pub h: u8,
    pub x_offset: i8,
    pub advance: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FontError {
    BadMagic,
    UnsupportedVersion(u16),
    Truncated,
}

/// A parsed (borrowed) glyph table — zero-copy over the baked blob.
#[derive(Clone, Copy, Debug)]
pub struct BitmapFont<'a> {
    records: &'a [u8],
    pub line_height: u16,
    pub ascent: u16,
    pub glyph_count: usize,
    /// Direct lookup for codepoints below 256 ([`with_index`](Self::with_index)).
    index: Option<&'a GlyphIndex>,
}

/// The record of every codepoint below 256 (ASCII and Latin-1, what most
/// interface text is made of), so a lookup is one load instead of a binary
/// search per character. Built once per font when a backend registers it
/// ([`GlyphIndex::build`]) and lent to [`BitmapFont::with_index`]; the
/// glyphs found are the same either way. 512 bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphIndex {
    slot: [u16; 256],
}

impl GlyphIndex {
    const NONE: u16 = u16::MAX;

    pub fn build(font: &BitmapFont<'_>) -> Self {
        let mut slot = [Self::NONE; 256];
        for i in 0..font.glyph_count.min(Self::NONE as usize) {
            let cp = font.codepoint_at(i);
            if cp < 256 && slot[cp as usize] == Self::NONE {
                slot[cp as usize] = i as u16;
            }
        }
        GlyphIndex { slot }
    }
}

impl<'a> BitmapFont<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, FontError> {
        if bytes.len() < HEADER {
            return Err(FontError::Truncated);
        }
        if &bytes[0..4] != TFNT_MAGIC {
            return Err(FontError::BadMagic);
        }
        let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        let version = u16_at(4);
        if version != TFNT_VERSION {
            return Err(FontError::UnsupportedVersion(version));
        }
        let glyph_count = u16_at(10) as usize;
        let end = HEADER + glyph_count * RECORD;
        if bytes.len() < end {
            return Err(FontError::Truncated);
        }
        Ok(BitmapFont {
            records: &bytes[HEADER..end],
            line_height: u16_at(6),
            ascent: u16_at(8),
            glyph_count,
            index: None,
        })
    }

    /// The same font, looking codepoints below 256 up in `index` (built
    /// from this table by [`GlyphIndex::build`]).
    pub fn with_index(self, index: &'a GlyphIndex) -> Self {
        BitmapFont {
            index: Some(index),
            ..self
        }
    }

    fn codepoint_at(&self, i: usize) -> u32 {
        let r = &self.records[i * RECORD..i * RECORD + 4];
        u32::from_le_bytes([r[0], r[1], r[2], r[3]])
    }

    fn record(&self, i: usize) -> Glyph {
        let r = &self.records[i * RECORD..(i + 1) * RECORD];
        Glyph {
            codepoint: u32::from_le_bytes([r[0], r[1], r[2], r[3]]),
            x: u16::from_le_bytes([r[4], r[5]]),
            y: u16::from_le_bytes([r[6], r[7]]),
            w: r[8],
            h: r[9],
            x_offset: r[10] as i8,
            advance: r[11],
        }
    }

    /// Exact glyph lookup (binary search over the sorted table).
    pub fn glyph(&self, ch: char) -> Option<Glyph> {
        let cp = ch as u32;
        if cp < 256
            && let Some(index) = self.index
        {
            let i = index.slot[cp as usize];
            return (i != GlyphIndex::NONE && (i as usize) < self.glyph_count)
                .then(|| self.record(i as usize));
        }
        let (mut lo, mut hi) = (0usize, self.glyph_count);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let g = self.record(mid);
            if g.codepoint == cp {
                return Some(g);
            }
            if g.codepoint < cp {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        None
    }

    /// Glyph used to draw `ch`: itself, else `?`.
    pub fn glyph_or_fallback(&self, ch: char) -> Option<Glyph> {
        self.glyph(ch).or_else(|| self.glyph('?'))
    }

    /// Width in pixels (unscaled) of one line (no `\n` handling).
    pub fn line_width(&self, line: &str) -> u32 {
        line.chars()
            .filter_map(|c| self.glyph_or_fallback(c))
            .map(|g| g.advance as u32)
            .sum()
    }

    /// Size in pixels of `text` at `scale` (widest line × line count).
    pub fn measure(&self, text: &str, scale: f32) -> Vec2 {
        let mut width = 0u32;
        let mut lines = 0u32;
        for line in text.split('\n') {
            width = width.max(self.line_width(line));
            lines += 1;
        }
        Vec2::new(
            width as f32 * scale,
            (lines * self.line_height as u32) as f32 * scale,
        )
    }
}

/// Lay `text` out and hand each glyph to `emit` as `(atlas region,
/// top-left screen position)` — the backend-independent part of
/// [`draw`], exposed for tests and custom effects (wavy text, typewriters).
pub fn layout(
    font: &BitmapFont,
    pos: Vec2,
    text: &str,
    params: &TextParams,
    emit: &mut dyn FnMut(Rect, Vec2),
) {
    let scale = params.scale;
    let mut y = pos.y;
    for line in text.split('\n') {
        // Left-aligned text (most of it) needs no width.
        let width = || font.line_width(line) as f32 * scale;
        let mut x = match params.align {
            TextAlign::Left => pos.x,
            TextAlign::Center => pos.x - width() * 0.5,
            TextAlign::Right => pos.x - width(),
        };
        // Integer pen positions: glyphs land on whole pixels at integer
        // scales, so filtering backends (3DS bilinear) stay crisp.
        x = floor(x);
        let yy = floor(y);
        for ch in line.chars() {
            let Some(g) = font.glyph_or_fallback(ch) else {
                continue;
            };
            if g.w > 0 && g.h > 0 {
                emit(
                    Rect::new(g.x as f32, g.y as f32, g.w as f32, g.h as f32),
                    Vec2::new(x + g.x_offset as f32 * scale, yy),
                );
            }
            x += g.advance as f32 * scale;
        }
        y += font.line_height as f32 * scale;
    }
}

/// Draw `text` with the atlas `atlas` (one sprite draw per visible glyph).
/// Backends implement [`Renderer::draw_text`] by calling this with the
/// glyph table they keep for the [`FontId`].
pub fn draw(
    renderer: &mut dyn Renderer,
    atlas: SpriteId,
    font: &BitmapFont,
    pos: Vec2,
    text: &str,
    params: &TextParams,
) {
    let sprite = SpriteParams {
        scale: Vec2::new(params.scale, params.scale),
        tint: params.tint,
        ..SpriteParams::DEFAULT
    };
    layout(font, pos, text, params, &mut |region, at| {
        renderer.draw_sprite(
            atlas,
            at,
            &SpriteParams {
                region: Some(region),
                ..sprite
            },
        );
    });
}

/// The atlas sprite of a font (same numeric handle).
pub const fn atlas_of(font: FontId) -> SpriteId {
    SpriteId(font.0)
}

/// Fixed-capacity string for formatting HUD text without an allocator
/// (`core::fmt::Write`, so `write!(buf, "x{:02}", coins)` works in `no_std`
/// game code). Output past `N` bytes is truncated on a char boundary.
#[derive(Clone, Copy)]
pub struct TextBuf<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> TextBuf<N> {
    pub const fn new() -> Self {
        TextBuf {
            bytes: [0; N],
            len: 0,
        }
    }

    pub fn as_str(&self) -> &str {
        // Only whole UTF-8 chars are ever copied in (see write_str).
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl<const N: usize> Default for TextBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> core::fmt::Write for TextBuf<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for ch in s.chars() {
            let mut tmp = [0u8; 4];
            let enc = ch.encode_utf8(&mut tmp).as_bytes();
            if self.len + enc.len() > N {
                return Err(core::fmt::Error);
            }
            self.bytes[self.len..self.len + enc.len()].copy_from_slice(enc);
            self.len += enc.len();
        }
        Ok(())
    }
}

impl<const N: usize> core::fmt::Debug for TextBuf<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self.as_str(), f)
    }
}

fn floor(v: f32) -> f32 {
    let t = v as i32 as f32;
    if t > v { t - 1.0 } else { t }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test encoder (the asset pipeline owns the real writer).
    fn encode(line_height: u16, ascent: u16, glyphs: &[Glyph]) -> std::vec::Vec<u8> {
        let mut sorted = glyphs.to_vec();
        sorted.sort_by_key(|g| g.codepoint);
        let mut out = std::vec::Vec::with_capacity(HEADER + sorted.len() * RECORD);
        out.extend_from_slice(TFNT_MAGIC);
        out.extend_from_slice(&TFNT_VERSION.to_le_bytes());
        out.extend_from_slice(&line_height.to_le_bytes());
        out.extend_from_slice(&ascent.to_le_bytes());
        out.extend_from_slice(&(sorted.len() as u16).to_le_bytes());
        for g in &sorted {
            out.extend_from_slice(&g.codepoint.to_le_bytes());
            out.extend_from_slice(&g.x.to_le_bytes());
            out.extend_from_slice(&g.y.to_le_bytes());
            out.extend_from_slice(&[g.w, g.h, g.x_offset as u8, g.advance]);
        }
        out
    }

    fn g(c: char, x: u16, w: u8, advance: u8) -> Glyph {
        Glyph {
            codepoint: c as u32,
            x,
            y: 0,
            w,
            h: 8,
            x_offset: 0,
            advance,
        }
    }

    fn font_bytes() -> std::vec::Vec<u8> {
        // Deliberately unsorted: encode sorts.
        encode(
            9,
            7,
            &[
                g('b', 6, 5, 6),
                g('a', 0, 5, 6),
                g('?', 12, 5, 6),
                g(' ', 0, 0, 4),
            ],
        )
    }

    #[test]
    fn the_index_finds_the_same_glyphs_as_the_search() {
        let bytes = font_bytes();
        let plain = BitmapFont::parse(&bytes).unwrap();
        let index = GlyphIndex::build(&plain);
        let fast = plain.with_index(&index);
        for cp in 0..300u32 {
            let ch = char::from_u32(cp).unwrap();
            assert_eq!(fast.glyph(ch), plain.glyph(ch), "{cp}");
            assert_eq!(
                fast.glyph_or_fallback(ch),
                plain.glyph_or_fallback(ch),
                "{cp}"
            );
        }
        for text in ["ab?", "a\nbb", "zz b", ""] {
            assert_eq!(fast.measure(text, 2.0), plain.measure(text, 2.0));
            for align in [TextAlign::Left, TextAlign::Center, TextAlign::Right] {
                let params = TextParams {
                    align,
                    ..TextParams::DEFAULT
                };
                let mut a = std::vec::Vec::new();
                let mut b = std::vec::Vec::new();
                layout(&plain, Vec2::new(3.0, 4.0), text, &params, &mut |r, p| {
                    a.push((r, p))
                });
                layout(&fast, Vec2::new(3.0, 4.0), text, &params, &mut |r, p| {
                    b.push((r, p))
                });
                assert_eq!(a, b);
            }
        }
    }

    #[test]
    fn parse_and_lookup() {
        let bytes = font_bytes();
        let f = BitmapFont::parse(&bytes).unwrap();
        assert_eq!(f.line_height, 9);
        assert_eq!(f.glyph_count, 4);
        assert_eq!(f.glyph('b').unwrap().x, 6);
        assert!(f.glyph('z').is_none());
        assert_eq!(f.glyph_or_fallback('z').unwrap().codepoint, '?' as u32);
    }

    #[test]
    fn rejects_bad_blobs() {
        let mut bytes = font_bytes();
        assert_eq!(
            BitmapFont::parse(&bytes[..20]).unwrap_err(),
            FontError::Truncated
        );
        bytes[0] = b'X';
        assert_eq!(BitmapFont::parse(&bytes).unwrap_err(), FontError::BadMagic);
    }

    #[test]
    fn measure_multiline_and_scale() {
        let bytes = font_bytes();
        let f = BitmapFont::parse(&bytes).unwrap();
        assert_eq!(f.measure("ab", 1.0), Vec2::new(12.0, 9.0));
        assert_eq!(f.measure("a b\nb", 2.0), Vec2::new(32.0, 36.0));
    }

    #[test]
    fn layout_aligns_and_skips_empty_glyphs() {
        let bytes = font_bytes();
        let f = BitmapFont::parse(&bytes).unwrap();
        let mut out = std::vec::Vec::new();
        let params = TextParams {
            align: TextAlign::Center,
            ..TextParams::DEFAULT
        };
        layout(
            &f,
            Vec2::new(100.0, 10.0),
            "a b\nab",
            &params,
            &mut |r, p| out.push((r.pos.x, p)),
        );
        // Line 1 is 16 px wide -> starts at 92; the space emits nothing.
        assert_eq!(out[0], (0.0, Vec2::new(92.0, 10.0)));
        assert_eq!(out[1], (6.0, Vec2::new(102.0, 10.0)));
        // Line 2: 12 px wide -> 94, one line height down.
        assert_eq!(out[2], (0.0, Vec2::new(94.0, 19.0)));
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn text_buf_formats_without_alloc_and_truncates() {
        use core::fmt::Write;
        let mut b = TextBuf::<8>::new();
        write!(b, "x{:02}", 7).unwrap();
        assert_eq!(b.as_str(), "x07");
        b.clear();
        assert!(b.write_str("ação-longa").is_err());
        assert_eq!(b.as_str(), "ação-l", "stops on a char boundary");
    }

    #[test]
    fn floor_handles_negatives() {
        assert_eq!(floor(-0.5), -1.0);
        assert_eq!(floor(2.7), 2.0);
        assert_eq!(floor(-3.0), -3.0);
    }
}
