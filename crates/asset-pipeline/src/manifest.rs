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
}

impl Platform {
    pub fn key(self) -> &'static str {
        match self {
            Platform::Pc => "pc",
            Platform::N64 => "n64",
            Platform::N3ds => "3ds",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pc" => Some(Platform::Pc),
            "n64" => Some(Platform::N64),
            "3ds" | "n3ds" => Some(Platform::N3ds),
            _ => None,
        }
    }

    /// Texture formats this platform can represent.
    pub fn supported_texture_formats(self) -> &'static [&'static str] {
        match self {
            Platform::Pc => &["RGBA8"],
            // libdragon mksprite formats that fit typical TMEM budgets.
            Platform::N64 => &["CI4", "CI8", "RGBA5551"],
            Platform::N3ds => &["RGBA8", "RGB565"],
        }
    }

    /// Format used when the manifest does not declare one for this platform.
    pub fn default_texture_format(self) -> Option<&'static str> {
        match self {
            Platform::Pc => Some("RGBA8"),
            // The N64 has no safe default: the developer must choose the
            // TMEM trade-off explicitly.
            Platform::N64 => None,
            Platform::N3ds => Some("RGBA8"),
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
}

impl ModelDecl {
    pub fn options(&self) -> crate::model::ModelOptions {
        let mut opts = crate::model::ModelOptions::default();
        if let Some(fps) = self.anim_fps {
            opts.anim_fps = fps;
        }
        opts.once = self.once.clone();
        opts
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpriteDecl {
    /// Source file, relative to `assets/shared/` (or a platform override).
    pub file: String,
    /// Per-platform texture format, e.g. `{ n64 = "CI4" }`.
    #[serde(default)]
    pub formats: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoundDecl {
    pub file: String,
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
        Ok(manifest)
    }
}

impl SpriteDecl {
    /// The texture format to bake for `platform`, validating support.
    /// Declared-but-unsupported and undeclared-with-no-default are errors.
    pub fn format_for(&self, logical: &str, platform: Platform) -> Result<String, String> {
        let declared = self.formats.get(platform.key());
        match declared {
            Some(f) => {
                if platform.supported_texture_formats().contains(&f.as_str()) {
                    Ok(f.clone())
                } else {
                    Err(format!(
                        "sprite `{logical}`: format `{f}` is not supported on {} (supported: {:?})",
                        platform.key(),
                        platform.supported_texture_formats()
                    ))
                }
            }
            None => platform
                .default_texture_format()
                .map(String::from)
                .ok_or_else(|| {
                    format!(
                        "sprite `{logical}`: no format declared for {} and this platform has no \
                     safe default — add `formats = {{ {} = \"CI4\" }}` (or CI8/RGBA5551) \
                     to the manifest",
                        platform.key(),
                        platform.key()
                    )
                }),
        }
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
        // PC falls back to RGBA8.
        assert_eq!(
            m.sprites["p"].format_for("p", Platform::Pc).unwrap(),
            "RGBA8"
        );
    }

    #[test]
    fn unsupported_format_is_an_error() {
        let m =
            manifest("version = 1\n[sprites.p]\nfile = \"p.png\"\nformats = { n64 = \"RGBA8\" }\n");
        let err = m.sprites["p"].format_for("p", Platform::N64).unwrap_err();
        assert!(err.contains("not supported"), "{err}");
    }
}
