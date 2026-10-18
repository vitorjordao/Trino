//! `assets/manifest.toml` parsing and format validation rules.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

/// A bake target. Mirrors the engine's platforms.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform {
    Pc,
    N64,
    N3ds,
    /// A web browser (issue #47): the PC's content, KTX2 textures. Where
    /// the manifest or the overrides say nothing about the Web, it takes
    /// the PC's choice (a model's `platforms`, `assets/pc/` overrides).
    Web,
}

impl Platform {
    pub const ALL: [Platform; 4] = [Platform::Pc, Platform::N64, Platform::N3ds, Platform::Web];

    /// The scene crate's name for this platform (variants resolve by it).
    pub fn target(self) -> trino_scene::Target {
        match self {
            Platform::Pc => trino_scene::Target::Pc,
            Platform::N64 => trino_scene::Target::N64,
            Platform::N3ds => trino_scene::Target::N3ds,
            Platform::Web => trino_scene::Target::Web,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Platform::Pc => "pc",
            Platform::N64 => "n64",
            Platform::N3ds => "3ds",
            Platform::Web => "web",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pc" => Some(Platform::Pc),
            "n64" => Some(Platform::N64),
            "3ds" | "n3ds" => Some(Platform::N3ds),
            "web" => Some(Platform::Web),
            _ => None,
        }
    }

    /// The platform whose choices this one takes where it has none of its
    /// own: the PC for the Web.
    pub fn fallback(self) -> Option<Platform> {
        match self {
            Platform::Web => Some(Platform::Pc),
            _ => None,
        }
    }

