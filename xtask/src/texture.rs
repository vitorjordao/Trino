//! `cargo xtask texture inspect <sprites/x|fonts/x> [--platform pc|n64|3ds|web]
//! [--json]` — what each platform keeps of a texture: its format and why
//! (`auto`'s weighing, with every candidate it measured), bytes, PSNR
//! against the master and its share of the platform's texture memory
//! (`trino_asset_pipeline::texture::inspect`).
//!
//! `cargo xtask texture report [--platform ..] [--json]` — every sprite and
//! font atlas of the project, biggest first, with each platform's totals
//! against RGBA8. A texture whose bake would fail (a declared format below
//! its `min_psnr`) is listed with the error and fails the command.
//!
//! On the 3DS the PSNR comes from the pipeline's own encoder (what the PC's
//! 3DS profile draws). With devkitPro installed and a power-of-two master,
//! `inspect` decodes the real `tex3ds` output too (`psnr_tex3ds`).
//!
//! On the Web the bytes are the download (the `.ktx2` file, or RGBA8) and
//! the memory is its VRAM once a browser transcodes it; the PSNR is of
//! Binomial's own decode (the transcoder the browser runs).

use std::path::Path;
use std::process::{Command, ExitCode};

use trino_asset_pipeline::texture::{self, Candidate, N3dsFormat, TextureReport, db};
use trino_asset_pipeline::{Manifest, Platform};

const USAGE: &str = "xtask: usage:
  cargo xtask texture inspect <sprites/x|fonts/x> [--platform pc|n64|3ds|web] [--json]
  cargo xtask texture report [--platform pc|n64|3ds|web] [--json]";

pub fn run(root: &Path, args: &[&str]) -> ExitCode {
    let (cmd, rest) = match args {
        ["inspect", logical, rest @ ..] => (Some(*logical), rest),
        ["report", rest @ ..] => (None, rest),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let Some((platforms, json)) = options(rest) else {
        return ExitCode::FAILURE;
    };
    match cmd {
        Some(logical) => inspect(root, logical, &platforms, json),
        None => report(root, &platforms, json),
    }
}

/// `--platform` and `--json`.
fn options(rest: &[&str]) -> Option<(Vec<Platform>, bool)> {
    let mut platforms = Platform::ALL.to_vec();
    let mut json = false;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match *a {
            "--json" => json = true,
            "--platform" => match it.next().and_then(|p| Platform::parse(p)) {
                Some(p) => platforms = vec![p],
                None => {
                    eprintln!("xtask: --platform needs pc, n64, 3ds or web");
                    return None;
                }
            },
            other => {
                eprintln!("xtask: unknown argument `{other}`\n{USAGE}");
                return None;
            }
        }
    }
    Some((platforms, json))
}

fn inspect(root: &Path, logical: &str, platforms: &[Platform], json: bool) -> ExitCode {
    let assets = root.join("assets");
    let reports = match texture::inspect(&assets, logical, platforms) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let measured: Vec<Option<f64>> = reports
        .iter()
        .map(|r| {
            (r.platform == Platform::N3ds)
                .then(|| tex3ds_psnr(&assets, logical, r))
                .flatten()
        })
        .collect();
    if json {
        let list: Vec<serde_json::Value> = reports
            .iter()
            .zip(&measured)
            .map(|(r, m)| to_json(r, *m))
            .collect();
        let doc = serde_json::json!({ "texture": logical, "platforms": list });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap());
    } else {
        println!("{logical}");
        for (r, m) in reports.iter().zip(&measured) {
            let share = match r.share_percent() {
                Some(p) if p < 0.1 => {
                    format!("{p:.3}% of {} ({} B)", r.memory, r.memory_budget.unwrap())
                }
                Some(p) => format!("{p:.1}% of {} ({} B)", r.memory, r.memory_budget.unwrap()),
                None => format!("{} (no budget)", r.memory),
            };
            let mut psnr = db(r.psnr);
            if let Some(m) = m {
                psnr.push_str(&format!(" (tex3ds {})", db(Some(*m))));
            }
            println!(
                "  {:<4} {:<8} {}x{}  {:>8} B  PSNR {psnr}  {share}",
                r.platform.key(),
                r.format,
                r.width,
                r.height,
                r.bytes
            );
            println!("       why: {}", r.reason);
            if let Some(n) = &r.note {
                println!("       note: {n}");
            }
            if !r.candidates.is_empty() {
                let list: Vec<String> = r
                    .candidates
                    .iter()
                    .map(|c| format!("{} {} B {}", c.format, c.bytes, db(c.psnr)))
                    .collect();
                println!("       measured: {}", list.join(" | "));
            }
        }
    }
    ExitCode::SUCCESS
}

