//! Bitmap fonts: a master (PNG glyph grid or TTF/OTF) → an RGBA atlas
//! (white glyphs, alpha coverage) + a `TFNT` glyph table
//! (`trino_core::text`). The atlas is baked by each platform's sprite path
//! (CI4 on N64 by default — two colors fit a 4-bit palette), the table is
//! copied verbatim.
//!
//! - **PNG grid**: row-major cells of `cell = [w, h]`, in codepoint order
//!   from `first` — or, with `chars = "..."`, cell *i* holds the *i*-th
//!   character of `chars` (any codepoints: accented letters, `¿`, `€`).
//!   `proportional = true` trims each glyph to its opaque columns
//!   (advance = width + `spacing`).
//! - **TTF/OTF**: rasterized at `size` pixels with `ab_glyph`; coverage is
//!   thresholded to hard pixels unless `antialias = true` (the N64-era
//!   look is crisp, and CI4 cannot hold coverage ramps).

use std::path::Path;

use serde::Deserialize;
use trino_core::text::{Glyph, TFNT_MAGIC, TFNT_VERSION};

/// `[fonts.<name>]` in the manifest.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FontDecl {
    /// `.png` glyph grid or `.ttf`/`.otf`, relative to `assets/shared/`.
    pub file: String,
    /// PNG grid: cell size in pixels.
    #[serde(default)]
    pub cell: Option<[u32; 2]>,
    /// PNG grid: codepoint of the first cell (default 32 = space).
    #[serde(default = "default_first")]
    pub first: u32,
    /// TTF: pixel height (line height before rounding).
    #[serde(default)]
    pub size: Option<f32>,
    /// TTF: characters to bake (default: printable ASCII). PNG grid: the
    /// character of each cell, in cell order (default: consecutive
    /// codepoints from `first`).
    #[serde(default)]
    pub chars: Option<String>,
    /// PNG grid: trim glyphs to their opaque columns (default true).
    #[serde(default = "default_true")]
    pub proportional: bool,
    /// Extra advance after each proportional glyph, pixels (default 1).
    #[serde(default = "default_spacing")]
    pub spacing: u32,
    /// PNG grid, monospaced (`proportional = false`): the advance per glyph
    /// (default: the cell width). Lets the atlas store 5-wide cells for a
    /// 6-pixel pitch — a smaller atlas that fits the N64's TMEM whole.
    #[serde(default)]
    pub advance: Option<u32>,
    /// TTF: keep coverage as alpha instead of thresholding.
    #[serde(default)]
    pub antialias: bool,
    /// Per-platform atlas texture format (N64 defaults to CI4 for fonts).
    #[serde(default)]
    pub formats: std::collections::BTreeMap<String, String>,
    /// `auto`'s quality preset, like a sprite's.
    #[serde(default)]
    pub quality: Option<String>,
    /// The least PSNR (dB) of the atlas, like a sprite's.
    #[serde(default)]
    pub min_psnr: Option<f64>,
    /// The asset groups it is in (`crate::groups`); default `["boot"]`.
    #[serde(default)]
    pub groups: Vec<String>,
    /// With `chars = "auto"`: characters baked besides the languages' own
    /// (dynamic text: names, scores). Issue #59, `crate::i18n`.
    #[serde(default)]
    pub extra_chars: Option<String>,
    /// With `chars = "auto"`: one atlas per language (`fonts/<name>@<lang>`)
    /// instead of one for all of them.
    #[serde(default)]
    pub per_language: bool,
    /// `chars` came from `"auto"` ([`crate::i18n::resolve`]).
    #[serde(skip)]
    pub auto_resolved: bool,
}

fn default_first() -> u32 {
    32
}
fn default_true() -> bool {
    true
}
fn default_spacing() -> u32 {
    1
}

/// A baked font: RGBA8 atlas + TFNT table.
#[derive(Debug, Clone, PartialEq)]
pub struct BakedFont {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub table: Vec<u8>,
    pub line_height: u16,
    pub glyphs: Vec<Glyph>,
}

/// Atlas sides stay within this (N64 `max_texture_dim` is 256).
const ATLAS_WIDTH: u32 = 256;

impl FontDecl {
    /// Atlas texture format for `platform` (fonts default to CI4 on N64;
    /// `"auto"` is resolved by `crate::texture::choose`).
    pub fn format_for(&self, logical: &str, platform: crate::Platform) -> Result<String, String> {
        if platform == crate::Platform::N64 && !self.formats.contains_key("n64") {
            return Ok("CI4".into());
        }
        self.sprite().format_for(logical, platform)
    }

