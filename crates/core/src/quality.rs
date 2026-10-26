//! Quality presets: one struct a game reads on every target (issue #55,
//! ADR 0023).
//!
//! A PC or a browser can be a Steam Deck, a laptop's integrated GPU or a
//! desktop card. [`QualitySettings`] gathers the knobs that scale a frame's
//! GPU cost (render scale, dynamic resolution, MSAA, ambient occlusion,
//! texture anisotropy, the upscale's sharpening, the frame limit) and the
//! two multipliers the engine applies to content on every target
//! ([`lod_bias`](QualitySettings::lod_bias) for LOD start distances,
//! [`particle_scale`](QualitySettings::particle_scale) for particle caps).
//!
//! - **PC and Web:** [`crate::Platform::quality`] is what the player chose
//!   (or what the first launch's calibration picked); the host persists it
//!   and its renderer applies what it understands. A project redefines each
//!   preset in `assets/quality.ron` (baked to the `quality/presets` blob, the
//!   text [`PresetTable::parse`] reads).
//! - **Consoles:** a fixed preset matching their [`crate::Caps`]
//!   ([`QualitySettings::fixed`]): the N64 is `Low`, the 3DS `Low` and the New
//!   3DS `Medium`, with every multiplier at 1.0 (console content is authored
//!   for its console). [`crate::Platform::set_quality`] is a documented
//!   no-op there.
//!
//! Persistence and the baked table share one text format: `key=value`
//! lines ([`QualitySettings::write_text`] / [`QualitySettings::parse_text`]),
//! readable and fixable by hand, parsed here without allocation.

use core::fmt::Write;

use crate::ao::AoQuality;
use crate::render::TexFilter;

/// A named set of [`QualitySettings`]. `Custom` is what a preset becomes
/// once one of its values is changed by hand.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord)]
pub enum QualityPreset {
    Low,
    Medium,
    #[default]
    High,
    Ultra,
    Custom,
}

impl QualityPreset {
    /// The four named presets, cheapest first (calibration walks them in
    /// this order).
    pub const NAMED: [QualityPreset; 4] = [
        QualityPreset::Low,
        QualityPreset::Medium,
        QualityPreset::High,
        QualityPreset::Ultra,
    ];
    pub const ALL: [QualityPreset; 5] = [
        QualityPreset::Low,
        QualityPreset::Medium,
        QualityPreset::High,
        QualityPreset::Ultra,
        QualityPreset::Custom,
    ];

    /// `low` / `medium` / `high` / `ultra` / `custom` (files, `--quality`,
    /// `TRINO_QUALITY`, `--json`).
    pub const fn key(self) -> &'static str {
        match self {
            QualityPreset::Low => "low",
            QualityPreset::Medium => "medium",
            QualityPreset::High => "high",
            QualityPreset::Ultra => "ultra",
            QualityPreset::Custom => "custom",
        }
    }

    /// `Low` / `Medium` / … (menus).
    pub const fn label(self) -> &'static str {
        match self {
            QualityPreset::Low => "Low",
            QualityPreset::Medium => "Medium",
            QualityPreset::High => "High",
            QualityPreset::Ultra => "Ultra",
            QualityPreset::Custom => "Custom",
        }
    }

    pub fn parse(s: &str) -> Option<QualityPreset> {
        QualityPreset::ALL
            .into_iter()
            .find(|p| s.trim().eq_ignore_ascii_case(p.key()))
    }

    /// Position in [`NAMED`](Self::NAMED) (`None` for `Custom`).
    pub const fn index(self) -> Option<usize> {
        match self {
            QualityPreset::Low => Some(0),
            QualityPreset::Medium => Some(1),
            QualityPreset::High => Some(2),
            QualityPreset::Ultra => Some(3),
            QualityPreset::Custom => None,
        }
    }
}

/// Dynamic resolution: the render scale follows the GPU's frame time
/// (a PI controller on the PC and the Web; ADR 0023 §3).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DynRes {
    /// The GPU frame time to hold, µs. `0` = from the frame limit: 90% of
    /// its interval (VSync at 60 Hz: 15 000 µs).
    pub target_us: u32,
    /// The render scale's range (within 0.25..=2.0).
    pub min_scale: f32,
    pub max_scale: f32,
}

impl DynRes {
    pub const DEFAULT: DynRes = DynRes {
        target_us: 0,
        min_scale: 0.5,
        max_scale: 1.0,
    };

    /// The target in µs for a frame `interval_us` long (`target_us`, or 90%
    /// of the interval).
    pub const fn target_for(&self, interval_us: u32) -> u32 {
        if self.target_us > 0 {
            self.target_us
        } else {
            interval_us / 10 * 9
        }
    }
}

