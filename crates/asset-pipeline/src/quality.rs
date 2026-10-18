//! The project's quality presets (issue #55, ADR 0023 §6): `assets/quality.ron`
//! says what Low, Medium, High and Ultra mean for this game (a preset
//! names only what it changes from the engine's built-in values), and
//! `trino.toml`'s `[pc.quality]` picks the first launch's preset
//! (`default = "auto"`: the calibration bench) and where the file is
//! (`presets = "assets/quality.ron"`).
//!
//! The PC and Web bakes turn both into one `quality/presets` blob: a
//! `default=` line, then `[low]`…`[ultra]` sections of `key=value` lines
//! (every field: [`trino_core::PresetTable::write_text`]). Hosts read it
//! with `trino_render_wgpu::quality::baked`. The consoles' presets are
//! fixed: their bakes carry nothing.
//!
//! ```ron
//! #![enable(implicit_some)]
//! (
//!     version: 1,
//!     low: (render_scale: 0.7, particle_scale: 0.4),
//!     ultra: (render_scale: 1.25, frame_limit: "unlimited"),
//! )
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use trino_core::{
    AoSetting, DynRes, PostSetting, PresetTable, QualityPreset, QualitySettings, ShadowSetting,
};

/// The schema version this engine reads.
pub const QUALITY_VERSION: u32 = 1;
/// The baked table's logical path (a blob).
pub const LOGICAL: &str = "quality/presets";
/// The conventional file, relative to the project root.
pub const FILE: &str = "assets/quality.ron";

/// `assets/quality.ron`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct QualityFile {
    pub version: u32,
    #[serde(default, skip_serializing_if = "PresetDecl::is_empty")]
    pub low: PresetDecl,
    #[serde(default, skip_serializing_if = "PresetDecl::is_empty")]
    pub medium: PresetDecl,
    #[serde(default, skip_serializing_if = "PresetDecl::is_empty")]
    pub high: PresetDecl,
    #[serde(default, skip_serializing_if = "PresetDecl::is_empty")]
    pub ultra: PresetDecl,
}

/// One preset's changes from the built-in values (absent: unchanged).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct PresetDecl {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub render_scale: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_resolution: Option<DynResDecl>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msaa: Option<u8>,
    /// `off` / `low` / `medium` / `high`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ao: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadows: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lod_bias: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub particle_scale: Option<f32>,
    /// `default` / `point` / `bilinear`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub texture_filter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anisotropy: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sharpen: Option<f32>,
    /// `vsync`, `unlimited` or a cap in fps (`"60"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_limit: Option<String>,
}

/// Dynamic resolution in the file: `Off`, or `On(...)` with the frame time
/// to hold (0: 90% of the frame limit's interval) and the scale's range.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum DynResDecl {
    Off,
    On {
        #[serde(default)]
        target_us: u32,
        min_scale: f32,
        max_scale: f32,
    },
}

impl PresetDecl {
    pub fn is_empty(&self) -> bool {
        *self == PresetDecl::default()
    }

    /// Every field of `q`, as the editor writes a preset it changed.
    pub fn full(q: &QualitySettings) -> PresetDecl {
        PresetDecl {
            render_scale: Some(q.render_scale),
            dynamic_resolution: Some(match q.dynamic_resolution {
                None => DynResDecl::Off,
                Some(d) => DynResDecl::On {
                    target_us: d.target_us,
                    min_scale: d.min_scale,
                    max_scale: d.max_scale,
                },
            }),
            msaa: Some(q.msaa),
            ao: Some(q.ao.key().into()),
            shadows: Some(q.shadows.key().into()),
            post: Some(q.post.key().into()),
            lod_bias: Some(q.lod_bias),
            particle_scale: Some(q.particle_scale),
            texture_filter: Some(trino_core::quality::filter_key(q.texture_filter).into()),
            anisotropy: Some(q.anisotropy),
            sharpen: Some(q.sharpen),
            frame_limit: Some(frame_limit_key(q.frame_limit)),
        }
    }

