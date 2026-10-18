//! Per-console capability tables.
//!
//! The N64 is the design ceiling for the whole engine. `Caps` makes each
//! target's budget explicit so the PC backend can run in "strict mode" and
//! reject content that would not fit on a console — at development time,
//! with an actionable error, instead of at port time.

/// Hardware budget of a target platform.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Caps {
    /// Internal render resolution (before any upscale on PC).
    pub screen_width: u16,
    pub screen_height: u16,
    /// Texture cache budget in bytes (N64 TMEM = 4096).
    pub texture_memory_bytes: u32,
    /// Largest texture dimension the hardware can address.
    pub max_texture_dim: u16,
    /// Soft budget used by strict mode to flag overdraw-heavy scenes.
    pub max_sprites_per_frame: u32,
    /// Rasterizer-side triangle cap (RDP / GPU), checked by strict mode. It
    /// is **not** the CPU T&L budget: on the N64 the time budget below binds
    /// far earlier.
    pub max_tris_per_frame: u32,
    /// Hard ceiling, in console microseconds, for one frame's 3D (T&L, sort
    /// and submit) as the calibrated cost model estimates it
    /// ([`crate::perf::N64_COST`]); strict mode panics above it. The N64's is
    /// 50 ms (20 fps). Tools warn earlier, at a 30 fps frame
    /// ([`crate::perf::FRAME_30FPS_US`]). `u32::MAX` = no time model for the
    /// target.
    pub max_3d_us_per_frame: u32,
    /// Output color depth in bits.
    pub color_depth_bits: u8,
    /// Save-data budget in bytes (`Platform::save_capacity`). The N64 value
    /// is a 16 Kbit cartridge EEPROM; design saves to fit it.
    pub save_bytes: u32,
    /// How local lights are evaluated ([`crate::light`], ADR 0010).
    pub light_tier: crate::light::LightTier,
    /// Most local lights a frame uses (the first ones given to
    /// `set_local_lights`). On the N64 every draw a light reaches leaves
    /// the RSP for the CPU T&L, so its cap is small.
    pub max_local_lights: u8,
    /// How ambient occlusion reaches the picture ([`crate::ao`], ADR
    /// 0014): a screen-space pass on the PC, baked vertex colors on the
    /// consoles.
    pub ao_tier: crate::ao::AoTier,
    /// How water surfaces are drawn ([`crate::water`], ADR 0015).
    pub water_tier: crate::water::WaterTier,
    /// Most ripples a frame's water evaluates (the first ones given to
    /// `set_water`): the 3DS vertex program holds 4 in its uniforms, and
    /// every target keeps the same cap so the surface moves alike. Strict
    /// mode panics on more.
    pub max_ripples: u8,
}

/// Why a piece of content does not fit in a target's [`Caps`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CapsError {
    /// Texture dimensions exceed `max_texture_dim`.
    TextureTooLarge {
        width: u16,
        height: u16,
        max_dim: u16,
    },
    /// Texture byte size exceeds `texture_memory_bytes`.
    TextureExceedsMemory { bytes: u32, budget: u32 },
}

impl Caps {
    /// Nintendo 64: 320x240, 4 KB TMEM, 16-bit framebuffer.
    pub const N64: Caps = Caps {
        screen_width: 320,
        screen_height: 240,
        texture_memory_bytes: 4096,
        max_texture_dim: 256,
        max_sprites_per_frame: 512,
        max_tris_per_frame: 4000,
        max_3d_us_per_frame: 50_000,
        color_depth_bits: 16,
        save_bytes: 2048,
        light_tier: crate::light::LightTier::PerVertex,
        max_local_lights: 2,
        ao_tier: crate::ao::AoTier::Baked,
        water_tier: crate::water::WaterTier::PerVertex,
        max_ripples: crate::water::MAX_RIPPLES as u8,
    };

    /// Nintendo 3DS (top screen): 400x240, 24-bit.
    pub const N3DS: Caps = Caps {
        screen_width: 400,
        screen_height: 240,
        texture_memory_bytes: 6 * 1024 * 1024,
        max_texture_dim: 1024,
        max_sprites_per_frame: 4096,
        max_tris_per_frame: 60_000,
        max_3d_us_per_frame: u32::MAX,
        color_depth_bits: 24,
        save_bytes: 64 * 1024,
        light_tier: crate::light::LightTier::PerVertex,
        max_local_lights: 4,
        ao_tier: crate::ao::AoTier::Baked,
        water_tier: crate::water::WaterTier::VertexProgram,
        max_ripples: crate::water::MAX_RIPPLES as u8,
    };

