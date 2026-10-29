//! Bitmap fonts end to end: a TTF master (the editor's bundled JetBrains
//! Mono, read-only) and PNG glyph grids → `bake_all` → atlas sprite +
//! `TFNT` table → `trino_core::text` measuring and laying out text.

mod common;

use std::path::{Path, PathBuf};

use common::*;
use trino_asset_pipeline::{LoadedAssets, Platform, bake_all, load_dir};
use trino_core::text::{BitmapFont, layout};
use trino_core::{Color, Rect, TextAlign, TextParams, Vec2, asset_id};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Bake `manifest` (with masters already in `assets/shared`) for PC.
fn bake(assets: &Path, manifest: &str) -> LoadedAssets {
    std::fs::write(assets.join("manifest.toml"), manifest).unwrap();
    let out = assets.join("../out");
    bake_all(assets, Platform::Pc, &out).unwrap_or_else(|e| panic!("{e}"));
    load_dir(&out, None).unwrap()
}

fn ttf_project(dir: &Path) -> PathBuf {
    let assets = dir.join("assets");
    std::fs::create_dir_all(assets.join("shared/fonts")).unwrap();
    std::fs::copy(
        repo().join("crates/editor/assets/fonts/JetBrainsMono-Regular.ttf"),
        assets.join("shared/fonts/mono.ttf"),
    )
    .unwrap();
    assets
}

const TTF_MANIFEST: &str = r#"version = 1
[fonts.mono]
file = "fonts/mono.ttf"
size = 12.0
chars = "Hi é?H\U0001F600"

[fonts.soft]
file = "fonts/mono.ttf"
size = 12.0
chars = "Hi"
antialias = true
formats = { n64 = "I8" }
"#;

#[test]
fn ttf_masters_rasterize_to_an_atlas_and_a_glyph_table() {
    let dir = tempfile::tempdir().unwrap();
    let assets = ttf_project(dir.path());
    let loaded = bake(&assets, TTF_MANIFEST);
    let table = |name: &str| {
        let id = asset_id(&format!("fonts/{name}"));
        let font = loaded.fonts.iter().find(|f| f.id == id).unwrap();
        let atlas = loaded.sprites.iter().find(|s| s.id == id).unwrap();
        (font.table.clone(), atlas)
    };
    let (mono_table, mono_atlas) = table("mono");
    let mono = BitmapFont::parse(&mono_table).unwrap();
    // Duplicates collapse; a character the font lacks (the emoji) is skipped.
    assert_eq!(mono.glyph_count, 5, "space, ?, H, i, é");
    assert!(mono.glyph('\u{1F600}').is_none());
    assert!(mono.line_height >= 12 && mono.line_height < 24);
    assert!(mono.ascent > 0 && mono.ascent <= mono.line_height);
    let (h, i, e, space) = (
        mono.glyph('H').unwrap(),
        mono.glyph('i').unwrap(),
        mono.glyph('é').unwrap(),
        mono.glyph(' ').unwrap(),
    );
    // A monospaced face: one advance for every glyph.
    assert_eq!(h.advance, i.advance);
    assert_eq!(e.advance, h.advance);
    assert!(h.w > 0 && h.h as u16 == mono.line_height);
    assert_eq!((space.w, space.h), (0, 0), "whitespace has no pixels");
    assert_eq!(space.advance, h.advance);

    // The atlas: packed tight (whole 16-texel widths up to 256, heights in
    // 4-texel steps: GitHub #99), white ink; thresholded alpha.
    assert!(mono_atlas.width.is_multiple_of(16) && mono_atlas.width <= 256);
    assert_eq!(mono_atlas.height % 4, 0);
    // No two glyph boxes share a texel.
    let boxes: Vec<_> = [h, i, e]
        .iter()
        .map(|g| (g.x as u32, g.y as u32, g.w as u32, g.h as u32))
        .collect();
    for (k, a) in boxes.iter().enumerate() {
        for b in &boxes[k + 1..] {
            let apart =
                a.0 + a.2 <= b.0 || b.0 + b.2 <= a.0 || a.1 + a.3 <= b.1 || b.1 + b.3 <= a.1;
            assert!(apart, "{a:?} overlaps {b:?}");
        }
    }
    let alphas = |rgba: &[u8]| -> Vec<u8> { rgba.iter().skip(3).step_by(4).copied().collect() };
    let hard = alphas(&mono_atlas.rgba);
    assert!(hard.iter().all(|&a| a == 0 || a == 255));
    assert!(hard.contains(&255));
    // The H glyph's box holds ink.
    let ink_in = |atlas: &trino_asset_pipeline::LoadedSprite, g: trino_core::text::Glyph| {
        (g.y as u32..g.y as u32 + g.h as u32).any(|y| {
            (g.x as u32..g.x as u32 + g.w as u32)
                .any(|x| atlas.rgba[((y * atlas.width + x) * 4 + 3) as usize] > 0)
        })
    };
    assert!(ink_in(mono_atlas, h) && ink_in(mono_atlas, e));

    // Antialiased: coverage ramps survive.
    let (_, soft_atlas) = table("soft");
    assert!(alphas(&soft_atlas.rgba).iter().any(|&a| a > 0 && a < 255));

    // Measure and lay out with the core's text module.
    let adv = h.advance as f32;
    let lh = mono.line_height as f32;
    assert_eq!(mono.measure("Hi\nH", 2.0), Vec2::new(4.0 * adv, 4.0 * lh));
    assert_eq!(mono.line_width("Hié"), 3 * h.advance as u32);
    // Unknown chars measure (and draw) as `?`.
    assert_eq!(
        mono.line_width("Z"),
        mono.glyph('?').unwrap().advance as u32
    );
    let pens = |align: TextAlign| {
        let mut at = Vec::new();
        let params = TextParams {
            align,
            ..TextParams::tinted(Color::WHITE)
        };
        layout(
            &mono,
            Vec2::new(100.0, 10.0),
            "Hi H",
            &params,
            &mut |_: Rect, p: Vec2| at.push(p),
        );
        at
    };
    let left = pens(TextAlign::Left);
    assert_eq!(left.len(), 3, "the space advances but draws nothing");
    assert_eq!(left[0].y, 10.0);
    // Pen positions advance by whole glyphs; each glyph sits at its offset.
    let off = |g: trino_core::text::Glyph| g.x_offset as f32;
    assert_eq!(left[0].x, 100.0 + off(h));
    assert_eq!(left[1].x, 100.0 + adv + off(i));
    assert_eq!(left[2].x, 100.0 + 3.0 * adv + off(h));
    let right = pens(TextAlign::Right);
    assert_eq!(right[0].x - left[0].x, -4.0 * adv);
    let center = pens(TextAlign::Center);
    assert_eq!(center[0].x - left[0].x, -2.0 * adv);

    // N64 atlas formats: fonts default to CI4, a declared one wins.
    let report = bake_all(&assets, Platform::N64, &dir.path().join("n64")).unwrap();
    let fmt = |logical: &str| {
        report
            .entries
            .iter()
            .find(|e| e.logical == logical && e.kind == "sprite")
            .unwrap()
            .format
            .clone()
    };
    assert_eq!(fmt("fonts/mono"), "CI4");
    assert_eq!(fmt("fonts/soft"), "I8");
}

