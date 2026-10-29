//! Bake: manifest + masters → native binary blobs + `index.toml`.
//!
//! PC blob formats (little-endian):
//! - `<hash>.sprite`: magic `TSPR`, u32 width, u32 height, RGBA8 pixels.
//! - `<hash>.3ds.sprite`: the same, holding what the 3DS samples from the
//!   sprite's 3DS format (`crate::texture`), written when that format is not
//!   RGBA8. The PC's 3DS profile draws it instead (`preview_3ds` in the
//!   index).
//! - `<hash>.bc`: magic `TBCN`, u32 width, u32 height, u32 format
//!   (`BcFormat::code`: 1, 3, 7), the blocks — written when the sprite's PC
//!   format is BCn (`compressed` in the index). The PC's native profile
//!   uploads it where the GPU samples BC; the `.sprite` master stays for
//!   tools, the console profiles and GPUs without BC.
//! - `<hash>.ktx2`: a KTX2 file of Basis Universal data (`crate::texture::ktx2`),
//!   written by a Web bake when the sprite's Web format is `KTX2-ETC1S` or
//!   `KTX2-UASTC` (`ktx2` in the index). The browser transcodes it to what
//!   its GPU samples.
//! - `<hash>.sound`: a `TSN2` sound or track (`trino_core::SoundBlob`):
//!   codec (PCM16 or Ogg Vorbis, `crate::audio`), default bus, loop and
//!   stream flags, rate, frames, payload. (v1 `TSND` — f32 mono — still
//!   loads.)
//! - `<hash>.mixer`: the project's mix (`assets/audio.toml`) as `TMIX`
//!   (`trino_core::MixerConfig`), logical path `audio/mixer`.
//! - `<hash>.blob`, logical path `net/config` (PC and Web, issue #67):
//!   `trino.toml`'s `[players]` and `[net]` as `key = value` lines.
//! - `<hash>.blob`, logical path `quality/presets` (PC and Web): the
//!   project's quality presets (`crate::quality`), `key=value` text.
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
    pub kind: &'static str, // "sprite" | "sound" | "music" | "model" | "font" | "blob" | "mixer"
    pub file: String,       // output file name
    pub format: String,
    /// A sprite's 3DS preview file (PC bakes only, when its 3DS format
    /// loses something: `crate::texture::N3dsFormat::preview`).
    pub preview_3ds: Option<String>,
    /// A sprite's N64 format (PC bakes only, `crate::texture::N64Format`
    /// name): what the PC's strict mode checks TMEM by on the N64 profile
    /// (GitHub #125).
    pub n64_format: Option<String>,
    /// A sprite's BCn blocks (PC bakes only, when its PC format is BCn).
    pub compressed: Option<String>,
    /// A sprite's KTX2 file (Web bakes only, when its Web format is
    /// `KTX2-ETC1S` or `KTX2-UASTC`).
    pub ktx2: Option<String>,
    /// A model's console-graded twin (PC bakes only, when it names a
    /// `grading` LUT): what the PC's console profiles draw (issue #64).
    pub graded: Option<String>,
    /// True if this bake changed the output bytes on disk.
    pub changed: bool,
    /// The groups whose bundles hold it (issue #56); empty = `boot`.
    pub groups: Vec<String>,
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
    bake_all_with(
        assets_root,
        platform,
        out_dir,
        crate::par::default_threads(),
    )
}

