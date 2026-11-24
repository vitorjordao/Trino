//! `mipmaps = true` (Bongo-Kart #178) through the bake: the PC's `TSPM`
//! master and BC chain, the Web's KTX2 levels, the loader's levels, and
//! the size rule (powers of two). The consoles' bakes don't change.

use std::path::{Path, PathBuf};

use trino_asset_pipeline::texture::{self, BcFormat};
use trino_asset_pipeline::{Platform, bake_all, load_dir};
use trino_core::asset_id;

const SIDE: u32 = 64;

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

/// Stripes and a gradient: a texture a far floor would shimmer with.
fn tile(w: u32, h: u32) -> Vec<u8> {
    let mut px = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let s = if (x / 2 + y) % 2 == 0 { 230 } else { 30 };
            px.extend_from_slice(&[s, (y * 255 / h) as u8, 120, 255]);
        }
    }
    px
}

fn project(dir: &Path, extra: &str) -> PathBuf {
    let root = dir.join("assets");
    write_png(
        &root.join("shared/sprites/floor.png"),
        SIDE,
        SIDE,
        &tile(SIDE, SIDE),
    );
    write_png(
        &root.join("shared/sprites/hud.png"),
        SIDE,
        SIDE,
        &tile(SIDE, SIDE),
    );
    write_png(&root.join("shared/sprites/odd.png"), 48, 48, &tile(48, 48));
    std::fs::write(
        root.join("manifest.toml"),
        format!(
            "version = 1\n\
             [sprites.floor]\nfile = \"sprites/floor.png\"\nformats = {{ n64 = \"CI8\", pc = \"BC1\", web = \"KTX2-UASTC\" }}\nmipmaps = true\n\
             [sprites.hud]\nfile = \"sprites/hud.png\"\nformats = {{ n64 = \"CI8\", pc = \"BC1\", web = \"KTX2-UASTC\" }}\n{extra}"
        ),
    )
    .unwrap();
    root
}

#[test]
fn the_pc_bake_writes_the_chain_and_the_loader_splits_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), "");
    let out = dir.path().join("pc");
    bake_all(&root, Platform::Pc, &out).unwrap_or_else(|e| panic!("{e}"));
    let floor = asset_id("sprites/floor");
    let master = std::fs::read(out.join(format!("{floor:08x}.sprite"))).unwrap();
    assert_eq!(&master[..4], b"TSPM", "a mipmapped master says so");
    let hud = asset_id("sprites/hud");
    let plain = std::fs::read(out.join(format!("{hud:08x}.sprite"))).unwrap();
    assert_eq!(&plain[..4], b"TSPR", "the others are unchanged");

    let loaded = load_dir(&out, None).unwrap();
    let s = loaded.sprites.iter().find(|s| s.id == floor).unwrap();
    assert!(s.mipmaps);
    assert_eq!(s.rgba, tile(SIDE, SIDE), "level 0 is the master");
    let c = s.compressed.as_ref().unwrap();
    assert_eq!(c.format, BcFormat::Bc1);
    assert_eq!(c.data.len() as u64, BcFormat::Bc1.chain_bytes(SIDE, SIDE));
    let levels = s.compressed_levels().unwrap();
    assert_eq!(levels.len(), trino_core::mip::levels(SIDE, SIDE) as usize);
    assert_eq!(levels.len(), 5, "64 .. 4");
    // Level 0's blocks are what a plain sprite's file holds: the chain only
    // adds levels.
    let h = loaded.sprites.iter().find(|s| s.id == hud).unwrap();
    assert!(!h.mipmaps);
    assert_eq!(levels[0], &h.compressed.as_ref().unwrap().data[..]);
    assert_eq!(h.compressed_levels().unwrap().len(), 1);
    // A GPU without BC gets level 0 decoded, like any BC sprite.
    assert_eq!(s.pc_pixels()[..], h.pc_pixels()[..]);
    // Each level is the chain trino_core::mip makes, encoded.
    let mut chain = vec![0u8; trino_core::mip::rgba8_chain_bytes(SIDE, SIDE)];
    chain[..(SIDE * SIDE * 4) as usize].copy_from_slice(&tile(SIDE, SIDE));
    trino_core::mip::fill_rgba8_chain(&mut chain, SIDE, SIDE);
    let at = trino_core::mip::rgba8_level_offset(SIDE, SIDE, 2);
    let l2 = &chain[at..at + 16 * 16 * 4];
    assert_eq!(levels[2], &BcFormat::Bc1.encode(16, 16, l2)[..]);
    // A chain one level short is a damaged file.
    let bc = std::fs::read(out.join(format!("{floor:08x}.bc"))).unwrap();
    std::fs::write(out.join(format!("{floor:08x}.bc")), &bc[..bc.len() - 8]).unwrap();
    assert!(load_dir(&out, None).err().unwrap().contains("bad BCn blob"));
}

#[test]
fn the_web_bake_writes_ktx2_levels() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), "");
    let out = dir.path().join("web");
    let report = bake_all(&root, Platform::Web, &out).unwrap_or_else(|e| panic!("{e}"));
    let entry = |l: &str| report.entries.iter().find(|e| e.logical == l).unwrap();
    let file = |l: &str| std::fs::read(out.join(entry(l).ktx2.as_ref().unwrap())).unwrap();
    let floor = texture::ktx2::parse(&file("sprites/floor")).unwrap();
    assert_eq!(floor.levels, 5);
    let hud = texture::ktx2::parse(&file("sprites/hud")).unwrap();
    assert_eq!(hud.levels, 1);
    // Level 0 decodes to the same pixels with or without the chain.
    assert_eq!(
        texture::ktx2::decode(&file("sprites/floor")).unwrap(),
        texture::ktx2::decode(&file("sprites/hud")).unwrap()
    );
}

#[test]
fn the_consoles_bake_the_same_bytes_and_odd_sizes_fail() {
    for platform in [Platform::N64, Platform::N3ds] {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), "");
        let out = dir.path().join("out");
        let report = bake_all(&root, platform, &out).unwrap_or_else(|e| panic!("{e}"));
        let format = |l: &str| {
            report
                .entries
                .iter()
                .find(|e| e.logical == l)
                .unwrap()
                .format
                .clone()
        };
        assert_eq!(
            format("sprites/floor"),
            format("sprites/hud"),
            "{platform:?}"
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let root = project(
        dir.path(),
        "[sprites.odd]\nfile = \"sprites/odd.png\"\nformats = { n64 = \"CI8\" }\nmipmaps = true\n",
    );
    let err = bake_all(&root, Platform::Pc, &dir.path().join("pc"))
        .err()
        .unwrap()
        .to_string();
    assert!(
        err.contains("power-of-two") && err.contains("48x48"),
        "{err}"
    );
}
