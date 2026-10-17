//! Material files: `assets/materials/<name>.material.ron`, referenced as
//! `materials/<name>`. A material is a named, reusable **surface** —
//! everything in it maps onto `trino_core::ModelParams`, which the engine's
//! software T&L evaluates identically on PC, N64 and 3DS. Nothing here can
//! exceed what the N64 can draw: there is no shader to write.
//!
//! ```ron
//! (
//!     version: 1,
//!     tint: (180, 120, 80, 255),
//!     shading: Flat,
//!     emissive: (0, 0, 0),
//!     double_sided: false,
//!     texture: Some("sprites/brick"),
//! )
//! ```
//!
//! `texture` is a sprite (logical path) sampled by meshes that carry UVs —
//! one texture per draw, power-of-two sized; on the N64 it must fit TMEM
//! (4 KB: 32x32 RGBA16, 64x64 CI4, ...).

use serde::{Deserialize, Serialize};
use trino_core::{Color, ModelParams, Shading};

pub const MATERIAL_VERSION: u32 = 1;

/// Mirror of `trino_core::Shading` (serde lives here, not in the core).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ShadingDef {
    /// Smooth per-vertex lighting.
    #[default]
    Gouraud,
    /// Faceted: one light value per triangle.
    Flat,
    /// Ignores light (full bright).
    Unlit,
}

impl ShadingDef {
    pub const ALL: [ShadingDef; 3] = [ShadingDef::Gouraud, ShadingDef::Flat, ShadingDef::Unlit];

    pub fn core(self) -> Shading {
        match self {
            ShadingDef::Gouraud => Shading::Gouraud,
            ShadingDef::Flat => Shading::Flat,
            ShadingDef::Unlit => Shading::Unlit,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ShadingDef::Gouraud => "Gouraud (smooth)",
            ShadingDef::Flat => "Flat (faceted)",
            ShadingDef::Unlit => "Unlit (full bright)",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MaterialDef {
    pub version: u32,
    /// Multiplies the mesh's vertex colors (alpha too).
    #[serde(default = "crate::white")]
    pub tint: [u8; 4],
    #[serde(default)]
    pub shading: ShadingDef,
    /// Added after lighting: glow.
    #[serde(default)]
    pub emissive: [u8; 3],
    #[serde(default)]
    pub double_sided: bool,
    /// Texture (sprite logical path) for meshes with UVs; overrides the
    /// model's baked default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture: Option<String>,
    /// Shininess 0..=255 for specular highlights (PC's per-pixel lighting
    /// only; consoles ignore it).
    #[serde(default, skip_serializing_if = "crate::is_zero_u8")]
    pub gloss: u8,
    /// Texture sampling (`Default` = the target's own).
    #[serde(default, skip_serializing_if = "FilterDef::is_default")]
    pub filter: FilterDef,
    /// A detail texture multiplied over `texture` (see
    /// `trino_core::DetailTexture` for the N64's TMEM budget).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<DetailDef>,
    /// A water surface: swell, ripples, the two layers flowing (see
    /// `trino_core::water`, ADR 0015). Translucency comes from the mesh's
    /// vertex alpha and `tint`'s alpha.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub water: Option<WaterDef>,
}

/// Mirror of `trino_core::Water` (serde lives here, not in the core).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct WaterDef {
    /// Texture scroll, repeats per second (u, v).
    #[serde(default = "WaterDef::default_flow")]
    pub flow_speed: (f32, f32),
    /// The detail layer's scroll, its repeats per second.
    #[serde(default = "WaterDef::default_detail_flow")]
    pub detail_flow: (f32, f32),
    /// Swell height, world units.
    #[serde(default = "WaterDef::default_amplitude")]
    pub wave_amplitude: f32,
    /// Swell crests per world unit.
    #[serde(default = "WaterDef::default_frequency")]
    pub wave_frequency: f32,
    /// How far ripples push the surface (0 = ignore them).
    #[serde(default = "WaterDef::default_response")]
    pub ripple_responsiveness: f32,
}

impl WaterDef {
    fn default_flow() -> (f32, f32) {
        let f = trino_core::Water::DEFAULT.flow_speed;
        (f.x, f.y)
    }
    fn default_detail_flow() -> (f32, f32) {
        let f = trino_core::Water::DEFAULT.detail_flow;
        (f.x, f.y)
    }
    fn default_amplitude() -> f32 {
        trino_core::Water::DEFAULT.wave_amplitude
    }
    fn default_frequency() -> f32 {
        trino_core::Water::DEFAULT.wave_frequency
    }
    fn default_response() -> f32 {
        trino_core::Water::DEFAULT.ripple_responsiveness
    }

    /// The editor's slider ranges (and `scene check`'s): (min, max).
    pub const FLOW_RANGE: (f32, f32) = (-2.0, 2.0);
    pub const AMPLITUDE_RANGE: (f32, f32) = (0.0, 0.5);
    pub const FREQUENCY_RANGE: (f32, f32) = (0.05, 4.0);
    pub const RESPONSE_RANGE: (f32, f32) = (0.0, 3.0);

    pub fn core(&self) -> trino_core::Water {
        trino_core::Water {
            flow_speed: trino_core::Vec2::new(self.flow_speed.0, self.flow_speed.1),
            detail_flow: trino_core::Vec2::new(self.detail_flow.0, self.detail_flow.1),
            wave_amplitude: self.wave_amplitude,
            wave_frequency: self.wave_frequency,
            ripple_responsiveness: self.ripple_responsiveness,
        }
    }

    /// What's out of range, if anything (`scene check`).
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut check = |name: &str, v: f32, (lo, hi): (f32, f32)| {
            if !v.is_finite() || v < lo || v > hi {
                out.push(format!("water {name} {v} is outside {lo}..={hi}"));
            }
        };
        check("flow_speed.u", self.flow_speed.0, Self::FLOW_RANGE);
        check("flow_speed.v", self.flow_speed.1, Self::FLOW_RANGE);
        check("detail_flow.u", self.detail_flow.0, Self::FLOW_RANGE);
        check("detail_flow.v", self.detail_flow.1, Self::FLOW_RANGE);
        check("wave_amplitude", self.wave_amplitude, Self::AMPLITUDE_RANGE);
        check("wave_frequency", self.wave_frequency, Self::FREQUENCY_RANGE);
        check(
            "ripple_responsiveness",
            self.ripple_responsiveness,
            Self::RESPONSE_RANGE,
        );
        out
    }
}

impl Default for WaterDef {
    fn default() -> Self {
        WaterDef {
            flow_speed: Self::default_flow(),
            detail_flow: Self::default_detail_flow(),
            wave_amplitude: Self::default_amplitude(),
            wave_frequency: Self::default_frequency(),
            ripple_responsiveness: Self::default_response(),
        }
    }
}

/// Mirror of `trino_core::TexFilter`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FilterDef {
    #[default]
    Default,
    Point,
    Bilinear,
}