/// [`bake_all`] on `threads` workers (issue #101): sprites, sounds and
/// models bake in parallel; the files, the report and the errors are the
/// same, in the same order, for any count.
pub fn bake_all_with(
    assets_root: &Path,
    platform: Platform,
    out_dir: &Path,
    threads: u32,
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

    // Handles are checked in manifest order first (serial), then the
    // assets bake on the workers.
    let mut sprites = Vec::new();
    for (name, decl) in &manifest.sprites {
        let logical = format!("sprites/{name}");
        let id = asset_id(&logical);
        if let Some(other) = by_id.insert(id, logical.clone()) {
            errors.push(format!("hash collision between `{other}` and `{logical}`"));
            continue;
        }
        sprites.push((logical, id, decl));
    }
    let baked = crate::par::map_ordered(threads, &sprites, |(logical, id, decl)| {
        let (logical, id) = (logical.clone(), *id);
        (|| -> Result<BakedEntry, String> {
            let source = resolve_source(assets_root, platform, &decl.file)?;
            let (w, h, rgba) = decode_png(&source)?;
            // The PC keeps the master (tools and the console profiles draw
            // it) plus its own BCn and the 3DS preview; a console bake here
            // only decides the format (their bakers convert:
            // `xtask/src/{n64,n3ds}.rs`).
            let file = format!("{id:08x}.sprite");
            let changed = write_if_changed(&out_dir.join(&file), &tspr(w, h, &rgba))?;
            // The N64's own master when it overrides this one (its format is
            // chosen from what the N64 build stages).
            let n64 = match platform {
                Platform::Pc => {
                    let n64 = resolve_source(assets_root, Platform::N64, &decl.file)?;
                    if n64 == source {
                        None
                    } else {
                        Some(decode_png(&n64)?)
                    }
                }
                _ => None,
            };
            let t = bake_texture(
                out_dir,
                id,
                &logical,
                platform,
                crate::texture::Source::Sprite,
                &|p| decl.settings(p),
                (
                    w,
                    h,
                    &rgba,
                    n64.as_ref().map(|(w, h, px)| (*w, *h, &px[..])),
                ),
            )?;
            Ok(BakedEntry {
                logical: logical.clone(),
                id,
                kind: "sprite",
                file,
                format: t.format,
                preview_3ds: t.preview_3ds,
                n64_format: t.n64_format,
                compressed: t.compressed,
                ktx2: t.ktx2,
                graded: None,
                changed: changed || t.changed,
                groups: Vec::new(),
            })
        })()
    });
    for result in baked {
        match result {
            Ok(entry) => report.entries.push(entry),
            Err(e) => errors.push(e),
        }
    }

    // Sounds and music share the blob format (`TSN2`, `crate::audio`); the
    // kind drives which audio registry they land in at load time. The PC
    // and the Web get their codec; a console bake writes PCM16 of what the
    // console will play (its own bakers make the real files in xtask).
    let audio_sections = [
        ("sounds", "sound", false, &manifest.sounds),
        ("music", "music", true, &manifest.music),
    ];
    let mut sounds = Vec::new();
    for (prefix, kind, music, section) in audio_sections {
        for (name, decl) in section {
            let logical = format!("{prefix}/{name}");
            let id = asset_id(&logical);
            if let Some(other) = by_id.insert(id, logical.clone()) {
                errors.push(format!("hash collision between `{other}` and `{logical}`"));
                continue;
            }
            sounds.push((logical, id, kind, music, decl));
        }
    }
    let baked = crate::par::map_ordered(threads, &sounds, |(logical, id, kind, music, decl)| {
        let (logical, id, kind, music) = (logical.clone(), *id, *kind, *music);
        (|| -> Result<BakedEntry, String> {
            let source = resolve_source(assets_root, platform, &decl.file)?;
            let settings = decl
                .audio_settings(music, platform)
                .map_err(|e| format!("`{logical}`: {e}"))?;
            let master = decode_master(&source)?;
            let enc = crate::audio::encode(platform, &settings, &master, id)
                .map_err(|e| format!("`{logical}`: {e}"))?;
            let (blob, format) = match enc.blob {
                Some(blob) if matches!(platform, Platform::Pc | Platform::Web) => {
                    (blob, settings.codec.label().to_uppercase())
                }
                _ => {
                    // N64 Opus has no host decode: the master at its rate.
                    let played = enc.decoded.clone().unwrap_or_else(|| {
                        crate::audio::resample(&master.samples, master.rate, enc.rate)
                    });
                    let pcm: Vec<u8> = crate::audio::to_pcm16(&played)
                        .iter()
                        .flat_map(|s| s.to_le_bytes())
                        .collect();
                    let blob = crate::audio::sound_blob(
                        trino_core::SoundCodec::Pcm16,
                        &settings,
                        enc.rate,
                        played.len(),
                        &pcm,
                    );
                    (blob, format!("PCM16 ({})", settings.codec.label()))
                }
            };
            let file = format!("{id:08x}.sound");
            let changed = write_if_changed(&out_dir.join(&file), &blob)?;
            Ok(BakedEntry {
                logical: logical.clone(),
                id,
                kind,
                file,
                format,
                preview_3ds: None,
                n64_format: None,
                compressed: None,
                ktx2: None,
                graded: None,
                changed,
                groups: Vec::new(),
            })
        })()
    });
    for result in baked {
        match result {
            Ok(entry) => report.entries.push(entry),
            Err(e) => errors.push(e),
        }
    }

    // The project's mix (`assets/audio.toml`) → the `audio/mixer` asset.
    let mix_path = assets_root.join(crate::audio::MixFile::FILE);
    if mix_path.exists() {
        let logical = trino_core::MixerConfig::LOGICAL.to_string();
        let id = asset_id(&logical);
        let result = (|| -> Result<BakedEntry, String> {
            if let Some(other) = by_id.insert(id, logical.clone()) {
                return Err(format!("hash collision between `{other}` and `{logical}`"));
            }
            let text = std::fs::read_to_string(&mix_path)
                .map_err(|e| format!("{}: {e}", mix_path.display()))?;
            let blob = crate::audio::MixFile::parse(&text)
                .and_then(|m| m.bake())
                .map_err(|e| format!("{}: {e}", mix_path.display()))?;
            let file = format!("{id:08x}.mixer");
            let changed = write_if_changed(&out_dir.join(&file), &blob)?;
            Ok(BakedEntry {
                logical: logical.clone(),
                id,
                kind: "mixer",
                file,
                format: "TMIX".into(),
                preview_3ds: None,
                n64_format: None,
                compressed: None,
                ktx2: None,
                graded: None,
                changed,
                groups: Vec::new(),
            })
        })();
        match result {
            Ok(entry) => report.entries.push(entry),
            Err(e) => errors.push(e),
        }
    }

    // The project's quality presets (`[pc.quality]`, `assets/quality.ron`)
    // → the `quality/presets` blob, for the PC and the Web (the consoles'
    // presets are fixed). Nothing to carry when the project changes none.
    if matches!(platform, Platform::Pc | Platform::Web) {
        let root = assets_root.parent().unwrap_or(assets_root);
        let result = crate::quality::ProjectQuality::load(root).and_then(|q| {
            if !root.join(&q.presets).exists() && q.default == "auto" {
                return Ok(None);
            }
            let logical = crate::quality::LOGICAL.to_string();
            let id = asset_id(&logical);
            if let Some(other) = by_id.insert(id, logical.clone()) {
                return Err(format!("hash collision between `{other}` and `{logical}`"));
            }
            let blob = q.bake(root)?;
            let file = format!("{id:08x}.blob");
            let changed = write_if_changed(&out_dir.join(&file), &blob)?;
            Ok(Some(BakedEntry {
                logical,
                id,
                kind: "blob",
                file,
                format: "TEXT".into(),
                preview_3ds: None,
                n64_format: None,
                compressed: None,
                ktx2: None,
                graded: None,
                changed,
                groups: Vec::new(),
            }))
        });
        match result {
            Ok(Some(entry)) => report.entries.push(entry),
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }

    // `trino.toml`'s `[players]` and `[net]` → the `net/config` blob, for
    // the PC and the Web (issue #67: the hosts that seat pads and run
    // network sessions). Nothing when the project declares neither.
    if matches!(platform, Platform::Pc | Platform::Web) {
        let root = assets_root.parent().unwrap_or(assets_root);
        let result = crate::net::ProjectNet::load(root).and_then(|n| {
            if !n.declared {
                return Ok(None);
            }
            let logical = crate::net::LOGICAL.to_string();
            let id = asset_id(&logical);
            if let Some(other) = by_id.insert(id, logical.clone()) {
                return Err(format!("hash collision between `{other}` and `{logical}`"));
            }
            let file = format!("{id:08x}.blob");
            let changed = write_if_changed(&out_dir.join(&file), &n.bake())?;
            Ok(Some(BakedEntry {
                logical,
                id,
                kind: "blob",
                file,
                format: "TEXT".into(),
                preview_3ds: None,
                n64_format: None,
                compressed: None,
                ktx2: None,
                graded: None,
                changed,
                groups: Vec::new(),
            }))
        });
        match result {
            Ok(Some(entry)) => report.entries.push(entry),
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }

    // The project's UI screens (`ui/*.ui.ron`) and string tables
    // (`assets/i18n/*.ron`) → `ui/<name>` and `i18n/<lang>` blobs, on every
    // target (issue #58).
    {
        let root = assets_root.parent().unwrap_or(assets_root);
        match crate::ui::bake(root, &manifest, platform) {
            Ok(blobs) => {
                for b in blobs {
                    let id = asset_id(&b.logical);
                    if let Some(other) = by_id.insert(id, b.logical.clone()) {
                        errors.push(format!(
                            "hash collision between `{other}` and `{}`",
                            b.logical
                        ));
                        continue;
                    }
                    let file = format!("{id:08x}.blob");
                    let format = String::from_utf8_lossy(&b.bytes[..4]).into_owned();
                    match write_if_changed(&out_dir.join(&file), &b.bytes) {
                        Ok(changed) => report.entries.push(BakedEntry {
                            logical: b.logical,
                            id,
                            kind: "blob",
                            file,
                            format,
                            preview_3ds: None,
                            n64_format: None,
                            compressed: None,
                            ktx2: None,
                            graded: None,
                            changed,
                            groups: Vec::new(),
                        }),
                        Err(e) => errors.push(e),
                    }
                }
            }
            Err(e) => errors.push(e),
        }
    }

    // Model bounds for baked scenes' `Auto` colliders, collected as models
    // bake (no second bake per scene).
    let mut model_bounds = BTreeMap::new();
    let mut models = Vec::new();
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
        models.push((logical, id, decl));
    }
    let baked = crate::par::map_ordered(threads, &models, |(logical, id, decl)| {
        let (logical, id) = (logical.clone(), *id);
        let mut bounds = None;
        let result = (|| -> Result<BakedEntry, String> {
            let source = resolve_source(assets_root, platform, &decl.file)?;
            // A console's bake here (tools) grades like the console's own.
            let mut opts = decl.options();
            opts.grading =
                manifest.console_grading(assets_root, platform, decl.grading.as_deref())?;
            let blob = crate::model::bake_model(&source, &opts)?;
            bounds = crate::model::tmdl_bounds(&blob).map(|b| (logical.clone(), b));
            let file = format!("{id:08x}.model");
            let mut changed = write_if_changed(&out_dir.join(&file), &blob)?;
            // The consoles' graded colors, for the PC's console profiles
            // (issue #64); the PC itself grades the frame.
            let graded = match (&decl.grading, platform) {
                (Some(g), Platform::Pc) => {
                    let mut opts = decl.options();
                    opts.grading = Some(manifest.lut_blob(assets_root, Platform::N64, g)?);
                    let twin = crate::model::bake_model(&source, &opts)?;
                    let file = format!("{id:08x}.graded.model");
                    changed |= write_if_changed(&out_dir.join(&file), &twin)?;
                    Some(file)
                }
                _ => None,
            };
            Ok(BakedEntry {
                logical: logical.clone(),
                id,
                kind: "model",
                file,
                format: "TMDL".into(),
                preview_3ds: None,
                n64_format: None,
                compressed: None,
                ktx2: None,
                graded,
                changed,
                groups: Vec::new(),
            })
        })();
        (result, bounds)
    });
    for (result, bounds) in baked {
        if let Some((logical, b)) = bounds {
            model_bounds.insert(logical, b);
        }
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
            let t = bake_texture(
                out_dir,
                id,
                &logical,
                platform,
                crate::texture::Source::FontAtlas,
                &|p| decl.settings(p),
                (font.width, font.height, &font.rgba, None),
            )?;
            Ok(vec![
                BakedEntry {
                    logical: logical.clone(),
                    id,
                    kind: "sprite",
                    file: sprite_file,
                    format: t.format,
                    preview_3ds: t.preview_3ds,
                    n64_format: t.n64_format,
                    compressed: t.compressed,
                    ktx2: t.ktx2,
                    graded: None,
                    changed: sprite_changed || t.changed,
                    groups: Vec::new(),
                },
                BakedEntry {
                    logical: logical.clone(),
                    id,
                    kind: "font",
                    file: table_file,
                    format: "TFNT".into(),
                    preview_3ds: None,
                    n64_format: None,
                    compressed: None,
                    ktx2: None,
                    graded: None,
                    changed: table_changed,
                    groups: Vec::new(),
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
                n64_format: None,
                compressed: None,
                ktx2: None,
                graded: None,
                changed,
                groups: Vec::new(),
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
                n64_format: None,
                compressed: None,
                ktx2: None,
                graded: None,
                changed,
                groups: Vec::new(),
            })
        })();
        match result {
            Ok(entry) => report.entries.push(entry),
            Err(e) => errors.push(e),
        }
    }

    // Grading LUTs (issue #64): `TLUT` blobs for the PC and the Web, which
    // grade at run time; the consoles' assets are graded by their bake.
    if matches!(platform, Platform::Pc | Platform::Web) {
        for name in manifest.luts.keys() {
            let logical = format!("luts/{name}");
            let id = asset_id(&logical);
            if let Some(other) = by_id.insert(id, logical.clone()) {
                errors.push(format!("hash collision between `{other}` and `{logical}`"));
                continue;
            }
            let result = (|| -> Result<BakedEntry, String> {
                let blob = manifest.lut_blob(assets_root, platform, &logical)?;
                let file = format!("{id:08x}.blob");
                let changed = write_if_changed(&out_dir.join(&file), &blob)?;
                // A LUT is a blob to every loader; hosts upload it to the
                // renderer by its magic (`PcRenderer::take_lut_blob`).
                Ok(BakedEntry {
                    logical: logical.clone(),
                    id,
                    kind: "blob",
                    file,
                    format: "TLUT".into(),
                    preview_3ds: None,
                    n64_format: None,
                    compressed: None,
                    ktx2: None,
                    graded: None,
                    changed,
                    groups: Vec::new(),
                })
            })();
            match result {
                Ok(entry) => report.entries.push(entry),
                Err(e) => errors.push(e),
            }
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
                n64_format: None,
                compressed: None,
                ktx2: None,
                graded: None,
                changed,
                groups: Vec::new(),
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

    for e in &mut report.entries {
        e.groups = manifest.bundles_of(&e.logical);
    }
    write_bundles(&manifest, platform, out_dir, &report).map_err(|e| BakeError(vec![e]))?;

    // A sprite whose N64 format changed with no new byte (a manifest edit)
    // still counts as changed: a running PC host re-uploads it and learns
    // the format its strict mode checks (GitHub #125).
    let before = std::fs::read_to_string(out_dir.join("index.toml")).unwrap_or_default();
    for e in report.entries.iter_mut().filter(|e| e.kind == "sprite") {
        if !e.changed && n64_format_in(&before, &e.logical) != e.n64_format.as_deref() {
            e.changed = true;
        }
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
        if let Some(c) = &e.compressed {
            index.push_str(&format!("compressed = \"{c}\"\n"));
        }
        if let Some(k) = &e.ktx2 {
            index.push_str(&format!("ktx2 = \"{k}\"\n"));
        }
        if let Some(g) = &e.graded {
            index.push_str(&format!("graded = \"{g}\"\n"));
        }
        if !e.groups.is_empty() {
            let names: Vec<String> = e.groups.iter().map(|g| format!("\"{g}\"")).collect();
            index.push_str(&format!("groups = [{}]\n", names.join(", ")));
        }
        if let Some(f) = &e.n64_format {
            index.push_str(&format!("n64_format = \"{f}\"\n"));
        }
    }
    write_if_changed(&out_dir.join("index.toml"), index.as_bytes())
        .map_err(|e| BakeError(vec![e]))?;

    Ok(report)
}

/// The `n64_format` an earlier `index.toml` gave the sprite `logical`.
fn n64_format_in<'a>(index: &'a str, logical: &str) -> Option<&'a str> {
    let key = format!("logical = \"{logical}\"\n");
    index
        .split("[[asset]]")
        .filter(|a| a.contains(&key) && a.contains("kind = \"sprite\""))
        .find_map(|a| {
            a.lines()
                .find_map(|l| l.strip_prefix("n64_format = \""))
                .and_then(|v| v.strip_suffix('"'))
        })
}

