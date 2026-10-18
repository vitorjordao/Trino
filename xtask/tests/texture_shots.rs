//! Evidence for the 3DS texture formats (#52), written to
//! `target/evidence/texture_formats/` (`cargo test -p xtask --test
//! texture_shots -- --ignored`):
//! - `formats_<name>.png`: the photo-like master with an alpha ramp, as each
//!   3DS format keeps it (`N3dsFormat::preview`, what the PC's 3DS profile
//!   draws), over a checkerboard, 4× nearest;
//! - `azahar_vs_pc.png`: the photo row of the 3DS golden
//!   (`tests/golden/3ds/texture_formats.png`, Azahar) above the same row
//!   drawn by the PC's 3DS profile, 3×.
//!
//! And for `auto`, BCn and the N64 staging (`auto_bc_and_n64_evidence`,
//! written to `target/evidence/texture_auto/`):
//! - `auto_sheet.png`: per texture (engine and Solid Eleven), the master
//!   and what the PC, the 3DS and the N64 keep of it, each platform's
//!   chosen format, over a checkerboard; the table is printed;
//! - `bc_photo.png`: the photo master, then BC1, BC3 and BC7, 4×;
//! - `n64_glow.png`: Solid Eleven's glow, then the N64's I4 as `mksprite`
//!   made it from the master (a solid disc) and as staged now.

use std::path::{Path, PathBuf};

use trino_asset_pipeline::texture::{self, N3dsFormat, N64Format, Source};
use trino_asset_pipeline::{BcFormat, Manifest, Platform};
use trino_platform_pc::{PcRenderer, SimProfile};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let mut dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgb => buf
            .chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        _ => buf,
    };
    (info.width, info.height, rgba)
}

fn write_png(out: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let mut enc = png::Encoder::new(std::fs::File::create(out).unwrap(), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

/// `rgba` over an 8-px checkerboard, scaled `k`× nearest.
fn present(w: u32, h: u32, rgba: &[u8], k: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * k * k * 4) as usize);
    for y in 0..h * k {
        for x in 0..w * k {
            let p = &rgba[(((y / k) * w + x / k) * 4) as usize..][..4];
            let bg = if ((x / 8) + (y / 8)) % 2 == 0 {
                200
            } else {
                120
            };
            let a = p[3] as u32;
            for &c in &p[..3] {
                out.push(((c as u32 * a + bg * (255 - a)) / 255) as u8);
            }
            out.push(255);
        }
    }
    out
}

