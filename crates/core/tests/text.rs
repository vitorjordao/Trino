//! Bitmap text end to end: a TFNT glyph table (built to the ADR 0005 §2
//! layout) → `BitmapFont::parse` → `text::layout` / `Renderer::draw_text`
//! on a backend that keeps the table, which issues one sprite-region draw
//! per glyph. Plus `TextBuf`, the allocator-free HUD string.

mod common;

use std::fmt::Write as _;

use common::{Recorder, glyph, sample_font, tfnt};
use trino_core::text::{self, BitmapFont, FontError};
use trino_core::{Color, FontId, Rect, Renderer, SpriteId, TextAlign, TextBuf, TextParams, Vec2};

const UI: FontId = FontId(0x0F0F);

fn drawn(r: &Recorder) -> Vec<(Rect, Vec2)> {
    r.sprites()
        .into_iter()
        .map(|(id, pos, p)| {
            assert_eq!(id, SpriteId(UI.0), "glyphs come from the font's atlas");
            (p.region.expect("every glyph draws a region"), pos)
        })
        .collect()
}

#[test]
fn tfnt_parses_and_looks_up_glyphs() {
    let bytes = sample_font();
    let f = BitmapFont::parse(&bytes).unwrap();
    assert_eq!((f.line_height, f.ascent, f.glyph_count), (9, 7, 5));
    let b = f.glyph('b').unwrap();
    assert_eq!((b.x, b.w, b.advance), (6, 5, 6));
    assert_eq!(f.glyph('j').unwrap().x_offset, -1);
    assert_eq!(f.glyph('z'), None);
    assert_eq!(f.glyph_or_fallback('z').unwrap().codepoint, '?' as u32);
    assert_eq!(f.line_width("ab"), 12);
    assert_eq!(f.line_width("a~"), 12, "unknown chars measure as `?`");
    assert_eq!(f.measure("ab\nb a", 1.0), Vec2::new(16.0, 18.0));
    assert_eq!(f.measure("", 3.0), Vec2::new(0.0, 27.0));
}

#[test]
fn tfnt_rejects_bad_tables() {
    assert_eq!(
        BitmapFont::parse(b"TFNT").unwrap_err(),
        FontError::Truncated
    );
    let good = sample_font();
    let mut bad = good.clone();
    bad[0] = b'X';
    assert_eq!(BitmapFont::parse(&bad).unwrap_err(), FontError::BadMagic);
    let mut v2 = good.clone();
    v2[4..6].copy_from_slice(&2u16.to_le_bytes());
    assert_eq!(
        BitmapFont::parse(&v2).unwrap_err(),
        FontError::UnsupportedVersion(2)
    );
    assert_eq!(
        BitmapFont::parse(&good[..good.len() - 1]).unwrap_err(),
        FontError::Truncated,
        "one record short"
    );
}

#[test]
fn draw_text_lays_out_lines_glyphs_and_alignment() {
    let mut r = Recorder::new();
    r.add_font(UI, sample_font());
    r.begin_frame(Color::BLACK);
    // Fractional pen positions snap down to whole pixels.
    r.draw_text(UI, Vec2::new(10.5, 20.0), "ab\nb a", &TextParams::DEFAULT);
    r.end_frame();
    let g = drawn(&r);
    assert_eq!(
        g,
        vec![
            (Rect::new(0.0, 0.0, 5.0, 8.0), Vec2::new(10.0, 20.0)),
            (Rect::new(6.0, 0.0, 5.0, 8.0), Vec2::new(16.0, 20.0)),
            // Second line, one line height down; the space draws nothing
            // but advances the pen by 4.
            (Rect::new(6.0, 0.0, 5.0, 8.0), Vec2::new(10.0, 29.0)),
            (Rect::new(0.0, 0.0, 5.0, 8.0), Vec2::new(20.0, 29.0)),
        ]
    );
    assert_eq!(r.stats().sprites, 4, "one sprite draw per visible glyph");

    for (align, x) in [
        (TextAlign::Left, 100.0),
        (TextAlign::Center, 94.0),
        (TextAlign::Right, 88.0),
    ] {
        r.begin_frame(Color::BLACK);
        let p = TextParams {
            align,
            ..TextParams::DEFAULT
        };
        r.draw_text(UI, Vec2::new(100.0, 0.0), "ab", &p);
        assert_eq!(drawn(&r)[0].1, Vec2::new(x, 0.0), "{align:?}");
    }
}

