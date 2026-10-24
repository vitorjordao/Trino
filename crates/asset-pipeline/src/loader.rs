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
    #[serde(default)]
    graded: Option<String>,
    /// The non-boot groups it is in (issue #56); empty = `boot`.
    #[serde(default)]
    groups: Vec<String>,
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
    /// Its console-graded twin (PC bakes of a model with `grading`, issue
    /// #64): what the PC's console profiles draw.
    pub graded: Option<Vec<u8>>,
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
/// live reload to load just what changed); `None` loads everything, every
/// group's assets included (tools and the editor show them all; a game host
/// loads [`load_boot`] and the rest through [`BundleLoad`]).
pub fn load_dir(out_dir: &Path, only: Option<&[u32]>) -> Result<LoadedAssets, String> {
    load_where(out_dir, |e| only.is_none_or(|f| f.contains(&e.id)))
}

/// The `boot` group's assets (issue #56): what a game host loads at start.
/// Without groups in the manifest that is every asset, as before.
pub fn load_boot(out_dir: &Path, only: Option<&[u32]>) -> Result<LoadedAssets, String> {
    load_where(out_dir, |e| {
        e.groups.is_empty() && only.is_none_or(|f| f.contains(&e.id))
    })
}

/// Which groups' bundles a PC or Web bake holds, by handle, read from its
/// index (`groups` lists).
pub fn baked_groups(out_dir: &Path) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = read_index(out_dir)?
        .assets
        .into_iter()
        .flat_map(|e| e.groups)
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

fn read_index(out_dir: &Path) -> Result<IndexFile, String> {
    let index_path = out_dir.join("index.toml");
    let text = std::fs::read_to_string(&index_path)
        .map_err(|e| format!("cannot read {}: {e}", index_path.display()))?;
    let index: IndexFile =
        toml::from_str(&text).map_err(|e| format!("{}: {e}", index_path.display()))?;
    if index.version != 1 {
        return Err(format!("unsupported index version {}", index.version));
    }
    Ok(index)
}

fn load_where(out_dir: &Path, keep: impl Fn(&IndexEntry) -> bool) -> Result<LoadedAssets, String> {
    let index = read_index(out_dir)?;
    let mut out = LoadedAssets::default();
    let read = |file: &str| {
        let path = out_dir.join(file);
        std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
    };
    for entry in index.assets {
        if !keep(&entry) {
            continue;
        }
        let bytes = read(&entry.file)?;
        let variants = Variants {
            preview_3ds: entry.preview_3ds.as_deref().map(read).transpose()?,
            compressed: entry.compressed.as_deref().map(read).transpose()?,
            ktx2: entry.ktx2.as_deref().map(read).transpose()?,
            graded: entry.graded.as_deref().map(read).transpose()?,
        };
        let path = out_dir.join(&entry.file);
        push_entry(
            &mut out,
            &entry.kind,
            entry.logical,
            entry.id,
            bytes,
            variants,
            &|| path.display().to_string(),
        )?;
    }
    Ok(out)
}

/// A sprite's extra files (`preview_3ds`, `compressed`, `ktx2`), a
/// model's (`graded`).
#[derive(Default)]
struct Variants {
    preview_3ds: Option<Vec<u8>>,
    compressed: Option<Vec<u8>>,
    ktx2: Option<Vec<u8>>,
    graded: Option<Vec<u8>>,
}