#[test]
#[ignore = "evidence: writes target/evidence/texture_formats"]
fn texture_format_evidence() {
    let out = root().join("target/evidence/texture_formats");
    // The photo master with an alpha ramp over its right half.
    let (w, h, photo) = read_png(&root().join("assets/shared/sprites/selftest_photo.png"));
    let master: Vec<u8> = photo
        .chunks(4)
        .enumerate()
        .flat_map(|(i, p)| {
            let (x, y) = (i as u32 % w, i as u32 / w);
            let a = if x < w / 2 {
                255
            } else {
                (y * 255 / (h - 1)) as u8
            };
            [p[0], p[1], p[2], a]
        })
        .collect();
    write_png(
        &out.join("formats_master.png"),
        w * 4,
        h * 4,
        &present(w, h, &master, 4),
    );
    for f in N3dsFormat::ALL {
        let p = f.preview(w, h, &master);
        let db = texture::psnr(&master, &p);
        println!(
            "{:<8} {:>6} B  PSNR {db:.1} dB",
            f.name(),
            f.texture_bytes(w, h)
        );
        let name = format!("formats_{}.png", f.tex3ds());
        write_png(&out.join(name), w * 4, h * 4, &present(w, h, &p, 4));
    }

    // Azahar's photo row vs the PC's 3DS profile.
    let golden = read_png(&root().join("tests/golden/3ds/texture_formats.png"));
    let baked = tempfile::tempdir().unwrap();
    trino_asset_pipeline::bake_all(
        &root().join("assets"),
        trino_asset_pipeline::Platform::Pc,
        baked.path(),
    )
    .unwrap();
    let assets = trino_asset_pipeline::load_dir(baked.path(), None).unwrap();
    let Ok(mut r) = pollster::block_on(PcRenderer::new_headless(SimProfile::N3ds)) else {
        eprintln!("skipping the PC frame: no GPU");
        return;
    };
    for s in assets.sprites {
        let px = s.pixels_for(trino_asset_pipeline::Platform::N3ds);
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, px);
    }
    for m in assets.models {
        r.upload_mesh(trino_core::ModelId(m.id), m.tmdl);
    }
    castle64::selftest::draw_texture_formats(&mut r, true);
    let pc = r.read_offscreen();
    let (fw, _) = r.internal_size();
    // The photo row: x 48..352, y 48..128 of the 400x240 frame.
    let (x0, y0, cw, ch, k) = (48u32, 48u32, 304u32, 80u32, 3u32);
    let crop = |src: &[u8]| -> Vec<u8> {
        (y0..y0 + ch)
            .flat_map(|y| (x0..x0 + cw).map(move |x| (y * fw + x) as usize * 4))
            .flat_map(|o| [src[o], src[o + 1], src[o + 2], 255])
            .collect()
    };
    let mut both = present(cw, ch, &crop(&golden.2), k);
    both.extend(present(cw, ch, &crop(&pc), k));
    write_png(&out.join("azahar_vs_pc.png"), cw * k, ch * k * 2, &both);
    write_png(&out.join("azahar_frame.png"), golden.0, golden.1, &golden.2);
    write_png(&out.join("pc_frame.png"), fw, 240, &pc);
    println!("wrote {}", out.display());
}

/// `rgba` (w×h) scaled nearest into a `side`-px cell, over the checkerboard.
fn cell(w: u32, h: u32, rgba: &[u8], side: u32) -> Vec<u8> {
    let k = (side / w.max(h)).max(1);
    let shown = present(w, h, rgba, k);
    let (sw, sh) = (w * k, h * k);
    let mut out = vec![0u8; (side * side * 4) as usize];
    for y in 0..side {
        for x in 0..side {
            let o = ((y * side + x) * 4) as usize;
            out[o..o + 4].copy_from_slice(&if x < sw && y < sh {
                let i = ((y * sw + x) * 4) as usize;
                [shown[i], shown[i + 1], shown[i + 2], 255]
            } else {
                [24, 24, 28, 255]
            });
        }
    }
    out
}

/// Cells laid out in rows, 4 px apart.
fn sheet(rows: &[Vec<Vec<u8>>], side: u32) -> (u32, u32, Vec<u8>) {
    let cols = rows.iter().map(Vec::len).max().unwrap() as u32;
    let gap = 4;
    let (w, h) = (
        cols * (side + gap) + gap,
        rows.len() as u32 * (side + gap) + gap,
    );
    let mut out = [16u8, 16, 18, 255].repeat((w * h) as usize);
    for (r, row) in rows.iter().enumerate() {
        for (c, px) in row.iter().enumerate() {
            let (x0, y0) = (gap + c as u32 * (side + gap), gap + r as u32 * (side + gap));
            for y in 0..side {
                let src = &px[(y * side * 4) as usize..][..(side * 4) as usize];
                let o = (((y0 + y) * w + x0) * 4) as usize;
                out[o..o + src.len()].copy_from_slice(src);
            }
        }
    }
    (w, h, out)
}