impl Default for DynRes {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Off / Low / Medium / High: the four steps of [`AoSetting`],
/// [`ShadowSetting`] and [`PostSetting`] (`off` / `low` / `medium` / `high`
/// in files).
macro_rules! level_enum {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord)]
        pub enum $name {
            Off,
            Low,
            #[default]
            Medium,
            High,
        }

        impl $name {
            pub const ALL: [$name; 4] = [$name::Off, $name::Low, $name::Medium, $name::High];

            /// 0 (off) ..= 3 (high).
            pub const fn level(self) -> usize {
                self as usize
            }

            pub const fn key(self) -> &'static str {
                ["off", "low", "medium", "high"][self as usize]
            }

            pub const fn label(self) -> &'static str {
                ["Off", "Low", "Medium", "High"][self as usize]
            }

            pub fn parse(s: &str) -> Option<$name> {
                $name::ALL
                    .into_iter()
                    .find(|l| s.trim().eq_ignore_ascii_case(l.key()))
            }
        }
    };
}

level_enum!(
    /// Screen-space ambient occlusion, as a cap over what the game asks
    /// for: `Off` drops the pass, `Low` and `Medium` lower the game's
    /// [`AoQuality`] to at most 8 / 16 samples, `High` keeps it as the game
    /// asks (up to 32). Radius and intensity stay the game's: a preset
    /// never makes the AO costlier than the game was authored with.
    /// Consoles bake their AO into vertex colors and ignore it.
    AoSetting
);
level_enum!(
    /// Shadow quality on the native profile (ADR 0025): `Off` caps the tier
    /// at planar (casters stay grounded, no map pass), `Low` draws a
    /// 1024-texel map with 2×2 PCF taps, `Medium` 2048 with 3×3, `High`
    /// 2048 with 4×4. A game's own `ShadowSettings::map_size` wins.
    /// Consoles draw their tier and ignore it.
    ShadowSetting
);
level_enum!(
    /// The post stack's level on the native profile and the Web (issue
    /// #64, ADR 0033): `Off` keeps the tonemap and the grading and drops
    /// bloom, vignette and grain; `Low` blooms from a quarter of the
    /// picture over at most 3 levels, `Medium` from half over 4, `High`
    /// from half over 6. Consoles ignore it (their grading is baked).
    PostSetting
);

impl AoSetting {
    /// The game's AO quality under this cap (`None`: no pass).
    pub const fn cap(self, game: AoQuality) -> Option<AoQuality> {
        match (self.quality(), game) {
            (None, _) => None,
            (Some(AoQuality::Low), _) => Some(AoQuality::Low),
            (Some(AoQuality::Medium), AoQuality::High) => Some(AoQuality::Medium),
            (Some(_), g) => Some(g),
        }
    }

    /// The pass's highest quality, `None` for `Off`.
    pub const fn quality(self) -> Option<AoQuality> {
        match self {
            AoSetting::Off => None,
            AoSetting::Low => Some(AoQuality::Low),
            AoSetting::Medium => Some(AoQuality::Medium),
            AoSetting::High => Some(AoQuality::High),
        }
    }
}

/// How often a frame is shown.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum FrameLimit {
    /// Wait for the display (FIFO presentation): no tearing, the monitor's
    /// rate.
    #[default]
    VSync,
    /// At most this many frames per second (a sleep-based limiter; 30, 40
    /// for a Steam Deck on battery, 60, 120, 144).
    Fps(u16),
    /// As fast as the GPU goes (mailbox, or immediate where there is none).
    Unlimited,
}

impl FrameLimit {
    /// The caps an options page offers, VSync and Unlimited around them.
    pub const CHOICES: [FrameLimit; 7] = [
        FrameLimit::VSync,
        FrameLimit::Fps(30),
        FrameLimit::Fps(40),
        FrameLimit::Fps(60),
        FrameLimit::Fps(120),
        FrameLimit::Fps(144),
        FrameLimit::Unlimited,
    ];

    /// The frame interval in µs for a display refreshing at `refresh_hz`
    /// (VSync), the cap's own, or the display's for `Unlimited` (the
    /// dynamic resolution's default target needs one).
    pub fn interval_us(self, refresh_hz: u32) -> u32 {
        let hz = match self {
            FrameLimit::Fps(n) if n > 0 => n as u32,
            _ => refresh_hz.max(1),
        };
        1_000_000 / hz
    }
}

