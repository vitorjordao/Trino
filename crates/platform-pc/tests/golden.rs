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

#[test]
fn basic_scene_matches_golden() {
    let renderer = pollster::block_on(PcRenderer::new_headless(SimProfile::N64));
    let mut renderer = match renderer {
        Ok(r) => r,
        Err(e) => {
            if std::env::var("TRINO_REQUIRE_GPU").is_ok() {
                panic!("TRINO_REQUIRE_GPU set but renderer init failed: {e}");
            }
            eprintln!("skipping golden test: {e}");
            return;
        }
    };

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
    // Plain draw.
    renderer.draw_sprite(checker, Vec2::new(10.0, 10.0), &SpriteParams::default());
    // Integer upscale + tint.
    renderer.draw_sprite(
        checker,
        Vec2::new(100.0, 40.0),
        &SpriteParams {
            scale: Vec2::new(3.0, 3.0),
            tint: Color::rgb(80, 255, 120),
            ..Default::default()
        },
    );
    // Flips.
    renderer.draw_sprite(
        grad,
        Vec2::new(200.0, 100.0),
        &SpriteParams {
            scale: Vec2::new(4.0, 4.0),
            flip_x: true,
            ..Default::default()
        },
    );
    // Alpha blending over the previous draws.
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

    let (w, h) = renderer.internal_size();
    let actual = renderer.read_offscreen();
    assert_eq!(actual.len(), (w * h * 4) as usize);

    let golden_path = golden_dir().join("basic_scene.png");
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
        if diff > MAX_CHANNEL_DIFF {
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
    if bad_pixels > 0 {
        let actual_path = golden_dir().join("actual/basic_scene.png");
        write_png(&actual_path, w, h, &actual);
        panic!(
            "golden mismatch: {bad_pixels} channel values differ by more than \
             {MAX_CHANNEL_DIFF} (worst {worst}); actual written to {}",
            actual_path.display()
        );
    }
}
