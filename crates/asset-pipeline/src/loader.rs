//! Runtime loading of PC-baked assets (`index.toml` + blobs).

use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct IndexFile {
    version: u32,
    #[serde(default, rename = "asset")]
    assets: Vec<IndexEntry>,
}

#[derive(Debug, Deserialize)]
struct IndexEntry {
    logical: String,
    id: u32,
    kind: String,
    file: String,
}

pub struct LoadedSprite {
    pub logical: String,
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct LoadedSound {
    pub logical: String,
    pub id: u32,
    pub sample_rate: u32,
    pub samples: Vec<f32>,
}

#[derive(Default)]
pub struct LoadedAssets {
    pub sprites: Vec<LoadedSprite>,
    pub sounds: Vec<LoadedSound>,
    pub music: Vec<LoadedSound>,
}

/// Load every baked asset from `out_dir`. `only` filters by handle (used by
/// live reload to load just what changed); `None` loads everything.
pub fn load_dir(out_dir: &Path, only: Option<&[u32]>) -> Result<LoadedAssets, String> {
    let index_path = out_dir.join("index.toml");
    let text = std::fs::read_to_string(&index_path)
        .map_err(|e| format!("cannot read {}: {e}", index_path.display()))?;
    let index: IndexFile =
        toml::from_str(&text).map_err(|e| format!("{}: {e}", index_path.display()))?;
    if index.version != 1 {
        return Err(format!("unsupported index version {}", index.version));
    }

    let mut out = LoadedAssets::default();
    for entry in index.assets {
        if let Some(filter) = only
            && !filter.contains(&entry.id)
        {
            continue;
        }
        let path = out_dir.join(&entry.file);
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        match entry.kind.as_str() {
            "sprite" => {
                let (magic, rest) = bytes.split_at(4);
                if magic != b"TSPR" || rest.len() < 8 {
                    return Err(format!("{}: bad sprite blob", path.display()));
                }
                let width = u32::from_le_bytes(rest[0..4].try_into().unwrap());
                let height = u32::from_le_bytes(rest[4..8].try_into().unwrap());
                let rgba = rest[8..].to_vec();
                if rgba.len() != (width * height * 4) as usize {
                    return Err(format!("{}: truncated sprite data", path.display()));
                }
                out.sprites.push(LoadedSprite {
                    logical: entry.logical,
                    id: entry.id,
                    width,
                    height,
                    rgba,
                });
            }
            "sound" | "music" => {
                let (magic, rest) = bytes.split_at(4);
                if magic != b"TSND" || rest.len() < 8 {
                    return Err(format!("{}: bad sound blob", path.display()));
                }
                let sample_rate = u32::from_le_bytes(rest[0..4].try_into().unwrap());
                let count = u32::from_le_bytes(rest[4..8].try_into().unwrap()) as usize;
                let data = &rest[8..];
                if data.len() != count * 4 {
                    return Err(format!("{}: truncated sound data", path.display()));
                }
                let samples = data
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                    .collect();
                let loaded = LoadedSound {
                    logical: entry.logical,
                    id: entry.id,
                    sample_rate,
                    samples,
                };
                if entry.kind == "music" {
                    out.music.push(loaded);
                } else {
                    out.sounds.push(loaded);
                }
            }
            other => return Err(format!("unknown asset kind `{other}`")),
        }
    }
    Ok(out)
}
