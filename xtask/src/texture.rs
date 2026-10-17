//! `cargo xtask texture inspect <sprites/x|fonts/x> [--platform pc|n64|3ds]
//! [--json]` — what each platform keeps of a texture: its format, bytes,
//! PSNR against the master and its share of the platform's texture memory
//! (`trino_asset_pipeline::texture::inspect`).
//!
//! On the 3DS the PSNR comes from the pipeline's own encoder (what the PC's
//! 3DS profile draws). With devkitPro installed and a power-of-two master,
//! the real `tex3ds` output is decoded too (`psnr_tex3ds`).

use std::path::Path;
use std::process::{Command, ExitCode};

use trino_asset_pipeline::texture::{self, N3dsFormat, TextureReport};
use trino_asset_pipeline::{Manifest, Platform};

const USAGE: &str = "xtask: usage:
  cargo xtask texture inspect <sprites/x|fonts/x> [--platform pc|n64|3ds] [--json]";

pub fn run(root: &Path, args: &[&str]) -> ExitCode {
    match args {
        ["inspect", logical, rest @ ..] => inspect(root, logical, rest),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn inspect(root: &Path, logical: &str, rest: &[&str]) -> ExitCode {
    let mut platforms = Platform::ALL.to_vec();
    let mut json = false;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match *a {
            "--json" => json = true,
            "--platform" => match it.next().and_then(|p| Platform::parse(p)) {
                Some(p) => platforms = vec![p],
                None => {
                    eprintln!("xtask: --platform needs pc, n64 or 3ds");
                    return ExitCode::FAILURE;
                }
            },
            other => {
                eprintln!("xtask: unknown argument `{other}`\n{USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }
    let assets = root.join("assets");
    let reports = match texture::inspect(&assets, logical, &platforms) {
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
                Some(p) => format!("{p:.1}% of {} ({} B)", r.memory, r.memory_budget.unwrap()),
                None => "no budget".into(),
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
            if let Some(n) = &r.note {
                println!("       note: {n}");
            }
        }
    }
    ExitCode::SUCCESS
}

fn db(v: Option<f64>) -> String {
    match v {
        Some(v) if v.is_infinite() => "lossless".into(),
        Some(v) => format!("{v:.1} dB"),
        None => "n/a".into(),
    }
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
