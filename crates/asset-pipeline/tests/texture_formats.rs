//! 3DS texture formats (issue #52): every format's preview against its
//! master (PSNR thresholds below), the PC bake's `preview_3ds` round trip,
//! `texture::inspect`, and — where devkitPro is installed — the preview
//! against what the real `tex3ds` writes.

use std::path::{Path, PathBuf};
use std::process::Command;

use trino_asset_pipeline::texture::{self, N3dsFormat};
use trino_asset_pipeline::{Platform, bake_all, load_dir};
use trino_core::asset_id;

const W: u32 = 64;
const H: u32 = 64;

/// A photo-like master: smooth color gradients, a hard-edged disc, a few
/// noisy texels and an alpha ramp in the right half (0 and 255 included).
fn master() -> Vec<u8> {
    let mut seed = 0x1234_5678u32;
    let mut noise = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed & 15) as i32 - 8
    };
    let mut out = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        for x in 0..W {
            let disc = (x as i32 - 20).pow(2) + (y as i32 - 40).pow(2) < 144;
            let (r, g, b) = if disc {
                (230, 40, 30)
            } else {
                (x * 4, y * 4, 255 - (x + y) * 2)
            };
            let n = noise();
            let c = |v: u32| (v as i32 + n).clamp(0, 255) as u8;
            let a = if x < W / 2 {
                255
            } else {
                ((y * 255) / (H - 1)) as u8
            };
            out.extend_from_slice(&[c(r), c(g), c(b), a]);
        }
    }
    out
}

/// The least PSNR (dB) each format's preview must keep: on [`master`] for
/// formats with alpha, on [`opaque`] for the others. Measured values (and
/// `tex3ds`'s, `preview_matches_tex3ds`) sit 0.5–1 dB above. RGBA5551's
/// 1-bit alpha flattens the ramp; LA8/L8/A8 drop the color by design, so
/// their bar is low but still catches a wrong channel.
fn threshold(f: N3dsFormat) -> f64 {
    match f {
        N3dsFormat::Rgba8 => f64::INFINITY,
        N3dsFormat::Rgb565 => 39.5,   // measured 40.5
        N3dsFormat::Rgba5551 => 16.0, // 16.8
        N3dsFormat::Rgba4 => 32.0,    // 32.7
        N3dsFormat::La8 => 12.0,      // 12.6
        N3dsFormat::L8 => 11.0,       // 11.5
        N3dsFormat::A8 => 7.0,        // 7.3
        N3dsFormat::Etc1 => 28.0,     // 28.75 (tex3ds medium: within 0.1)
        N3dsFormat::Etc1a4 => 28.0,   // 28.45
    }
}

/// [`master`] made opaque: what a format without alpha is for.
fn opaque() -> Vec<u8> {
    master()
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect()
}

#[test]
fn every_format_keeps_its_psnr() {
    for f in N3dsFormat::ALL {
        let m = if f.has_alpha() { master() } else { opaque() };
        let p = f.preview(W, H, &m);
        assert_eq!(p.len(), m.len());
        let db = texture::psnr(&m, &p);
        println!("{:<8} {db:.2} dB", f.name());
        assert!(
            db >= threshold(f),
            "{}: {db:.2} dB < {}",
            f.name(),
            threshold(f)
        );
        if !f.has_alpha() {
            assert!(
                p.chunks_exact(4).all(|t| t[3] == 255),
                "{} is opaque",
                f.name()
            );
        }
    }
}

/// ETC1 on an opaque image (the format's intended use) keeps 28 dB, like
/// `tex3ds`'s medium quality; odd sizes clamp their edge blocks.
#[test]
fn etc1_keeps_opaque_images() {
    let opaque = opaque();
    let db = texture::psnr(&opaque, &N3dsFormat::Etc1.preview(W, H, &opaque));
    assert!(db >= threshold(N3dsFormat::Etc1), "ETC1 {db:.2} dB");
    // Odd sizes (edge blocks clamped) work too.
    let small: Vec<u8> = opaque[..(10 * 6 * 4)].to_vec();
    assert_eq!(N3dsFormat::Etc1.preview(10, 6, &small).len(), small.len());
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

/// A project with one sprite per 3DS format.
fn project(dir: &Path) -> PathBuf {
    let root = dir.join("assets");
    write_png(&root.join("shared/sprites/m.png"), W, H, &master());
    let mut manifest = String::from("version = 1\n");
    for f in N3dsFormat::ALL {
        manifest.push_str(&format!(
            "\n[sprites.{}]\nfile = \"sprites/m.png\"\nformats = {{ n64 = \"RGBA5551\", 3ds = \"{}\" }}\n",
            f.tex3ds(),
            f.name()
        ));
    }
    std::fs::write(root.join("manifest.toml"), manifest).unwrap();
    root
}

#[test]
fn pc_bake_writes_and_loads_the_3ds_preview() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path());
    let out = dir.path().join("baked");
    bake_all(&root, Platform::Pc, &out).unwrap_or_else(|e| panic!("{e}"));
    let index = std::fs::read_to_string(out.join("index.toml")).unwrap();
    let loaded = load_dir(&out, None).unwrap();
    for f in N3dsFormat::ALL {
        let id = asset_id(&format!("sprites/{}", f.tex3ds()));
        let s = loaded.sprites.iter().find(|s| s.id == id).unwrap();
        assert_eq!(s.rgba, master(), "the PC keeps the master");
        assert_eq!(s.pixels_for(Platform::Pc), &s.rgba[..]);
        if f == N3dsFormat::Rgba8 {
            assert!(s.preview_3ds.is_none(), "RGBA8 needs no preview");
            continue;
        }
        assert!(index.contains(&format!("preview_3ds = \"{id:08x}.3ds.sprite\"")));
        assert_eq!(
            s.pixels_for(Platform::N3ds),
            &f.preview(W, H, &master())[..]
        );
    }
    // The 3DS and N64 bakes write no preview of their own.
    let n64 = dir.path().join("n64");
    bake_all(&root, Platform::N64, &n64).unwrap();
    assert!(
        !std::fs::read_to_string(n64.join("index.toml"))
            .unwrap()
            .contains("preview_3ds")
    );
}