/// Everything the quality presets set. Construct with
/// [`QualitySettings::preset`] or a table's [`PresetTable::get`] and change
/// fields with `..`; [`clamped`](Self::clamped) keeps every value in range.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct QualitySettings {
    /// Which preset this is, or `Custom` once a value changed by hand.
    pub preset: QualityPreset,
    /// The 3D's resolution as a fraction of the game's picture
    /// (0.5..=2.0; 1.0 = the picture's, the output's at pixel scale 1).
    /// The 2D drawn after the frame's last 3D draw (the HUD) always stays
    /// at the picture's resolution. With dynamic resolution on it is the
    /// starting scale.
    pub render_scale: f32,
    /// `Some`: the render scale follows the frame time within its range.
    pub dynamic_resolution: Option<DynRes>,
    /// Samples per pixel of the 3D: 1 or 4 (2 and 8 round to 4).
    pub msaa: u8,
    pub ao: AoSetting,
    /// Caps the shadows' tier and sizes the map (ADR 0025).
    pub shadows: ShadowSetting,
    /// #64 (not consumed yet).
    pub post: PostSetting,
    /// Multiplies the LOD start distances ([`crate::scene::Lods::pick`]):
    /// above 1 detail stays farther out, below 1 it drops sooner.
    pub lod_bias: f32,
    /// Multiplies particle caps ([`crate::ParticleSystem::limit`]).
    pub particle_scale: f32,
    /// What `TexFilter::Default` means for 3D textures on the PC's own
    /// picture (`Default` = the target's: nearest on the PC).
    pub texture_filter: TexFilter,
    /// Anisotropic filtering of bilinear 3D textures on the PC and WebGPU:
    /// 1 (off), 2, 4, 8 or 16.
    pub anisotropy: u8,
    /// Sharpening of the upscaled 3D (contrast-adaptive, 0 = plain
    /// bilinear, 1 = strongest). It runs only while the render scale is
    /// below 1.
    pub sharpen: f32,
    pub frame_limit: FrameLimit,
}

impl QualitySettings {
    /// Built-in presets (what `assets/quality.ron` overrides).
    pub const LOW: QualitySettings = QualitySettings {
        preset: QualityPreset::Low,
        render_scale: 0.75,
        dynamic_resolution: Some(DynRes::DEFAULT),
        msaa: 1,
        ao: AoSetting::Off,
        shadows: ShadowSetting::Off,
        post: PostSetting::Low,
        lod_bias: 0.6,
        particle_scale: 0.5,
        texture_filter: TexFilter::Default,
        anisotropy: 1,
        sharpen: 0.0,
        frame_limit: FrameLimit::VSync,
    };
    pub const MEDIUM: QualitySettings = QualitySettings {
        preset: QualityPreset::Medium,
        render_scale: 1.0,
        dynamic_resolution: Some(DynRes {
            min_scale: 0.6,
            ..DynRes::DEFAULT
        }),
        msaa: 1,
        ao: AoSetting::Low,
        shadows: ShadowSetting::Low,
        post: PostSetting::Medium,
        lod_bias: 0.8,
        particle_scale: 0.75,
        texture_filter: TexFilter::Default,
        anisotropy: 4,
        sharpen: 0.2,
        frame_limit: FrameLimit::VSync,
    };
    /// The PC's look before presets existed (4× MSAA, the game's AO).
    pub const HIGH: QualitySettings = QualitySettings {
        preset: QualityPreset::High,
        render_scale: 1.0,
        dynamic_resolution: None,
        msaa: 4,
        ao: AoSetting::High,
        shadows: ShadowSetting::Medium,
        post: PostSetting::High,
        lod_bias: 1.0,
        particle_scale: 1.0,
        texture_filter: TexFilter::Default,
        anisotropy: 8,
        sharpen: 0.4,
        frame_limit: FrameLimit::VSync,
    };
    /// High supersampled: the 3D at 1.25× the picture, filtered down.
    pub const ULTRA: QualitySettings = QualitySettings {
        preset: QualityPreset::Ultra,
        render_scale: 1.25,
        dynamic_resolution: None,
        msaa: 4,
        ao: AoSetting::High,
        shadows: ShadowSetting::High,
        post: PostSetting::High,
        lod_bias: 1.5,
        particle_scale: 1.0,
        texture_filter: TexFilter::Default,
        anisotropy: 16,
        sharpen: 0.5,
        frame_limit: FrameLimit::VSync,
    };

    /// A built-in preset (`Custom` gives `High`'s values, marked custom).
    pub const fn preset(p: QualityPreset) -> QualitySettings {
        let mut q = match p {
            QualityPreset::Low => Self::LOW,
            QualityPreset::Medium => Self::MEDIUM,
            QualityPreset::High | QualityPreset::Custom => Self::HIGH,
            QualityPreset::Ultra => Self::ULTRA,
        };
        q.preset = p;
        q
    }