    /// PC: generous defaults; the internal resolution follows the active
    /// console-simulation profile, not this table.
    pub const PC: Caps = Caps {
        screen_width: 1920,
        screen_height: 1080,
        texture_memory_bytes: u32::MAX,
        max_texture_dim: 4096,
        max_sprites_per_frame: u32::MAX,
        max_tris_per_frame: u32::MAX,
        max_3d_us_per_frame: u32::MAX,
        color_depth_bits: 32,
        save_bytes: 1024 * 1024,
        light_tier: crate::light::LightTier::PerPixel,
        max_local_lights: 8,
        ao_tier: crate::ao::AoTier::ScreenSpace,
        water_tier: crate::water::WaterTier::PerPixel,
        max_ripples: crate::water::MAX_RIPPLES as u8,
    };

    /// A web browser (WebGPU, or WebGL2 where there is none; issue #47):
    /// the PC's tiers with a budget a phone's browser holds. 4096 is the
    /// texture size every WebGL2 device samples (the spec's floor is 2048,
    /// no shipping browser stops there), 1 GiB the texture memory of a
    /// mid-range phone's tab, 1 MiB of save the `localStorage` quota keeps
    /// with room (it stores base64). The triangle cap is strict mode's
    /// fillrate warning for mobile GPUs, not a desktop limit.
    pub const WEB: Caps = Caps {
        screen_width: 1920,
        screen_height: 1080,
        texture_memory_bytes: 1 << 30,
        max_texture_dim: 4096,
        max_sprites_per_frame: u32::MAX,
        max_tris_per_frame: 1_000_000,
        max_3d_us_per_frame: u32::MAX,
        color_depth_bits: 32,
        save_bytes: 1024 * 1024,
        light_tier: crate::light::LightTier::PerPixel,
        max_local_lights: 8,
        ao_tier: crate::ao::AoTier::ScreenSpace,
        water_tier: crate::water::WaterTier::PerPixel,
        max_ripples: crate::water::MAX_RIPPLES as u8,
    };

    /// Validate a texture of `width`x`height` at `bytes_per_pixel` against
    /// this budget.
    pub fn validate_texture(
        &self,
        width: u16,
        height: u16,
        bytes_per_pixel: u32,
    ) -> Result<(), CapsError> {
        if width > self.max_texture_dim || height > self.max_texture_dim {
            return Err(CapsError::TextureTooLarge {
                width,
                height,
                max_dim: self.max_texture_dim,
            });
        }
        let bytes = width as u32 * height as u32 * bytes_per_pixel;
        if bytes > self.texture_memory_bytes {
            return Err(CapsError::TextureExceedsMemory {
                bytes,
                budget: self.texture_memory_bytes,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn n64_rejects_texture_over_tmem() {
        // 64x64 @ 2 bytes/pixel = 8192 bytes > 4096 TMEM.
        let err = Caps::N64.validate_texture(64, 64, 2).unwrap_err();
        assert_eq!(
            err,
            CapsError::TextureExceedsMemory {
                bytes: 8192,
                budget: 4096
            }
        );
    }

    #[test]
    fn n64_accepts_texture_within_tmem() {
        // 32x32 @ 2 bytes/pixel = 2048 bytes.
        assert!(Caps::N64.validate_texture(32, 32, 2).is_ok());
        // 64x64 @ 0.5 byte/pixel (CI4) would be 2048 bytes, but the API takes
        // whole bytes; CI4 is validated by the asset pipeline in Fase 2.
    }

    #[test]
    fn n64_rejects_oversized_dimension() {
        let err = Caps::N64.validate_texture(512, 16, 2).unwrap_err();
        assert!(matches!(
            err,
            CapsError::TextureTooLarge { max_dim: 256, .. }
        ));
    }

    #[test]
    fn pc_accepts_large_textures() {
        assert!(Caps::PC.validate_texture(4096, 4096, 4).is_ok());
    }

    #[test]
    fn web_has_the_pc_tiers_and_a_browser_budget() {
        let web = Caps::WEB;
        assert_eq!(web.light_tier, Caps::PC.light_tier);
        assert_eq!(web.ao_tier, Caps::PC.ao_tier);
        assert_eq!(web.water_tier, Caps::PC.water_tier);
        assert!(web.validate_texture(4096, 4096, 4).is_ok());
        assert!(matches!(
            web.validate_texture(8192, 16, 4),
            Err(CapsError::TextureTooLarge { max_dim: 4096, .. })
        ));
        assert!(web.save_bytes >= Caps::N3DS.save_bytes);
    }
}
