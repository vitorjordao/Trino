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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelDecl {
    /// Source `.gltf`/`.glb`, relative to `assets/shared/` (or an override).
    pub file: String,
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
