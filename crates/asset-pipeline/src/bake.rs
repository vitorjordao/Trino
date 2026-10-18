//! Bake: manifest + masters → native binary blobs + `index.toml`.
//!
//! PC blob formats (little-endian):
//! - `<hash>.sprite`: magic `TSPR`, u32 width, u32 height, RGBA8 pixels.
//! - `<hash>.3ds.sprite`: the same, holding what the 3DS samples from the
//!   sprite's 3DS format (`crate::texture`), written when that format is not
//!   RGBA8. The PC's 3DS profile draws it instead (`preview_3ds` in the
//!   index).
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
    pub kind: &'static str, // "sprite" | "sound" | "music" | "model" | "font" | "blob"
    pub file: String,       // output file name
    pub format: String,
    /// A sprite's 3DS preview file (PC bakes only, when its 3DS format
    /// loses something: `crate::texture::N3dsFormat::preview`).
    pub preview_3ds: Option<String>,
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
            let file = format!("{id:08x}.sprite");
            let changed = write_if_changed(&out_dir.join(&file), &tspr(w, h, &rgba))?;
            let (preview_3ds, preview_changed) = match platform {
                Platform::Pc => {
                    let n3ds = decl.format_for(&logical, Platform::N3ds)?;
                    write_preview_3ds(out_dir, id, &n3ds, w, h, &rgba)?
                }
                _ => (None, false),
            };
            Ok(BakedEntry {
                logical: logical.clone(),
                id,
                kind: "sprite",
                file,
                format,
                preview_3ds,
                changed: changed || preview_changed,
            })
        })();
        match result {
            Ok(entry) => report.entries.push(entry),
            Err(e) => errors.push(e),
        }
    }

    // Sounds and music share the blob format; the kind drives which audio
    // registry they land in at load time.
    let audio_sections = [
        ("sounds", "sound", &manifest.sounds),
        ("music", "music", &manifest.music),
    ];
    for (prefix, kind, section) in audio_sections {
        for (name, decl) in section {
            let logical = format!("{prefix}/{name}");
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
                    kind,
                    file,
                    format: "F32_MONO".into(),
                    preview_3ds: None,
                    changed,
                })
            })();
            match result {
                Ok(entry) => report.entries.push(entry),
                Err(e) => errors.push(e),
            }
        }
    }

    // Model bounds for baked scenes' `Auto` colliders, collected as models
    // bake (no second bake per scene).
    let mut model_bounds = BTreeMap::new();
    for (name, decl) in &manifest.models {
        let logical = format!("models/{name}");
        let id = asset_id(&logical);
        if let Some(other) = by_id.insert(id, logical.clone()) {
            errors.push(format!("hash collision between `{other}` and `{logical}`"));
            continue;
        }
        if !decl.bakes_for(platform) {
            continue;
        }
        let result = (|| -> Result<BakedEntry, String> {
            let source = resolve_source(assets_root, platform, &decl.file)?;
            let blob = crate::model::bake_model(&source, &decl.options())?;
            if let Some(b) = crate::model::tmdl_bounds(&blob) {
                model_bounds.insert(logical.clone(), b);
            }
            let file = format!("{id:08x}.model");
            let changed = write_if_changed(&out_dir.join(&file), &blob)?;
            Ok(BakedEntry {
                logical: logical.clone(),
                id,
                kind: "model",
                file,
                format: "TMDL".into(),
                preview_3ds: None,
                changed,
            })
        })();
        match result {
            Ok(entry) => report.entries.push(entry),
            Err(e) => errors.push(e),
        }
    }

    for (name, decl) in &manifest.fonts {
        let logical = format!("fonts/{name}");
        let id = asset_id(&logical);
        if let Some(other) = by_id.insert(id, logical.clone()) {
            errors.push(format!("hash collision between `{other}` and `{logical}`"));
            continue;
        }
        let result = (|| -> Result<Vec<BakedEntry>, String> {
            let format = decl.format_for(&logical, platform)?;
            let source = resolve_source(assets_root, platform, &decl.file)?;
            let font = crate::font::bake_font(&source, decl)
                .map_err(|e| format!("font `{logical}`: {e}"))?;
            let atlas = tspr(font.width, font.height, &font.rgba);
            // The atlas is a sprite under the font's own handle
            // (`text::atlas_of`); the table is the `font` entry.
            let sprite_file = format!("{id:08x}.sprite");
            let table_file = format!("{id:08x}.font");
            let sprite_changed = write_if_changed(&out_dir.join(&sprite_file), &atlas)?;
            let table_changed = write_if_changed(&out_dir.join(&table_file), &font.table)?;
            let (preview_3ds, preview_changed) = match platform {
                Platform::Pc => {
                    let n3ds = decl.format_for(&logical, Platform::N3ds)?;
                    write_preview_3ds(out_dir, id, &n3ds, font.width, font.height, &font.rgba)?
                }
                _ => (None, false),
            };
            Ok(vec![
                BakedEntry {
                    logical: logical.clone(),
                    id,
                    kind: "sprite",
                    file: sprite_file,
                    format,
                    preview_3ds,
                    changed: sprite_changed || preview_changed,
                },
                BakedEntry {
                    logical: logical.clone(),
                    id,
                    kind: "font",
                    file: table_file,
                    format: "TFNT".into(),
                    preview_3ds: None,
                    changed: table_changed,
                },
            ])
        })();
        match result {
            Ok(entries) => report.entries.extend(entries),
            Err(e) => errors.push(e),
        }
    }

    for (name, decl) in &manifest.blobs {
        let logical = format!("blobs/{name}");
        let id = asset_id(&logical);
        if let Some(other) = by_id.insert(id, logical.clone()) {
            errors.push(format!("hash collision between `{other}` and `{logical}`"));
            continue;
        }
        let result = (|| -> Result<BakedEntry, String> {
            let source = resolve_source(assets_root, platform, &decl.file)?;
            let bytes = std::fs::read(&source).map_err(|e| format!("{}: {e}", source.display()))?;
            let file = format!("{id:08x}.blob");
            let changed = write_if_changed(&out_dir.join(&file), &bytes)?;
            Ok(BakedEntry {
                logical: logical.clone(),
                id,
                kind: "blob",
                file,
                format: "RAW".into(),
                preview_3ds: None,
                changed,
            })
        })();
        match result {
            Ok(entry) => report.entries.push(entry),
            Err(e) => errors.push(e),
        }
    }

    for (name, decl) in &manifest.particles {
        let logical = format!("particles/{name}");
        let id = asset_id(&logical);
        if let Some(other) = by_id.insert(id, logical.clone()) {
            errors.push(format!("hash collision between `{other}` and `{logical}`"));
            continue;
        }
        let result = (|| -> Result<BakedEntry, String> {
            let source = resolve_source(assets_root, platform, &decl.file)?;
            let blob = crate::particles::bake(&source, platform)?;
            let file = format!("{id:08x}.blob");
            let changed = write_if_changed(&out_dir.join(&file), &blob)?;
            // An effect is a blob to every loader (`Platform::blob`).
            Ok(BakedEntry {
                logical: logical.clone(),
                id,
                kind: "blob",
                file,
                format: "TPFX".into(),
                preview_3ds: None,
                changed,
            })
        })();
        match result {
            Ok(entry) => report.entries.push(entry),
            Err(e) => errors.push(e),
        }
    }

    for (name, decl) in &manifest.scenes {
        let logical = format!("scenes/{name}");
        let id = asset_id(&logical);
        if let Some(other) = by_id.insert(id, logical.clone()) {
            errors.push(format!("hash collision between `{other}` and `{logical}`"));
            continue;
        }
        let result = (|| -> Result<BakedEntry, String> {
            let blob = crate::model::bake_scene_with_bounds(
                assets_root,
                &decl.file,
                model_bounds.clone(),
                platform,
            )?;
            let file = format!("{id:08x}.blob");
            let changed = write_if_changed(&out_dir.join(&file), &blob)?;
            // A baked scene is a blob to every loader (`Platform::blob`).
            Ok(BakedEntry {
                logical: logical.clone(),
                id,
                kind: "blob",
                file,
                format: "TSCN".into(),
                preview_3ds: None,
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
        if let Some(p) = &e.preview_3ds {
            index.push_str(&format!("preview_3ds = \"{p}\"\n"));
        }
    }
    write_if_changed(&out_dir.join("index.toml"), index.as_bytes())
        .map_err(|e| BakeError(vec![e]))?;

    Ok(report)
}

/// A `TSPR` blob: magic, u32 width, u32 height, RGBA8.
fn tspr(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(12 + rgba.len());
    blob.extend_from_slice(b"TSPR");
    blob.extend_from_slice(&w.to_le_bytes());
    blob.extend_from_slice(&h.to_le_bytes());
    blob.extend_from_slice(rgba);
    blob
}

/// Write `<id>.3ds.sprite`, what the 3DS samples from `format`, unless the
/// format keeps every bit (RGBA8). Returns the file name and whether it
/// changed.
fn write_preview_3ds(
    out_dir: &Path,
    id: u32,
    format: &str,
    w: u32,
    h: u32,
    rgba: &[u8],
) -> Result<(Option<String>, bool), String> {
    let format = crate::texture::N3dsFormat::parse(format)
        .ok_or_else(|| format!("unknown 3DS format `{format}`"))?;
    if format == crate::texture::N3dsFormat::Rgba8 {
        return Ok((None, false));
    }
    let file = format!("{id:08x}.3ds.sprite");
    let changed = write_if_changed(
        &out_dir.join(&file),
        &tspr(w, h, &format.preview(w, h, rgba)),
    )?;
    Ok((Some(file), changed))
}

pub(crate) fn decode_png(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
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
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
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
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&[l, r]| (l + r) * 0.5)
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
