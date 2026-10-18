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
    #[serde(default)]
    preview_3ds: Option<String>,
    #[serde(default)]
    compressed: Option<String>,
    #[serde(default)]
    ktx2: Option<String>,
}

/// A sprite's PC blocks (`<id>.bc`, `crate::texture::BcFormat`).
pub struct CompressedSprite {
    pub format: crate::texture::BcFormat,
    pub data: Vec<u8>,
}

pub struct LoadedSprite {
    pub logical: String,
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// What the 3DS samples from the sprite's 3DS format, when that format
    /// loses something (`crate::texture`): the PC's 3DS profile draws it.
    pub preview_3ds: Option<Vec<u8>>,
    /// The PC's BCn blocks, when the sprite's PC format is BCn: the native
    /// profile uploads them where the GPU samples BC.
    pub compressed: Option<CompressedSprite>,
    /// The Web's KTX2 file (a Web bake whose format for the sprite is
    /// KTX2): what a browser loads instead of the master.
    pub ktx2: Option<Vec<u8>>,
}

impl LoadedSprite {
    /// What the PC's native profile shows on a GPU without BC: the blocks
    /// decoded (the same pixels a BC GPU samples), or the master.
    pub fn pc_pixels(&self) -> std::borrow::Cow<'_, [u8]> {
        match &self.compressed {
            Some(c) => c.format.decode(self.width, self.height, &c.data).map_or(
                std::borrow::Cow::Borrowed(&self.rgba[..]),
                std::borrow::Cow::Owned,
            ),
            None => std::borrow::Cow::Borrowed(&self.rgba),
        }
    }

    /// The pixels a renderer simulating `platform` should upload: the 3DS
    /// preview on the 3DS, the master everywhere else.
    pub fn pixels_for(&self, platform: crate::Platform) -> &[u8] {
        match (platform, &self.preview_3ds) {
            (crate::Platform::N3ds, Some(p)) => p,
            _ => &self.rgba,
        }
    }
}

pub struct LoadedSound {
    pub logical: String,
    pub id: u32,
    pub sample_rate: u32,
    pub frames: usize,
    pub codec: trino_core::SoundCodec,
    pub bus: trino_core::Bus,
    pub looped: bool,
    pub stream: bool,
    /// The `TSN2` blob as baked (`trino_core::SoundBlob`; a v1 `TSND` comes
    /// converted to PCM16). `PcAudio::upload_*_blob` takes it as is.
    pub blob: Vec<u8>,
}

impl LoadedSound {
    /// Mono samples at `sample_rate` (Vorbis decoded).
    pub fn samples(&self) -> Result<Vec<f32>, String> {
        let b = trino_core::SoundBlob::parse(&self.blob).map_err(String::from)?;
        match b.codec {
            trino_core::SoundCodec::Pcm16 => Ok(crate::audio::from_pcm16(
                &b.payload
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&c| i16::from_le_bytes(c))
                    .collect::<Vec<_>>(),
            )),
            trino_core::SoundCodec::Vorbis => crate::audio::vorbis::decode(b.payload).map(|d| d.1),
        }
    }
}

pub struct LoadedModel {
    pub logical: String,
    pub id: u32,
    /// The whole TMDL blob (parsed lazily by `trino_core::render3d::Mesh`).
    pub tmdl: Vec<u8>,
}

/// A font's glyph table (`TFNT`); its atlas arrives as a sprite with the
/// same id.
pub struct LoadedFont {
    pub logical: String,
    pub id: u32,
    pub table: Vec<u8>,
}

/// Raw bytes of a `[blobs.*]` entry (or a baked scene).
pub struct LoadedBlob {
    pub logical: String,
    pub id: u32,
    pub bytes: Vec<u8>,
}