    /// What `crate::texture::choose` needs for the atlas on `platform`.
    pub fn settings(&self, platform: crate::Platform) -> crate::texture::TextureSettings {
        let mut s = self.sprite().settings(platform);
        if platform == crate::Platform::N64 {
            s.default = Some("CI4");
        }
        s
    }

    fn sprite(&self) -> crate::manifest::SpriteDecl {
        crate::manifest::SpriteDecl {
            file: self.file.clone(),
            formats: self.formats.clone(),
            quality: self.quality.clone(),
            min_psnr: self.min_psnr,
            groups: self.groups.clone(),
            cvd_icon: None,
            grading: None,
        }
    }
}

pub fn bake_font(source: &Path, decl: &FontDecl) -> Result<BakedFont, String> {
    if decl.chars.as_deref() == Some("auto") {
        return Err(format!(
            "{}: chars = \"auto\" was not resolved (load the manifest with Manifest::load)",
            source.display()
        ));
    }
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => bake_grid(source, decl),
        "ttf" | "otf" => bake_ttf(source, decl),
        other => Err(format!(
            "{}: unsupported font master `.{other}` (use a .png glyph grid or .ttf/.otf)",
            source.display()
        )),
    }
}

fn bake_grid(source: &Path, decl: &FontDecl) -> Result<BakedFont, String> {
    let [cw, ch] = decl.cell.ok_or_else(|| {
        format!(
            "{}: PNG fonts need `cell = [w, h]` in the manifest",
            source.display()
        )
    })?;
    if cw == 0 || ch == 0 || cw > 255 || ch > 255 {
        return Err(format!("{}: bad cell size {cw}x{ch}", source.display()));
    }
    let (w, h, rgba) = crate::bake::decode_png(source)?;
    if w % cw != 0 || h % ch != 0 {
        return Err(format!(
            "{}: {w}x{h} is not a whole number of {cw}x{ch} cells",
            source.display()
        ));
    }
    let cols = w / cw;
    let rows = h / ch;
    let alpha = |x: u32, y: u32| rgba[((y * w + x) * 4 + 3) as usize];
    let listed: Option<Vec<char>> = decl.chars.as_ref().map(|c| c.chars().collect());
    if let Some(list) = &listed
        && list.len() as u32 > cols * rows
    {
        return Err(format!(
            "{}: `chars` lists {} characters but the grid has {} cells",
            source.display(),
            list.len(),
            cols * rows
        ));
    }
    let cells = listed.as_ref().map_or(cols * rows, |l| l.len() as u32);
    let mut glyphs = Vec::new();
    for i in 0..cells {
        let (cx, cy) = ((i % cols) * cw, (i / cols) * ch);
        let opaque: Vec<u32> = (0..cw)
            .filter(|&x| (0..ch).any(|y| alpha(cx + x, cy + y) > 0))
            .collect();
        let cp = match &listed {
            Some(list) => list[i as usize] as u32,
            None => decl.first + i,
        };
        let glyph = if !decl.proportional {
            Glyph {
                codepoint: cp,
                x: cx as u16,
                y: cy as u16,
                w: if opaque.is_empty() { 0 } else { cw as u8 },
                h: ch as u8,
                x_offset: 0,
                advance: decl.advance.unwrap_or(cw).min(255) as u8,
            }
        } else if let (Some(&l), Some(&r)) = (opaque.first(), opaque.last()) {
            Glyph {
                codepoint: cp,
                x: (cx + l) as u16,
                y: cy as u16,
                w: (r - l + 1) as u8,
                h: ch as u8,
                x_offset: 0,
                advance: (r - l + 1 + decl.spacing).min(255) as u8,
            }
        } else if cp == ' ' as u32 {
            // An empty cell is only meaningful as whitespace.
            Glyph {
                codepoint: cp,
                x: cx as u16,
                y: cy as u16,
                w: 0,
                h: 0,
                x_offset: 0,
                advance: (cw / 2).max(2) as u8,
            }
        } else {
            continue;
        };
        glyphs.push(glyph);
    }
    // Atlas = the master itself, recolored to white (tint does the color).
    let rgba = whiten(&rgba);
    finish(w, h, rgba, ch as u16, ch as u16, glyphs)
}

