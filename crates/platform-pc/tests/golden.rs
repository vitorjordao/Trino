//! Golden-image test: render a known scene headless (offscreen framebuffer),
//! read the pixels back and compare against `tests/golden/pc/*.png` at the
//! repo root.
//!
//! - Regenerate goldens with `cargo xtask test --bless` (sets TRINO_BLESS=1)
//!   and review the image diff in the PR.
//! - Machines without a GPU adapter skip the test, unless TRINO_REQUIRE_GPU
//!   is set (CI sets it after installing Mesa/lavapipe).
//! - The scene is intentionally axis-aligned: nearest sampling at integer
//!   positions is bit-stable across GPUs; a small tolerance absorbs 8-bit
//!   blend rounding differences.

use std::path::PathBuf;

use trino_core::{Color, Renderer, SpriteId, SpriteParams, Vec2};
use trino_platform_pc::{PcRenderer, SimProfile};

const MAX_CHANNEL_DIFF: u8 = 2;
// The N64-look golden tolerates one RGBA5551 quantization step (255/31 ≈ 8.2):
// dither ties may round differently across GPU drivers.
const MAX_CHANNEL_DIFF_5551: u8 = 9;

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/pc")
}

fn checkerboard(size: u32, a: [u8; 4], b: [u8; 4]) -> Vec<u8> {
    let cell = (size / 4).max(1);
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let pick = ((x / cell) + (y / cell)).is_multiple_of(2);
            pixels.extend_from_slice(if pick { &a } else { &b });
        }
    }
    pixels
}

fn gradient(size: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let v = (x * 255 / size.max(1)) as u8;
            let w = (y * 255 / size.max(1)) as u8;
            pixels.extend_from_slice(&[v, w, 128, 255]);
        }
    }
    pixels
}

fn write_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(rgba).unwrap();
}

fn read_png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
    let file = std::fs::File::open(path).unwrap();
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    assert_eq!(
        info.color_type,
        png::ColorType::Rgba,
        "golden must be RGBA8"
    );
    (info.width, info.height, buf)
}

/// Headless renderer for a sim profile, or None on GPU-less machines
/// (unless TRINO_REQUIRE_GPU insists).
fn headless_renderer(profile: SimProfile) -> Option<PcRenderer> {
    match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(r) => Some(r),
        Err(e) => {
            if std::env::var("TRINO_REQUIRE_GPU").is_ok() {
                panic!("TRINO_REQUIRE_GPU set but renderer init failed: {e}");
            }
            eprintln!("skipping golden test: {e}");
            None
        }
    }
}

/// The reference scene both goldens render: plain draw, integer upscale +
/// tint, flip, and alpha blending.
fn draw_test_scene(renderer: &mut PcRenderer) {
    let checker = SpriteId(1);
    let grad = SpriteId(2);
    renderer.upload_sprite(
        checker,
        16,
        16,
        &checkerboard(16, [230, 70, 70, 255], [250, 250, 250, 255]),
    );
    renderer.upload_sprite(grad, 8, 8, &gradient(8));

    renderer.begin_frame(Color::rgb(24, 26, 40));
    renderer.draw_sprite(checker, Vec2::new(10.0, 10.0), &SpriteParams::default());
    renderer.draw_sprite(
        checker,
        Vec2::new(100.0, 40.0),
        &SpriteParams {
            scale: Vec2::new(3.0, 3.0),
            tint: Color::rgb(80, 255, 120),
            ..Default::default()
        },
    );
    renderer.draw_sprite(
        grad,
        Vec2::new(200.0, 100.0),
        &SpriteParams {
            scale: Vec2::new(4.0, 4.0),
            flip_x: true,
            ..Default::default()
        },
    );
    renderer.draw_sprite(
        checker,
        Vec2::new(210.0, 110.0),
        &SpriteParams {
            scale: Vec2::new(2.0, 2.0),
            tint: Color::rgba(255, 255, 255, 128),
            ..Default::default()
        },
    );
    renderer.end_frame();
}

/// Bless-or-compare the renderer's framebuffer against `tests/golden/pc/<name>.png`.
fn check_golden(renderer: &PcRenderer, name: &str, max_channel_diff: u8) {
    check_golden_tolerant(renderer, name, max_channel_diff, 0);
}

/// Like [`check_golden`], allowing up to `allowed_bad` channel values to
/// exceed the tolerance — for content with GPU-rasterized triangle edges,
/// where fill-rule ties can flip a few boundary pixels across drivers.
fn check_golden_tolerant(
    renderer: &PcRenderer,
    name: &str,
    max_channel_diff: u8,
    allowed_bad: usize,
) {
    let (w, h) = renderer.internal_size();
    let actual = renderer.read_offscreen();
    assert_eq!(actual.len(), (w * h * 4) as usize);

    let golden_path = golden_dir().join(format!("{name}.png"));
    if std::env::var("TRINO_BLESS").is_ok() {
        write_png(&golden_path, w, h, &actual);
        eprintln!("blessed golden: {}", golden_path.display());
        return;
    }

    assert!(
        golden_path.exists(),
        "golden image missing at {} — run `cargo xtask test --bless` once and commit it",
        golden_path.display()
    );
    let (gw, gh, expected) = read_png(&golden_path);
    assert_eq!((gw, gh), (w, h), "golden resolution mismatch");

    let mut bad_pixels = 0usize;
    let mut worst = 0u8;
    for (i, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
        let diff = a.abs_diff(*e);
        if diff > max_channel_diff {
            bad_pixels += 1;
            worst = worst.max(diff);
            if bad_pixels == 1 {
                eprintln!(
                    "first mismatch at byte {i} (pixel {}): actual {a} vs golden {e}",
                    i / 4
                );
            }
        }
    }
    if bad_pixels > allowed_bad {
        let actual_path = golden_dir().join(format!("actual/{name}.png"));
        write_png(&actual_path, w, h, &actual);
        panic!(
            "golden mismatch: {bad_pixels} channel values differ by more than \
             {max_channel_diff} (allowed {allowed_bad}, worst {worst}); actual \
             written to {}",
            actual_path.display()
        );
    }
}

