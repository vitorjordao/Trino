//! PC BCn textures (#52 phase 3) through the runner's upload path: a
//! sprite baked as BC1, BC3 or BC7 goes to the GPU as blocks where it
//! samples BC, and as those blocks decoded where it doesn't
//! (`PcRenderer::set_bc(false)`, like `TRINO_BC=0` or a GPU without BC) —
//! and both draw the same picture.

use std::path::Path;

use trino_asset_pipeline::{BcFormat, LoadedSprite, Platform, bake_all, load_dir};
use trino_core::{Color, Renderer, SpriteId, SpriteParams, Vec2};
use trino_platform_pc::{PcRenderer, SimProfile};

const SIDE: u32 = 32;

fn write_png(path: &Path, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut enc = png::Encoder::new(std::fs::File::create(path).unwrap(), SIDE, SIDE);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

/// A smooth color field; `alpha` adds a vertical ramp.
fn master(alpha: bool) -> Vec<u8> {
    (0..SIDE * SIDE)
        .flat_map(|i| {
            let (x, y) = (i % SIDE, i / SIDE);
            let a = if alpha { (y * 8 + 7) as u8 } else { 255 };
            [(x * 8) as u8, (y * 4 + 60) as u8, (200 - x * 3) as u8, a]
        })
        .collect()
}

/// Bake three sprites, one per BC format, and load them back.
fn sprites(dir: &Path) -> Vec<LoadedSprite> {
    let root = dir.join("assets");
    write_png(&root.join("shared/sprites/opaque.png"), &master(false));
    write_png(&root.join("shared/sprites/alpha.png"), &master(true));
    std::fs::write(
        root.join("manifest.toml"),
        "version = 1\n\
         [sprites.bc1]\nfile = \"sprites/opaque.png\"\nformats = { n64 = \"RGBA5551\", pc = \"BC1\" }\n\
         [sprites.bc3]\nfile = \"sprites/alpha.png\"\nformats = { n64 = \"RGBA5551\", pc = \"BC3\" }\n\
         [sprites.bc7]\nfile = \"sprites/alpha.png\"\nformats = { n64 = \"RGBA5551\", pc = \"BC7\" }\n",
    )
    .unwrap();
    let out = dir.join("baked");
    bake_all(&root, Platform::Pc, &out).unwrap_or_else(|e| panic!("{e}"));
    let mut s = load_dir(&out, None).unwrap().sprites;
    s.sort_by_key(|s| s.logical.clone());
    s
}

/// Upload through the runner and draw each sprite at 1:1 side by side.
fn render(r: &mut PcRenderer, sprites: &[LoadedSprite]) -> Vec<u8> {
    for s in sprites {
        trino_runner_pc::upload_sprite(r, s, Platform::Pc);
    }
    r.begin_frame(Color::rgb(30, 30, 30));
    for (i, s) in sprites.iter().enumerate() {
        let pos = Vec2::new(8.0 + i as f32 * 40.0, 8.0);
        r.draw_sprite(SpriteId(s.id), pos, &SpriteParams::DEFAULT);
    }
    r.end_frame();
    r.read_offscreen()
}

fn renderer() -> Option<PcRenderer> {
    match pollster::block_on(PcRenderer::new_headless(SimProfile::Native)) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("skipping: no GPU ({e})");
            None
        }
    }
}

#[test]
fn bc_uploads_draw_like_their_decoded_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let sprites = sprites(dir.path());
    let formats: Vec<BcFormat> = sprites
        .iter()
        .map(|s| s.compressed.as_ref().unwrap().format)
        .collect();
    assert_eq!(formats, [BcFormat::Bc1, BcFormat::Bc3, BcFormat::Bc7]);
    let (Some(mut gpu), Some(mut cpu)) = (renderer(), renderer()) else {
        return;
    };
    let bc_bytes: u64 = sprites
        .iter()
        .map(|s| s.compressed.as_ref().unwrap().data.len() as u64)
        .sum();

    // Forced off: decoded RGBA8, 4 bytes a texel.
    cpu.set_bc(false);
    assert!(!cpu.supports_bc());
    let fallback = render(&mut cpu, &sprites);
    let m = cpu.sprite_memory();
    assert_eq!((m.compressed, m.bytes), (0, 3 * (SIDE * SIDE * 4) as u64));

    let blocks = render(&mut gpu, &sprites);
    let m = gpu.sprite_memory();
    if gpu.supports_bc() {
        assert_eq!((m.compressed, m.compressed_bytes), (3, bc_bytes));
        assert_eq!(bc_bytes, 32 * 32 / 2 + 2 * 32 * 32, "BC1 4 bits, BC3/BC7 8");
    } else {
        eprintln!("this GPU has no BC: both renders took the fallback");
        assert_eq!(m.compressed, 0);
    }
    // Same picture: the GPU's BC decoder against bcdec's (BC1's 1/3 and
    // 2/3 points round differently between vendors, within a few levels).
    let (w, _) = gpu.internal_size();
    let mut worst = 0;
    for y in 8..8 + SIDE {
        for x in 8..8 + 3 * 40 {
            let o = ((y * w + x) * 4) as usize;
            for c in 0..4 {
                worst = worst.max((blocks[o + c] as i32 - fallback[o + c] as i32).abs());
            }
        }
    }
    assert!(worst <= 4, "BC upload vs decoded fallback: off by {worst}");

    // The fallback is exactly the decoded blocks: the opaque BC1 sprite
    // drawn 1:1 shows them texel for texel.
    let decoded = sprites[0].pc_pixels();
    for y in 0..SIDE {
        for x in 0..SIDE {
            let o = (((y + 8) * w + x + 8) * 4) as usize;
            let t = ((y * SIDE + x) * 4) as usize;
            assert_eq!(fallback[o..o + 3], decoded[t..t + 3], "texel {x},{y}");
        }
    }

    // `set_bc(true)` comes back only where the device has BC.
    gpu.set_bc(true);
    cpu.set_bc(true);
    assert_eq!(cpu.supports_bc(), gpu.supports_bc());
}