/// The bytes a platform would take with RGBA8 (the 3DS pads), for the
/// report's savings; none on the N64 (no RGBA8 there).
fn rgba8_bytes(p: Platform, w: u32, h: u32) -> Option<u64> {
    match p {
        Platform::Pc => Some(w as u64 * h as u64 * 4),
        Platform::N3ds => Some(N3dsFormat::Rgba8.texture_bytes(w, h)),
        Platform::Web => Some(w as u64 * h as u64 * 4),
        Platform::N64 => None,
    }
}

fn report(root: &Path, platforms: &[Platform], json: bool) -> ExitCode {
    let entries = match texture::report_project(&root.join("assets"), platforms) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let size = |e: &texture::TextureEntry| {
        e.platforms
            .iter()
            .filter_map(|(_, r)| r.as_ref().ok().map(|r| r.bytes))
            .max()
            .unwrap_or(0)
    };
    let mut entries = entries;
    entries.sort_by(|a, b| size(b).cmp(&size(a)).then(a.logical.cmp(&b.logical)));
    let failed = entries
        .iter()
        .any(|e| e.platforms.iter().any(|(_, r)| r.is_err()));
    // Per-platform totals: bytes, bytes as RGBA8, textures `auto` picked.
    let totals: Vec<serde_json::Value> = platforms
        .iter()
        .map(|&p| {
            let reports = || {
                entries
                    .iter()
                    .flat_map(|e| &e.platforms)
                    .filter(move |(q, _)| *q == p)
                    .filter_map(|(_, r)| r.as_ref().ok())
            };
            let bytes: u64 = reports().map(|r| r.bytes).sum();
            let rgba8: Option<u64> = reports().map(|r| rgba8_bytes(p, r.width, r.height)).sum();
            let mut formats = std::collections::BTreeMap::<&str, u32>::new();
            for r in reports() {
                *formats.entry(r.format.as_str()).or_default() += 1;
            }
            serde_json::json!({
                "platform": p.key(),
                "bytes": bytes,
                "rgba8_bytes": rgba8,
                "auto": reports().filter(|r| r.auto).count(),
                "formats": formats,
            })
        })
        .collect();
    if json {
        let list: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                let per: Vec<serde_json::Value> = e
                    .platforms
                    .iter()
                    .map(|(p, r)| match r {
                        Ok(r) => to_json(r, None),
                        Err(err) => serde_json::json!({ "platform": p.key(), "error": err }),
                    })
                    .collect();
                serde_json::json!({ "texture": e.logical, "platforms": per })
            })
            .collect();
        let doc = serde_json::json!({ "textures": list, "totals": totals });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap());
    } else {
        println!("{} textures, biggest first", entries.len());
        for e in &entries {
            let cells: Vec<String> = e
                .platforms
                .iter()
                .map(|(p, r)| match r {
                    Ok(r) => format!(
                        "{} {}{} {} B {}",
                        p.key(),
                        r.format,
                        if r.auto { "*" } else { "" },
                        r.bytes,
                        db(r.psnr)
                    ),
                    Err(_) => format!("{} FAILS", p.key()),
                })
                .collect();
            println!("  {:<32} {}", e.logical, cells.join("  |  "));
            for (_, r) in &e.platforms {
                if let Err(err) = r {
                    println!("      error: {err}");
                }
            }
        }
        println!("  (* = picked by auto)");
        for t in &totals {
            let rgba8 = t["rgba8_bytes"]
                .as_u64()
                .map_or(String::new(), |b| format!(" (RGBA8: {b} B)"));
            println!(
                "  total {:<4} {} B{rgba8}, {} by auto: {}",
                t["platform"].as_str().unwrap(),
                t["bytes"],
                t["auto"],
                t["formats"]
            );
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn candidate_json(c: &Candidate) -> serde_json::Value {
    serde_json::json!({
        "format": c.format,
        "bytes": c.bytes,
        "psnr": c.psnr.filter(|v| v.is_finite()),
        "lossless": c.psnr.is_some_and(f64::is_infinite),
        "fits": c.fits,
        "note": c.note,
    })
}

/// JSON numbers can't be infinite: lossless is `"psnr": null, "lossless": true`.
fn to_json(r: &TextureReport, tex3ds: Option<f64>) -> serde_json::Value {
    let finite = |v: Option<f64>| v.filter(|v| v.is_finite());
    serde_json::json!({
        "platform": r.platform.key(),
        "format": r.format,
        "width": r.width,
        "height": r.height,
        "bytes": r.bytes,
        "psnr": finite(r.psnr),
        "lossless": r.psnr.is_some_and(f64::is_infinite),
        "psnr_tex3ds": finite(tex3ds),
        "memory": if r.memory.is_empty() { None } else { Some(r.memory) },
        "memory_bytes": r.memory_bytes,
        "memory_budget": r.memory_budget,
        "memory_percent": r.share_percent(),
        "note": r.note,
        "auto": r.auto,
        "reason": r.reason,
        "threshold": r.threshold,
        "content": r.content,
        "candidates": r.candidates.iter().map(candidate_json).collect::<Vec<_>>(),
    })
}

/// PSNR of the real `tex3ds` output, when devkitPro is installed and the
/// master is a sprite with power-of-two sides (raw output is unpadded).
fn tex3ds_psnr(assets: &Path, logical: &str, r: &TextureReport) -> Option<f64> {
    let format = N3dsFormat::parse(&r.format)?;
    let pow2 = |n: u32| n >= 8 && n.is_power_of_two();
    if !pow2(r.width) || !pow2(r.height) {
        return None;
    }
    let name = logical.strip_prefix("sprites/")?;
    let tex3ds = crate::n3ds::tex3ds_tool().ok()?;
    let manifest = Manifest::load(&assets.join("manifest.toml")).ok()?;
    let source = trino_asset_pipeline::resolve_source(
        assets,
        Platform::N3ds,
        &manifest.sprites.get(name)?.file,
    )
    .ok()?;
    let dir = assets.join("../target/texture-inspect");
    std::fs::create_dir_all(&dir).ok()?;
    let out = dir.join(format!("{name}.{}.raw", format.tex3ds()));
    let ok = Command::new(tex3ds)
        .args(["-f", format.tex3ds(), "-r", "-z", "none", "-o"])
        .arg(&out)
        .arg(&source)
        .output()
        .ok()?
        .status
        .success();
    if !ok {
        return None;
    }
    let raw = std::fs::read(&out).ok()?;
    let decoded = texture::decode_tex3ds_raw(format, r.width, r.height, &raw)?;
    let master = png_rgba(&source)?;
    Some(texture::psnr(&master, &decoded))
}

fn png_rgba(path: &Path) -> Option<Vec<u8>> {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    match info.color_type {
        png::ColorType::Rgba => Some(buf),
        png::ColorType::Rgb => Some(
            buf.as_chunks::<3>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect(),
        ),
        _ => None,
    }
}
