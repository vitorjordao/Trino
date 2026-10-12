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
//! )
//! ```

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
}

impl Default for MaterialDef {
    fn default() -> Self {
        MaterialDef {
            version: MATERIAL_VERSION,
            tint: crate::white(),
            shading: ShadingDef::Gouraud,
            emissive: [0; 3],
            double_sided: false,
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