fn bake_ttf(source: &Path, decl: &FontDecl) -> Result<BakedFont, String> {
    use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
    let size = decl.size.ok_or_else(|| {
        format!(
            "{}: TTF fonts need `size = <pixels>` in the manifest",
            source.display()
        )
    })?;
    let bytes = std::fs::read(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let font = FontVec::try_from_vec(bytes).map_err(|e| format!("{}: {e}", source.display()))?;
    let scaled = font.as_scaled(PxScale::from(size));
    let ascent = scaled.ascent().ceil();
    let line_height = (scaled.ascent() - scaled.descent()).ceil().max(1.0) as u32;
    if line_height > 255 {
        return Err(format!("{}: size {size} is too large", source.display()));
    }
    let chars: Vec<char> = match &decl.chars {
        Some(s) => {
            let mut v: Vec<char> = s.chars().collect();
            v.sort();
            v.dedup();
            v
        }
        None => (32u8..=126).map(char::from).collect(),
    };

    // Rasterize every glyph into a cell `line_height` tall whose top is the
    // ascender line, then shelf-pack the cells.
    struct Cell {
        cp: u32,
        w: u32,
        x_offset: i32,
        advance: u32,
        pixels: Vec<u8>, // alpha, w * line_height
    }
    let mut cells = Vec::new();
    for c in chars {
        let id = font.glyph_id(c);
        if id.0 == 0 && c != ' ' {
            continue; // not in the font
        }
        let advance = scaled.h_advance(id).round().max(0.0) as u32;
        let glyph = id.with_scale_and_position(PxScale::from(size), ab_glyph::point(0.0, ascent));
        let Some(outline) = font.outline_glyph(glyph) else {
            cells.push(Cell {
                cp: c as u32,
                w: 0,
                x_offset: 0,
                advance,
                pixels: Vec::new(),
            });
            continue;
        };
        let b = outline.px_bounds();
        let (x0, y0) = (b.min.x.floor() as i32, b.min.y.floor() as i32);
        let w = (b.max.x.ceil() as i32 - x0).max(1) as u32;
        let mut pixels = vec![0u8; (w * line_height) as usize];
        outline.draw(|gx, gy, cov| {
            let y = y0 + gy as i32;
            if y < 0 || y >= line_height as i32 || gx >= w {
                return;
            }
            let a = if decl.antialias {
                (cov.clamp(0.0, 1.0) * 255.0).round() as u8
            } else if cov >= 0.5 {
                255
            } else {
                0
            };
            let at = (y as u32 * w + gx) as usize;
            pixels[at] = pixels[at].max(a);
        });
        cells.push(Cell {
            cp: c as u32,
            w,
            x_offset: x0,
            advance,
            pixels,
        });
    }

    let widths: Vec<u32> = cells.iter().map(|c| c.w).collect();
    let (width, height, placed) = pack_shelves(&widths, line_height);
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let mut glyphs = Vec::new();
    for (cell, &(px, py)) in cells.iter().zip(&placed) {
        for yy in 0..line_height {
            for xx in 0..cell.w {
                let a = cell.pixels[(yy * cell.w + xx) as usize];
                let at = (((py + yy) * width + px + xx) * 4) as usize;
                // White ink, transparent texels black (see `whiten`).
                let c = if a > 0 { 255 } else { 0 };
                rgba[at..at + 4].copy_from_slice(&[c, c, c, a]);
            }
        }
        if cell.x_offset.unsigned_abs() > 127 || cell.advance > 255 {
            return Err(format!(
                "{}: glyph U+{:04X} metrics out of range at size {size}",
                source.display(),
                cell.cp
            ));
        }
        glyphs.push(Glyph {
            codepoint: cell.cp,
            x: px as u16,
            y: py as u16,
            w: cell.w as u8,
            h: if cell.w == 0 { 0 } else { line_height as u8 },
            x_offset: cell.x_offset as i8,
            advance: cell.advance as u8,
        });
    }
    finish(
        width,
        height,
        rgba,
        line_height as u16,
        ascent as u16,
        glyphs,
    )
}

/// Atlas widths [`pack_shelves`] tries: whole 16-texel steps, so an I4
/// row is a whole number of the N64's 8-byte TMEM lines.
const WIDTH_STEP: u32 = 16;

/// Pack glyph cells `line_height` tall and `widths` wide into shelves:
/// the atlas width (a multiple of [`WIDTH_STEP`] up to [`ATLAS_WIDTH`])
/// with the fewest texels and a height within [`ATLAS_WIDTH`] too, the widest cells first into the first shelf
/// with room. The height is the shelves', rounded up to 4 texels (BCn and
/// KTX2 blocks), not to a power of two: the N64 clamps text rectangles and
/// the 3DS's `tex3ds` pads the texture itself. Returns the width, the
/// height and each cell's top-left corner, in `widths`' order.
///
/// Before GitHub #99 the shelves were always 256 wide, filled in codepoint
/// order, and the height went up to a power of two: 61 glyphs of 11 px
/// Roboto Condensed (13 px lines) needed a second shelf, and 26 lines
/// became 32 — 4 KB of I4, the N64's whole TMEM.
pub fn pack_shelves(widths: &[u32], line_height: u32) -> (u32, u32, Vec<(u32, u32)>) {
    let widest = widths.iter().copied().max().unwrap_or(0);
    let mut order: Vec<usize> = (0..widths.len()).collect();
    // Widest first; equal widths keep their order (deterministic).
    order.sort_by_key(|&i| core::cmp::Reverse(widths[i]));
    let pack = |width: u32| {
        let mut shelves: Vec<u32> = Vec::new(); // used width of each shelf
        let mut placed = vec![(0, 0); widths.len()];
        for &i in &order {
            let w = widths[i];
            let shelf = match shelves.iter().position(|&used| used + w <= width) {
                Some(s) => s,
                None => {
                    shelves.push(0);
                    shelves.len() - 1
                }
            };
            placed[i] = (shelves[shelf], shelf as u32 * line_height);
            shelves[shelf] += w;
        }
        let height = (shelves.len().max(1) as u32 * line_height).next_multiple_of(4);
        (width, height, placed)
    };
    let first = widest.next_multiple_of(WIDTH_STEP).max(WIDTH_STEP);
    let last = ATLAS_WIDTH.max(first);
    (first..=last)
        .step_by(WIDTH_STEP as usize)
        .map(pack)
        // No taller than the N64's largest texture side either: a 16×332
        // atlas drew the glyphs below row 256 wrong there (the football
        // template's clock colon). A font too big for 256×256 keeps the
        // widest atlas, as before.
        .filter(|(w, h, _)| *h <= ATLAS_WIDTH || *w == last)
        // Fewest texels; on a tie the wider atlas (fewer shelves).
        .min_by_key(|(w, h, _)| (w * h, core::cmp::Reverse(*w)))
        .expect("the widest atlas is always a candidate")
}

/// Make every pixel white, keeping alpha (tint colors text at draw time).
/// White ink, coverage in alpha — and transparent texels **black**: the
/// N64's intensity formats (I4/I8) derive alpha from RGB, so a white but
/// transparent background would turn every glyph cell into a solid block.
fn whiten(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[_, _, _, a]| {
            let c = if a > 0 { 255 } else { 0 };
            [c, c, c, a]
        })
        .collect()
}