impl FilterDef {
    pub const ALL: [FilterDef; 3] = [FilterDef::Default, FilterDef::Point, FilterDef::Bilinear];

    pub fn core(self) -> trino_core::TexFilter {
        match self {
            FilterDef::Default => trino_core::TexFilter::Default,
            FilterDef::Point => trino_core::TexFilter::Point,
            FilterDef::Bilinear => trino_core::TexFilter::Bilinear,
        }
    }

    fn is_default(&self) -> bool {
        *self == FilterDef::Default
    }

    pub fn label(self) -> &'static str {
        match self {
            FilterDef::Default => "Default",
            FilterDef::Point => "Point",
            FilterDef::Bilinear => "Bilinear",
        }
    }
}

/// A detail texture: a sprite (logical path) tiling `2^scale_log2` times
/// per repeat of the base texture.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DetailDef {
    pub texture: String,
    #[serde(default)]
    pub scale_log2: u8,
}

impl Default for MaterialDef {
    fn default() -> Self {
        MaterialDef {
            version: MATERIAL_VERSION,
            tint: crate::white(),
            shading: ShadingDef::Gouraud,
            emissive: [0; 3],
            double_sided: false,
            texture: None,
            gloss: 0,
            filter: FilterDef::Default,
            detail: None,
            water: None,
        }
    }
}

impl MaterialDef {
    pub fn from_ron(text: &str) -> Result<MaterialDef, String> {
        let m: MaterialDef =
            ron::from_str(text).map_err(|e| format!("material parse error: {e}"))?;
        if m.version > MATERIAL_VERSION {
            return Err(format!(
                "material version {} is newer than this engine understands ({MATERIAL_VERSION}) \
                 — update Trino",
                m.version
            ));
        }
        Ok(m)
    }

    pub fn to_ron(&self) -> String {
        crate::to_ron_pretty(self)
    }

    pub fn load(path: &std::path::Path) -> Result<MaterialDef, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::from_ron(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        crate::write_text(path, &self.to_ron())
    }