/// What `platform` samples of `logical`, by the format `auto` or the
/// manifest chose: (format, PSNR, pixels).
fn kept(
    assets: &Path,
    manifest: &Manifest,
    logical: &str,
    platform: Platform,
) -> (String, f64, Vec<u8>) {
    let (settings, m) = texture::master(assets, manifest, logical, platform).unwrap();
    let c = texture::choose(
        platform, logical, m.source, &settings, m.width, m.height, &m.rgba,
    )
    .unwrap();
    let (w, h) = (m.width, m.height);
    let px = match platform {
        Platform::Pc => {
            BcFormat::parse(&c.format).map_or(m.rgba.clone(), |f| f.preview(w, h, &m.rgba))
        }
        Platform::N3ds => N3dsFormat::parse(&c.format).unwrap().preview(w, h, &m.rgba),
        Platform::N64 => {
            let f = N64Format::parse(&c.format).unwrap();
            f.preview(w, h, &texture::n64::stage(m.source, f, &m.rgba))
                .unwrap_or_else(|_| m.rgba.clone())
        }
        Platform::Web => texture::Ktx2Mode::parse(&c.format).map_or(m.rgba.clone(), |k| {
            texture::ktx2::encode(k, w, h, &m.rgba).unwrap().decoded
        }),
    };
    (c.format, texture::psnr(&m.rgba, &px), px)
}

#[test]
#[ignore = "evidence: writes target/evidence/texture_auto"]
fn auto_bc_and_n64_evidence() {
    let out = root().join("target/evidence/texture_auto");
    let side = 128;
    let mut rows = Vec::new();
    for (dir, logical) in [
        ("assets", "sprites/tex_bricks"),
        ("assets", "sprites/tex_planks"),
        ("assets", "sprites/tex_water"),
        ("assets", "sprites/player"),
        ("assets", "sprites/coin"),
        ("assets", "sprites/impostor-selftest-boar"),
        ("assets", "sprites/selftest_photo"),
        ("examples/solid-eleven/assets", "sprites/se_glow"),
        ("examples/solid-eleven/assets", "fonts/se_ui"),
    ] {
        let assets = root().join(dir);
        let manifest = Manifest::load(&assets.join("manifest.toml")).unwrap();
        let (_, m) = texture::master(&assets, &manifest, logical, Platform::Pc).unwrap();
        let mut row = vec![cell(m.width, m.height, &m.rgba, side)];
        let mut line = format!("{logical:<36}");
        for p in [Platform::Pc, Platform::N3ds, Platform::N64] {
            let (f, db, px) = kept(&assets, &manifest, logical, p);
            line.push_str(&format!(" {:<4} {f:<8} {db:>5.1} dB", p.key()));
            row.push(cell(m.width, m.height, &px, side));
        }
        println!("{line}");
        rows.push(row);
    }
    let (w, h, px) = sheet(&rows, side);
    write_png(&out.join("auto_sheet.png"), w, h, &px);

    // BC1 / BC3 / BC7 on the photo.
    let (pw, ph, photo) = read_png(&root().join("assets/shared/sprites/selftest_photo.png"));
    let mut row = vec![cell(pw, ph, &photo, 256)];
    for f in BcFormat::ALL {
        let p = f.preview(pw, ph, &photo);
        println!(
            "{:<4} {:>5} B  PSNR {:.1} dB",
            f.name(),
            f.texture_bytes(pw, ph),
            texture::psnr(&photo, &p)
        );
        row.push(cell(pw, ph, &p, 256));
    }
    let (w, h, px) = sheet(&[row], 256);
    write_png(&out.join("bc_photo.png"), w, h, &px);

    // The N64 glow: mksprite's solid disc from the master vs the staging.
    let (gw, gh, glow) =
        read_png(&root().join("examples/solid-eleven/assets/shared/sprites/se_glow.png"));
    let raw = N64Format::I4.preview(gw, gh, &glow).unwrap();
    let staged = N64Format::I4
        .preview(
            gw,
            gh,
            &texture::n64::stage(Source::Sprite, N64Format::I4, &glow),
        )
        .unwrap();
    println!(
        "se_glow I4: as the master {:.1} dB, staged {:.1} dB",
        texture::psnr(&glow, &raw),
        texture::psnr(&glow, &staged)
    );
    let row = vec![
        cell(gw, gh, &glow, 192),
        cell(gw, gh, &raw, 192),
        cell(gw, gh, &staged, 192),
    ];
    let (w, h, px) = sheet(&[row], 192);
    write_png(&out.join("n64_glow.png"), w, h, &px);
    println!("wrote {}", out.display());
}
