//! Evidence for the 3DS texture formats (#52), written to
//! `target/evidence/texture_formats/` (`cargo test -p xtask --test
//! texture_shots -- --ignored`):
//! - `formats_<name>.png`: the photo-like master with an alpha ramp, as each
//!   3DS format keeps it (`N3dsFormat::preview`, what the PC's 3DS profile
//!   draws), over a checkerboard, 4× nearest;
//! - `azahar_vs_pc.png`: the photo row of the 3DS golden
//!   (`tests/golden/3ds/texture_formats.png`, Azahar) above the same row
//!   drawn by the PC's 3DS profile, 3×.

use std::path::{Path, PathBuf};

use trino_asset_pipeline::texture::{self, N3dsFormat};
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