fn finish(
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    line_height: u16,
    ascent: u16,
    mut glyphs: Vec<Glyph>,
) -> Result<BakedFont, String> {
    glyphs.sort_by_key(|g| g.codepoint);
    glyphs.dedup_by_key(|g| g.codepoint);
    if glyphs.len() > u16::MAX as usize {
        return Err("font has too many glyphs".into());
    }
    let table = encode_tfnt(line_height, ascent, &glyphs);
    Ok(BakedFont {
        width,
        height,
        rgba,
        table,
        line_height,
        glyphs,
    })
}

/// Write a TFNT glyph table (format in `trino_core::text`). `glyphs` must be
/// sorted by codepoint.
pub fn encode_tfnt(line_height: u16, ascent: u16, glyphs: &[Glyph]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + glyphs.len() * 12);
    out.extend_from_slice(TFNT_MAGIC);
    out.extend_from_slice(&TFNT_VERSION.to_le_bytes());
    out.extend_from_slice(&line_height.to_le_bytes());
    out.extend_from_slice(&ascent.to_le_bytes());
    out.extend_from_slice(&(glyphs.len() as u16).to_le_bytes());
    for g in glyphs {
        out.extend_from_slice(&g.codepoint.to_le_bytes());
        out.extend_from_slice(&g.x.to_le_bytes());
        out.extend_from_slice(&g.y.to_le_bytes());
        out.extend_from_slice(&[g.w, g.h, g.x_offset as u8, g.advance]);
    }
    out
}

