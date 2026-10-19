//! Localization evidence (issue #59), not a gate: the engine's tables drawn
//! in each language with the font atlas that language bakes, on the N64
//! look, the 3DS profile and the PC.
//!
//! ```sh
//! cargo test -p xtask --test i18n_shots -- --ignored --nocapture
//! ```
//!
//! Writes `target/evidence/i18n/languages_<profile>.png`.

use std::path::PathBuf;

use trino_core::i18n::{Arg, LangId, Localizer, StrKey};
use trino_core::{Color, FontId, Platform, Renderer, TextBuf, TextParams, Vec2};
use trino_platform_pc::{PcPlatform, PcRenderer, SimProfile};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

#[test]
#[ignore = "local tool: screenshots (needs a GPU)"]
fn i18n_shots() {
    let out = root().join("target/evidence/i18n");
    std::fs::create_dir_all(&out).unwrap();
    let baked = tempfile::tempdir().unwrap();
    trino_asset_pipeline::bake_all(
        &root().join("assets"),
        trino_asset_pipeline::Platform::Pc,
        baked.path(),
    )
    .unwrap();
    for (profile, tag) in [
        (SimProfile::N64, "n64"),
        (SimProfile::N3ds, "3ds"),
        (SimProfile::Native, "pc"),
    ] {
        let assets = trino_asset_pipeline::load_dir(baked.path(), None).unwrap();
        let mut r = pollster::block_on(PcRenderer::new_headless(profile)).expect("a GPU");
        if profile == SimProfile::Native {
            r.set_internal_size(640, 360);
        }
        let mut platform = PcPlatform::new(&profile.caps());
        for s in assets.sprites {
            r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
        }
        for f in assets.fonts {
            r.upload_font(trino_core::FontId(f.id), f.table);
        }
        for b in assets.blobs {
            platform.insert_blob(trino_core::BlobId(b.id), b.bytes);
        }
        let mut loc: Localizer<'static, 4> = Localizer::new();
        assert!(loc.load(&platform) >= 3);
        let base = FontId::from_path("fonts/i18n");
        let label = FontId::from_path("fonts/trino5x7");
        r.begin_frame(Color::rgb(18, 22, 40));
        let mut y = 6.0;
        let mut buf = TextBuf::<64>::new();
        for lang in ["en-US", "pt-BR", "es-ES"] {
            loc.set_language(LangId::new(lang));
            let font = loc.font(base);
            let name = loc.table(LangId::new(lang)).unwrap().name();
            let atlas = if font == base { "fonts/i18n" } else { lang };
            let mut head = TextBuf::<64>::new();
            let _ = core::fmt::Write::write_fmt(&mut head, format_args!("{lang}  atlas {atlas}"));
            r.draw_text(
                label,
                Vec2::new(6.0, y),
                head.as_str(),
                &TextParams::DEFAULT,
            );
            y += 11.0;
            let yellow = TextParams {
                tint: Color::rgb(255, 214, 90),
                ..TextParams::DEFAULT
            };
            r.draw_text(font, Vec2::new(6.0, y), name, &yellow);
            y += 13.0;
            let lines = [
                loc.tr(StrKey::new("selftest.hello")).to_string(),
                loc.tr_fmt(
                    StrKey::new("selftest.price"),
                    &[Arg::money("amount", 120_000)],
                    &mut buf,
                )
                .to_string(),
                [0, 1, 2]
                    .map(|n| {
                        loc.tr_fmt(StrKey::new("selftest.coins"), &[Arg::int("n", n)], &mut buf)
                            .to_string()
                    })
                    .join(", "),
                loc.tr(StrKey::new("selftest.only_en")).to_string(),
            ];
            for l in lines {
                r.draw_text(font, Vec2::new(14.0, y), &l, &TextParams::DEFAULT);
                y += 13.0;
            }
            y += 4.0;
        }
        r.end_frame();
        let (w, h) = r.internal_size();
        let rgba = r.read_offscreen();
        let path = out.join(format!("languages_{tag}.png"));
        let file = std::fs::File::create(&path).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&rgba).unwrap();
        eprintln!("screenshot: {}", path.display());
        let _ = platform.system_language();
    }
}