#[test]
fn ttf_errors_name_the_problem() {
    let dir = tempfile::tempdir().unwrap();
    let assets = ttf_project(dir.path());
    std::fs::write(assets.join("shared/fonts/fake.ttf"), b"definitely not sfnt").unwrap();
    std::fs::write(
        assets.join("manifest.toml"),
        r#"version = 1
[fonts.huge]
file = "fonts/mono.ttf"
size = 400.0
[fonts.fake]
file = "fonts/fake.ttf"
size = 8.0
"#,
    )
    .unwrap();
    let err = bake_all(&assets, Platform::Pc, &dir.path().join("out")).unwrap_err();
    assert_eq!(err.0.len(), 2, "{err}");
    let text = err.to_string();
    assert!(text.contains("size 400 is too large"), "{text}");
    assert!(text.contains("font `fonts/fake`"), "{text}");
}

#[test]
fn png_grids_trim_proportional_glyphs_and_keep_monospaced_cells() {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path().join("assets");
    // Four 4x4 cells from ' ' (32): empty, one column at x = 1, two
    // columns at x = 1..=2, empty.
    write_png_with(
        &assets.join("shared/fonts/grid.png"),
        16,
        4,
        |x, _| match (x / 4, x % 4) {
            (1, 1) | (2, 1) | (2, 2) => [9, 9, 9, 255],
            _ => [0, 0, 0, 0],
        },
    );
    let loaded = bake(
        &assets,
        r#"version = 1
[fonts.prop]
file = "fonts/grid.png"
cell = [4, 4]
spacing = 2

[fonts.mono]
file = "fonts/grid.png"
cell = [4, 4]
first = 97
proportional = false
"#,
    );
    let font = |name: &str| {
        let id = asset_id(&format!("fonts/{name}"));
        let f = loaded.fonts.iter().find(|f| f.id == id).unwrap();
        BitmapFont::parse(Box::leak(f.table.clone().into_boxed_slice())).unwrap()
    };
    let prop = font("prop");
    // ' ' (empty) is whitespace: half a cell; '"' (32 + 3, empty) is dropped.
    assert_eq!(prop.glyph_count, 3);
    let space = prop.glyph(' ').unwrap();
    assert_eq!((space.w, space.advance), (0, 2));
    let bang = prop.glyph('!').unwrap();
    assert_eq!(
        (bang.x, bang.w, bang.advance),
        (5, 1, 3),
        "trimmed + spacing"
    );
    let quote = prop.glyph('"').unwrap();
    assert_eq!((quote.x, quote.w, quote.advance), (9, 2, 4));
    assert!(prop.glyph('#').is_none());
    assert_eq!(prop.line_height, 4);

    // Monospaced from 'a': every cell keeps its box and the cell advance;
    // empty cells keep their slot with no pixels.
    let mono = font("mono");
    assert_eq!(mono.glyph_count, 4);
    let a = mono.glyph('a').unwrap();
    let b = mono.glyph('b').unwrap();
    assert_eq!((a.w, a.advance), (0, 4));
    assert_eq!((b.x, b.w, b.h, b.advance), (4, 4, 4, 4));

    // Cell sizes the format cannot hold are rejected.
    for cell in ["[0, 4]", "[300, 4]"] {
        std::fs::write(
            assets.join("manifest.toml"),
            format!("version = 1\n[fonts.bad]\nfile = \"fonts/grid.png\"\ncell = {cell}\n"),
        )
        .unwrap();
        let err = bake_all(&assets, Platform::Pc, &dir.path().join("bad")).unwrap_err();
        assert!(err.to_string().contains("bad cell size"), "{err}");
    }
}
