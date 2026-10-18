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
        .as_chunks::<4>()
        .0
        .iter()
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
                p.as_chunks::<4>().0.iter().all(|t| t[3] == 255),
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
    assert_eq!(r.len(), 4);
    let (pc, n64, n3ds, web) = (&r[0], &r[1], &r[2], &r[3]);
    // The PC is `auto` by default: a color image with an alpha ramp takes
    // a byte a texel (BC3 or BC7, whichever keeps more).
    assert!(pc.auto && pc.format.starts_with("BC"), "{}", pc.reason);
    assert_eq!(pc.bytes, 64 * 64);
    assert!(pc.psnr.unwrap() >= 35.0 && pc.share_percent().is_none());
    assert_eq!((n3ds.format.as_str(), n3ds.bytes), ("ETC1", 64 * 64 / 2));
    let preview = N3dsFormat::Etc1.preview(W, H, &master());
    assert_eq!(n3ds.psnr, Some(texture::psnr(&master(), &preview)));
    assert_eq!(n3ds.memory_budget, Some(6 * 1024 * 1024));
    // 64×64 RGBA5551 is 8 KB: twice TMEM, and said so.
    assert_eq!((n64.format.as_str(), n64.bytes), ("RGBA5551", 8192));
    assert!(n64.share_percent().unwrap() > 100.0);
    assert!(n64.note.as_deref().unwrap().contains("TMEM bytes > 4096"));
    // The Web is `auto` too: a KTX2 (Basis Universal) file, measured
    // through Binomial's own decode; its bytes are the download, its
    // memory the VRAM once transcoded (alpha: a byte a texel).
    assert!(
        web.auto && web.format.starts_with("KTX2-"),
        "{}",
        web.reason
    );
    assert!(web.psnr.unwrap() >= 35.0, "{:?}", web.psnr);
    assert!(web.bytes < 64 * 64 * 4);
    assert_eq!((web.memory, web.memory_bytes), ("VRAM", 64 * 64));
    assert_eq!(web.candidates.len(), 3, "ETC1S, UASTC, RGBA8");
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
                for (a, b) in real
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(ours.as_chunks::<4>().0.iter())
                {
                    assert_eq!(a[3], b[3], "{} alpha", f.name());
                }
            }
            _ => {
                for (i, (a, b)) in real
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(ours.as_chunks::<4>().0.iter())
                    .enumerate()
                {
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

// ---------------------------------------------------------------------------
// Phase 2 and 3 (#52): `auto`, `min_psnr`, `texture report`, PC BCn.

/// A project with one sprite per content kind, every format undeclared
/// (the PC and 3DS default to `auto`; the N64 asks `auto` explicitly).
fn auto_project(dir: &Path) -> PathBuf {
    let root = dir.join("assets");
    let opaque = opaque();
    let mask: Vec<u8> = master()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [255, 255, 255, p[3]])
        .collect();
    write_png(&root.join("shared/sprites/photo.png"), W, H, &opaque);
    write_png(&root.join("shared/sprites/mask.png"), W, H, &mask);
    write_png(
        &root.join("shared/sprites/odd.png"),
        6,
        6,
        &opaque[..6 * 6 * 4],
    );
    std::fs::write(
        root.join("manifest.toml"),
        "version = 1\n\
         [sprites.photo]\nfile = \"sprites/photo.png\"\nformats = { n64 = \"auto\" }\n\
         [sprites.mask]\nfile = \"sprites/mask.png\"\nformats = { n64 = \"RGBA5551\" }\n\
         [sprites.odd]\nfile = \"sprites/odd.png\"\nformats = { n64 = \"auto\" }\nquality = \"low\"\n",
    )
    .unwrap();
    root
}

#[test]
fn the_pc_bake_writes_bcn_the_loader_reads_it_back() {
    let dir = tempfile::tempdir().unwrap();
    let root = auto_project(dir.path());
    let out = dir.path().join("pc");
    let report = bake_all(&root, Platform::Pc, &out).unwrap_or_else(|e| panic!("{e}"));
    let entry = |l: &str| report.entries.iter().find(|e| e.logical == l).unwrap();
    // Opaque: BC1 (4 bits a texel); a white mask: one byte a texel.
    assert_eq!(entry("sprites/photo").format, "BC1");
    assert!(["BC3", "BC7"].contains(&entry("sprites/mask").format.as_str()));
    // 6x6 is not whole 4x4 blocks: RGBA8, no BCn file.
    assert_eq!(entry("sprites/odd").format, "RGBA8");
    assert!(entry("sprites/odd").compressed.is_none());
    let index = std::fs::read_to_string(out.join("index.toml")).unwrap();
    let photo = asset_id("sprites/photo");
    assert!(index.contains(&format!("format = \"BC1\"\npreview_3ds = \"{photo:08x}.3ds.sprite\"\ncompressed = \"{photo:08x}.bc\"")), "{index}");

    let loaded = load_dir(&out, None).unwrap();
    let s = loaded.sprites.iter().find(|s| s.id == photo).unwrap();
    assert_eq!(
        s.rgba,
        opaque(),
        "the master stays for tools and console profiles"
    );
    let c = s.compressed.as_ref().unwrap();
    assert_eq!(c.format, texture::BcFormat::Bc1);
    assert_eq!(
        c.data.len() as u64,
        texture::BcFormat::Bc1.texture_bytes(W, H)
    );
    // What a GPU without BC gets: the blocks decoded, the same as the
    // preview `auto` measured.
    let decoded = s.pc_pixels();
    assert_eq!(
        &decoded[..],
        &texture::BcFormat::Bc1.preview(W, H, &opaque())[..]
    );
    assert!(texture::psnr(&s.rgba, &decoded) >= 35.0);
    // A sprite without BCn: the master.
    let odd = loaded
        .sprites
        .iter()
        .find(|s| s.id == asset_id("sprites/odd"))
        .unwrap();
    assert!(odd.compressed.is_none() && odd.pc_pixels()[..] == odd.rgba[..]);

    // A corrupt BCn file is an error, not a panic.
    std::fs::write(out.join(format!("{photo:08x}.bc")), b"TBCN").unwrap();
    assert!(load_dir(&out, None).err().unwrap().contains("bad BCn blob"));
}

#[test]
fn auto_decides_the_same_on_every_bake_and_in_inspect() {
    let dir = tempfile::tempdir().unwrap();
    let root = auto_project(dir.path());
    for platform in Platform::ALL {
        let out = dir.path().join(platform.key());
        let report = bake_all(&root, platform, &out).unwrap_or_else(|e| panic!("{e}"));
        for e in &report.entries {
            let r = &texture::inspect(&root, &e.logical, &[platform]).unwrap()[0];
            assert_eq!(r.format, e.format, "{} on {}", e.logical, platform.key());
        }
    }
    let r = texture::inspect(&root, "sprites/photo", &Platform::ALL).unwrap();
    // Every `auto` line names its content and the formats it measured.
    for line in &r {
        assert!(line.auto, "{line:?}");
        assert!(line.reason.contains("opaque color"), "{}", line.reason);
        assert!(!line.candidates.is_empty());
        assert!(line.psnr.unwrap() >= 35.0, "{}", line.reason);
    }
    // The photo's noise keeps ETC1 under 35 dB: the 3DS moves up to a
    // 16-bit format, and says why.
    assert_eq!(r[2].format, "RGB565", "{}", r[2].reason);
    assert!(r[2].reason.contains("not ETC1 28."), "{}", r[2].reason);
    // `quality = "low"` keeps 30 dB.
    let odd = texture::inspect(&root, "sprites/odd", &[Platform::N3ds]).unwrap();
    assert_eq!(odd[0].threshold, Some(30.0));
}

#[test]
fn min_psnr_fails_the_bake_with_what_would_pass() {
    let dir = tempfile::tempdir().unwrap();
    let root = auto_project(dir.path());
    let text = std::fs::read_to_string(root.join("manifest.toml")).unwrap();
    std::fs::write(
        root.join("manifest.toml"),
        text.replace(
            "formats = { n64 = \"RGBA5551\" }",
            "formats = { n64 = \"RGBA5551\", 3ds = \"ETC1\" }\nmin_psnr = 30",
        ),
    )
    .unwrap();
    // The PC bake decides the 3DS format too, so it fails first.
    let err = bake_all(&root, Platform::Pc, &dir.path().join("pc"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("`sprites/mask`: 3ds ETC1 keeps"), "{err}");
    assert!(err.contains("< min_psnr 30 (white mask)"), "{err}");
    assert!(err.contains("`auto` (picks A8, lossless)"), "{err}");
    assert!(err.contains("or lower `min_psnr`"), "{err}");
    // The report lists it and keeps going.
    let entries = texture::report_project(&root, &[Platform::N3ds]).unwrap();
    assert_eq!(entries.len(), 3);
    let mask = entries
        .iter()
        .find(|e| e.logical == "sprites/mask")
        .unwrap();
    assert!(mask.platforms[0].1.is_err());
    assert!(entries.iter().filter(|e| e.platforms[0].1.is_ok()).count() == 2);
}

/// Palette, gray and 16-bit masters decode to RGBA8 (an N64 override keeps
/// its CI palette as an indexed PNG, and the N64 bake measures it).
#[test]
fn indexed_gray_and_16_bit_masters_decode() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str,
                 color: png::ColorType,
                 depth: png::BitDepth,
                 data: &[u8],
                 palette: Option<&[u8]>| {
        let path = dir.path().join(name);
        let mut enc = png::Encoder::new(std::fs::File::create(&path).unwrap(), 2, 1);
        enc.set_color(color);
        enc.set_depth(depth);
        if let Some(p) = palette {
            enc.set_palette(p.to_vec());
            enc.set_trns(vec![255, 0]);
        }
        enc.write_header().unwrap().write_image_data(data).unwrap();
        trino_asset_pipeline::decode_png(&path).unwrap()
    };
    let indexed = write(
        "i.png",
        png::ColorType::Indexed,
        png::BitDepth::Eight,
        &[1, 0],
        Some(&[10, 20, 30, 40, 50, 60]),
    );
    assert_eq!(indexed, (2, 1, vec![40, 50, 60, 0, 10, 20, 30, 255]));
    let gray = write(
        "g.png",
        png::ColorType::Grayscale,
        png::BitDepth::Eight,
        &[7, 200],
        None,
    );
    assert_eq!(gray.2, [7, 7, 7, 255, 200, 200, 200, 255]);
    let ga = write(
        "ga.png",
        png::ColorType::GrayscaleAlpha,
        png::BitDepth::Eight,
        &[9, 128, 1, 2],
        None,
    );
    assert_eq!(ga.2, [9, 9, 9, 128, 1, 1, 1, 2]);
    let deep = write(
        "d.png",
        png::ColorType::Rgb,
        png::BitDepth::Sixteen,
        &[0xff, 0, 0x80, 0, 0, 0, 1, 2, 3, 4, 5, 6],
        None,
    );
    assert_eq!(deep.2, [255, 128, 0, 255, 1, 3, 5, 255]);
}