#[test]
fn draw_text_scales_tints_offsets_and_falls_back() {
    let mut r = Recorder::new();
    r.add_font(UI, sample_font());
    r.begin_frame(Color::BLACK);
    let red = Color::rgb(255, 0, 0);
    let p = TextParams {
        scale: 2.0,
        tint: red,
        align: TextAlign::Left,
    };
    r.draw_text(UI, Vec2::ZERO, "aj~\na", &p);
    let s = r.sprites();
    assert_eq!(s.len(), 4);
    assert!(s.iter().all(|(_, _, p)| p.tint == red));
    assert!(s.iter().all(|(_, _, p)| p.scale == Vec2::new(2.0, 2.0)));
    // `a` at 0, `j` at 12 shifted by its -1 offset (x2), `~` drawn as `?`.
    assert_eq!(s[0].1, Vec2::new(0.0, 0.0));
    assert_eq!(s[1].1, Vec2::new(10.0, 0.0));
    assert_eq!(s[2].2.region.unwrap().pos.x, 12.0);
    assert_eq!(s[2].1, Vec2::new(22.0, 0.0));
    assert_eq!(s[3].1, Vec2::new(0.0, 18.0), "line height scales too");
    assert_eq!(r.measure_text(UI, "ab\nb a", 2.0), Vec2::new(32.0, 36.0));

    // Unknown fonts draw and measure nothing.
    r.begin_frame(Color::BLACK);
    r.draw_text(FontId(1), Vec2::ZERO, "ab", &TextParams::DEFAULT);
    assert!(r.sprites().is_empty());
    assert_eq!(r.measure_text(FontId(1), "ab", 1.0), Vec2::ZERO);
}

#[test]
fn fonts_without_a_fallback_skip_unknown_chars() {
    let bytes = tfnt(8, 6, &[glyph('x', 0, 4, 5)]);
    let f = BitmapFont::parse(&bytes).unwrap();
    let mut out = Vec::new();
    text::layout(
        &f,
        Vec2::new(-3.5, 1.0),
        "x?x",
        &TextParams::DEFAULT,
        &mut |r, p| {
            out.push((r, p));
        },
    );
    // `?` is missing too: nothing drawn, no advance; negative pens floor.
    assert_eq!(
        out,
        vec![
            (Rect::new(0.0, 0.0, 4.0, 8.0), Vec2::new(-4.0, 1.0)),
            (Rect::new(0.0, 0.0, 4.0, 8.0), Vec2::new(1.0, 1.0)),
        ]
    );
    assert_eq!(f.line_width("x?x"), 10);
    assert_eq!(text::atlas_of(FontId(77)), SpriteId(77));
}

#[test]
fn textbuf_formats_without_an_allocator_and_truncates_on_char_boundaries() {
    let mut hud: TextBuf<16> = TextBuf::new();
    let (coins, label) = (7, "coins");
    write!(hud, "x{coins:02} {label}").unwrap();
    assert_eq!(hud.as_str(), "x07 coins");
    hud.clear();
    assert_eq!(hud.as_str(), "");

    let mut tiny: TextBuf<4> = TextBuf::default();
    assert!(write!(tiny, "héllo").is_err(), "overflow reports an error");
    assert_eq!(tiny.as_str(), "hél", "whole chars only, up to 4 bytes");
    assert_eq!(format!("{tiny:?}"), "\"hél\"");

    // A 2-byte char that does not fit is dropped whole.
    let mut three: TextBuf<3> = TextBuf::new();
    assert!(write!(three, "aéé").is_err());
    assert_eq!(three.as_str(), "aé");
}