    /// Only what `q` changes from `base`: what the editor writes, so a
    /// file stays as short as the project's changes.
    pub fn diff(q: &QualitySettings, base: &QualitySettings) -> PresetDecl {
        let full = PresetDecl::full(q);
        let b = PresetDecl::full(base);
        fn keep<T: Clone + PartialEq>(a: &Option<T>, b: &Option<T>) -> Option<T> {
            if a != b { a.clone() } else { None }
        }
        PresetDecl {
            render_scale: keep(&full.render_scale, &b.render_scale),
            dynamic_resolution: keep(&full.dynamic_resolution, &b.dynamic_resolution),
            msaa: keep(&full.msaa, &b.msaa),
            ao: keep(&full.ao, &b.ao),
            shadows: keep(&full.shadows, &b.shadows),
            post: keep(&full.post, &b.post),
            lod_bias: keep(&full.lod_bias, &b.lod_bias),
            particle_scale: keep(&full.particle_scale, &b.particle_scale),
            texture_filter: keep(&full.texture_filter, &b.texture_filter),
            anisotropy: keep(&full.anisotropy, &b.anisotropy),
            sharpen: keep(&full.sharpen, &b.sharpen),
            frame_limit: keep(&full.frame_limit, &b.frame_limit),
        }
    }

    /// `base` with these changes, or the first value out of its range.
    pub fn apply(&self, base: QualitySettings) -> Result<QualitySettings, String> {
        let mut q = base;
        let range = |what: &str, v: f32, lo: f32, hi: f32| {
            if v.is_finite() && (lo..=hi).contains(&v) {
                Ok(v)
            } else {
                Err(format!("{what} {v} ({lo} to {hi})"))
            }
        };
        if let Some(v) = self.render_scale {
            q.render_scale = range("render_scale", v, 0.5, 2.0)?;
        }
        if let Some(d) = self.dynamic_resolution {
            q.dynamic_resolution = match d {
                DynResDecl::Off => None,
                DynResDecl::On {
                    target_us,
                    min_scale,
                    max_scale,
                } => {
                    let lo = range("dynamic_resolution min_scale", min_scale, 0.25, 2.0)?;
                    let hi = range("dynamic_resolution max_scale", max_scale, 0.25, 2.0)?;
                    if lo > hi {
                        return Err(format!(
                            "dynamic_resolution: min_scale {lo} is above max_scale {hi}"
                        ));
                    }
                    Some(DynRes {
                        target_us,
                        min_scale: lo,
                        max_scale: hi,
                    })
                }
            };
        }
        if let Some(v) = self.msaa {
            if !matches!(v, 1 | 4) {
                return Err(format!("msaa {v} (1 or 4)"));
            }
            q.msaa = v;
        }
        let level = |what: &str, v: &str| {
            AoSetting::parse(v)
                .map(|l| l.level())
                .ok_or_else(|| format!("{what} `{v}` (off, low, medium, high)"))
        };
        if let Some(v) = &self.ao {
            q.ao = AoSetting::ALL[level("ao", v)?];
        }
        if let Some(v) = &self.shadows {
            q.shadows = ShadowSetting::ALL[level("shadows", v)?];
        }
        if let Some(v) = &self.post {
            q.post = PostSetting::ALL[level("post", v)?];
        }
        if let Some(v) = self.lod_bias {
            q.lod_bias = range("lod_bias", v, 0.1, 4.0)?;
        }
        if let Some(v) = self.particle_scale {
            q.particle_scale = range("particle_scale", v, 0.1, 4.0)?;
        }
        if let Some(v) = &self.texture_filter {
            q.texture_filter = trino_core::quality::parse_filter(v)
                .ok_or_else(|| format!("texture_filter `{v}` (default, point, bilinear)"))?;
        }
        if let Some(v) = self.anisotropy {
            if !matches!(v, 1 | 2 | 4 | 8 | 16) {
                return Err(format!("anisotropy {v} (1, 2, 4, 8 or 16)"));
            }
            q.anisotropy = v;
        }
        if let Some(v) = self.sharpen {
            q.sharpen = range("sharpen", v, 0.0, 1.0)?;
        }
        if let Some(v) = &self.frame_limit {
            q.frame_limit = trino_core::quality::parse_frame_limit(v)
                .ok_or_else(|| format!("frame_limit `{v}` (vsync, unlimited or 10 to 1000 fps)"))?;
        }
        Ok(q)
    }
}

/// `vsync` / `unlimited` / the cap.
pub fn frame_limit_key(l: trino_core::FrameLimit) -> String {
    match l {
        trino_core::FrameLimit::VSync => "vsync".into(),
        trino_core::FrameLimit::Fps(n) => n.to_string(),
        trino_core::FrameLimit::Unlimited => "unlimited".into(),
    }
}

