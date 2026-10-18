//! PC "N64 look" vs the real thing: the showcase's first frame rendered by
//! the PC backend (N64 profile, 16-bit dither, VI pass off because the
//! console dump is read from RDRAM, before the VI) is compared with the
//! frame ares produced from the ROM (`tests/golden/n64/showcase.png`,
//! written by `cargo xtask test n64 --bless`). Different rasterizers, so
//! not pixel-exact — the test bounds how far the emulation may drift.
//! GPU-gated like the goldens.

use std::path::PathBuf;

use trino_core::Game;
use trino_platform_pc::{PcRenderer, SimProfile};

fn read_png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
    let mut dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

#[test]
fn pc_n64_look_tracks_the_ares_frame() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let golden = root.join("tests/golden/n64/showcase.png");
    let baked = root.join("target/assets/pc");
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    let mut r = match pollster::block_on(PcRenderer::new_headless(SimProfile::N64)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            return;
        }
    };
    r.set_n64_look(true);
    r.set_vi_dedither(false);
    for s in assets.sprites {
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for m in assets.models {
        r.upload_mesh(trino_core::ModelId(m.id), m.tmdl);
    }
    for f in assets.fonts {
        r.upload_font(trino_core::FontId(f.id), f.table);
    }
    // The scene the ROM plays: its N64 variants.
    let mut game = showcase::ShowcaseGame::n64();
    game.render(&mut r);
    let pc = r.read_offscreen();
    let (w, h, ares) = read_png(&golden);
    assert_eq!((w, h), r.internal_size());

    // Per pixel: the largest channel difference.
    let diffs: Vec<u8> = pc
        .chunks(4)
        .zip(ares.chunks(4))
        .map(|(a, b)| (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap())
        .collect();
    if std::env::var_os("TRINO_PARITY_DUMP").is_some() {
        let out = root.join("tests/golden/n64/actual/pc_look.png");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        let mut enc = png::Encoder::new(std::fs::File::create(&out).unwrap(), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&pc).unwrap();
    }
    let n = diffs.len() as f32;
    let mean = diffs.iter().map(|&d| d as f32).sum::<f32>() / n;
    // Within two 5-bit steps; the rest are triangle edges (different
    // rasterizers) — measured 2.5 mean / 97.9% when written.
    let close = diffs.iter().filter(|&&d| d <= 17).count() as f32 / n;
    eprintln!(
        "pc vs ares: mean max-channel diff {mean:.2}, {:.1}% within 2 steps",
        close * 100.0
    );
    assert!(mean < 4.0, "mean diff {mean:.2}");
    assert!(
        close > 0.95,
        "only {:.1}% of pixels within 2 steps",
        close * 100.0
    );
}
