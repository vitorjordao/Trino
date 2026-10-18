//! The showcase's river on the PC's native profile (ADR 0015): the
//! `showcase-river` scene (the scene camera, the hero wading in, the robot
//! on its patrol) against `tests/golden/pc/showcase_river.png`, the same
//! state the N64 and 3DS goldens dump (`showcase_river.png` under
//! `tests/golden/n64` and `tests/golden/3ds`). Per-pixel water: normals,
//! Fresnel, glint, depth foam, ripples. GPU-gated like the other goldens;
//! `TRINO_BLESS=1` rewrites it. The frame's GPU T&L draws are also held to
//! the CPU path (`tnl_oracle`, issue #51).

use std::path::PathBuf;

use trino_core::{Game, Vec2};
use trino_platform_pc::{PcRenderer, SimProfile};

struct NullAudio;
impl trino_core::Audio for NullAudio {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

fn read_png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
    let mut dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(rgba).unwrap();
}

#[test]
fn the_river_matches_its_golden_on_the_native_profile() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let baked = root.join("target/assets/pc");
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    let mut r = match pollster::block_on(PcRenderer::new_headless(SimProfile::Native)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            return;
        }
    };
    for s in &assets.sprites {
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for m in assets.models {
        r.upload_mesh(trino_core::ModelId(m.id), m.tmdl);
    }
    for f in assets.fonts {
        r.upload_font(trino_core::FontId(f.id), f.table);
    }
    r.set_internal_size(640, 480);
    let frame = |r: &mut PcRenderer| {
        let mut g = showcase::ShowcaseGame::new();
        showcase::perf::river_scene(&mut g, Vec2::new(640.0, 480.0), &mut NullAudio);
        g.render(r);
        r.read_offscreen()
    };
    if let Some(d) = trino_platform_pc::tnl_oracle::compare(&mut r, frame) {
        eprintln!("showcase_river: {d}");
        let (w, h, img) = d.side_by_side();
        write_png(
            &root.join("target/evidence/pc-gpu-tnl/showcase_river.png"),
            w,
            h,
            &img,
        );
        d.assert_within("showcase_river");
    }
    let img = frame(&mut r);
    let path = root.join("tests/golden/pc/showcase_river.png");
    if std::env::var_os("TRINO_BLESS").is_some() {
        write_png(&path, 640, 480, &img);
        eprintln!("blessed {}", path.display());
        return;
    }
    let (w, h, want) = read_png(&path);
    assert_eq!((w, h), (640, 480));
    // Drivers round the per-pixel shading differently: a few values off.
    let bad = img
        .iter()
        .zip(&want)
        .filter(|(a, b)| a.abs_diff(**b) > 4)
        .count();
    if bad > 4000 {
        write_png(
            &root.join("tests/golden/pc/actual/showcase_river.png"),
            640,
            480,
            &img,
        );
        panic!("showcase_river: {bad} channel values differ by more than 4");
    }
}
