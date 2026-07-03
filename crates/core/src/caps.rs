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
    /// Soft budget for Fase 7 (3D).
    pub max_tris_per_frame: u32,
    /// Output color depth in bits.
    pub color_depth_bits: u8,
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
        color_depth_bits: 16,
    };

    /// Nintendo 3DS (top screen): 400x240, 24-bit.
    pub const N3DS: Caps = Caps {
        screen_width: 400,
        screen_height: 240,
        texture_memory_bytes: 6 * 1024 * 1024,
        max_texture_dim: 1024,
        max_sprites_per_frame: 4096,
        max_tris_per_frame: 60_000,
        color_depth_bits: 24,
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
        color_depth_bits: 32,
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
}