    /// A console's fixed preset: the label `p` (`Low` on the N64 and the
    /// 3DS, `Medium` on the New 3DS) with nothing scaled: render scale and
    /// multipliers 1.0, one sample, AO off (the consoles' is baked), the
    /// target's filter, VSync.
    pub const fn fixed(p: QualityPreset) -> QualitySettings {
        QualitySettings {
            preset: p,
            render_scale: 1.0,
            dynamic_resolution: None,
            msaa: 1,
            ao: AoSetting::Off,
            shadows: ShadowSetting::Off,
            post: PostSetting::Off,
            lod_bias: 1.0,
            particle_scale: 1.0,
            texture_filter: TexFilter::Default,
            anisotropy: 1,
            sharpen: 0.0,
            frame_limit: FrameLimit::VSync,
        }
    }

    /// A console's fixed preset from its `Caps` (any tier's table): `Low` on
    /// the N64 (the Expansion Pak adds memory, not speed) and the 3DS,
    /// `Medium` on the New 3DS (three times the CPU). Any other table (the
    /// PC, the Web) gives `Low` too: those targets report their own.
    pub const fn console(caps: &crate::Caps) -> QualitySettings {
        match (caps.cpu_mhz, caps.hw_tier) {
            (268 | 804, crate::HwTier::Expanded) => Self::fixed(QualityPreset::Medium),
            _ => Self::fixed(QualityPreset::Low),
        }
    }

    /// The Steam Deck's preset (ADR 0023 §5): Medium at the Deck's 800 lines,
    /// capped at 60 fps (40 or 30 on battery are one option away).
    pub const STEAM_DECK: QualitySettings = QualitySettings {
        frame_limit: FrameLimit::Fps(60),
        ..Self::MEDIUM
    };

    /// Every value in its range: render scale 0.5..=2.0, MSAA 1 or 4,
    /// anisotropy a power of two in 1..=16, multipliers 0.1..=4, sharpen
    /// 0..=1, a dynamic range within 0.25..=2.0 (min ≤ max). NaN takes the
    /// High preset's value.
    pub fn clamped(self) -> QualitySettings {
        let h = Self::HIGH;
        let fix = |v: f32, lo: f32, hi: f32, d: f32| if v.is_nan() { d } else { v.clamp(lo, hi) };
        let dynamic_resolution = self.dynamic_resolution.map(|d| {
            let lo = fix(d.min_scale, 0.25, 2.0, 0.5);
            let hi = fix(d.max_scale, 0.25, 2.0, 1.0).max(lo);
            DynRes {
                target_us: d.target_us.min(1_000_000),
                min_scale: lo,
                max_scale: hi,
            }
        });
        QualitySettings {
            render_scale: fix(self.render_scale, 0.5, 2.0, h.render_scale),
            dynamic_resolution,
            msaa: if self.msaa >= 2 { 4 } else { 1 },
            lod_bias: fix(self.lod_bias, 0.1, 4.0, h.lod_bias),
            particle_scale: fix(self.particle_scale, 0.1, 4.0, h.particle_scale),
            anisotropy: match self.anisotropy {
                0 | 1 => 1,
                2..=3 => 2,
                4..=7 => 4,
                8..=15 => 8,
                _ => 16,
            },
            sharpen: fix(self.sharpen, 0.0, 1.0, h.sharpen),
            ..self
        }
    }

    /// A LOD start distance `d` with [`lod_bias`](Self::lod_bias) applied.
    pub fn lod_distance(&self, d: f32) -> f32 {
        d * self.lod_bias
    }

    /// A particle cap `n` with [`particle_scale`](Self::particle_scale)
    /// applied (at least 1 when `n` is).
    pub fn particle_cap(&self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        ((n as f32 * self.particle_scale) as usize).max(1)
    }

    /// The render scale the frame starts at (a dynamic range clamps it).
    pub fn start_scale(&self) -> f32 {
        match self.dynamic_resolution {
            Some(d) => crate::math::clamp_or(self.render_scale, d.min_scale, d.max_scale),
            None => self.render_scale,
        }
    }