    /// The engine-side surface, with an optional per-entity tint on top.
    pub fn params(&self, extra_tint: Option<[u8; 4]>) -> ModelParams {
        let mul = |a: u8, b: u8| ((a as u16 * b as u16 + 127) / 255) as u8;
        let t = match extra_tint {
            Some(e) => std::array::from_fn(|i| mul(self.tint[i], e[i])),
            None => self.tint,
        };
        ModelParams {
            tint: Color::rgba(t[0], t[1], t[2], t[3]),
            emissive: Color::rgb(self.emissive[0], self.emissive[1], self.emissive[2]),
            shading: self.shading.core(),
            double_sided: self.double_sided,
            anim: None,
            texture: self.texture.as_deref().map(trino_core::SpriteId::from_path),
            gloss: self.gloss,
            filter: self.filter.core(),
            detail: self.detail.as_ref().map(|d| {
                trino_core::DetailTexture::new(
                    trino_core::SpriteId::from_path(&d.texture),
                    d.scale_log2,
                )
            }),
            water: self.water.map(|w| w.core()),
            ..ModelParams::DEFAULT
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let m = MaterialDef {
            tint: [10, 20, 30, 255],
            shading: ShadingDef::Flat,
            emissive: [1, 2, 3],
            double_sided: true,
            ..MaterialDef::default()
        };
        assert_eq!(MaterialDef::from_ron(&m.to_ron()).unwrap(), m);
        let min = MaterialDef::from_ron("(version: 1)").unwrap();
        assert_eq!(min, MaterialDef::default());
        assert!(
            MaterialDef::from_ron("(version: 9)")
                .unwrap_err()
                .contains("update")
        );
    }

    #[test]
    fn gloss_round_trips_and_defaults_to_matte() {
        let m = MaterialDef {
            gloss: 120,
            ..MaterialDef::default()
        };
        let text = m.to_ron();
        assert!(text.contains("gloss: 120"), "{text}");
        assert_eq!(MaterialDef::from_ron(&text).unwrap(), m);
        assert_eq!(m.params(None).gloss, 120);
        // Matte materials don't write the field (files stay as they were).
        assert!(!MaterialDef::default().to_ron().contains("gloss"));
        assert_eq!(MaterialDef::default().params(None).gloss, 0);
    }

    #[test]
    fn filter_and_detail_round_trip_and_map_onto_the_core() {
        let m = MaterialDef {
            texture: Some("sprites/bricks".into()),
            filter: FilterDef::Point,
            detail: Some(DetailDef {
                texture: "sprites/grain".into(),
                scale_log2: 9,
            }),
            ..MaterialDef::default()
        };
        assert_eq!(MaterialDef::from_ron(&m.to_ron()).unwrap(), m);
        let p = m.params(None);
        assert_eq!(p.filter, trino_core::TexFilter::Point);
        let d = p.detail.unwrap();
        assert_eq!(d.sprite, trino_core::SpriteId::from_path("sprites/grain"));
        assert_eq!(
            d.scale_log2,
            trino_core::DetailTexture::MAX_SCALE_LOG2,
            "clamped"
        );
        let plain = MaterialDef::default().to_ron();
        assert!(
            !plain.contains("filter") && !plain.contains("detail"),
            "{plain}"
        );
    }

    #[test]
    fn water_round_trips_with_defaults_and_maps_onto_the_core() {
        let text = "(version: 1, tint: (255, 255, 255, 200), water: Some((wave_amplitude: 0.1)))";
        let m = MaterialDef::from_ron(text).unwrap();
        let w = m.water.unwrap();
        assert_eq!(w.wave_amplitude, 0.1);
        assert_eq!(w.flow_speed, WaterDef::default().flow_speed);
        assert_eq!(MaterialDef::from_ron(&m.to_ron()).unwrap(), m);
        let p = m.params(None).water.unwrap();
        assert_eq!(p.wave_amplitude, 0.1);
        assert_eq!(
            p.ripple_responsiveness,
            trino_core::Water::DEFAULT.ripple_responsiveness
        );
        assert!(w.problems().is_empty());
        let bad = WaterDef {
            wave_amplitude: 3.0,
            ..w
        };
        assert_eq!(bad.problems().len(), 1, "{:?}", bad.problems());
        assert!(!MaterialDef::default().to_ron().contains("water"));
    }

    #[test]
    fn params_map_onto_the_core_surface() {
        let m = MaterialDef {
            tint: [255, 128, 0, 255],
            shading: ShadingDef::Unlit,
            emissive: [0, 0, 50],
            double_sided: true,
            ..MaterialDef::default()
        };
        let p = m.params(Some([128, 255, 255, 255]));
        assert_eq!(p.tint, Color::rgba(128, 128, 0, 255));
        assert_eq!(p.shading, Shading::Unlit);
        assert_eq!(p.emissive, Color::rgb(0, 0, 50));
        assert!(p.double_sided);
        assert_eq!(MaterialDef::default().params(None), ModelParams::DEFAULT);
    }
}
