//! Per-console capability tables.
//!
//! The N64 is the design ceiling for the whole engine. `Caps` makes each
//! target's budget explicit so the PC backend can run in "strict mode" and
//! reject content that would not fit on a console — at development time,
//! with an actionable error, instead of at port time.
//!
//! Two consoles come in two hardware tiers ([`HwTier`], ADR 0020): an N64
//! with the Expansion Pak (8 MB of RDRAM instead of 4) and the New Nintendo
//! 3DS family (804 MHz with an L2 cache instead of 268 MHz, a second
//! application core). One ROM or app runs on both; the backend detects the
//! tier at boot and its renderer reports [`Caps::N64_EXPANDED`] or
//! [`Caps::N3DS_NEW`]. The base tables stay the design ceiling.

/// Which variant of its console a target is ([`Caps::hw_tier`],
/// [`crate::Platform::hw_tier`]). Detected at boot on the consoles, never
/// built in: content that wants more on the bigger variant says so as data
/// (the `n64_expanded` / `n3ds_new` keys of scenes, particle effects, LOD
/// starts and the manifest), and code can branch on it at update time.
///
/// | Target | `Base` | `Expanded` |
/// |---|---|---|
/// | N64 | 4 MB RDRAM | Expansion Pak: 8 MB |
/// | 3DS | 3DS / 2DS: 268 MHz, one application core | New 3DS / New 2DS XL: 804 MHz + 2 MB L2, a second application core |
/// | PC, Web | always (their scaling is quality presets, issue #55) | — |
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord)]
pub enum HwTier {
    #[default]
    Base,
    Expanded,
}

impl HwTier {
    pub const ALL: [HwTier; 2] = [HwTier::Base, HwTier::Expanded];

    /// `base` / `expanded` (boot log, `--json` output).
    pub const fn key(self) -> &'static str {
        match self {
            HwTier::Base => "base",
            HwTier::Expanded => "expanded",
        }
    }

    pub const fn is_expanded(self) -> bool {
        matches!(self, HwTier::Expanded)
    }
}

/// Which kind of target a [`Caps`] table describes (the PC's console
/// profiles report the console's): what UI screens' `visible_on` and
/// per-target tools key on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord)]
pub enum Family {
    #[default]
    Pc,
    N64,
    N3ds,
    Web,
}

impl Family {
    pub const ALL: [Family; 4] = [Family::Pc, Family::N64, Family::N3ds, Family::Web];