#[derive(Default)]
pub struct LoadedAssets {
    pub sprites: Vec<LoadedSprite>,
    pub sounds: Vec<LoadedSound>,
    pub music: Vec<LoadedSound>,
    pub models: Vec<LoadedModel>,
    pub fonts: Vec<LoadedFont>,
    pub blobs: Vec<LoadedBlob>,
    /// The project's mix (`TMIX`, `trino_core::MixerConfig::parse`).
    pub mixer: Option<Vec<u8>>,
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
                // A truncated file must be an error, not a `split_at` panic.
                if bytes.len() < 12 || !bytes.starts_with(b"TSPR") {
                    return Err(format!("{}: bad sprite blob", path.display()));
                }
                let rest = &bytes[4..];
                let width = u32::from_le_bytes(rest[0..4].try_into().unwrap());
                let height = u32::from_le_bytes(rest[4..8].try_into().unwrap());
                let rgba = rest[8..].to_vec();
                if rgba.len() != (width * height * 4) as usize {
                    return Err(format!("{}: truncated sprite data", path.display()));
                }
                let preview_3ds = match &entry.preview_3ds {
                    Some(file) => {
                        let path = out_dir.join(file);
                        let bytes =
                            std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                        if bytes.len() != 12 + rgba.len()
                            || !bytes.starts_with(b"TSPR")
                            || bytes[4..12] != rest[0..8]
                        {
                            return Err(format!("{}: bad 3DS preview", path.display()));
                        }
                        Some(bytes[12..].to_vec())
                    }
                    None => None,
                };
                let compressed = match &entry.compressed {
                    Some(file) => {
                        let path = out_dir.join(file);
                        let bytes =
                            std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                        let bad = || format!("{}: bad BCn blob", path.display());
                        if bytes.len() < 16
                            || !bytes.starts_with(b"TBCN")
                            || bytes[4..12] != rest[0..8]
                        {
                            return Err(bad());
                        }
                        let code = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
                        let format = crate::texture::BcFormat::from_code(code).ok_or_else(bad)?;
                        let data = bytes[16..].to_vec();
                        if data.len() as u64 != format.texture_bytes(width, height) {
                            return Err(bad());
                        }
                        Some(CompressedSprite { format, data })
                    }
                    None => None,
                };
                let ktx2 = match &entry.ktx2 {
                    Some(file) => {
                        let path = out_dir.join(file);
                        let bytes =
                            std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                        crate::texture::ktx2::parse(&bytes)
                            .map_err(|e| format!("{}: {e}", path.display()))?;
                        Some(bytes)
                    }
                    None => None,
                };
                out.sprites.push(LoadedSprite {
                    logical: entry.logical,
                    id: entry.id,
                    width,
                    height,
                    rgba,
                    preview_3ds,
                    compressed,
                    ktx2,
                });
            }
            "sound" | "music" => {
                let blob = if bytes.starts_with(b"TSND") {
                    tsnd_to_tsn2(&bytes, entry.kind == "music")
                        .ok_or_else(|| format!("{}: truncated sound data", path.display()))?
                } else {
                    bytes
                };
                let b = trino_core::SoundBlob::parse(&blob)
                    .map_err(|e| format!("{}: bad sound blob ({e})", path.display()))?;
                let loaded = LoadedSound {
                    logical: entry.logical,
                    id: entry.id,
                    sample_rate: b.sample_rate,
                    frames: b.frames as usize,
                    codec: b.codec,
                    bus: b.bus,
                    looped: b.looped,
                    stream: b.stream,
                    blob,
                };
                if entry.kind == "music" {
                    out.music.push(loaded);
                } else {
                    out.sounds.push(loaded);
                }
            }
            "mixer" => {
                trino_core::MixerConfig::parse(&bytes)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                out.mixer = Some(bytes);
            }
            "model" => {
                if !bytes.starts_with(b"TMDL") {
                    return Err(format!("{}: bad model blob", path.display()));
                }
                out.models.push(LoadedModel {
                    logical: entry.logical,
                    id: entry.id,
                    tmdl: bytes,
                });
            }
            "font" => {
                if let Err(e) = trino_core::text::BitmapFont::parse(&bytes) {
                    return Err(format!("{}: bad font table: {e:?}", path.display()));
                }
                out.fonts.push(LoadedFont {
                    logical: entry.logical,
                    id: entry.id,
                    table: bytes,
                });
            }
            "blob" => out.blobs.push(LoadedBlob {
                logical: entry.logical,
                id: entry.id,
                bytes,
            }),
            other => return Err(format!("unknown asset kind `{other}`")),
        }
    }
    Ok(out)
}

/// A v1 `TSND` (f32 mono, no flags) as a PCM16 `TSN2` with the kind's
/// defaults: music loops and streams on the music bus.
fn tsnd_to_tsn2(bytes: &[u8], music: bool) -> Option<Vec<u8>> {
    let rest = bytes.get(4..12)?;
    let rate = u32::from_le_bytes(rest[0..4].try_into().ok()?);
    let count = u32::from_le_bytes(rest[4..8].try_into().ok()?) as usize;
    let data = bytes.get(12..)?;
    if data.len() != count * 4 {
        return None;
    }
    let pcm: Vec<u8> = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&c| f32::from_le_bytes(c))
        .flat_map(|s| ((s * 32767.0).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes())
        .collect();
    let header = trino_core::SoundBlob {
        codec: trino_core::SoundCodec::Pcm16,
        bus: if music {
            trino_core::Bus::Music
        } else {
            trino_core::Bus::Sfx
        },
        looped: music,
        stream: false,
        sample_rate: rate,
        frames: count as u32,
        payload: &pcm,
    }
    .header();
    let mut out = header.to_vec();
    out.extend_from_slice(&pcm);
    Some(out)
}