/// Encode RGBA8 as a PNG (console bakers feed atlases to mksprite/tex3ds).
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::text::BitmapFont;

    fn decl(toml_src: &str) -> FontDecl {
        toml::from_str(toml_src).unwrap()
    }

    /// 3 cells of 4x2: ' ' (empty), '!' (one column), '"' (two columns).
    fn grid_png(dir: &Path) -> std::path::PathBuf {
        let (w, h) = (12u32, 2u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let mut set = |x: u32, y: u32| {
            let at = ((y * w + x) * 4) as usize;
            rgba[at..at + 4].copy_from_slice(&[10, 20, 30, 255]);
        };
        set(5, 0);
        set(5, 1);
        set(9, 0);
        set(10, 1);
        let path = dir.join("grid.png");
        std::fs::write(&path, encode_png(w, h, &rgba).unwrap()).unwrap();
        path
    }

    #[test]
    fn monospaced_grid_can_advance_past_its_cell() {
        let dir = tempfile::tempdir().unwrap();
        let path = grid_png(dir.path());
        let baked = bake_font(
            &path,
            &decl("file = \"x.png\"\ncell = [4, 2]\nproportional = false\nadvance = 6\n"),
        )
        .unwrap();
        let font = BitmapFont::parse(&baked.table).unwrap();
        let bang = font.glyph('!').unwrap();
        assert_eq!((bang.w, bang.advance), (4, 6));
    }

    #[test]
    fn grid_font_with_listed_chars_maps_any_codepoint() {
        let dir = tempfile::tempdir().unwrap();
        let path = grid_png(dir.path());
        let baked = bake_font(
            &path,
            &decl("file = \"x.png\"\ncell = [4, 2]\nchars = \" çÃ\"\n"),
        )
        .unwrap();
        let font = BitmapFont::parse(&baked.table).unwrap();
        assert_eq!(font.glyph('ç').unwrap().x, 5);
        assert_eq!(font.glyph('Ã').unwrap().x, 9);
        assert!(
            font.glyph('!').is_none(),
            "cells follow `chars`, not `first`"
        );
        // More characters than cells is an authoring error.
        let err = bake_font(
            &path,
            &decl("file = \"x.png\"\ncell = [4, 2]\nchars = \"abcd\"\n"),
        )
        .unwrap_err();
        assert!(err.contains("4 characters"), "{err}");
    }

    #[test]
    fn grid_font_trims_and_round_trips_through_core() {
        let dir = tempfile::tempdir().unwrap();
        let path = grid_png(dir.path());
        let baked = bake_font(&path, &decl("file = \"x.png\"\ncell = [4, 2]\n")).unwrap();
        let font = BitmapFont::parse(&baked.table).unwrap();
        assert_eq!(font.line_height, 2);
        let space = font.glyph(' ').unwrap();
        assert_eq!((space.w, space.advance), (0, 2));
        let bang = font.glyph('!').unwrap();
        assert_eq!((bang.x, bang.w, bang.advance), (5, 1, 2));
        let quote = font.glyph('"').unwrap();
        assert_eq!((quote.x, quote.w, quote.advance), (9, 2, 3));
        // Atlas is whitened, alpha kept; transparent texels are black
        // (intensity formats take alpha from RGB).
        let at = 5 * 4; // pixel (5, 0)
        assert_eq!(&baked.rgba[at..at + 4], &[255, 255, 255, 255]);
        assert_eq!(&baked.rgba[0..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn monospace_grid_keeps_cells() {
        let dir = tempfile::tempdir().unwrap();
        let path = grid_png(dir.path());
        let baked = bake_font(
            &path,
            &decl("file = \"x.png\"\ncell = [4, 2]\nproportional = false\n"),
        )
        .unwrap();
        let font = BitmapFont::parse(&baked.table).unwrap();
        assert_eq!(font.glyph('!').unwrap().x, 4);
        assert_eq!(font.glyph('!').unwrap().advance, 4);
        assert_eq!(font.glyph(' ').unwrap().w, 0);
    }

    #[test]
    fn grid_errors_are_actionable() {
        let dir = tempfile::tempdir().unwrap();
        let path = grid_png(dir.path());
        let err = bake_font(&path, &decl("file = \"x.png\"\n")).unwrap_err();
        assert!(err.contains("cell"), "{err}");
        let err = bake_font(&path, &decl("file = \"x.png\"\ncell = [5, 2]\n")).unwrap_err();
        assert!(err.contains("whole number"), "{err}");
    }

    #[test]
    fn ttf_font_bakes_ascii() {
        let ttf = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../editor/assets/fonts/JetBrainsMono-Regular.ttf");
        let baked = bake_font(&ttf, &decl("file = \"x.ttf\"\nsize = 12\n")).unwrap();
        let font = BitmapFont::parse(&baked.table).unwrap();
        assert_eq!(font.glyph_count, 95);
        let a = font.glyph('A').unwrap();
        assert!(a.w > 0 && a.advance >= a.w, "{a:?}");
        assert_eq!(font.glyph(' ').unwrap().w, 0);
        assert!(
            baked.width.is_multiple_of(16) && baked.width <= 256,
            "{}",
            baked.width
        );
        assert_eq!(baked.height % 4, 0);
        // 95 ASCII glyphs in less than the old 256-wide shelves' power of
        // two (256x64 at this size).
        assert!(
            baked.width * baked.height < 256 * 64,
            "{}x{}",
            baked.width,
            baked.height
        );
        // Thresholded: alpha is 0 or 255 only.
        assert!(
            baked
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[3] == 0 || p[3] == 255)
        );
        // Transparent texels are black, as in PNG fonts: the N64's I4/I8
        // take alpha from intensity (a white background drew solid blocks).
        for aa in [false, true] {
            let d = if aa {
                "file = \"x.ttf\"\nsize = 12\nantialias = true\n"
            } else {
                "file = \"x.ttf\"\nsize = 12\n"
            };
            let baked = bake_font(&ttf, &decl(d)).unwrap();
            for p in baked.rgba.as_chunks::<4>().0 {
                if p[3] == 0 {
                    assert_eq!(p, &[0, 0, 0, 0], "antialias {aa}");
                } else {
                    assert_eq!(&p[..3], &[255, 255, 255], "antialias {aa}");
                }
            }
        }
    }

    /// The shelf packer (GitHub #99): every cell inside the atlas, none
    /// overlapping, the fewest texels among the widths it may pick.
    #[test]
    fn shelves_pack_tight_and_apart() {
        // castle64's pt-BR glyph widths are ~2..10 px on 13 px lines.
        let widths: Vec<u32> = (0..61).map(|i| 2 + (i * 7) % 9).collect();
        let (w, h, at) = pack_shelves(&widths, 13);
        assert!(
            w.is_multiple_of(16) && w <= 256 && h.is_multiple_of(4),
            "{w}x{h}"
        );
        let total: u32 = widths.iter().map(|w| w * 13).sum();
        assert!(w * h >= total);
        // The old packer: 256 wide, rows in order, a power of two tall.
        assert!(w * h < 256 * 32, "{w}x{h}");
        for (i, (&(x, y), &cw)) in at.iter().zip(&widths).enumerate() {
            assert!(x + cw <= w && y + 13 <= h, "cell {i} at {x},{y}");
            for (&(x2, y2), &cw2) in at[i + 1..].iter().zip(&widths[i + 1..]) {
                assert!(y != y2 || x + cw <= x2 || x2 + cw2 <= x, "cells overlap");
            }
        }
        // Deterministic, and a glyph wider than the narrowest width fits.
        assert_eq!(pack_shelves(&widths, 13).2, at);
        let (w, _, _) = pack_shelves(&[40, 3], 10);
        assert!(w >= 48);
        // Nothing to pack: one empty shelf.
        assert_eq!(pack_shelves(&[], 10).1, 12);
        // Never taller than the N64's 256-texel side when a wider atlas
        // fits: 120 narrow glyphs on 26 px lines would be 16 wide and
        // ~1 500 tall by area alone.
        let narrow = vec![5u32; 120];
        let (w, h, _) = pack_shelves(&narrow, 26);
        assert!(h <= 256, "{w}x{h}");
    }

    #[test]
    fn n64_defaults_fonts_to_ci4() {
        let d = decl("file = \"x.png\"\ncell = [4, 2]\n");
        assert_eq!(
            d.format_for("fonts/x", crate::Platform::N64).unwrap(),
            "CI4"
        );
        assert_eq!(
            d.format_for("fonts/x", crate::Platform::Pc).unwrap(),
            "auto"
        );
        // A default, not a declaration: `choose` reports it as such.
        let s = d.settings(crate::Platform::N64);
        assert_eq!((s.format, s.default), (None, Some("CI4")));
    }
}