/// `groups/<name>.tbnd` for every group but boot (issue #56): each entry's
/// baked files, compressed as the group says. Bundles of groups that no
/// longer exist are removed.
fn write_bundles(
    manifest: &Manifest,
    platform: Platform,
    out_dir: &Path,
    report: &BakeReport,
) -> Result<(), String> {
    use trino_core::bundle::EntryKind as K;
    let dir = out_dir.join("groups");
    let mut keep = Vec::new();
    for group in manifest.group_names().into_iter().skip(1) {
        let mut b =
            crate::groups::BundleBuilder::new(&group, manifest.compression(&group, platform)?);
        for e in report.entries.iter().filter(|e| e.groups.contains(&group)) {
            let read = |file: &str| {
                let p = out_dir.join(file);
                std::fs::read(&p).map_err(|err| format!("{}: {err}", p.display()))
            };
            let kind =
                K::parse(e.kind).ok_or_else(|| format!("`{}`: kind {}", e.logical, e.kind))?;
            b.add_inline(kind, e.id, None, &read(&e.file)?);
            let variants = [
                (K::SpritePreview3ds, &e.preview_3ds),
                (K::SpriteBc, &e.compressed),
                (K::SpriteKtx2, &e.ktx2),
                (K::ModelGraded, &e.graded),
            ];
            for (kind, file) in variants {
                if let Some(f) = file {
                    b.add_inline(kind, e.id, None, &read(f)?);
                }
            }
        }
        let file = format!("{group}.tbnd");
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        write_if_changed(&dir.join(&file), &b.finish())?;
        keep.push(file);
    }
    if let Ok(read) = std::fs::read_dir(&dir) {
        for f in read.flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            if name.ends_with(".tbnd") && !keep.contains(&name) {
                let _ = std::fs::remove_file(f.path());
            }
        }
    }
    Ok(())
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

