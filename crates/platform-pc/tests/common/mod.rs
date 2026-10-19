//! Shared fixtures for the PC backend's integration tests: a synthetic
//! project (manifest + generated masters) baked by the real asset
//! pipeline, a GPU-gated headless renderer and pixel probes.

#![allow(dead_code)]

use std::path::Path;

use trino_core::{BlobId, FontId, ModelId, SpriteId};
use trino_platform_pc::{PcPlatform, PcRenderer, SimProfile};

/// Logical paths of the fixture project's assets.
pub const RED: &str = "sprites/red";
pub const WHITE: &str = "sprites/white";
/// 8x8: left half green, right half blue.
pub const SPLIT: &str = "sprites/split";
/// 16x16 solid green (POT, fits N64 TMEM): the textured model's texture.
pub const TEX: &str = "sprites/tex";
pub const BOX: &str = "models/box";
pub const TBOX: &str = "models/tbox";
pub const BALL: &str = "models/ball";
pub const FONT: &str = "fonts/blocks";
pub const BLOB: &str = "blobs/data";
pub const BEEP: &str = "sounds/beep";
pub const SONG: &str = "music/song";
/// Bytes of the fixture blob.
pub const BLOB_BYTES: &[u8] = b"trino-blob\x00\x01\x02";

pub fn sprite(logical: &str) -> SpriteId {
    SpriteId::from_path(logical)
}
pub fn model(logical: &str) -> ModelId {
    ModelId::from_path(logical)
}
pub fn font() -> FontId {
    FontId::from_path(FONT)
}

/// One GPU test at a time in a test binary: RADV, with renderers created on
/// parallel test threads, crashed inside `libvulkan` (GitHub #94, fixed by
/// [`trino_platform_pc::renderer::headless_flags`]) and, rarely, drew a
/// frame with the previous frame's pixels (`render_e2e`: 2 runs in 25
/// parallel, 0 in 30 serial). Hold the guard for the whole test.
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn gpu() -> std::sync::MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

/// Headless renderer for `profile`, or `None` on GPU-less machines unless
/// `TRINO_REQUIRE_GPU` insists (CI on lavapipe).
pub fn headless(profile: SimProfile) -> Option<PcRenderer> {
    match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(r) => Some(r),
        Err(e) => {
            assert!(
                std::env::var("TRINO_REQUIRE_GPU").is_err(),
                "TRINO_REQUIRE_GPU set but renderer init failed: {e}"
            );
            eprintln!("skipping GPU test: {e}");
            None
        }
    }
}

pub fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

pub fn solid(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
    (0..w * h).flat_map(|_| c).collect()
}

