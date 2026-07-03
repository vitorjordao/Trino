//! Bake: manifest + masters → native binary blobs + `index.toml`.
//!
//! PC blob formats (little-endian):
//! - `<hash>.sprite`: magic `TSPR`, u32 width, u32 height, RGBA8 pixels.
//! - `<hash>.sound`:  magic `TSND`, u32 sample_rate, u32 frame count,
//!   f32 mono samples.
//!
//! Writes are skipped when the output bytes are unchanged, so watchers can
//! diff by "did the file change" and reload only what moved.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use trino_core::asset_id;

use crate::manifest::{Manifest, Platform};
use crate::resolve::resolve_source;

#[derive(Debug)]
pub struct BakeError(pub Vec<String>);

impl fmt::Display for BakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "asset bake failed with {} error(s):", self.0.len())?;
        for e in &self.0 {
            writeln!(f, "  - {e}")?;
        }
        Ok(())
    }
}

impl std::error::Error for BakeError {}

/// One baked asset, as recorded in `index.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct BakedEntry {
    pub logical: String,
    pub id: u32,
    pub kind: &'static str, // "sprite" | "sound"
    pub file: String,       // output file name
    pub format: String,
    /// True if this bake changed the output bytes on disk.
    pub changed: bool,
}

#[derive(Debug, Default)]
pub struct BakeReport {
    pub entries: Vec<BakedEntry>,
}

impl BakeReport {
    pub fn changed_ids(&self) -> Vec<u32> {
        self.entries
            .iter()
            .filter(|e| e.changed)
            .map(|e| e.id)
            .collect()
    }
}

/// Bake every asset in `<assets_root>/manifest.toml` for `platform` into
/// `out_dir`. Collects **all** errors instead of stopping at the first.
pub fn bake_all(
    assets_root: &Path,
    platform: Platform,
    out_dir: &Path,
) -> Result<BakeReport, BakeError> {
    let manifest =
        Manifest::load(&assets_root.join("manifest.toml")).map_err(|e| BakeError(vec![e]))?;

    let mut errors = Vec::new();
    let mut by_id: BTreeMap<u32, String> = BTreeMap::new();
    let mut report = BakeReport::default();

    if let Err(e) = std::fs::create_dir_all(out_dir) {
        return Err(BakeError(vec![format!(
            "cannot create {}: {e}",
            out_dir.display()
        )]));
    }

    for (name, decl) in &manifest.sprites {
        let logical = format!("sprites/{name}");
        let id = asset_id(&logical);
        if let Some(other) = by_id.insert(id, logical.clone()) {
            errors.push(format!("hash collision between `{other}` and `{logical}`"));
            continue;
        }
        let result = (|| -> Result<BakedEntry, String> {
            let format = decl.format_for(&logical, platform)?;
            let source = resolve_source(assets_root, platform, &decl.file)?;
            let (w, h, rgba) = decode_png(&source)?;
            // PC bakes RGBA8 regardless; console formats are converted by
            // their platform bakers (Fase 4/5) — the format is validated for
            // every platform *now* so errors surface before a console port.
            let mut blob = Vec::with_capacity(12 + rgba.len());
            blob.extend_from_slice(b"TSPR");
            blob.extend_from_slice(&w.to_le_bytes());
            blob.extend_from_slice(&h.to_le_bytes());
            blob.extend_from_slice(&rgba);
            let file = format!("{id:08x}.sprite");
            let changed = write_if_changed(&out_dir.join(&file), &blob)?;
            Ok(BakedEntry {
                logical: logical.clone(),
                id,
                kind: "sprite",
                file,
                format,
                changed,
            })
        })();
        match result {
            Ok(entry) => report.entries.push(entry),
            Err(e) => errors.push(e),
        }
    }

    for (name, decl) in &manifest.sounds {
        let logical = format!("sounds/{name}");
        let id = asset_id(&logical);
        if let Some(other) = by_id.insert(id, logical.clone()) {
            errors.push(format!("hash collision between `{other}` and `{logical}`"));
            continue;
        }
        let result = (|| -> Result<BakedEntry, String> {
            let source = resolve_source(assets_root, platform, &decl.file)?;
            let (rate, samples) = decode_wav(&source)?;
            let mut blob = Vec::with_capacity(12 + samples.len() * 4);
            blob.extend_from_slice(b"TSND");
            blob.extend_from_slice(&rate.to_le_bytes());
            blob.extend_from_slice(&(samples.len() as u32).to_le_bytes());
            for s in &samples {
                blob.extend_from_slice(&s.to_le_bytes());
            }
            let file = format!("{id:08x}.sound");
            let changed = write_if_changed(&out_dir.join(&file), &blob)?;
            Ok(BakedEntry {
                logical: logical.clone(),
                id,
                kind: "sound",
                file,
                format: "F32_MONO".into(),
                changed,
            })
        })();
        match result {
            Ok(entry) => report.entries.push(entry),
            Err(e) => errors.push(e),
        }
    }

    if !errors.is_empty() {
        return Err(BakeError(errors));
    }

    // index.toml: deterministic (BTreeMap iteration order), snapshot-testable.
    let mut index = String::from("version = 1\n");
    for e in &report.entries {
        index.push_str(&format!(
            "\n[[asset]]\nlogical = \"{}\"\nid = {}\nkind = \"{}\"\nfile = \"{}\"\nformat = \"{}\"\n",
            e.logical, e.id, e.kind, e.file, e.format
        ));
    }
    write_if_changed(&out_dir.join("index.toml"), index.as_bytes())
        .map_err(|e| BakeError(vec![e]))?;

    Ok(report)
}

fn decode_png(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| format!("{}: image too large", path.display()))?;
    let mut buf = vec![0; size];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .chunks_exact(3)
            .flat_map(|px| [px[0], px[1], px[2], 255])
            .collect(),
        other => {
            return Err(format!(
                "{}: unsupported PNG color type {other:?} (use RGB or RGBA)",
                path.display()
            ));
        }
    };
    Ok((info.width, info.height, rgba))
}

/// Decode a PCM WAV to mono f32 (stereo is averaged).
fn decode_wav(path: &Path) -> Result<(u32, Vec<f32>), String> {
    let mut reader =
        hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let spec = reader.spec();
    let to_mono = |samples: Vec<f32>| -> Vec<f32> {
        if spec.channels == 2 {
            samples
                .chunks_exact(2)
                .map(|c| (c[0] + c[1]) * 0.5)
                .collect()
        } else {
            samples
        }
    };
    if spec.channels > 2 {
        return Err(format!(
            "{}: {} channels not supported (mono/stereo only)",
            path.display(),
            spec.channels
        ));
    }
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| format!("{}: {e}", path.display()))?,
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max))
                .collect::<Result<_, _>>()
                .map_err(|e| format!("{}: {e}", path.display()))?
        }
    };
    Ok((spec.sample_rate, to_mono(samples)))
}

/// Returns Ok(true) if the file was (re)written, Ok(false) if identical.
fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    if let Ok(existing) = std::fs::read(path)
        && existing == bytes
    {
        return Ok(false);
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}