#[test]
fn a_bad_3ds_format_fails_the_pc_bake_with_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("assets");
    write_png(&root.join("shared/sprites/m.png"), 8, 8, &[255; 8 * 8 * 4]);
    std::fs::write(
        root.join("manifest.toml"),
        "version = 1\n[sprites.m]\nfile = \"sprites/m.png\"\nformats = { n64 = \"CI4\", 3ds = \"BC1\" }\n",
    )
    .unwrap();
    let err = bake_all(&root, Platform::Pc, &dir.path().join("out"))
        .unwrap_err()
        .to_string();
    for name in [
        "BC1", "ETC1", "ETC1A4", "RGBA4", "RGBA5551", "LA8", "L8", "A8",
    ] {
        assert!(err.contains(name), "{name} missing from: {err}");
    }
}

#[test]
fn inspect_reports_each_platform() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path());
    let r = texture::inspect(&root, "sprites/etc1", &Platform::ALL).unwrap();
    assert_eq!(r.len(), 3);
    let (pc, n64, n3ds) = (&r[0], &r[1], &r[2]);
    assert_eq!((pc.format.as_str(), pc.bytes), ("RGBA8", 64 * 64 * 4));
    assert!(pc.psnr.unwrap().is_infinite() && pc.share_percent().is_none());
    assert_eq!((n3ds.format.as_str(), n3ds.bytes), ("ETC1", 64 * 64 / 2));
    let preview = N3dsFormat::Etc1.preview(W, H, &master());
    assert_eq!(n3ds.psnr, Some(texture::psnr(&master(), &preview)));
    assert_eq!(n3ds.memory_budget, Some(6 * 1024 * 1024));
    // 64×64 RGBA5551 is 8 KB: twice TMEM, and said so.
    assert_eq!((n64.format.as_str(), n64.bytes), ("RGBA5551", 8192));
    assert!(n64.share_percent().unwrap() > 100.0);
    assert!(n64.note.as_deref().unwrap().contains("overflow TMEM"));
    assert!(texture::inspect(&root, "sprites/ghost", &Platform::ALL).is_err());
    assert!(texture::inspect(&root, "models/x", &Platform::ALL).is_err());
}

fn tex3ds() -> Option<PathBuf> {
    let dkp = std::env::var_os("DEVKITPRO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/devkitpro"));
    let t = dkp.join("tools/bin/tex3ds");
    t.is_file().then_some(t)
}

/// The preview against the real encoder: the uncompressed formats texel
/// for texel (luminance within 1), ETC1/ETC1A4 within 1.5 dB of `tex3ds`'s
/// own PSNR. Skips without devkitPro (`TRINO_REQUIRE_TEX3DS=1` fails).
#[test]
fn preview_matches_tex3ds() {
    let Some(tex3ds) = tex3ds() else {
        assert!(
            std::env::var("TRINO_REQUIRE_TEX3DS").is_err(),
            "tex3ds not found"
        );
        eprintln!("skipping: tex3ds not found");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let m = master();
    let png = dir.path().join("m.png");
    write_png(&png, W, H, &m);
    for f in N3dsFormat::ALL {
        let raw = dir.path().join(format!("{}.raw", f.tex3ds()));
        let ok = Command::new(&tex3ds)
            .args(["-f", f.tex3ds(), "-r", "-z", "none", "-o"])
            .arg(&raw)
            .arg(&png)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "tex3ds -f {} failed", f.tex3ds());
        let real =
            texture::decode_tex3ds_raw(f, W, H, &std::fs::read(&raw).unwrap()).expect("decodes");
        let ours = f.preview(W, H, &m);
        let (db_real, db_ours) = (texture::psnr(&m, &real), texture::psnr(&m, &ours));
        println!(
            "{:<8} tex3ds {db_real:.2} dB, trino {db_ours:.2} dB",
            f.name()
        );
        match f {
            N3dsFormat::Etc1 | N3dsFormat::Etc1a4 => {
                assert!(
                    db_ours >= db_real - 1.5,
                    "{}: {db_ours:.2} vs {db_real:.2}",
                    f.name()
                );
                // Same alpha nibbles in ETC1A4.
                for (a, b) in real.chunks_exact(4).zip(ours.chunks_exact(4)) {
                    assert_eq!(a[3], b[3], "{} alpha", f.name());
                }
            }
            _ => {
                for (i, (a, b)) in real.chunks_exact(4).zip(ours.chunks_exact(4)).enumerate() {
                    let tol = if matches!(f, N3dsFormat::L8 | N3dsFormat::La8) {
                        1
                    } else {
                        0
                    };
                    let close = (0..4).all(|c| (a[c] as i32 - b[c] as i32).abs() <= tol);
                    assert!(close, "{} texel {i}: tex3ds {a:?}, trino {b:?}", f.name());
                }
            }
        }
    }
}