impl QualityFile {
    fn presets(&self) -> [&PresetDecl; 4] {
        [&self.low, &self.medium, &self.high, &self.ultra]
    }

    pub fn parse(text: &str) -> Result<QualityFile, String> {
        let f: QualityFile = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(text)
            .map_err(|e| format!("quality parse error: {e}"))?;
        if f.version != QUALITY_VERSION {
            return Err(format!(
                "version {} (expected {QUALITY_VERSION})",
                f.version
            ));
        }
        f.table()?;
        Ok(f)
    }

    /// The presets: the built-in ones with this file's changes.
    pub fn table(&self) -> Result<PresetTable, String> {
        let mut t = PresetTable::BUILT_IN;
        for ((q, d), p) in t
            .presets
            .iter_mut()
            .zip(self.presets())
            .zip(QualityPreset::NAMED)
        {
            *q = d.apply(*q).map_err(|e| format!("{}: {e}", p.key()))?;
        }
        Ok(t)
    }

    /// The file for `table`: each preset as its changes from the built-in
    /// values.
    pub fn from_table(table: &PresetTable) -> QualityFile {
        let built_in = PresetTable::BUILT_IN;
        let d = |i: usize| PresetDecl::diff(&table.presets[i], &built_in.presets[i]);
        QualityFile {
            version: QUALITY_VERSION,
            low: d(0),
            medium: d(1),
            high: d(2),
            ultra: d(3),
        }
    }

    pub fn to_ron(&self) -> String {
        let config = ron::ser::PrettyConfig::default()
            .struct_names(false)
            .extensions(ron::extensions::Extensions::IMPLICIT_SOME);
        let body = ron::ser::to_string_pretty(self, config).expect("serialization cannot fail");
        format!(
            "// Quality presets (issue #55): what each preset changes from the engine's\n\
             // built-in values. `cargo xtask quality show` prints the result.\n{body}\n"
        )
    }
}

/// `trino.toml`'s `[pc.quality]`.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectQuality {
    /// `auto` (the calibration bench) or a preset name.
    pub default: String,
    /// The presets file, relative to the project root.
    pub presets: PathBuf,
}

impl Default for ProjectQuality {
    fn default() -> Self {
        ProjectQuality {
            default: "auto".into(),
            presets: PathBuf::from(FILE),
        }
    }
}

#[derive(Deserialize, Default)]
struct TomlRoot {
    #[serde(default)]
    pc: TomlPc,
}

#[derive(Deserialize, Default)]
struct TomlPc {
    #[serde(default)]
    quality: TomlQuality,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TomlQuality {
    default: Option<String>,
    presets: Option<String>,
}

impl ProjectQuality {
    /// `<root>/trino.toml`'s `[pc.quality]` (the defaults without one).
    pub fn load(root: &Path) -> Result<ProjectQuality, String> {
        let path = root.join("trino.toml");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(ProjectQuality::default());
        };
        ProjectQuality::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str) -> Result<ProjectQuality, String> {
        let t: TomlRoot = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut q = ProjectQuality::default();
        if let Some(d) = t.pc.quality.default {
            let ok = d.eq_ignore_ascii_case("auto")
                || QualityPreset::parse(&d).is_some_and(|p| p != QualityPreset::Custom);
            if !ok {
                return Err(format!(
                    "[pc.quality] default `{d}` (auto, low, medium, high, ultra)"
                ));
            }
            q.default = d.to_ascii_lowercase();
        }
        if let Some(p) = t.pc.quality.presets {
            q.presets = PathBuf::from(p);
        }
        Ok(q)
    }

    /// The presets: the file's when it exists, else the built-in ones.
    pub fn table(&self, root: &Path) -> Result<PresetTable, String> {
        let path = root.join(&self.presets);
        match std::fs::read_to_string(&path) {
            Ok(text) => QualityFile::parse(&text)
                .and_then(|f| f.table())
                .map_err(|e| format!("{}: {e}", path.display())),
            Err(_) => Ok(PresetTable::BUILT_IN),
        }
    }

    /// The `quality/presets` blob for this project.
    pub fn bake(&self, root: &Path) -> Result<Vec<u8>, String> {
        Ok(baked_text(&self.default, &self.table(root)?).into_bytes())
    }
}