/// A texture's bake on `platform`: the format it takes there, and on a
/// PC bake its BCn file and 3DS preview.
struct BakedTexture {
    format: String,
    preview_3ds: Option<String>,
    n64_format: Option<String>,
    compressed: Option<String>,
    ktx2: Option<String>,
    changed: bool,
}

/// A master's width, height and RGBA8 pixels.
type Pixels<'a> = (u32, u32, &'a [u8]);

fn bake_texture(
    out_dir: &Path,
    id: u32,
    logical: &str,
    platform: Platform,
    source: crate::texture::Source,
    settings: &dyn Fn(Platform) -> crate::texture::TextureSettings,
    // The master, and the N64's own master when it overrides it.
    (w, h, rgba, n64): (u32, u32, &[u8], Option<Pixels<'_>>),
) -> Result<BakedTexture, String> {
    let choose = |p: Platform| crate::texture::choose(p, logical, source, &settings(p), w, h, rgba);
    let choice = choose(platform)?;
    let mut t = BakedTexture {
        format: choice.format,
        preview_3ds: None,
        n64_format: None,
        compressed: None,
        ktx2: None,
        changed: false,
    };
    if platform == Platform::Web {
        // The Web's KTX2 beside the master (the browser loads one of the
        // two: `xtask/src/web.rs` packs the KTX2 when there is one).
        if let Some(mode) = crate::texture::Ktx2Mode::parse(&t.format) {
            let file = format!("{id:08x}.ktx2");
            let e = crate::texture::ktx2::encode_cached(mode, w, h, rgba)?;
            t.changed |= write_if_changed(&out_dir.join(&file), &e.ktx2)?;
            t.ktx2 = Some(file);
        }
        return Ok(t);
    }
    if platform != Platform::Pc {
        return Ok(t);
    }
    // The N64's format too (GitHub #125): strict mode on the N64 profile
    // checks TMEM by it. A texture the N64 can't take has none (strict
    // mode then assumes 16-bit texels, as before).
    let (nw, nh, npx) = n64.unwrap_or((w, h, rgba));
    t.n64_format = crate::texture::choose(
        Platform::N64,
        logical,
        source,
        &settings(Platform::N64),
        nw,
        nh,
        npx,
    )
    .ok()
    .map(|c| c.format);
    if let Some(bc) = crate::texture::BcFormat::parse(&t.format) {
        let file = format!("{id:08x}.bc");
        let mut blob = b"TBCN".to_vec();
        for v in [w, h, bc.code()] {
            blob.extend_from_slice(&v.to_le_bytes());
        }
        blob.extend_from_slice(&bc.encode(w, h, rgba));
        t.changed |= write_if_changed(&out_dir.join(&file), &blob)?;
        t.compressed = Some(file);
    }
    // The 3DS format is decided on the PC too, so a bad one fails here
    // before a console build, and the 3DS profile draws its result.
    let n3ds =
        crate::texture::N3dsFormat::parse(&choose(Platform::N3ds)?.format).expect("a 3DS format");
    if n3ds != crate::texture::N3dsFormat::Rgba8 {
        let file = format!("{id:08x}.3ds.sprite");
        t.changed |=
            write_if_changed(&out_dir.join(&file), &tspr(w, h, &n3ds.preview(w, h, rgba)))?;
        t.preview_3ds = Some(file);
    }
    Ok(t)
}

pub fn decode_png(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    // Palette (an N64 override keeps its CI palette as an indexed PNG),
    // low-bit gray and 16-bit masters all come out 8 bits a channel.
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
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
        png::ColorType::Grayscale => buf.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[v, a]| [v, v, v, a])
            .collect(),
        other => {
            return Err(format!(
                "{}: unsupported PNG color type {other:?}",
                path.display()
            ));
        }
    };
    Ok((info.width, info.height, rgba))
}

/// Decode a PCM WAV to mono f32 (stereo is averaged).
/// A sound master (WAV, mono or stereo — stereo is averaged) as the audio
/// bake takes it.
pub fn decode_master(path: &Path) -> Result<crate::audio::Master, String> {
    let (rate, samples) = decode_wav(path)?;
    Ok(crate::audio::Master { rate, samples })
}

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
