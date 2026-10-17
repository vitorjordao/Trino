//! Ambient occlusion: darker creases, corners and contact areas, where
//! less of the sky's ambient light reaches.
//!
//! Each target gets it the way its hardware affords ([`AoTier`], from
//! [`crate::Caps::ao_tier`], ADR 0014):
//!
//! - [`AoTier::ScreenSpace`] (PC native): a screen-space pass (SSAO) over
//!   the frame's depth buffer, after the 3D and before the 2D drawn on top
//!   of it, with the settings a game gives
//!   [`crate::Renderer::set_ambient_occlusion`].
//! - [`AoTier::Baked`] (N64, 3DS and the PC's console profiles): no runtime
//!   pass. The asset pipeline bakes each model's occlusion into its vertex
//!   colors (`ao = { ... }` on a `[models.*]` entry), which every target
//!   already draws: zero cost at run time, the same picture everywhere.
//!
//! Baked AO is in the model, so it shows on the PC too. A game that also
//! enables screen-space AO gets the contact shadows between models on top.

/// How a target darkens occluded surfaces ([`crate::Caps::ao_tier`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AoTier {
    /// A screen-space pass over the depth buffer (PC native).
    ScreenSpace,
    /// Only what the asset bake wrote into the vertex colors (consoles
    /// and the PC's console profiles).
    Baked,
}

/// Samples per pixel of the screen-space pass.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum AoQuality {
    /// 8 samples.
    Low,
    /// 16 samples.
    #[default]
    Medium,
    /// 32 samples.
    High,
}

impl AoQuality {
    pub const fn samples(self) -> u32 {
        match self {
            AoQuality::Low => 8,
            AoQuality::Medium => 16,
            AoQuality::High => 32,
        }
    }
}

/// Screen-space ambient occlusion settings
/// ([`crate::Renderer::set_ambient_occlusion`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AmbientOcclusion {
    /// How far around a point occluders count, world units (meters).
    pub radius: f32,
    /// Strength, 0 (none) to 2 (twice the measured occlusion).
    pub intensity: f32,
    /// Depth an occluder must stand in front of a sample by, world units:
    /// keeps flat and gently curved surfaces from shadowing themselves.
    pub bias: f32,
    pub quality: AoQuality,
}

impl AmbientOcclusion {
    pub const DEFAULT: AmbientOcclusion = AmbientOcclusion {
        radius: 0.5,
        intensity: 1.0,
        bias: 0.025,
        quality: AoQuality::Medium,
    };

    /// The settings with every field in its valid range: a positive
    /// radius, intensity 0..=2, a bias of 0 or more. NaN takes the
    /// default.
    pub fn clamped(&self) -> AmbientOcclusion {
        let d = AmbientOcclusion::DEFAULT;
        let fix = |v: f32, lo: f32, hi: f32, def: f32| {
            if v.is_nan() { def } else { v.clamp(lo, hi) }
        };
        AmbientOcclusion {
            radius: fix(self.radius, 0.01, 100.0, d.radius),
            intensity: fix(self.intensity, 0.0, 2.0, d.intensity),
            bias: fix(self.bias, 0.0, 10.0, d.bias),
            quality: self.quality,
        }
    }
}

impl Default for AmbientOcclusion {
    fn default() -> Self {
        AmbientOcclusion::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_presets_have_8_16_32_samples() {
        assert_eq!(AoQuality::Low.samples(), 8);
        assert_eq!(AoQuality::Medium.samples(), 16);
        assert_eq!(AoQuality::High.samples(), 32);
        assert_eq!(AoQuality::default(), AoQuality::Medium);
    }

    #[test]
    fn clamped_keeps_valid_settings_and_fixes_the_rest() {
        let ok = AmbientOcclusion::DEFAULT;
        assert_eq!(ok.clamped(), ok);
        let wild = AmbientOcclusion {
            radius: -1.0,
            intensity: 9.0,
            bias: f32::NAN,
            quality: AoQuality::High,
        }
        .clamped();
        assert_eq!(wild.radius, 0.01);
        assert_eq!(wild.intensity, 2.0);
        assert_eq!(wild.bias, AmbientOcclusion::DEFAULT.bias);
        assert_eq!(wild.quality, AoQuality::High);
    }
}