    /// Texture formats this platform can represent (`"auto"` is accepted
    /// besides them: `crate::texture::choose`).
    pub fn supported_texture_formats(self) -> &'static [&'static str] {
        match self {
            // BCn where the GPU samples it (`crate::texture::BcFormat`);
            // the runner decodes to RGBA8 on a GPU without BC.
            Platform::Pc => &["RGBA8", "BC1", "BC3", "BC7"],
            // libdragon mksprite formats that fit typical TMEM budgets. I4/I8
            // (intensity, alpha = intensity) suit white masks tinted at draw
            // time — fonts, UI panels — and upload no palette per blit.
            Platform::N64 => &["CI4", "CI8", "RGBA5551", "I4", "I8"],
            // The PICA200's formats (`tex3ds -f`, `crate::texture`): ETC1
            // is 4 bits a texel, ETC1A4 8 with 4-bit alpha; L8/LA8 are gray,
            // A8 a white mask (the shim tints it).
            Platform::N3ds => &[
                "RGBA8", "RGB565", "RGBA5551", "RGBA4", "LA8", "L8", "A8", "ETC1", "ETC1A4",
            ],
            // KTX2 files of Basis Universal data, transcoded in the browser
            // to BC, ETC2 or ASTC (`crate::texture::ktx2`), or RGBA8.
            Platform::Web => &["RGBA8", "KTX2-ETC1S", "KTX2-UASTC"],
        }
    }

    /// Format used when the manifest does not declare one for this
    /// platform: `auto` (the smallest format that keeps the texture's
    /// `quality`) on the PC, the 3DS and the Web.
    pub fn default_texture_format(self) -> Option<&'static str> {
        match self {
            Platform::Pc | Platform::Web => Some("auto"),
            // The N64 has no safe default: the developer must choose the
            // TMEM trade-off explicitly (`auto` included).
            Platform::N64 => None,
            Platform::N3ds => Some("auto"),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    #[serde(default)]
    pub sprites: BTreeMap<String, SpriteDecl>,
    #[serde(default)]
    pub sounds: BTreeMap<String, SoundDecl>,
    /// Like sounds, but baked with loop metadata (N64) and played on the
    /// music channel. Logical paths are `music/<name>`.
    #[serde(default)]
    pub music: BTreeMap<String, SoundDecl>,
    /// 3D models: glTF masters baked to the portable TMDL format.
    /// Logical paths are `models/<name>`.
    #[serde(default)]
    pub models: BTreeMap<String, ModelDecl>,
    /// Bitmap fonts (PNG glyph grid or TTF) → atlas sprite + glyph table.
    /// Logical paths are `fonts/<name>`.
    #[serde(default)]
    pub fonts: BTreeMap<String, crate::font::FontDecl>,
    /// Raw data delivered byte-for-byte on every target
    /// (`Platform::blob`). Logical paths are `blobs/<name>`.
    #[serde(default)]
    pub blobs: BTreeMap<String, BlobDecl>,
    /// Editor scenes baked to `TSCN` blobs for runtime loading
    /// (`trino_core::scene::BakedScene`). Logical paths are `scenes/<name>`;
    /// `file` is relative to the **project root** (`scenes/x.scene.ron`).
    #[serde(default)]
    pub scenes: BTreeMap<String, BlobDecl>,
    /// Particle effects (`*.particles.ron`, relative to `assets/shared/` or
    /// an override) baked per platform to `TPFX` blobs with that platform's
    /// particle cap (`trino_core::particles::Effect`). Logical paths are
    /// `particles/<name>`.
    #[serde(default)]
    pub particles: BTreeMap<String, BlobDecl>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlobDecl {
    /// Source file: relative to `assets/shared/` (or a platform override)
    /// for `[blobs.*]`, to the project root for `[scenes.*]`.
    pub file: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelDecl {
    /// Source `.gltf`/`.glb` (or a `.prim.ron` placeholder), relative to
    /// `assets/shared/` (or an override).
    pub file: String,
    /// Animation clip sampling rate (frames per second). Default 15.
    #[serde(default)]
    pub anim_fps: Option<f32>,
    /// Clip names that play once and hold the last frame (default: loop).
    #[serde(default)]
    pub once: Vec<String>,
    /// Textured model: a sprite logical path (`sprites/brick`) baked as the
    /// default texture; UVs are kept (glTF `TEXCOORD_0`, generated for fake
    /// objects). Without it textures are sampled into vertex colors.
    #[serde(default)]
    pub texture: Option<String>,
    /// `"rigid"` (default: each vertex follows its heaviest joint — the
    /// N64-era look, cheapest) or `"smooth"` (two-joint blend per vertex;
    /// costs a second transform per skinned vertex).
    #[serde(default)]
    pub skinning: Option<String>,
    /// Bake only for these platforms (`["pc"]`: a model too big for a
    /// console's RAM). Default: every platform. A game draws only what
    /// the running platform has (`draw_model` of a missing model draws
    /// nothing).
    #[serde(default)]
    pub platforms: Option<Vec<String>>,
    /// Bake ambient occlusion into the vertex colors (`ao = true`, or
    /// `ao = { strength = 0.8, distance = 0.5, rays = 64, ground = true }`):
    /// how the consoles get AO, at no run-time cost (`crate::ao`).
    #[serde(default)]
    pub ao: Option<crate::ao::AoDecl>,
}

impl ModelDecl {
    /// Is this model baked for `platform`? A list naming the PC bakes for
    /// the Web too (a PC-class model is a Web model unless the list says
    /// `web` itself).
    pub fn bakes_for(&self, platform: Platform) -> bool {
        self.platforms.as_ref().is_none_or(|ps| {
            let named = |t: Platform| ps.iter().any(|p| Platform::parse(p) == Some(t));
            named(platform) || platform.fallback().is_some_and(named)
        })
    }

    pub fn options(&self) -> crate::model::ModelOptions {
        let mut opts = crate::model::ModelOptions::default();
        if let Some(fps) = self.anim_fps {
            opts.anim_fps = fps;
        }
        opts.once = self.once.clone();
        opts.texture = self.texture.clone();
        opts.smooth = self.skinning.as_deref() == Some("smooth");
        opts.ao = self.ao.as_ref().and_then(|a| a.bake());
        opts
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpriteDecl {
    /// Source file, relative to `assets/shared/` (or a platform override).
    pub file: String,
    /// Per-platform texture format, e.g. `{ n64 = "CI4", 3ds = "auto" }`.
    #[serde(default)]
    pub formats: BTreeMap<String, String>,
    /// `auto`'s quality preset: `low` (30 dB), `normal` (35, the default)
    /// or `high` (40).
    #[serde(default)]
    pub quality: Option<String>,
    /// The least PSNR (dB) against the master: `auto` keeps it, and a
    /// declared format below it fails the bake.
    #[serde(default)]
    pub min_psnr: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoundDecl {
    pub file: String,
    /// Bake loop metadata — needed on the N64, where the loop lives in the
    /// wav64. Sounds default to `false` (set it for looping voices: ambient
    /// sources, engines); music defaults to `true` (set `false` for
    /// stingers, which play once).
    #[serde(default, rename = "loop")]
    pub looped: Option<bool>,
    /// Play from storage instead of loading whole (#53). Music defaults to
    /// `true`, sounds to `false`.
    #[serde(default)]
    pub stream: Option<bool>,
    /// `"auto"` (the default: N64 VADPCM, 3DS DSP-ADPCM, PC and Web
    /// Vorbis), one codec for every platform, or a table
    /// `{ n64 = "opus", 3ds = "pcm" }` (platforms left out take `auto`;
    /// the Web takes the PC's). `crate::audio::Codec` lists them.
    #[serde(default)]
    pub codec: Option<PerPlatform<String>>,
    /// Lossy codecs only (Vorbis): 0..=10, like `oggenc -q`. Default 5.
    #[serde(default)]
    pub quality: Option<f32>,
    /// Resample the master per platform: `22050`, or
    /// `{ n64 = 22050, 3ds = 32728 }` (left out = the master's rate).
    #[serde(default)]
    pub sample_rate: Option<PerPlatform<u32>>,
    /// The mixer bus it plays on by default (`trino_core::Bus` labels:
    /// music, sfx, voice, ambience, ui). Default: `sfx` for sounds, `music`
    /// for music.
    #[serde(default)]
    pub bus: Option<String>,
}

/// One value for every platform, or a table by platform key (`pc`, `n64`,
/// `3ds`, `web`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum PerPlatform<T> {
    All(T),
    Each(BTreeMap<String, T>),
}

impl<T: Clone> PerPlatform<T> {
    /// The value for `platform`: its own key (`3ds` or `n3ds`), else the
    /// fallback platform's (the PC's for the Web), else `None`.
    pub fn get(&self, platform: Platform) -> Option<T> {
        match self {
            PerPlatform::All(v) => Some(v.clone()),
            PerPlatform::Each(map) => {
                let of = |p: Platform| {
                    map.iter()
                        .find(|(k, _)| Platform::parse(k) == Some(p))
                        .map(|(_, v)| v.clone())
                };
                of(platform).or_else(|| platform.fallback().and_then(of))
            }
        }
    }

    /// Keys that name no platform.
    pub fn unknown_keys(&self) -> Vec<String> {
        match self {
            PerPlatform::All(_) => Vec::new(),
            PerPlatform::Each(map) => map
                .keys()
                .filter(|k| Platform::parse(k).is_none())
                .cloned()
                .collect(),
        }
    }
}

impl SoundDecl {
    /// Loop flag of a `[sounds.*]` entry.
    pub fn sound_loops(&self) -> bool {
        self.looped.unwrap_or(false)
    }

    /// Loop flag of a `[music.*]` entry.
    pub fn music_loops(&self) -> bool {
        self.looped.unwrap_or(true)
    }

    /// The bake settings of this sound (`music` = a `[music.*]` entry) for
    /// `platform`; errors name the key and the accepted values.
    pub fn audio_settings(
        &self,
        music: bool,
        platform: Platform,
    ) -> Result<crate::audio::AudioSettings, String> {
        crate::audio::AudioSettings::from_decl(self, music, platform)
    }
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let manifest: Manifest =
            toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if manifest.version != 1 {
            return Err(format!(
                "{}: unsupported manifest version {} (expected 1)",
                path.display(),
                manifest.version
            ));
        }
        let textures = manifest
            .sprites
            .iter()
            .map(|(n, d)| (format!("sprites/{n}"), d.quality.as_deref(), d.min_psnr))
            .chain(
                manifest
                    .fonts
                    .iter()
                    .map(|(n, d)| (format!("fonts/{n}"), d.quality.as_deref(), d.min_psnr)),
            );
        for (logical, quality, min_psnr) in textures {
            if let Some(q) = quality
                && crate::texture::Quality::parse(q).is_none()
            {
                return Err(format!(
                    "{}: `{logical}`: quality `{q}` (low, normal, high)",
                    path.display()
                ));
            }
            if let Some(v) = min_psnr
                && !(v.is_finite() && v >= 0.0)
            {
                return Err(format!(
                    "{}: `{logical}`: min_psnr {v} (a number of dB, 0 or more)",
                    path.display()
                ));
            }
        }
        let audio = [
            ("sounds", false, &manifest.sounds),
            ("music", true, &manifest.music),
        ];
        for (prefix, music, section) in audio {
            for (name, decl) in section {
                for platform in Platform::ALL {
                    decl.audio_settings(music, platform)
                        .map_err(|e| format!("{}: `{prefix}/{name}`: {e}", path.display()))?;
                }
            }
        }
        for (name, m) in &manifest.models {
            for p in m.platforms.iter().flatten() {
                if Platform::parse(p).is_none() {
                    return Err(format!(
                        "{}: model `{name}`: unknown platform `{p}` (pc, n64, 3ds, web)",
                        path.display()
                    ));
                }
            }
        }
        Ok(manifest)
    }
}

impl SpriteDecl {
    /// What `crate::texture::choose` needs for `platform`.
    pub fn settings(&self, platform: Platform) -> crate::texture::TextureSettings {
        texture_settings(
            self.formats.get(platform.key()),
            self.quality.as_deref(),
            self.min_psnr,
        )
    }

    /// The declared format for `platform` (`"auto"` included) or the
    /// platform's default, validating support; `crate::texture::choose`
    /// resolves `auto` from the pixels. Declared-but-unsupported and
    /// undeclared-with-no-default are errors.
    pub fn format_for(&self, logical: &str, platform: Platform) -> Result<String, String> {
        let declared = self.formats.get(platform.key());
        match declared {
            Some(f) => {
                if f == "auto" || platform.supported_texture_formats().contains(&f.as_str()) {
                    Ok(f.clone())
                } else {
                    Err(format!(
                        "sprite `{logical}`: format `{f}` is not supported on {} (supported: auto, {})",
                        platform.key(),
                        platform.supported_texture_formats().join(", ")
                    ))
                }
            }
            None => platform
                .default_texture_format()
                .map(String::from)
                .ok_or_else(|| {
                    format!(
                        "sprite `{logical}`: no format declared for {} and this platform has no \
                     safe default — add `formats = {{ {} = \"auto\" }}` (or CI4/CI8/RGBA5551/\
                     I4/I8) to the manifest",
                        platform.key(),
                        platform.key()
                    )
                }),
        }
    }
}

/// Settings from a sprite's or font's manifest keys (validated at load).
pub(crate) fn texture_settings(
    format: Option<&String>,
    quality: Option<&str>,
    min_psnr: Option<f64>,
) -> crate::texture::TextureSettings {
    crate::texture::TextureSettings {
        format: format.cloned(),
        quality: quality
            .and_then(crate::texture::Quality::parse)
            .unwrap_or_default(),
        min_psnr,
        default: None,
    }
}

/// Text-level manifest edits that keep the file's comments and layout —
/// what the editor (and scripts) use to add or retarget models.
pub mod edit {
    /// Append `[models.<name>]` with `file`. Errors if the name exists.
    pub fn add_model(text: &str, name: &str, file: &str) -> Result<String, String> {
        if has_table(text, "models", name) {
            return Err(format!("model `{name}` already exists in the manifest"));
        }
        let mut out = text.trim_end().to_string();
        out.push_str(&format!("\n\n[models.{name}]\nfile = \"{file}\"\n"));
        Ok(out)
    }

    /// Append `[sprites.<name>]` with `file` and an explicit N64 format
    /// (the N64 has no safe default). Errors if the name exists.
    pub fn add_sprite(text: &str, name: &str, file: &str, n64: &str) -> Result<String, String> {
        if has_table(text, "sprites", name) {
            return Err(format!("sprite `{name}` already exists in the manifest"));
        }
        let mut out = text.trim_end().to_string();
        out.push_str(&format!(
            "\n\n[sprites.{name}]\nfile = \"{file}\"\nformats = {{ n64 = \"{n64}\" }}\n"
        ));
        Ok(out)
    }

    /// Append `[blobs.<name>]` with `file`. Errors if the name exists.
    pub fn add_blob(text: &str, name: &str, file: &str) -> Result<String, String> {
        if has_table(text, "blobs", name) {
            return Err(format!("blob `{name}` already exists in the manifest"));
        }
        let mut out = text.trim_end().to_string();
        out.push_str(&format!("\n\n[blobs.{name}]\nfile = \"{file}\"\n"));
        Ok(out)
    }

    /// A valid logical name from a file stem (`My Rock 2` -> `my-rock-2`).
    pub fn name_from_stem(stem: &str) -> String {
        let mut out = String::new();
        for c in stem.chars() {
            if c.is_ascii_alphanumeric() {
                out.push(c.to_ascii_lowercase());
            } else if !out.ends_with('-') && !out.is_empty() {
                out.push('-');
            }
        }
        out.trim_end_matches('-').to_string()
    }

    /// Point `[models.<name>]` at a new `file` (placeholder → real art).
    pub fn set_model_file(text: &str, name: &str, file: &str) -> Result<String, String> {
        let header = format!("[models.{name}]");
        let mut lines: Vec<String> = text.lines().map(String::from).collect();
        let start = lines
            .iter()
            .position(|l| l.trim() == header)
            .ok_or_else(|| format!("model `{name}` not found in the manifest"))?;
        for line in lines.iter_mut().skip(start + 1) {
            let t = line.trim_start();
            if t.starts_with('[') {
                break;
            }
            if t.starts_with("file") && t[4..].trim_start().starts_with('=') {
                *line = format!("file = \"{file}\"");
                let mut out = lines.join("\n");
                out.push('\n');
                return Ok(out);
            }
        }
        Err(format!("model `{name}` has no `file` line"))
    }

    /// Set (`Some(value)`, a TOML value such as `{ strength = 0.8 }`) or
    /// remove (`None`) the `ao` key of `[models.<name>]`: baked vertex
    /// ambient occlusion (`crate::ao`). Keeps every other line.
    pub fn set_model_ao(text: &str, name: &str, value: Option<&str>) -> Result<String, String> {
        set_key(text, "models", name, "ao", value)
    }

    /// Set or remove (`None`) one platform's entry of the `formats` table
    /// of `[sprites.<name>]` or `[fonts.<name>]` (`kind`): `"auto"`, `"ETC1"`,
    /// ... — what the editor's texture inspector writes. An empty table
    /// removes the line. Keeps every other line.
    pub fn set_texture_format(
        text: &str,
        kind: &str,
        name: &str,
        platform: &str,
        format: Option<&str>,
    ) -> Result<String, String> {
        let current = table_value(text, kind, name, "formats")?;
        let mut formats: std::collections::BTreeMap<String, String> = match current {
            Some(v) => toml::from_str::<toml::Table>(&format!("v = {v}"))
                .ok()
                .and_then(|t| t.get("v").cloned())
                .and_then(|v| v.try_into().ok())
                .ok_or_else(|| format!("`[{kind}.{name}]`: `formats` is not a table of strings"))?,
            None => Default::default(),
        };
        match format {
            Some(f) => {
                formats.insert(platform.to_string(), f.to_string());
            }
            None => {
                formats.remove(platform);
            }
        }
        let value = (!formats.is_empty()).then(|| {
            let items: Vec<String> = formats
                .iter()
                .map(|(k, v)| format!("{k} = \"{v}\""))
                .collect();
            format!("{{ {} }}", items.join(", "))
        });
        set_key(text, kind, name, "formats", value.as_deref())
    }

    /// Set or remove a texture's `quality` (`"high"`, written quoted) or
    /// `min_psnr` (a number): `key` is either.
    pub fn set_texture_key(
        text: &str,
        kind: &str,
        name: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<String, String> {
        let value = match (key, value) {
            ("quality", Some(q)) => Some(format!("\"{q}\"")),
            ("min_psnr", Some(v)) => Some(v.to_string()),
            (_, None) => None,
            (other, _) => {
                return Err(format!(
                    "`{other}` is not a texture key (quality, min_psnr)"
                ));
            }
        };
        set_key(text, kind, name, key, value.as_deref())
    }

    /// Set or remove (`None`) a key of `[sounds.<name>]` / `[music.<name>]`
    /// (`kind`) that is the same on every platform: `quality` (0..=10),
    /// `stream` and `loop` (`true`/`false`) or `bus` (a bus label, written
    /// quoted) — what the editor's audio inspector writes. Keeps every
    /// other line.
    pub fn set_audio_key(
        text: &str,
        kind: &str,
        name: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<String, String> {
        let value = match (key, value) {
            ("quality" | "stream" | "loop" | "bus", None) => None,
            ("quality", Some(v)) => {
                let q: f32 = v
                    .parse()
                    .map_err(|_| format!("quality `{v}` is not a number"))?;
                if !(0.0..=10.0).contains(&q) {
                    return Err(format!("quality {q} (0 to 10)"));
                }
                Some(v.to_string())
            }
            ("stream" | "loop", Some(v @ ("true" | "false"))) => Some(v.to_string()),
            ("stream" | "loop", Some(v)) => return Err(format!("{key} `{v}` (true or false)")),
            ("bus", Some(b)) => {
                if trino_core::Bus::from_label(b).is_none_or(|b| b == trino_core::Bus::Master) {
                    return Err(format!("bus `{b}` (music, sfx, voice, ambience, ui)"));
                }
                Some(format!("\"{b}\""))
            }
            (other, _) => {
                return Err(format!(
                    "`{other}` is not an audio key (quality, stream, loop, bus)"
                ));
            }
        };
        set_key(text, kind, name, key, value.as_deref())
    }

    /// Set or remove (`None`) one platform's entry of a per-platform audio
    /// key of `[sounds.<name>]` / `[music.<name>]`: `codec` (`"vadpcm"`,
    /// ..., written quoted) or `sample_rate` (a number). A value shared by
    /// every platform (`codec = "pcm"`) becomes a table first, so the other
    /// platforms keep it; an empty table removes the line. The table is
    /// written in platform order (pc, n64, 3ds, web), keys that name no
    /// platform kept after them.
    pub fn set_audio_platform(
        text: &str,
        kind: &str,
        name: &str,
        key: &str,
        platform: crate::Platform,
        value: Option<&str>,
    ) -> Result<String, String> {
        let literal = match (key, value) {
            ("codec" | "sample_rate", None) => None,
            ("codec", Some(c)) => {
                if c != "auto" && crate::audio::Codec::parse(c).is_none() {
                    return Err(format!(
                        "codec `{c}` (auto, pcm, vorbis, vadpcm, opus, dspadpcm)"
                    ));
                }
                Some(toml::Value::String(c.to_string()))
            }
            ("sample_rate", Some(r)) => {
                let r: u32 = r
                    .parse()
                    .map_err(|_| format!("sample_rate `{r}` is not a whole number"))?;
                if !(4_000..=96_000).contains(&r) {
                    return Err(format!("sample_rate {r} (4000 to 96000)"));
                }
                Some(toml::Value::Integer(r as i64))
            }
            (other, _) => {
                return Err(format!(
                    "`{other}` is not a per-platform audio key (codec, sample_rate)"
                ));
            }
        };
        let current = table_value(text, kind, name, key)?;
        // (key as written, value), in file order.
        let mut entries: Vec<(String, toml::Value)> = match current {
            None => Vec::new(),
            Some(v) => {
                let parsed = toml::from_str::<toml::Table>(&format!("v = {v}"))
                    .ok()
                    .and_then(|mut t| t.remove("v"))
                    .ok_or_else(|| format!("`[{kind}.{name}]`: `{key}` doesn't parse"))?;
                match parsed {
                    toml::Value::Table(t) => t.into_iter().collect(),
                    // The Web falls back to the PC's value.
                    shared => [
                        crate::Platform::Pc,
                        crate::Platform::N64,
                        crate::Platform::N3ds,
                    ]
                    .iter()
                    .map(|p| (p.key().to_string(), shared.clone()))
                    .collect(),
                }
            }
        };
        entries.retain(|(k, _)| crate::Platform::parse(k) != Some(platform));
        if let Some(v) = literal {
            entries.push((platform.key().to_string(), v));
        }
        let order = |k: &str| {
            crate::Platform::parse(k)
                .and_then(|p| crate::Platform::ALL.iter().position(|q| *q == p))
                .unwrap_or(crate::Platform::ALL.len())
        };
        entries.sort_by_key(|(k, _)| order(k));
        let value = (!entries.is_empty()).then(|| {
            let items: Vec<String> = entries.iter().map(|(k, v)| format!("{k} = {v}")).collect();
            format!("{{ {} }}", items.join(", "))
        });
        set_key(text, kind, name, key, value.as_deref())
    }

    /// The lines of `[kind.name]`: (header index, end) with the blank lines
    /// and comments before the next header left out.
    fn table_span(lines: &[String], kind: &str, name: &str) -> Result<(usize, usize), String> {
        let header = format!("[{kind}.{name}]");
        let start = lines
            .iter()
            .position(|l| l.trim() == header)
            .ok_or_else(|| format!("`{kind}/{name}` not found in the manifest"))?;
        let mut end = lines
            .iter()
            .skip(start + 1)
            .position(|l| l.trim_start().starts_with('['))
            .map_or(lines.len(), |i| start + 1 + i);
        while end > start + 1 && {
            let t = lines[end - 1].trim();
            t.is_empty() || t.starts_with('#')
        } {
            end -= 1;
        }
        Ok((start, end))
    }

    fn is_key(line: &str, key: &str) -> bool {
        let t = line.trim_start();
        t.starts_with(key) && t[key.len()..].trim_start().starts_with('=')
    }

    /// The raw value text of `key` in `[kind.name]`, if set.
    fn table_value(
        text: &str,
        kind: &str,
        name: &str,
        key: &str,
    ) -> Result<Option<String>, String> {
        let lines: Vec<String> = text.lines().map(String::from).collect();
        let (start, end) = table_span(&lines, kind, name)?;
        Ok((start + 1..end)
            .find(|&i| is_key(&lines[i], key))
            .map(|i| lines[i].split_once('=').unwrap().1.trim().to_string()))
    }

    /// Set (`Some`, a TOML value) or remove (`None`) `key` in
    /// `[kind.name]`, keeping every other line.
    fn set_key(
        text: &str,
        kind: &str,
        name: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<String, String> {
        let mut lines: Vec<String> = text.lines().map(String::from).collect();
        let (start, end) = table_span(&lines, kind, name).map_err(|_| {
            let what = kind.trim_end_matches('s');
            format!("{what} `{name}` not found in the manifest")
        })?;
        let at = (start + 1..end).find(|&i| is_key(&lines[i], key));
        match (at, value) {
            (Some(i), Some(v)) => lines[i] = format!("{key} = {v}"),
            (Some(i), None) => {
                lines.remove(i);
            }
            (None, Some(v)) => lines.insert(end, format!("{key} = {v}")),
            (None, None) => return Ok(text.to_string()),
        }
        let mut out = lines.join("\n");
        out.push('\n');
        Ok(out)
    }

    pub fn has_table(text: &str, kind: &str, name: &str) -> bool {
        let header = format!("[{kind}.{name}]");
        text.lines().any(|l| l.trim() == header)
    }

    /// Valid logical name: lowercase ASCII letters, digits, `_` and `-`.
    pub fn valid_name(name: &str) -> bool {
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn set_model_ao_adds_replaces_and_removes_only_that_line() {
            let text = "# top\nversion = 1\n\n[models.rock]\nfile = \"models/rock.glb\"\n\n# a crate\n[models.crate]\nfile = \"models/crate.glb\"\nanim_fps = 12.0\n";
            let added = set_model_ao(text, "rock", Some("{ strength = 0.8 }")).unwrap();
            assert!(added.contains(
                "[models.rock]\nfile = \"models/rock.glb\"\nao = { strength = 0.8 }\n\n# a crate"
            ));
            let parsed: crate::Manifest = toml::from_str(&added).unwrap();
            assert!(parsed.models["rock"].options().ao.is_some());
            assert!(parsed.models["crate"].options().ao.is_none());
            let replaced = set_model_ao(&added, "rock", Some("true")).unwrap();
            assert!(replaced.contains("ao = true\n") && !replaced.contains("strength"));
            // The last table: appended at its end.
            let last = set_model_ao(text, "crate", Some("true")).unwrap();
            assert!(last.ends_with("anim_fps = 12.0\nao = true\n"));
            let removed = set_model_ao(&replaced, "rock", None).unwrap();
            assert_eq!(removed, text);
            assert_eq!(set_model_ao(text, "rock", None).unwrap(), text);
            assert!(set_model_ao(text, "ghost", None).is_err());
        }

        #[test]
        fn texture_edits_touch_one_key_and_parse_back() {
            let text = "version = 1\n\n[sprites.grass]\nfile = \"sprites/grass.png\"\nformats = { n64 = \"CI8\" }\n\n# fonts\n[fonts.ui]\nfile = \"fonts/ui.ttf\"\n";
            let a = set_texture_format(text, "sprites", "grass", "3ds", Some("ETC1")).unwrap();
            assert!(
                a.contains("formats = { 3ds = \"ETC1\", n64 = \"CI8\" }\n\n# fonts"),
                "{a}"
            );
            let b = set_texture_format(&a, "sprites", "grass", "3ds", Some("auto")).unwrap();
            let c = set_texture_key(&b, "sprites", "grass", "quality", Some("high")).unwrap();
            let d = set_texture_key(&c, "sprites", "grass", "min_psnr", Some("38")).unwrap();
            let m: crate::Manifest = toml::from_str(&d).unwrap();
            let s = m.sprites["grass"].settings(crate::Platform::N3ds);
            assert_eq!(s.format.as_deref(), Some("auto"));
            assert_eq!(
                (s.quality, s.min_psnr),
                (crate::texture::Quality::High, Some(38.0))
            );
            // A font gets its own table; removing the last entry drops the
            // line.
            let f = set_texture_format(&d, "fonts", "ui", "pc", Some("BC3")).unwrap();
            assert!(
                f.ends_with("[fonts.ui]\nfile = \"fonts/ui.ttf\"\nformats = { pc = \"BC3\" }\n"),
                "{f}"
            );
            let g = set_texture_format(&f, "fonts", "ui", "pc", None).unwrap();
            assert_eq!(g, d);
            let back = set_texture_key(&d, "sprites", "grass", "min_psnr", None).unwrap();
            let back = set_texture_key(&back, "sprites", "grass", "quality", None).unwrap();
            let back = set_texture_format(&back, "sprites", "grass", "3ds", None).unwrap();
            assert_eq!(back, text);
            assert!(set_texture_format(text, "sprites", "ghost", "pc", None).is_err());
            assert!(set_texture_key(text, "sprites", "grass", "file", Some("x")).is_err());
        }

        #[test]
        fn audio_edits_touch_one_key_and_parse_back() {
            use crate::Platform;
            use crate::audio::Codec;
            let text = "version = 1\n\n[sounds.beep]\nfile = \"sounds/beep.wav\"\ncodec = \"pcm\"\n\n# music\n[music.theme]\nfile = \"music/theme.wav\"\n";
            // A shared codec becomes a table: the other platforms keep it.
            let a = set_audio_platform(
                text,
                "sounds",
                "beep",
                "codec",
                Platform::N64,
                Some("vadpcm"),
            )
            .unwrap();
            assert!(
                a.contains("codec = { pc = \"pcm\", n64 = \"vadpcm\", 3ds = \"pcm\" }\n\n# music"),
                "{a}"
            );
            let m: crate::Manifest = toml::from_str(&a).unwrap();
            let s = |m: &crate::Manifest, p| m.sounds["beep"].audio_settings(false, p).unwrap();
            assert_eq!(s(&m, Platform::N64).codec, Codec::Vadpcm);
            assert_eq!(
                s(&m, Platform::Web).codec,
                Codec::Pcm,
                "the Web takes the PC's"
            );
            // A table entry written as `n3ds` is replaced, not duplicated.
            let b = a.replace("3ds = ", "n3ds = ");
            let b = set_audio_platform(&b, "sounds", "beep", "codec", Platform::N3ds, Some("auto"))
                .unwrap();
            assert!(b.contains("3ds = \"auto\" }") && !b.contains("n3ds"), "{b}");
            // Music: a per-platform rate, then the shared keys.
            let c = set_audio_platform(
                text,
                "music",
                "theme",
                "sample_rate",
                Platform::N64,
                Some("22050"),
            )
            .unwrap();
            assert!(c.ends_with("sample_rate = { n64 = 22050 }\n"), "{c}");
            let c = set_audio_key(&c, "music", "theme", "quality", Some("6")).unwrap();
            let c = set_audio_key(&c, "music", "theme", "stream", Some("false")).unwrap();
            let c = set_audio_key(&c, "music", "theme", "bus", Some("ambience")).unwrap();
            let c = set_audio_key(&c, "music", "theme", "loop", Some("false")).unwrap();
            let m: crate::Manifest = toml::from_str(&c).unwrap();
            let t = m.music["theme"]
                .audio_settings(true, Platform::N64)
                .unwrap();
            assert_eq!(t.sample_rate, Some(22050));
            assert_eq!((t.quality, t.stream, t.looped), (6.0, false, false));
            assert_eq!(t.bus, trino_core::Bus::Ambience);
            // Removing the last entry and every key gives the text back.
            let d = set_audio_platform(&c, "music", "theme", "sample_rate", Platform::N64, None)
                .unwrap();
            let d = ["quality", "stream", "bus", "loop"].iter().fold(d, |t, k| {
                set_audio_key(&t, "music", "theme", k, None).unwrap()
            });
            assert_eq!(d, text);
            // Bad values never reach the file.
            assert!(set_audio_key(text, "music", "theme", "quality", Some("11")).is_err());
            assert!(set_audio_key(text, "music", "theme", "bus", Some("master")).is_err());
            assert!(set_audio_key(text, "music", "theme", "stream", Some("yes")).is_err());
            assert!(set_audio_key(text, "music", "theme", "file", Some("x")).is_err());
            assert!(
                set_audio_platform(text, "sounds", "beep", "codec", Platform::Pc, Some("mp3"))
                    .is_err()
            );
            assert!(
                set_audio_platform(
                    text,
                    "sounds",
                    "beep",
                    "sample_rate",
                    Platform::Pc,
                    Some("100")
                )
                .is_err()
            );
            assert!(set_audio_key(text, "sounds", "ghost", "loop", Some("true")).is_err());
        }

        #[test]
        fn add_and_retarget_keep_comments() {
            let text =
                "# header comment\nversion = 1\n\n[models.cube]\nfile = \"models/cube.glb\"\n";
            let added = add_model(text, "crate", "models/proxies/crate.prim.ron").unwrap();
            assert!(added.starts_with("# header comment"));
            assert!(has_table(&added, "models", "crate"));
            assert!(add_model(&added, "crate", "x").is_err());
            let moved = set_model_file(&added, "crate", "models/crate.glb").unwrap();
            assert!(moved.contains("[models.crate]\nfile = \"models/crate.glb\""));
            assert!(
                moved.contains("file = \"models/cube.glb\""),
                "other tables untouched"
            );
            let parsed: super::super::Manifest = toml::from_str(&moved).unwrap();
            assert_eq!(parsed.models["crate"].file, "models/crate.glb");
            assert!(valid_name("rock-02") && !valid_name("Rock") && !valid_name(""));
            let with_sprite =
                add_sprite(&moved, "bark", "sprites/imported/bark.png", "CI8").unwrap();
            let parsed: super::super::Manifest = toml::from_str(&with_sprite).unwrap();
            assert_eq!(parsed.sprites["bark"].formats["n64"], "CI8");
            assert!(add_sprite(&with_sprite, "bark", "x", "CI4").is_err());
            assert_eq!(name_from_stem("My Rock (2)"), "my-rock-2");
            assert!(valid_name(&name_from_stem("KayKit_Door v1")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(text: &str) -> Manifest {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn the_web_takes_the_pcs_choices_and_its_own_formats() {
        let m = manifest(
            r#"
            version = 1
            [models.big]
            file = "big.glb"
            platforms = ["pc"]
            [models.tiny]
            file = "tiny.glb"
            platforms = ["n64"]
            [models.own]
            file = "own.glb"
            platforms = ["web"]
            [sprites.s]
            file = "s.png"
            formats = { web = "KTX2-ETC1S", pc = "BC1" }
            "#,
        );
        // A PC-only model is a Web model; a console-only one is not.
        assert!(m.models["big"].bakes_for(Platform::Web));
        assert!(!m.models["big"].bakes_for(Platform::N64));
        assert!(!m.models["tiny"].bakes_for(Platform::Web));
        assert!(
            m.models["own"].bakes_for(Platform::Web) && !m.models["own"].bakes_for(Platform::Pc)
        );
        // Texture formats are the Web's own (no fallback: BC1 is not one).
        assert_eq!(
            m.sprites["s"].format_for("s", Platform::Web).unwrap(),
            "KTX2-ETC1S"
        );
        let bad = manifest(
            "version = 1
[sprites.s]
file = \"s.png\"
formats = { web = \"BC1\" }
",
        );
        let e = bad.sprites["s"].format_for("s", Platform::Web).unwrap_err();
        assert!(e.contains("KTX2-UASTC"), "{e}");
        assert_eq!(Platform::parse("web"), Some(Platform::Web));
        assert_eq!(Platform::Web.fallback(), Some(Platform::Pc));
        assert_eq!(Platform::Web.default_texture_format(), Some("auto"));
    }

    #[test]
    fn parses_minimal_manifest() {
        let m = manifest(
            r#"
            version = 1
            [sprites.player]
            file = "sprites/player.png"
            formats = { n64 = "CI4" }
            [sounds.beep]
            file = "sounds/beep.wav"
            "#,
        );
        assert_eq!(m.sprites.len(), 1);
        assert_eq!(m.sounds["beep"].file, "sounds/beep.wav");
    }

    #[test]
    fn n64_requires_explicit_format() {
        let m = manifest("version = 1\n[sprites.p]\nfile = \"p.png\"\n");
        let err = m.sprites["p"].format_for("p", Platform::N64).unwrap_err();
        assert!(err.contains("no format declared"), "{err}");
        // PC and 3DS fall back to auto.
        assert_eq!(
            m.sprites["p"].format_for("p", Platform::Pc).unwrap(),
            "auto"
        );
        assert_eq!(
            m.sprites["p"].format_for("p", Platform::N3ds).unwrap(),
            "auto"
        );
    }

    #[test]
    fn quality_and_min_psnr_are_validated_at_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.toml");
        let load = |extra: &str| {
            std::fs::write(
                &path,
                format!("version = 1\n[sprites.p]\nfile = \"p.png\"\n{extra}\n"),
            )
            .unwrap();
            Manifest::load(&path)
        };
        let m = load("quality = \"high\"\nmin_psnr = 38.5").unwrap();
        let s = m.sprites["p"].settings(Platform::N3ds);
        assert_eq!(s.quality, crate::texture::Quality::High);
        assert_eq!((s.format, s.min_psnr), (None, Some(38.5)));
        assert!(
            load("quality = \"ultra\"")
                .unwrap_err()
                .contains("low, normal, high")
        );
        assert!(load("min_psnr = -1.0").unwrap_err().contains("min_psnr"));
    }

    #[test]
    fn unsupported_format_is_an_error() {
        let m =
            manifest("version = 1\n[sprites.p]\nfile = \"p.png\"\nformats = { n64 = \"RGBA8\" }\n");
        let err = m.sprites["p"].format_for("p", Platform::N64).unwrap_err();
        assert!(err.contains("not supported"), "{err}");
    }
}