    /// The settings as `key=value` lines (the `.quality` file, the baked
    /// preset table, `--json`-free tooling): every field, one per line.
    pub fn write_text(&self, out: &mut impl Write) -> core::fmt::Result {
        writeln!(out, "preset={}", self.preset.key())?;
        writeln!(out, "render_scale={:.2}", self.render_scale)?;
        match self.dynamic_resolution {
            None => writeln!(out, "dynamic_resolution=off")?,
            Some(d) => writeln!(
                out,
                "dynamic_resolution={},{:.2},{:.2}",
                d.target_us, d.min_scale, d.max_scale
            )?,
        }
        writeln!(out, "msaa={}", self.msaa)?;
        writeln!(out, "ao={}", self.ao.key())?;
        writeln!(out, "shadows={}", self.shadows.key())?;
        writeln!(out, "post={}", self.post.key())?;
        writeln!(out, "lod_bias={:.2}", self.lod_bias)?;
        writeln!(out, "particle_scale={:.2}", self.particle_scale)?;
        writeln!(out, "texture_filter={}", filter_key(self.texture_filter))?;
        writeln!(out, "anisotropy={}", self.anisotropy)?;
        writeln!(out, "sharpen={:.2}", self.sharpen)?;
        match self.frame_limit {
            FrameLimit::VSync => writeln!(out, "frame_limit=vsync"),
            FrameLimit::Fps(n) => writeln!(out, "frame_limit={n}"),
            FrameLimit::Unlimited => writeln!(out, "frame_limit=unlimited"),
        }
    }

    /// Apply one `key=value` pair; `false` when the key is unknown or the
    /// value doesn't parse (the field keeps its value).
    pub fn set_field(&mut self, key: &str, value: &str) -> bool {
        let v = value.trim();
        let num = |v: &str| v.parse::<f32>().ok().filter(|x| x.is_finite());
        match key.trim() {
            "preset" => QualityPreset::parse(v).map(|p| self.preset = p).is_some(),
            "render_scale" => num(v).map(|x| self.render_scale = x).is_some(),
            "dynamic_resolution" => {
                if v.eq_ignore_ascii_case("off") || v.eq_ignore_ascii_case("none") {
                    self.dynamic_resolution = None;
                    return true;
                }
                if v.eq_ignore_ascii_case("on") {
                    self.dynamic_resolution = Some(self.dynamic_resolution.unwrap_or_default());
                    return true;
                }
                let mut it = v.split(',');
                let (Some(t), Some(lo), Some(hi), None) =
                    (it.next(), it.next(), it.next(), it.next())
                else {
                    return false;
                };
                match (t.trim().parse::<u32>(), num(lo.trim()), num(hi.trim())) {
                    (Ok(t), Some(lo), Some(hi)) => {
                        self.dynamic_resolution = Some(DynRes {
                            target_us: t,
                            min_scale: lo,
                            max_scale: hi,
                        });
                        true
                    }
                    _ => false,
                }
            }
            "msaa" => v.parse().map(|n| self.msaa = n).is_ok(),
            "ao" => AoSetting::parse(v).map(|l| self.ao = l).is_some(),
            "shadows" => ShadowSetting::parse(v).map(|l| self.shadows = l).is_some(),
            "post" => PostSetting::parse(v).map(|l| self.post = l).is_some(),
            "lod_bias" => num(v).map(|x| self.lod_bias = x).is_some(),
            "particle_scale" => num(v).map(|x| self.particle_scale = x).is_some(),
            "texture_filter" => parse_filter(v).map(|f| self.texture_filter = f).is_some(),
            "anisotropy" => v.parse().map(|n| self.anisotropy = n).is_ok(),
            "sharpen" => num(v).map(|x| self.sharpen = x).is_some(),
            "frame_limit" => parse_frame_limit(v).map(|l| self.frame_limit = l).is_some(),
            _ => false,
        }
    }

    /// Read [`write_text`](Self::write_text) lines over `base`: unknown keys,
    /// broken values, blank lines and `#` comments keep `base`'s values. The
    /// result is [`clamped`](Self::clamped).
    pub fn parse_text(base: QualitySettings, text: &str) -> QualitySettings {
        let mut q = base;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                q.set_field(k, v);
            }
        }
        q.clamped()
    }
}

impl Default for QualitySettings {
    fn default() -> Self {
        Self::HIGH
    }
}

/// `default` / `point` / `bilinear`.
pub const fn filter_key(f: TexFilter) -> &'static str {
    match f {
        TexFilter::Default => "default",
        TexFilter::Point => "point",
        TexFilter::Bilinear => "bilinear",
    }
}

pub fn parse_filter(s: &str) -> Option<TexFilter> {
    [TexFilter::Default, TexFilter::Point, TexFilter::Bilinear]
        .into_iter()
        .find(|f| s.trim().eq_ignore_ascii_case(filter_key(*f)))
}