/// Parse one baked asset into `out`; `what` names it in errors.
fn push_entry(
    out: &mut LoadedAssets,
    kind: &str,
    logical: String,
    id: u32,
    bytes: Vec<u8>,
    variants: Variants,
    what: &dyn Fn() -> String,
) -> Result<(), String> {
    match kind {
        "sprite" => {
            // A truncated file must be an error, not a `split_at` panic.
            if bytes.len() < 12 || !bytes.starts_with(b"TSPR") {
                return Err(format!("{}: bad sprite blob", what()));
            }
            let rest = &bytes[4..];
            let width = u32::from_le_bytes(rest[0..4].try_into().unwrap());
            let height = u32::from_le_bytes(rest[4..8].try_into().unwrap());
            let rgba = rest[8..].to_vec();
            if rgba.len() != (width * height * 4) as usize {
                return Err(format!("{}: truncated sprite data", what()));
            }
            let preview_3ds = match variants.preview_3ds {
                Some(bytes) => {
                    if bytes.len() != 12 + rgba.len()
                        || !bytes.starts_with(b"TSPR")
                        || bytes[4..12] != rest[0..8]
                    {
                        return Err(format!("{}: bad 3DS preview", what()));
                    }
                    Some(bytes[12..].to_vec())
                }
                None => None,
            };
            let compressed = match variants.compressed {
                Some(bytes) => {
                    let bad = || format!("{}: bad BCn blob", what());
                    if bytes.len() < 16 || !bytes.starts_with(b"TBCN") || bytes[4..12] != rest[0..8]
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
            let ktx2 = match variants.ktx2 {
                Some(bytes) => {
                    crate::texture::ktx2::parse(&bytes).map_err(|e| format!("{}: {e}", what()))?;
                    Some(bytes)
                }
                None => None,
            };
            out.sprites.push(LoadedSprite {
                logical,
                id,
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
                tsnd_to_tsn2(&bytes, kind == "music")
                    .ok_or_else(|| format!("{}: truncated sound data", what()))?
            } else {
                bytes
            };
            let b = trino_core::SoundBlob::parse(&blob)
                .map_err(|e| format!("{}: bad sound blob ({e})", what()))?;
            let loaded = LoadedSound {
                logical,
                id,
                sample_rate: b.sample_rate,
                frames: b.frames as usize,
                codec: b.codec,
                bus: b.bus,
                looped: b.looped,
                stream: b.stream,
                blob,
            };
            if kind == "music" {
                out.music.push(loaded);
            } else {
                out.sounds.push(loaded);
            }
        }
        "mixer" => {
            trino_core::MixerConfig::parse(&bytes).map_err(|e| format!("{}: {e}", what()))?;
            out.mixer = Some(bytes);
        }
        "model" => {
            if !bytes.starts_with(b"TMDL") {
                return Err(format!("{}: bad model blob", what()));
            }
            if variants
                .graded
                .as_ref()
                .is_some_and(|g| !g.starts_with(b"TMDL"))
            {
                return Err(format!("{}: bad graded model blob", what()));
            }
            out.models.push(LoadedModel {
                logical,
                id,
                tmdl: bytes,
                graded: variants.graded,
            });
        }
        "font" => {
            if let Err(e) = trino_core::text::BitmapFont::parse(&bytes) {
                return Err(format!("{}: bad font table: {e:?}", what()));
            }
            out.fonts.push(LoadedFont {
                logical,
                id,
                table: bytes,
            });
        }
        "blob" => out.blobs.push(LoadedBlob { logical, id, bytes }),
        other => return Err(format!("unknown asset kind `{other}`")),
    }
    Ok(())
}

/// Decoded bytes a [`BundleLoad::step_many`] worker must get to be worth
/// its thread.
pub const MIN_BYTES_PER_WORKER: u64 = 512 * 1024;

/// One asset group's bundle (`groups/<name>.tbnd`, issue #56) read a step at
/// a time: what a PC host's `load_group` runs within its frame budget.
pub struct BundleLoad {
    bytes: Vec<u8>,
    next: usize,
    tier: trino_core::HwTier,
    done: u64,
    total: u64,
    name: String,
}

impl BundleLoad {
    /// Read the bundle file and check its table.
    pub fn open(path: &Path, tier: trino_core::HwTier) -> Result<BundleLoad, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_bytes(bytes, path.display().to_string(), tier)
    }

    pub fn from_bytes(
        bytes: Vec<u8>,
        name: String,
        tier: trino_core::HwTier,
    ) -> Result<BundleLoad, String> {
        let b = trino_core::bundle::Bundle::parse(&bytes)
            .map_err(|e| format!("{name}: {}", e.label()))?;
        if b.file_len() != bytes.len() as u64 {
            return Err(format!(
                "{name}: {}",
                trino_core::bundle::BundleError::Truncated.label()
            ));
        }
        let total = b.read_total(tier);
        Ok(BundleLoad {
            bytes,
            next: 0,
            tier,
            done: 0,
            total,
            name,
        })
    }

    /// The group's handle (`trino_core::GroupId`).
    pub fn group(&self) -> trino_core::GroupId {
        trino_core::GroupId(self.bundle().group())
    }

    fn bundle(&self) -> trino_core::bundle::Bundle<'_> {
        trino_core::bundle::Bundle::parse(&self.bytes).expect("checked by open")
    }

    pub fn is_done(&self) -> bool {
        self.next >= self.bundle().len()
    }

    /// (bytes read, bytes in all) of the entries for this tier.
    pub fn progress(&self) -> (u64, u64) {
        (self.done, self.total)
    }

    /// Decode the next asset (a sprite with its preview, BCn and KTX2
    /// entries) into `out`. Entries of another tier are skipped.
    pub fn step(&mut self, out: &mut LoadedAssets) -> Result<(), String> {
        self.step_many(1, out)
    }

    /// Decode the next `threads` assets at once, one per worker (issue
    /// #101: decompression and hashing are the load's CPU work), into
    /// `out` in bundle order: the same assets, in the same order, as
    /// `threads` calls to [`step`](Self::step).
    pub fn step_many(&mut self, threads: u32, out: &mut LoadedAssets) -> Result<(), String> {
        use trino_core::bundle::EntryKind as K;
        let b = trino_core::bundle::Bundle::parse(&self.bytes).expect("checked by open");
        // Plan the next assets: each one's entry and its variants' (slot
        // 0 = 3DS preview, 1 = BCn, 2 = KTX2, 3 = a graded model).
        let mut plans: Vec<(usize, Vec<(usize, usize)>)> = Vec::new();
        while self.next < b.len() && plans.len() < threads.max(1) as usize {
            let at = self.next;
            let first = b.entry(at);
            self.next += 1;
            if !first.loads_on(self.tier) {
                continue;
            }
            let mut variants = Vec::new();
            while self.next < b.len() {
                let e = b.entry(self.next);
                let slot = match e.kind {
                    K::SpritePreview3ds => 0,
                    K::SpriteBc => 1,
                    K::SpriteKtx2 => 2,
                    K::ModelGraded => 3,
                    _ => break,
                };
                self.next += 1;
                if e.id == first.id && e.loads_on(self.tier) {
                    variants.push((slot, self.next - 1));
                }
            }
            plans.push((at, variants));
        }
        let name = &self.name;
        let decode = |e: &trino_core::bundle::Entry| -> Result<Vec<u8>, String> {
            let stored = b
                .stored(e)
                .ok_or_else(|| format!("{name}: external entry"))?;
            let mut raw = vec![0u8; e.raw as usize];
            trino_core::bundle::decode(e, stored, &mut raw)
                .map_err(|err| format!("{name}: entry {:08x}: {}", e.id, err.label()))?;
            if trino_core::bundle::hash(&raw) != e.hash {
                return Err(format!("{name}: entry {:08x}: hash mismatch", e.id));
            }
            Ok(raw)
        };
        // Threads only where the bytes pay for them: spawning workers costs
        // more than decoding a step of small entries (a 315 KB group took
        // 1.6 ms on one thread and 4.0 ms on 15).
        let bytes: u64 = plans
            .iter()
            .map(|(at, slots)| {
                b.entry(*at).raw as u64
                    + slots
                        .iter()
                        .map(|&(_, i)| b.entry(i).raw as u64)
                        .sum::<u64>()
            })
            .sum();
        let workers = threads.min((bytes / MIN_BYTES_PER_WORKER).max(1) as u32);
        let decoded = crate::par::map_ordered(workers, &plans, |(at, slots)| {
            let first = b.entry(*at);
            let bytes = decode(&first)?;
            let mut read = first.read_bytes() as u64;
            let mut variants = Variants::default();
            for &(slot, i) in slots {
                let e = b.entry(i);
                let v = Some(decode(&e)?);
                match slot {
                    0 => variants.preview_3ds = v,
                    1 => variants.compressed = v,
                    2 => variants.ktx2 = v,
                    _ => variants.graded = v,
                }
                read += e.read_bytes() as u64;
            }
            Ok::<_, String>((first.kind, first.id, bytes, variants, read))
        });
        for d in decoded {
            let (kind, id, bytes, variants, read) = d?;
            self.done += read;
            let name = self.name.clone();
            push_entry(
                out,
                kind.label(),
                format!("{id:08x}"),
                id,
                bytes,
                variants,
                &|| format!("{name}: entry {id:08x}"),
            )?;
        }
        Ok(())
    }
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
