//! PC "3DS look" vs the real thing: castle64's hub perf scene rendered by
//! the PC backend (N3ds profile) is compared with the frame Azahar produced
//! from the self-test app (`tests/golden/3ds/castle64_hub.png`, written by
//! `cargo xtask test 3ds --bless`). Different rasterizers (and Azahar's GPU
//! output depends on the host's OpenGL), so the test bounds the drift
//! instead of matching pixels: a backend that loses a model, breaks a
//! texture or gets depth wrong over large areas fails here. Both screens:
//! the top (the world) and the bottom (castle64's touch-screen panel,
//! `castle64_hub_bottom.png`). GPU-gated like the goldens;
//! `TRINO_PARITY_DUMP=1` writes the PC frames into the goldens' `actual/`.

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

/// A headless PC renderer on the 3DS profile with the engine's assets, each
/// sprite as its 3DS format samples it (`preview_3ds`). `None` (skip)
/// without a GPU.
fn n3ds_renderer(root: &std::path::Path) -> Option<PcRenderer> {
    let baked = tempfile::tempdir().unwrap();
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        baked.path(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(baked.path(), None).unwrap();
    let mut r = match pollster::block_on(PcRenderer::new_headless(SimProfile::N3ds)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            return None;
        }
    };
    for s in assets.sprites {
        let px = s.pixels_for(trino_asset_pipeline::Platform::N3ds);
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, px);
    }
    for m in assets.models {
        r.upload_mesh(trino_core::ModelId(m.id), m.tmdl);
    }
    for f in assets.fonts {
        r.upload_font(trino_core::FontId(f.id), f.table);
    }
    Some(r)
}

/// castle64's hub perf scene rendered by a headless PC renderer on the 3DS
/// profile (both screens). `None` (skip) without a GPU.
fn render_hub(root: &std::path::Path) -> Option<PcRenderer> {
    let mut r = n3ds_renderer(root)?;
    let hub = &castle64::perf::SCENES[0];
    let mut game = castle64::perf::scene_game(hub, Vec2::new(400.0, 240.0), &mut NullAudio);
    game.render(&mut r);
    Some(r)
}

fn write_png(out: &std::path::Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let mut enc = png::Encoder::new(std::fs::File::create(out).unwrap(), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

/// The PC frame quantized like the console dump (RGBA5551) vs the
/// Azahar golden: (mean max-channel diff, share of pixels > 32 apart).
fn drift(pc_rgba: &[u8], size: (u32, u32), golden: &std::path::Path, dump: &str) -> (f32, f32) {
    let expand = |c: u8| ((c >> 3) << 3) | (c >> 5);
    let pc: Vec<u8> = pc_rgba
        .chunks(4)
        .flat_map(|p| [expand(p[0]), expand(p[1]), expand(p[2]), 255])
        .collect();
    if std::env::var_os("TRINO_PARITY_DUMP").is_some() {
        write_png(
            &golden.with_file_name("actual").join(dump),
            size.0,
            size.1,
            &pc,
        );
    }
    let (w, h, azahar) = read_png(golden);
    assert_eq!((w, h), size);
    let diffs: Vec<u8> = pc
        .chunks(4)
        .zip(azahar.chunks(4))
        .map(|(a, b)| (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap())
        .collect();
    let n = diffs.len() as f32;
    let mean = diffs.iter().map(|&d| d as f32).sum::<f32>() / n;
    let far = diffs.iter().filter(|&&d| d > 32).count() as f32 / n;
    (mean, far)
}

#[test]
fn pc_3ds_look_tracks_the_azahar_frame() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let Some(r) = render_hub(&root) else {
        return;
    };
    let golden = root.join("tests/golden/3ds/castle64_hub.png");
    let (mean, far) = drift(
        &r.read_offscreen(),
        r.internal_size(),
        &golden,
        "pc_look.png",
    );
    eprintln!(
        "pc vs azahar: mean max-channel diff {mean:.2}, {:.2}% of pixels differ by > 32",
        far * 100.0
    );
    // Measured 0.60 mean / 1.00% far (triangle edges, HUD glyph filtering)
    // when written. Before castle64 set its own light it was 24.5 / 31%:
    // the console renderer still held the self-test showcase's fogged
    // light — the kind of drift this test exists to catch.
    assert!(mean < 2.5, "mean diff {mean:.2}");
    assert!(far < 0.025, "{:.2}% of pixels far apart", far * 100.0);
}

/// The bottom screen of the same frame (castle64's map, counters with a 3D
/// star, touch buttons) vs Azahar's touch-screen dump
/// (`tests/golden/3ds/castle64_hub_bottom.png`). `TRINO_PARITY_DUMP=1`
/// also writes the stacked two-screen picture the PC window shows.
#[test]
fn pc_3ds_bottom_screen_tracks_the_azahar_frame() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let Some(r) = render_hub(&root) else {
        return;
    };
    let golden = root.join("tests/golden/3ds/castle64_hub_bottom.png");
    if std::env::var_os("TRINO_PARITY_DUMP").is_some() {
        let stacked = r.present_preview((800, 960));
        write_png(
            &golden.with_file_name("actual").join("pc_stacked.png"),
            800,
            960,
            &stacked,
        );
    }
    let bottom = r.read_second_screen();
    let (mean, far) = drift(&bottom, (320, 240), &golden, "pc_look_bottom.png");
    eprintln!(
        "pc vs azahar (bottom): mean max-channel diff {mean:.2}, {:.2}% of pixels differ by > 32",
        far * 100.0
    );
    assert!(mean < 2.5, "mean diff {mean:.2}");
    assert!(far < 0.025, "{:.2}% of pixels far apart", far * 100.0);
}

/// One card per 3DS texture format (#52, `draw_texture_formats`): the PC's
/// 3DS profile draws each sprite's decoded 3DS format, which must look like
/// what Azahar sampled from the `.t3x` (`tests/golden/3ds/texture_formats.png`).
#[test]
fn pc_3ds_texture_formats_track_the_azahar_frame() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let Some(mut r) = n3ds_renderer(&root) else {
        return;
    };
    castle64::selftest::draw_texture_formats(&mut r, true);
    let golden = root.join("tests/golden/3ds/texture_formats.png");
    let (mean, far) = drift(
        &r.read_offscreen(),
        r.internal_size(),
        &golden,
        "pc_texture_formats.png",
    );
    eprintln!(
        "pc vs azahar (texture formats): mean max-channel diff {mean:.2}, {:.2}% of pixels differ by > 32",
        far * 100.0
    );
    assert!(mean < 1.0, "mean diff {mean:.2}");
    assert!(far < 0.01, "{:.2}% of pixels far apart", far * 100.0);
}