/// `vsync`, `unlimited` or a number of frames per second.
pub fn parse_frame_limit(s: &str) -> Option<FrameLimit> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("vsync") {
        Some(FrameLimit::VSync)
    } else if s.eq_ignore_ascii_case("unlimited") || s.eq_ignore_ascii_case("off") {
        Some(FrameLimit::Unlimited)
    } else {
        s.parse::<u16>()
            .ok()
            .filter(|n| (10..=1000).contains(n))
            .map(FrameLimit::Fps)
    }
}

/// What the four named presets mean for a project: the built-in values,
/// overridden by `assets/quality.ron` (baked to the `quality/presets` blob
/// as `[low]` / `[medium]` / `[high]` / `[ultra]` sections of
/// [`QualitySettings::write_text`] lines).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PresetTable {
    pub presets: [QualitySettings; 4],
}

impl PresetTable {
    pub const BUILT_IN: PresetTable = PresetTable {
        presets: [
            QualitySettings::LOW,
            QualitySettings::MEDIUM,
            QualitySettings::HIGH,
            QualitySettings::ULTRA,
        ],
    };

    /// The preset `p` (`Custom` gives High's values, marked custom).
    pub fn get(&self, p: QualityPreset) -> QualitySettings {
        match p.index() {
            Some(i) => self.presets[i],
            None => QualitySettings {
                preset: QualityPreset::Custom,
                ..self.presets[2]
            },
        }
    }

    /// The baked text: each `[preset]` section's lines override that
    /// preset's built-in values (a section names only what changes). Lines
    /// before the first section and unknown sections are ignored; each
    /// preset keeps its own name whatever its `preset=` line says.
    pub fn parse(text: &str) -> PresetTable {
        let mut t = PresetTable::BUILT_IN;
        let mut at: Option<usize> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                at = QualityPreset::parse(name).and_then(QualityPreset::index);
                continue;
            }
            if let (Some(i), Some((k, v))) = (at, line.split_once('='))
                && k.trim() != "preset"
            {
                t.presets[i].set_field(k, v);
            }
        }
        for (q, p) in t.presets.iter_mut().zip(QualityPreset::NAMED) {
            *q = q.clamped();
            q.preset = p;
        }
        t
    }

    /// The whole table as [`parse`](Self::parse) reads it.
    pub fn write_text(&self, out: &mut impl Write) -> core::fmt::Result {
        for q in &self.presets {
            writeln!(out, "[{}]", q.preset.key())?;
            q.write_text(out)?;
        }
        Ok(())
    }

    /// The named preset whose values equal `q` (ignoring its `preset` and
    /// `frame_limit`, which the player picks apart), else `Custom`.
    pub fn classify(&self, q: &QualitySettings) -> QualityPreset {
        let same = |a: &QualitySettings| {
            QualitySettings {
                preset: QualityPreset::Custom,
                frame_limit: FrameLimit::VSync,
                ..*a
            } == QualitySettings {
                preset: QualityPreset::Custom,
                frame_limit: FrameLimit::VSync,
                ..*q
            }
        };
        self.presets
            .iter()
            .find(|p| same(p))
            .map_or(QualityPreset::Custom, |p| p.preset)
    }
}

impl Default for PresetTable {
    fn default() -> Self {
        Self::BUILT_IN
    }
}

/// The GPU a PC or a browser runs on, as far as the host can tell
/// (`wgpu::AdapterInfo::device_type` on the PC).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum GpuClass {
    #[default]
    Unknown,
    /// A CPU rasterizer (llvmpipe, lavapipe, SwiftShader, WARP).
    Software,
    /// Shares the CPU's memory: laptops, the Steam Deck, Chromebooks, phones.
    Integrated,
    Discrete,
}

impl GpuClass {
    pub const fn key(self) -> &'static str {
        match self {
            GpuClass::Unknown => "unknown",
            GpuClass::Software => "software",
            GpuClass::Integrated => "integrated",
            GpuClass::Discrete => "discrete",
        }
    }
}

/// How a host arrived at its quality (`TRINO_FRAME`'s `quality_from=`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum QualitySource {
    /// A console's fixed preset.
    #[default]
    Fixed,
    /// The project's default (`[pc.quality] default`), no calibration.
    Default,
    /// The first launch's calibration (or "Detect again").
    Detected,
    /// The player's saved choice (`<save_name>.quality`, `localStorage`).
    Saved,
    /// Forced for this run (`TRINO_QUALITY`, `run pc --quality`).
    Forced,
    /// The Steam Deck's preset.
    SteamDeck,
    /// A phone's or a tablet's browser: the project's Low.
    Mobile,
}