#[test]
fn basic_scene_matches_golden() {
    let Some(mut renderer) = headless_renderer(SimProfile::N64) else {
        return;
    };
    // This golden checks the raw sprite pass; the output emulation has its
    // own golden below.
    renderer.set_n64_look(false);
    draw_test_scene(&mut renderer);
    check_golden(&renderer, "basic_scene", MAX_CHANNEL_DIFF);
}

#[test]
fn n64_look_scene_matches_golden() {
    let Some(mut renderer) = headless_renderer(SimProfile::N64) else {
        return;
    };
    // Explicit (it already defaults to on for the N64 profile) so the test
    // does not depend on the TRINO_LOOK env override.
    renderer.set_n64_look(true);
    draw_test_scene(&mut renderer);
    check_golden(&renderer, "n64_look_scene", MAX_CHANNEL_DIFF_5551);
}

#[test]
fn n3ds_scene_matches_golden() {
    // 400x240 + bilinear sprite sampling (the 3DS GPU default).
    let Some(mut renderer) = headless_renderer(SimProfile::N3ds) else {
        return;
    };
    draw_test_scene(&mut renderer);
    check_golden(&renderer, "n3ds_scene", MAX_CHANNEL_DIFF);
}

#[test]
fn platformer_scene_matches_golden() {
    // The real showcase game after a deterministic input script, rendered
    // with procedural stand-in sprites (goldens must not depend on the
    // committed PNG masters).
    let Some(mut renderer) = headless_renderer(SimProfile::N64) else {
        return;
    };
    renderer.set_n64_look(false);

    renderer.upload_sprite(
        platformer::HERO,
        16,
        16,
        &checkerboard(16, [40, 80, 200, 255], [255, 205, 148, 255]),
    );
    renderer.upload_sprite(
        platformer::GROUND,
        16,
        16,
        &checkerboard(16, [106, 190, 48, 255], [143, 86, 59, 255]),
    );
    renderer.upload_sprite(
        platformer::BRICK,
        16,
        16,
        &checkerboard(16, [172, 50, 50, 255], [96, 44, 44, 255]),
    );
    renderer.upload_sprite(platformer::COIN, 16, 16, &gradient(16));
    renderer.upload_sprite(
        platformer::FLAG,
        16,
        16,
        &checkerboard(16, [60, 200, 80, 255], [120, 120, 130, 255]),
    );

    struct NullAudio;
    impl trino_core::Audio for NullAudio {
        fn play_sound(&mut self, _: trino_core::SoundId) {}
        fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
    }

    use trino_core::{Button, Game, InputState};
    let mut game = platformer::PlatformerGame::new(Vec2::new(320.0, 240.0));
    let mut audio = NullAudio;
    let mut right = InputState::default();
    right.set(Button::DpadRight, true);
    let mut jump = right;
    jump.set(Button::A, true);
    for i in 0..90 {
        let input = if i == 60 { &jump } else { &right };
        game.update(input, &mut audio, 1.0 / 60.0);
    }
    game.render(&mut renderer);

    check_golden(&renderer, "platformer_scene", MAX_CHANNEL_DIFF);
}

#[test]
fn cube3d_scene_matches_golden() {
    // The 3D pipeline end to end: glTF master -> TMDL bake -> upload ->
    // software T&L -> triangle rasterization.
    let Some(mut renderer) = headless_renderer(SimProfile::N64) else {
        return;
    };
    renderer.set_n64_look(false);

    let glb = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/shared/models/cube.glb");
    let tmdl = trino_asset_pipeline::bake_model_tmdl(&glb).expect("cube.glb bakes");
    let cube = trino_core::ModelId(1);
    renderer.upload_mesh(cube, tmdl);

    use trino_core::render3d::Camera3;
    use trino_core::{Renderer as _, Transform3, Vec3};
    renderer.begin_frame(Color::rgb(24, 26, 40));
    renderer.set_camera(&Camera3::default());
    renderer.draw_model(
        cube,
        &Transform3 {
            rotation: Vec3::new(0.5, 0.8, 0.0),
            ..Default::default()
        },
        trino_core::Material::VertexLit,
    );
    renderer.end_frame();

    // Triangle edges may flip a pixel across drivers: allow a small number
    // of out-of-tolerance channel values.
    check_golden_tolerant(&renderer, "cube3d_scene", MAX_CHANNEL_DIFF, 512);
}

#[test]
fn strict_mode_rejects_texture_over_budget() {
    let Some(mut renderer) = headless_renderer(SimProfile::N64) else {
        return;
    };
    renderer.set_strict(true);
    // 64x64 @ RGBA5551 (2 bytes/pixel on N64) = 8192 bytes > 4096 TMEM.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        renderer.upload_sprite(SpriteId(3), 64, 64, &vec![255u8; 64 * 64 * 4]);
    }));
    let err = result.expect_err("strict mode must reject a 64x64 sprite on N64");
    let msg = err
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "non-string panic".into());
    assert!(
        msg.contains("strict mode") && msg.contains("manifest.toml"),
        "panic message must be actionable, got: {msg}"
    );
}