/// The blob's text: `default=`, then the table.
pub fn baked_text(default: &str, table: &PresetTable) -> String {
    let mut s = format!("default={default}\n");
    let _ = table.write_text(&mut s);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::FrameLimit;

    #[test]
    fn a_file_changes_only_what_it_names() {
        let f = QualityFile::parse(
            "#![enable(implicit_some)]\n(version: 1, low: (render_scale: 0.6, ao: \"low\"), \
             ultra: (frame_limit: \"unlimited\", dynamic_resolution: On(min_scale: 0.8, max_scale: 1.0)))",
        )
        .unwrap();
        let t = f.table().unwrap();
        assert_eq!(t.get(QualityPreset::Low).render_scale, 0.6);
        assert_eq!(t.get(QualityPreset::Low).ao, AoSetting::Low);
        assert_eq!(t.get(QualityPreset::Low).msaa, QualitySettings::LOW.msaa);
        assert_eq!(t.get(QualityPreset::Medium), QualitySettings::MEDIUM);
        let u = t.get(QualityPreset::Ultra);
        assert_eq!(u.frame_limit, FrameLimit::Unlimited);
        assert_eq!(u.dynamic_resolution.unwrap().min_scale, 0.8);
        // Without the implicit_some header too (the engine enables it).
        let plain = QualityFile::parse("(version: 1, high: (msaa: 1))").unwrap();
        assert_eq!(plain.table().unwrap().get(QualityPreset::High).msaa, 1);
    }

    #[test]
    fn bad_files_say_what_is_wrong() {
        for (text, want) in [
            ("(version: 2)", "version 2"),
            ("(version: 1, low: (msaa: 2))", "low: msaa 2"),
            ("(version: 1, high: (ao: \"max\"))", "high: ao `max`"),
            (
                "(version: 1, medium: (render_scale: 3.0))",
                "medium: render_scale 3",
            ),
            (
                "(version: 1, ultra: (frame_limit: \"fast\"))",
                "frame_limit `fast`",
            ),
            (
                "(version: 1, low: (dynamic_resolution: On(min_scale: 1.0, max_scale: 0.5)))",
                "min_scale 1 is above",
            ),
            ("(version: 1, low: (blur: 1.0))", "blur"),
            ("(version: 1, epic: ())", "epic"),
        ] {
            let e = QualityFile::parse(text).unwrap_err();
            assert!(e.contains(want), "{text}: {e}");
        }
    }

    #[test]
    fn files_round_trip_through_the_table_and_stay_short() {
        let mut t = PresetTable::BUILT_IN;
        assert_eq!(
            QualityFile::from_table(&t)
                .to_ron()
                .matches("render_scale")
                .count(),
            0,
            "the built-in table writes no changes"
        );
        t.presets[0].render_scale = 0.6;
        t.presets[3].frame_limit = FrameLimit::Fps(144);
        t.presets[1].dynamic_resolution = None;
        let text = QualityFile::from_table(&t).to_ron();
        let back = QualityFile::parse(&text).unwrap();
        assert_eq!(back.table().unwrap(), t, "{text}");
        assert!(back.high.is_empty());
        assert_eq!(back.ultra.frame_limit.as_deref(), Some("144"));
        // Every field written in full parses back too.
        let full = QualityFile {
            version: 1,
            low: PresetDecl::full(&QualitySettings::ULTRA),
            ..QualityFile::default()
        };
        let t = QualityFile::parse(&full.to_ron()).unwrap().table().unwrap();
        assert_eq!(
            t.presets[0],
            QualitySettings {
                preset: QualityPreset::Low,
                ..QualitySettings::ULTRA
            }
        );
    }

    #[test]
    fn the_project_section_and_the_baked_blob() {
        assert_eq!(
            ProjectQuality::parse("").unwrap(),
            ProjectQuality::default()
        );
        let q = ProjectQuality::parse(
            "[project]\nname = \"x\"\n[pc]\npackage = \"x\"\n[pc.quality]\ndefault = \"Medium\"\npresets = \"assets/q.ron\"\n",
        )
        .unwrap();
        assert_eq!(q.default, "medium");
        assert_eq!(q.presets, PathBuf::from("assets/q.ron"));
        assert!(ProjectQuality::parse("[pc.quality]\ndefault = \"epic\"\n").is_err());
        assert!(ProjectQuality::parse("[pc.quality]\nlevel = 1\n").is_err());
        let mut t = PresetTable::BUILT_IN;
        t.presets[0].lod_bias = 0.5;
        let text = baked_text("low", &t);
        assert!(text.starts_with("default=low\n[low]\n"), "{text}");
        assert_eq!(PresetTable::parse(&text), t);
    }
}