impl QualitySource {
    pub const fn key(self) -> &'static str {
        match self {
            QualitySource::Fixed => "fixed",
            QualitySource::Default => "default",
            QualitySource::Detected => "detected",
            QualitySource::Saved => "saved",
            QualitySource::Forced => "forced",
            QualitySource::SteamDeck => "steam-deck",
            QualitySource::Mobile => "mobile",
        }
    }
}

/// The quality as a PC or Web host reports it
/// ([`crate::Platform::quality_info`]; `None` on consoles).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct QualityInfo {
    /// In effect (after the last [`crate::Platform::set_quality`]).
    pub settings: QualitySettings,
    /// The 3D's render scale this frame (dynamic resolution moves it).
    pub scale: f32,
    /// The last calibration's pick (`None`: it never ran here).
    pub detected: Option<QualityPreset>,
    pub source: QualitySource,
    pub gpu: GpuClass,
    /// A handheld PC the host recognized (the Steam Deck).
    pub handheld: bool,
    /// The last measured GPU frame time, µs (0 = not measured yet).
    pub gpu_us: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TextBuf;

    #[test]
    fn presets_grow_in_cost_and_round_trip_their_keys() {
        let t = PresetTable::BUILT_IN;
        for w in t.presets.windows(2) {
            let (a, b) = (w[0], w[1]);
            assert!(a.start_scale() <= b.start_scale());
            assert!(a.msaa <= b.msaa && a.anisotropy <= b.anisotropy);
            assert!(a.ao.level() <= b.ao.level());
            assert!(a.lod_bias <= b.lod_bias && a.particle_scale <= b.particle_scale);
        }
        for p in QualityPreset::ALL {
            assert_eq!(QualityPreset::parse(p.key()), Some(p));
            assert_eq!(QualitySettings::preset(p).preset, p);
        }
        assert_eq!(QualityPreset::parse(" ULTRA "), Some(QualityPreset::Ultra));
        assert_eq!(QualityPreset::parse("epic"), None);
        // High is what the PC drew before presets: 4× MSAA, nothing scaled.
        let h = QualitySettings::HIGH;
        assert_eq!(
            (h.msaa, h.render_scale, h.dynamic_resolution),
            (4, 1.0, None)
        );
        assert_eq!(QualitySettings::default(), h);
    }

    #[test]
    fn ao_settings_cap_the_games_quality() {
        use AoQuality::{High, Low, Medium};
        for game in [Low, Medium, High] {
            assert_eq!(AoSetting::Off.cap(game), None);
            assert_eq!(AoSetting::Low.cap(game), Some(Low));
            assert_eq!(
                AoSetting::High.cap(game),
                Some(game),
                "High keeps the game's"
            );
        }
        assert_eq!(AoSetting::Medium.cap(High), Some(Medium));
        assert_eq!(AoSetting::Medium.cap(Low), Some(Low));
        assert_eq!(QualitySettings::HIGH.ao, AoSetting::High);
    }

    #[test]
    fn console_presets_scale_nothing() {
        for p in [QualityPreset::Low, QualityPreset::Medium] {
            let q = QualitySettings::fixed(p);
            assert_eq!(q.preset, p);
            assert_eq!(
                (q.lod_bias, q.particle_scale, q.render_scale),
                (1.0, 1.0, 1.0)
            );
            assert_eq!((q.msaa, q.dynamic_resolution), (1, None));
            assert_eq!(q.lod_distance(12.0), 12.0);
            assert_eq!(q.particle_cap(48), 48);
        }
        use crate::Caps;
        assert_eq!(
            QualitySettings::console(&Caps::N64).preset,
            QualityPreset::Low
        );
        assert_eq!(
            QualitySettings::console(&Caps::N64_EXPANDED).preset,
            QualityPreset::Low
        );
        assert_eq!(
            QualitySettings::console(&Caps::N3DS).preset,
            QualityPreset::Low
        );
        assert_eq!(
            QualitySettings::console(&Caps::N3DS_NEW).preset,
            QualityPreset::Medium
        );
    }

    #[test]
    fn text_round_trips_every_field() {
        let q = QualitySettings {
            preset: QualityPreset::Custom,
            render_scale: 0.85,
            dynamic_resolution: Some(DynRes {
                target_us: 16_667,
                min_scale: 0.5,
                max_scale: 1.25,
            }),
            msaa: 4,
            ao: AoSetting::High,
            shadows: ShadowSetting::Low,
            post: PostSetting::Off,
            lod_bias: 1.5,
            particle_scale: 0.25,
            texture_filter: TexFilter::Bilinear,
            anisotropy: 16,
            sharpen: 0.35,
            frame_limit: FrameLimit::Fps(144),
        };
        let mut buf = TextBuf::<512>::new();
        q.write_text(&mut buf).unwrap();
        let back = QualitySettings::parse_text(QualitySettings::LOW, buf.as_str());
        assert_eq!(back, q, "{}", buf.as_str());
        // Every frame limit and dynamic switch.
        for (text, limit) in [
            ("vsync", FrameLimit::VSync),
            ("60", FrameLimit::Fps(60)),
            ("unlimited", FrameLimit::Unlimited),
        ] {
            assert_eq!(parse_frame_limit(text), Some(limit));
        }
        assert_eq!(parse_frame_limit("5"), None);
        let off = QualitySettings::parse_text(q, "dynamic_resolution=off");
        assert_eq!(off.dynamic_resolution, None);
        let on = QualitySettings::parse_text(off, "dynamic_resolution=on");
        assert_eq!(on.dynamic_resolution, Some(DynRes::DEFAULT));
    }

    #[test]
    fn broken_text_keeps_the_base_and_values_are_clamped() {
        let base = QualitySettings::MEDIUM;
        let q = QualitySettings::parse_text(
            base,
            "# a comment\n\ngarbage\nmsaa=x\nao=extreme\nrender_scale=9\nanisotropy=5\n\
             lod_bias=nan\ndynamic_resolution=1,2\nsharpen=-1\n",
        );
        assert_eq!(q.msaa, base.msaa);
        assert_eq!(q.ao, base.ao);
        assert_eq!(q.render_scale, 2.0);
        assert_eq!(q.anisotropy, 4);
        assert_eq!(q.lod_bias, base.lod_bias);
        assert_eq!(q.dynamic_resolution, base.dynamic_resolution);
        assert_eq!(q.sharpen, 0.0);
        let c = QualitySettings {
            msaa: 8,
            dynamic_resolution: Some(DynRes {
                target_us: 0,
                min_scale: 1.5,
                max_scale: 0.5,
            }),
            ..base
        }
        .clamped();
        assert_eq!(c.msaa, 4);
        let d = c.dynamic_resolution.unwrap();
        assert!(d.min_scale <= d.max_scale);
    }

    #[test]
    fn a_preset_table_overrides_only_what_it_names() {
        let t = PresetTable::parse(
            "ignored=1\n[high]\nao=low\nmsaa=4\nshadows=high\npreset=low\n[ultra]\n\
             render_scale=1.5\n[nonsense]\nmsaa=1\n",
        );
        assert_eq!(t.get(QualityPreset::Low), QualitySettings::LOW);
        let h = t.get(QualityPreset::High);
        assert_eq!(h.preset, QualityPreset::High);
        assert_eq!(h.shadows, ShadowSetting::High);
        assert_eq!(h.ao, AoSetting::Low);
        assert_eq!(t.get(QualityPreset::Ultra).render_scale, 1.5);
        assert_eq!(t.get(QualityPreset::Custom).preset, QualityPreset::Custom);
        // The table round-trips through its own text.
        let mut buf = TextBuf::<2048>::new();
        t.write_text(&mut buf).unwrap();
        assert_eq!(PresetTable::parse(buf.as_str()), t);
        // Classify: a preset's values (any frame limit) or Custom.
        let mut q = t.get(QualityPreset::Ultra);
        q.frame_limit = FrameLimit::Fps(30);
        assert_eq!(t.classify(&q), QualityPreset::Ultra);
        q.msaa = 1;
        assert_eq!(t.classify(&q), QualityPreset::Custom);
    }

    #[test]
    fn frame_limits_and_dynamic_targets() {
        assert_eq!(FrameLimit::VSync.interval_us(60), 16_666);
        assert_eq!(FrameLimit::Fps(30).interval_us(144), 33_333);
        assert_eq!(FrameLimit::Unlimited.interval_us(0), 1_000_000);
        assert_eq!(DynRes::DEFAULT.target_for(16_666), 14_994);
        let d = DynRes {
            target_us: 16_667,
            ..DynRes::DEFAULT
        };
        assert_eq!(d.target_for(33_333), 16_667);
        // The starting scale sits in the dynamic range.
        let q = QualitySettings {
            render_scale: 2.0,
            ..QualitySettings::LOW
        };
        assert_eq!(q.start_scale(), 1.0);
        assert_eq!(QualitySettings::LOW.particle_cap(1), 1);
        assert_eq!(QualitySettings::LOW.particle_cap(64), 32);
        assert_eq!(QualitySettings::STEAM_DECK.frame_limit, FrameLimit::Fps(60));
    }
}