    /// `pc` / `n64` / `3ds` / `web` (CLI flags, `--json`).
    pub const fn key(self) -> &'static str {
        match self {
            Family::Pc => "pc",
            Family::N64 => "n64",
            Family::N3ds => "3ds",
            Family::Web => "web",
        }
    }

    /// The base console's (or host's) table.
    pub const fn caps(self) -> Caps {
        match self {
            Family::Pc => Caps::PC,
            Family::N64 => Caps::N64,
            Family::N3ds => Caps::N3DS,
            Family::Web => Caps::WEB,
        }
    }
}

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
    /// Main memory a game can plan for, in bytes: the N64's RDRAM (4 MiB,
    /// 8 MiB with the Expansion Pak; code, framebuffers and the heap all
    /// come out of it), the 3DS's application memory region (64 MiB on
    /// every model for a `.3dsx`: the New 3DS's bigger regions need a title
    /// that asks for them). `u32::MAX` = no budget (PC, Web).
    pub ram_bytes: u32,
    /// CPU cores the game may use (the 3DS: application cores; the New
    /// 3DS adds one, `platform-3ds`'s worker runs there). `0` = the host's,
    /// not a budget (PC, Web).
    pub cpu_cores: u8,
    /// Main CPU clock in MHz (`0` = the host's).
    pub cpu_mhz: u16,
    /// Which variant of the console this table describes.
    pub hw_tier: HwTier,
    /// Advisory "more when the hardware allows" knobs, in percent of the
    /// base tier's: how many characters (`crowd`) and particles a game may
    /// afford ([`crowd_scale`](Self::crowd_scale),
    /// [`particle_scale`](Self::particle_scale)). Only CPU or GPU headroom
    /// raises them: the Expansion Pak adds memory, not speed, so the N64's
    /// stay at 100.
    pub crowd_percent: u16,
    pub particle_percent: u16,
    /// Main-thread time a frame may spend loading asset groups
    /// ([`crate::Platform::load_group`], issue #56), in µs: enough to keep a
    /// loading screen animating at the console's frame rate. On the 3DS the
    /// worker core's decoding is not counted, only what the main thread does
    /// (reading, uploading) or a job that ran inline.
    pub load_budget_us: u32,
    /// How shadows are drawn ([`crate::shadow`], ADR 0025): blobs on the
    /// N64, planar projections on the 3DS, a shadow map on the PC and the
    /// Web.
    pub shadow_tier: crate::shadow::ShadowTier,
    /// A stereoscopic screen ([`crate::stereo`], issue #63): the 3DS top
    /// screen draws one picture per eye while the 3D slider is up. False on
    /// the N64, the PC and the Web (the PC's 3DS profile previews it), and
    /// on an old 2DS or a New 2DS XL (the 3DS backend clears it at boot).
    pub stereo: bool,
    /// How the finished 3D is post-processed ([`crate::post`], issue #64):
    /// the whole stack (HDR, tonemap, bloom, LUT, vignette, grain) on the PC
    /// and the Web, grading baked into colors and glow billboards on the
    /// consoles.
    pub post_tier: crate::post::PostTier,
    /// [`crate::ShadowCast::Mesh`] casters a frame projects below the
    /// Planar tier (the N64's: each is a CPU T&L draw); 255 = every one.
    pub max_planar_shadows: u8,
    /// The depth buffer's precision as the engine's z (`1 - NEAR/z`) sees
    /// it: fixed-point bits, and with `depth_mantissa` > 0 a compressed
    /// format whose step also shrinks toward the far plane (the RDP's
    /// 18-bit z: 11 mantissa bits after the leading ones). See
    /// [`depth_step`](Self::depth_step).
    pub depth_bits: u8,
    pub depth_mantissa: u8,
    /// The kind of target.
    pub family: Family,
    /// Pixels a UI keeps clear at the left/right and top/bottom of the main
    /// screen ([`crate::Renderer::safe_area`]): the N64's TV overscan
    /// (action-safe 5%). Zero on screens that show every pixel; the Web's
    /// come from the page (a phone's notch) at run time.
    pub safe_area: (u8, u8),
    /// Most UI nodes one screen lays out ([`crate::ui`]); strict mode fails
    /// a screen past it.
    pub max_ui_nodes: u16,
    /// Most 2D draws (fills, sprites, glyphs) one UI screen issues per
    /// frame; strict mode fails a screen past it.
    pub max_ui_draws: u16,
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
        ram_bytes: 4 * 1024 * 1024,
        cpu_cores: 1,
        cpu_mhz: 93,
        hw_tier: HwTier::Base,
        crowd_percent: 100,
        particle_percent: 100,
        load_budget_us: 4_000,
        shadow_tier: crate::shadow::ShadowTier::Blob,
        stereo: false,
        post_tier: crate::post::PostTier::Baked,
        max_planar_shadows: 2,
        // The RDP's z: 18 bits, stored compressed (3-bit exponent, 11-bit
        // mantissa).
        depth_bits: 18,
        depth_mantissa: 11,
        family: Family::N64,
        safe_area: (16, 12),
        // 128 (issue #95): a 41-row menu list lays out in ~TBD ms and draws
        // in ~TBD ms on the console (`TRINO_UI_COST`, the ares self-test).
        max_ui_nodes: 128,
        max_ui_draws: 384,
    };

    /// An N64 with the Expansion Pak: [`Caps::N64`] with 8 MiB of RDRAM.
    /// The CPU, the RSP, the RDP and TMEM are the same, so every frame
    /// budget and the scaling knobs are too; what grows is what fits in
    /// memory (more resident models, textures, sounds, bigger levels).
    /// libdragon's heap takes the extra 4 MiB by itself.
    pub const N64_EXPANDED: Caps = Caps {
        ram_bytes: 8 * 1024 * 1024,
        hw_tier: HwTier::Expanded,
        ..Caps::N64
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
        ram_bytes: 64 * 1024 * 1024,
        cpu_cores: 1,
        cpu_mhz: 268,
        hw_tier: HwTier::Base,
        crowd_percent: 100,
        particle_percent: 100,
        load_budget_us: 6_000,
        shadow_tier: crate::shadow::ShadowTier::Planar,
        stereo: true,
        post_tier: crate::post::PostTier::Baked,
        max_planar_shadows: 255,
        // DEPTH24_STENCIL8, the 3D in its band (35%) of citro2d's range.
        depth_bits: 22,
        depth_mantissa: 0,
        family: Family::N3ds,
        safe_area: (0, 0),
        max_ui_nodes: 128,
        max_ui_draws: 2048,
    };

    /// A New Nintendo 3DS (or New 2DS XL) with the speedup on: 804 MHz, the
    /// 2 MB L2 cache, a second application core. The GPU and the screens
    /// are the 3DS's. The CPU-bound 3D path (draws off `gpu_check`) and the
    /// game's own update run up to 3x faster, hence twice the crowds and
    /// particles. The memory budget stays the `.3dsx`'s 64 MiB.
    pub const N3DS_NEW: Caps = Caps {
        cpu_cores: 2,
        cpu_mhz: 804,
        hw_tier: HwTier::Expanded,
        crowd_percent: 200,
        particle_percent: 200,
        ..Caps::N3DS
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
        ram_bytes: u32::MAX,
        cpu_cores: 0,
        cpu_mhz: 0,
        hw_tier: HwTier::Base,
        crowd_percent: 100,
        particle_percent: 100,
        load_budget_us: 8_000,
        shadow_tier: crate::shadow::ShadowTier::Map,
        stereo: false,
        post_tier: crate::post::PostTier::Full,
        max_planar_shadows: 255,
        // Depth32Float: z near 1 keeps 24 bits; two for the rasterizer's
        // interpolation.
        depth_bits: 22,
        depth_mantissa: 0,
        family: Family::Pc,
        safe_area: (0, 0),
        max_ui_nodes: 512,
        max_ui_draws: 16_384,
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
        ram_bytes: u32::MAX,
        cpu_cores: 0,
        cpu_mhz: 0,
        hw_tier: HwTier::Base,
        crowd_percent: 100,
        particle_percent: 100,
        load_budget_us: 8_000,
        shadow_tier: crate::shadow::ShadowTier::Map,
        stereo: false,
        post_tier: crate::post::PostTier::Full,
        max_planar_shadows: 255,
        // Depth32Float: z near 1 keeps 24 bits; two for the rasterizer's
        // interpolation.
        depth_bits: 22,
        depth_mantissa: 0,
        family: Family::Web,
        safe_area: (0, 0),
        max_ui_nodes: 512,
        max_ui_draws: 16_384,
    };

    /// The same console's table for `tier`: [`Caps::N64`] ↔
    /// [`Caps::N64_EXPANDED`], [`Caps::N3DS`] ↔ [`Caps::N3DS_NEW`]. Targets
    /// with one tier (PC, Web) return themselves. Fields a host overrode
    /// (the PC's internal resolution) are kept.
    pub const fn with_tier(self, tier: HwTier) -> Caps {
        // The consoles are told apart by their CPU clock (93 MHz; 268 or
        // 804 MHz); the PC and the Web leave it 0.
        let (base, expanded) = match self.cpu_mhz {
            93 => (Caps::N64, Caps::N64_EXPANDED),
            268 | 804 => (Caps::N3DS, Caps::N3DS_NEW),
            _ => return self,
        };
        let table = match tier {
            HwTier::Base => base,
            HwTier::Expanded => expanded,
        };
        Caps {
            ram_bytes: table.ram_bytes,
            cpu_cores: table.cpu_cores,
            cpu_mhz: table.cpu_mhz,
            hw_tier: table.hw_tier,
            crowd_percent: table.crowd_percent,
            particle_percent: table.particle_percent,
            ..self
        }
    }

    /// One step of the depth buffer at the engine's `z` (`1 - NEAR/z`,
    /// 0..1): `2^-depth_bits`, or for a compressed format the coarser of
    /// that and the mantissa's step (the RDP's: `(1 - z) / 2^11`, an upper
    /// bound within each exponent). What a blob or planar shadow is moved
    /// toward the camera by, a few times ([`crate::shadow::shadow_z`]).
    pub fn depth_step(&self, z: f32) -> f32 {
        let fixed = 1.0 / (1u32 << self.depth_bits) as f32;
        if self.depth_mantissa == 0 {
            return fixed;
        }
        let mantissa = (1.0 - z) / (1u32 << self.depth_mantissa) as f32;
        fixed.max(mantissa)
    }

    /// The base tier's table of the same console ([`with_tier`](Self::with_tier)).
    pub const fn base(self) -> Caps {
        self.with_tier(HwTier::Base)
    }

    /// How many more characters this tier affords than the base one (1.0
    /// on every base tier and on the Expansion Pak, 2.0 on the New 3DS).
    /// Advisory: multiply a crowd authored for the base console.
    pub fn crowd_scale(&self) -> f32 {
        self.crowd_percent as f32 / 100.0
    }

    /// How many more particles this tier affords (see
    /// [`crowd_scale`](Self::crowd_scale)). Particle effects can also name
    /// a cap per tier in their file (`n64_expanded`, `n3ds_new`).
    pub fn particle_scale(&self) -> f32 {
        self.particle_percent as f32 / 100.0
    }

    /// `n` characters authored for the base tier, scaled for this one, in
    /// integers (`n × crowd_percent / 100`, rounded down).
    pub const fn crowd(&self, n: u32) -> u32 {
        n * self.crowd_percent as u32 / 100
    }

    /// `n` particles authored for the base tier, scaled for this one.
    pub const fn particles(&self, n: u32) -> u32 {
        n * self.particle_percent as u32 / 100
    }

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
        assert_eq!(web.shadow_tier, Caps::PC.shadow_tier);
        assert!(web.validate_texture(4096, 4096, 4).is_ok());
        assert!(matches!(
            web.validate_texture(8192, 16, 4),
            Err(CapsError::TextureTooLarge { max_dim: 4096, .. })
        ));
        assert!(web.save_bytes >= Caps::N3DS.save_bytes);
    }

    #[test]
    fn post_tiers_follow_the_hardware() {
        use crate::post::PostTier;
        assert_eq!(Caps::N64.post_tier, PostTier::Baked);
        assert_eq!(Caps::N64_EXPANDED.post_tier, PostTier::Baked);
        assert_eq!(Caps::N3DS.post_tier, PostTier::Baked);
        assert_eq!(Caps::N3DS_NEW.post_tier, PostTier::Baked);
        assert_eq!(Caps::PC.post_tier, PostTier::Full);
        assert_eq!(Caps::WEB.post_tier, PostTier::Full);
    }

    #[test]
    fn shadow_tiers_follow_the_hardware() {
        use crate::shadow::ShadowTier;
        assert_eq!(Caps::N64.shadow_tier, ShadowTier::Blob);
        assert_eq!(Caps::N64_EXPANDED.shadow_tier, ShadowTier::Blob);
        assert_eq!(Caps::N3DS.shadow_tier, ShadowTier::Planar);
        assert_eq!(Caps::N3DS_NEW.shadow_tier, ShadowTier::Planar);
        assert_eq!(Caps::PC.shadow_tier, ShadowTier::Map);
        assert_eq!(Caps::WEB.shadow_tier, ShadowTier::Map);
        // The N64 projects a couple of hero casters; the rest are blobs.
        assert_eq!(Caps::N64.max_planar_shadows, 2);
        assert_eq!((Caps::N64.depth_bits, Caps::N64.depth_mantissa), (18, 11));
    }

    #[test]
    fn expanded_tiers_only_change_what_their_hardware_adds() {
        // The Expansion Pak: memory only.
        let x = Caps::N64_EXPANDED;
        assert_eq!(x.ram_bytes, 2 * Caps::N64.ram_bytes);
        assert_eq!(x.hw_tier, HwTier::Expanded);
        assert_eq!(
            Caps {
                ram_bytes: Caps::N64.ram_bytes,
                hw_tier: HwTier::Base,
                ..x
            },
            Caps::N64
        );
        assert_eq!((x.crowd_scale(), x.particle_scale()), (1.0, 1.0));
        // The New 3DS: CPU (clock, a core), so the scaling knobs.
        let n = Caps::N3DS_NEW;
        assert_eq!((n.cpu_mhz, n.cpu_cores), (804, 2));
        assert_eq!((n.crowd_scale(), n.particle_scale()), (2.0, 2.0));
        assert_eq!((n.crowd(10), n.particles(8)), (20, 16));
        assert_eq!(n.ram_bytes, Caps::N3DS.ram_bytes);
        assert_eq!(
            Caps {
                cpu_cores: 1,
                cpu_mhz: 268,
                hw_tier: HwTier::Base,
                crowd_percent: 100,
                particle_percent: 100,
                ..n
            },
            Caps::N3DS
        );
        // Base tiers scale by 1.
        for c in [Caps::N64, Caps::N3DS, Caps::PC, Caps::WEB] {
            assert_eq!(c.hw_tier, HwTier::Base);
            assert_eq!((c.crowd(7), c.particles(7)), (7, 7));
        }
    }

    #[test]
    fn with_tier_maps_each_console_to_its_other_table() {
        assert_eq!(Caps::N64.with_tier(HwTier::Expanded), Caps::N64_EXPANDED);
        assert_eq!(Caps::N64_EXPANDED.with_tier(HwTier::Base), Caps::N64);
        assert_eq!(Caps::N3DS.with_tier(HwTier::Expanded), Caps::N3DS_NEW);
        assert_eq!(Caps::N3DS_NEW.base(), Caps::N3DS);
        for c in [Caps::N64, Caps::N3DS, Caps::N64_EXPANDED, Caps::N3DS_NEW] {
            assert_eq!(c.with_tier(c.hw_tier), c);
        }
        // One tier on the PC and the Web.
        assert_eq!(Caps::PC.with_tier(HwTier::Expanded), Caps::PC);
        assert_eq!(Caps::WEB.with_tier(HwTier::Expanded), Caps::WEB);
        // A host's own resolution survives (the PC's N64 profile).
        let sim = Caps {
            screen_width: 640,
            ..Caps::N64
        };
        let x = sim.with_tier(HwTier::Expanded);
        assert_eq!(
            (x.screen_width, x.ram_bytes),
            (640, Caps::N64_EXPANDED.ram_bytes)
        );
        assert_eq!(HwTier::ALL.map(HwTier::key), ["base", "expanded"]);
    }
}