/// 16-bit PCM mono WAV with a constant `value`.
pub fn write_wav(path: &Path, rate: u32, frames: usize, value: i16) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let data_len = (frames * 2) as u32;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for _ in 0..frames {
        b.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

/// Write the fixture project's `assets/` (manifest + masters) under `root`.
pub fn write_project(root: &Path) {
    let shared = root.join("shared");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(
        root.join("manifest.toml"),
        r#"
version = 1

[sprites.red]
file = "sprites/red.png"
formats = { n64 = "CI4" }

[sprites.white]
file = "sprites/white.png"
formats = { n64 = "CI4" }

[sprites.split]
file = "sprites/split.png"
formats = { n64 = "CI4" }

[sprites.tex]
file = "sprites/tex.png"
formats = { n64 = "CI4" }

[sounds.beep]
file = "sounds/beep.wav"

[music.song]
file = "music/song.wav"

[models.box]
file = "models/box.prim.ron"

[models.tbox]
file = "models/tbox.prim.ron"
texture = "sprites/tex"

[models.ball]
file = "models/ball.prim.ron"

[fonts.blocks]
file = "fonts/blocks.png"
cell = [4, 6]
chars = "AB?"

[blobs.data]
file = "data/data.bin"
"#,
    )
    .unwrap();
    write_png(
        &shared.join("sprites/red.png"),
        8,
        8,
        &solid(8, 8, [255, 0, 0, 255]),
    );
    write_png(
        &shared.join("sprites/white.png"),
        8,
        8,
        &solid(8, 8, [255, 255, 255, 255]),
    );
    let split: Vec<u8> = (0..64)
        .flat_map(|i| {
            if i % 8 < 4 {
                [0, 255, 0, 255]
            } else {
                [0, 0, 255, 255]
            }
        })
        .collect();
    write_png(&shared.join("sprites/split.png"), 8, 8, &split);
    write_png(
        &shared.join("sprites/tex.png"),
        16,
        16,
        &solid(16, 16, [0, 255, 0, 255]),
    );
    // Three 4x6 cells, each glyph a 3x6 opaque block (advance 3 + 1).
    let glyphs: Vec<u8> = (0..6 * 12)
        .flat_map(|i| {
            if (i % 12) % 4 < 3 {
                [0, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        })
        .collect();
    write_png(&shared.join("fonts/blocks.png"), 12, 6, &glyphs);
    let prim = |name: &str, text: &str| {
        let p = shared.join("models").join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    prim(
        "box.prim.ron",
        "(shape: Box(size: (1.0, 1.0, 1.0)), color: (255, 255, 255, 255))",
    );
    prim(
        "tbox.prim.ron",
        "(shape: Box(size: (1.0, 1.0, 1.0)), color: (255, 255, 255, 255))",
    );
    prim(
        "ball.prim.ron",
        "(shape: Sphere(radius: 1.0), segments: 48, color: (255, 255, 255, 255))",
    );
    std::fs::create_dir_all(shared.join("data")).unwrap();
    std::fs::write(shared.join("data/data.bin"), BLOB_BYTES).unwrap();
    write_wav(&shared.join("sounds/beep.wav"), 22_050, 2_205, 8_192);
    write_wav(&shared.join("music/song.wav"), 22_050, 22_050, -4_096);
}

/// Bake the fixture project for the PC and load it back.
pub fn bake_project(dir: &Path) -> trino_asset_pipeline::LoadedAssets {
    let (root, out) = (dir.join("assets"), dir.join("baked"));
    write_project(&root);
    trino_asset_pipeline::bake_all(&root, trino_asset_pipeline::Platform::Pc, &out)
        .unwrap_or_else(|e| panic!("{e}"));
    trino_asset_pipeline::load_dir(&out, None).unwrap()
}

/// Upload every loaded asset the way a host does.
pub fn upload(
    assets: &trino_asset_pipeline::LoadedAssets,
    renderer: &mut PcRenderer,
    platform: Option<&mut PcPlatform>,
) {
    for s in &assets.sprites {
        renderer.upload_sprite(SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for f in &assets.fonts {
        renderer.upload_font(FontId(f.id), f.table.clone());
    }
    for m in &assets.models {
        renderer.upload_mesh(ModelId(m.id), m.tmdl.clone());
    }
    if let Some(p) = platform {
        for b in &assets.blobs {
            p.insert_blob(BlobId(b.id), b.bytes.clone());
        }
    }
}

/// RGBA at (x, y) of a tightly packed `width`-wide buffer.
pub fn px(buf: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * width + x) * 4) as usize;
    [buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]
}

/// Every channel of `got` within `tol` of `want` (RGB only).
#[track_caller]
pub fn assert_near(got: [u8; 4], want: [u8; 3], tol: u8, what: &str) {
    for c in 0..3 {
        assert!(
            got[c].abs_diff(want[c]) <= tol,
            "{what}: got {got:?}, want ~{want:?} (±{tol})"
        );
    }
}

/// How a frame drawn under dynamic resolution (the 3D in the top-left
/// corner of targets kept for the range's top) compares with the same
/// scale drawn in targets of its own size: the largest channel difference
/// and the pixels with a channel more than 1/255 apart.
pub fn corner_diff(corner: &[u8], own: &[u8]) -> (u8, usize) {
    assert_eq!(corner.len(), own.len());
    let mut max = 0;
    let mut far = 0;
    for (a, b) in corner.as_chunks::<4>().0.iter().zip(own.as_chunks::<4>().0) {
        let d = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap();
        max = max.max(d);
        far += usize::from(d > 1);
    }
    (max, far)
}

/// The largest difference [`corner_diff`] may report (GitHub #93).
///
/// Without 4× MSAA both frames are the same to the bit on every driver: the
/// corner's UVs land on the own-size targets' texels. With it, AMD's RADV
/// resolves a partly covered pixel 1/255 differently in a 240×136 or
/// 288×164 target than in a 480×272 one (implementation-defined averaging;
/// not its DCC, FMASK or HiZ compression: `RADV_DEBUG=nodcc,nofmask,nohiz`
/// changes nothing), on geometry edges only. Lavapipe resolves both the
/// same. The upscale's contrast-adaptive sharpening then pushes a pixel
/// away from its four neighbours by up to `4k` (`k` = 1/8 at sharpen 0, 1/5
/// at 1, as in `upscale.wgsl`), so a 1/255 input difference leaves up to
/// `(1 + 4k) / (1 - 4k)` of it in the output.
pub fn corner_tolerance(msaa: u8, sharpen: f32) -> u8 {
    if msaa <= 1 {
        return 0;
    }
    if sharpen <= 0.0 {
        return 1;
    }
    let k = 1.0 / (8.0 + (5.0 - 8.0) * sharpen.clamp(0.0, 1.0));
    ((1.0 + 4.0 * k) / (1.0 - 4.0 * k)).ceil() as u8
}

/// The resolve differences are edge pixels: fewer than 0.5 % of the picture
/// (the GPU T&L oracle's share, ADR 0016).
pub fn corner_far_limit(pixels: usize) -> usize {
    pixels / 200
}
