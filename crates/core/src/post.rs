//! Post-processing (issue #64, ADR 0033): what happens to the finished 3D
//! before the HUD draws over it — exposure and a tonemapper over an HDR
//! target, bloom, color grading through a 3D LUT, vignette and film grain;
//! and, set apart because a game drives it every frame from its speed, a
//! radial [`SpeedBlur`] ([`crate::Renderer::set_speed_blur`]).
//!
//! Each target gets it the way its hardware affords ([`PostTier`], from
//! [`crate::Caps::post_tier`]):
//!
//! - [`PostTier::Full`] (the PC's native profile and the Web): the 3D draws
//!   into an `Rgba16Float` target, then bloom (a dual-filter chain at half
//!   resolution and below) and one fragment pass — exposure, tonemap, LUT,
//!   vignette, grain — write the picture the 2D draws on. The settings are
//!   what a game gives [`crate::Renderer::set_post`] each frame.
//! - [`PostTier::Baked`] (N64, 3DS and the PC's console profiles): no
//!   full-screen pass. The asset bake applies a scene's LUT to the vertex
//!   colors of the models that ask for it (`grading = "luts/x"` on a
//!   `[models.*]` entry) and to a baked scene's light, fog and clear colors
//!   (`bake_grading = true` on `[scenes.*]`), so the console shows the same
//!   mood at zero run-time cost; [`Glow`] halos stand in for bloom. The
//!   PC's console profiles grade their picture with the LUT alone, the
//!   preview of what the bake writes.
//!
//! Off by default: without `set_post` nothing is allocated and every frame
//! is drawn exactly as before.
//!
//! The math here is the reference: the PC shader is held to [`tone`],
//! [`Lut::apply`] and [`SpeedBlur::gather`], and the console bake calls
//! [`Lut::apply_color`].

use crate::math::{Color, Vec2, Vec3};
use crate::render::{BillboardParams, Renderer, SpriteId};

/// How a target post-processes its frame ([`crate::Caps::post_tier`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PostTier {
    /// No full-screen pass: grading baked into vertex colors and scene
    /// colors, glow as billboards (consoles, the PC's console profiles).
    Baked,
    /// The whole stack at run time (PC native, Web).
    Full,
}

/// The curve that maps scene light (0..∞) onto the display (0..1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Tonemap {
    /// Clamp: what an 8-bit target does (the default: no change).
    #[default]
    None,
    /// `x / (1 + x)` per channel.
    Reinhard,
    /// Narkowicz's fit of the ACES filmic curve.
    Aces,
    /// AgX (Sobotka), the minimal polynomial fit (Wrensch 2023): highlights
    /// desaturate toward white instead of skewing hue.
    AgX,
}

impl Tonemap {
    pub const ALL: [Tonemap; 4] = [
        Tonemap::None,
        Tonemap::Reinhard,
        Tonemap::Aces,
        Tonemap::AgX,
    ];

    /// The name in RON files and on the command line.
    pub const fn key(self) -> &'static str {
        match self {
            Tonemap::None => "none",
            Tonemap::Reinhard => "reinhard",
            Tonemap::Aces => "aces",
            Tonemap::AgX => "agx",
        }
    }

    pub fn parse(s: &str) -> Option<Tonemap> {
        Tonemap::ALL
            .into_iter()
            .find(|t| t.key().eq_ignore_ascii_case(s))
    }

    /// The shader's index for this curve.
    pub const fn index(self) -> u32 {
        self as u32
    }
}

/// Light spilling around the brightest pixels ([`PostSettings::bloom`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Bloom {
    /// Linear-light brightness (the brightest channel, after exposure)
    /// where the bloom reaches full strength; a soft knee starts it at half
    /// of this. 0.8 lets an unlit white (1.0) glow: most Trino content is
    /// 8-bit vertex color, so only per-pixel lights and water glints pass
    /// 1.0.
    pub threshold: f32,
    /// How much of the blurred light is added back, 0..=4.
    pub intensity: f32,
    /// How far it spreads, 0..=1 (the levels of the blur chain; a quality
    /// preset caps them).
    pub radius: f32,
}

impl Bloom {
    pub const DEFAULT: Bloom = Bloom {
        threshold: 0.8,
        intensity: 0.6,
        radius: 0.6,
    };

    /// The blur levels `radius` asks for (1..=6) under the preset's cap.
    pub fn levels(&self, cap: u32) -> u32 {
        let want = 1 + (self.radius.clamp(0.0, 1.0) * 5.0 + 0.5) as u32;
        want.clamp(1, cap.max(1))
    }
}

impl Default for Bloom {
    fn default() -> Self {
        Bloom::DEFAULT
    }
}

/// A color-grading LUT: the handle of a `[luts.*]` asset (`asset_id` of its
/// logical path, `"luts/dusk"`), baked as a `TLUT` blob on the PC and the
/// Web. The consoles have none at run time: the bake grades their colors.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct LutId(pub u32);

impl LutId {
    pub const fn of(logical_path: &str) -> LutId {
        LutId(crate::asset::asset_id(logical_path))
    }

    /// The blob the LUT is baked as (the same hash).
    pub const fn blob(self) -> crate::platform::BlobId {
        crate::platform::BlobId(self.0)
    }
}

/// One frame's post-processing ([`crate::Renderer::set_post`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PostSettings {
    /// Multiplies linear light before the tonemap (1 = none; 2 = one stop
    /// brighter).
    pub exposure: f32,
    pub tonemap: Tonemap,
    pub bloom: Option<Bloom>,
    /// Graded through this LUT after the tonemap (display space).
    pub grading: Option<LutId>,
    /// A second LUT the grading mixes toward (Bongo-Kart #181): the picture
    /// through `grading` (ungraded without one) and through this one,
    /// mixed by `grading_mix` ([`grade_mix`]). A time of day or a weather
    /// passing between two looks without a LUT baked per step. The Full
    /// tier only: the consoles' bakes grade by `grading` alone.
    pub grading_to: Option<LutId>,
    /// 0..=1: the weight of `grading_to` (0 = `grading` alone).
    pub grading_mix: f32,
    /// Darkens the corners, 0..=1.
    pub vignette: f32,
    /// Film grain (monochrome, changes every frame), 0..=1.
    pub grain: f32,
}

impl PostSettings {
    /// Every stage neutral: the stack runs and changes nothing (within
    /// 1/255 of the frame without it).
    pub const DEFAULT: PostSettings = PostSettings {
        exposure: 1.0,
        tonemap: Tonemap::None,
        bloom: None,
        grading: None,
        grading_to: None,
        grading_mix: 0.0,
        vignette: 0.0,
        grain: 0.0,
    };

    /// A filmic start: AgX, a soft bloom, a light vignette.
    pub const FILMIC: PostSettings = PostSettings {
        exposure: 1.0,
        tonemap: Tonemap::AgX,
        bloom: Some(Bloom::DEFAULT),
        grading: None,
        grading_to: None,
        grading_mix: 0.0,
        vignette: 0.2,
        grain: 0.0,
    };

    /// The settings with every field in its valid range (NaN takes the
    /// default).
    pub fn clamped(&self) -> PostSettings {
        let fix = |v: f32, lo: f32, hi: f32, def: f32| {
            if v.is_nan() { def } else { v.clamp(lo, hi) }
        };
        let d = Bloom::DEFAULT;
        PostSettings {
            exposure: fix(self.exposure, 0.01, 16.0, 1.0),
            tonemap: self.tonemap,
            bloom: self.bloom.map(|b| Bloom {
                threshold: fix(b.threshold, 0.0, 16.0, d.threshold),
                intensity: fix(b.intensity, 0.0, 4.0, d.intensity),
                radius: fix(b.radius, 0.0, 1.0, d.radius),
            }),
            grading: self.grading,
            grading_to: self.grading_to,
            grading_mix: fix(self.grading_mix, 0.0, 1.0, 0.0),
            vignette: fix(self.vignette, 0.0, 1.0, 0.0),
            grain: fix(self.grain, 0.0, 1.0, 0.0),
        }
    }
}

impl Default for PostSettings {
    fn default() -> Self {
        PostSettings::DEFAULT
    }
}

/// A pixel `c` (display space, 0..=1) graded through `a` (unchanged
/// without one) and mixed toward its grading through `b` by `mix`
/// ([`PostSettings::grading_to`], Bongo-Kart #181): the reference of the
/// PC's and the Web's composite.
pub fn grade_mix(a: Option<&Lut<'_>>, b: Option<&Lut<'_>>, mix: f32, c: [f32; 3]) -> [f32; 3] {
    let ga = a.map_or(c, |l| l.apply(c));
    match b {
        Some(l) if mix > 0.0 => {
            let gb = l.apply(c);
            [
                ga[0] + (gb[0] - ga[0]) * mix,
                ga[1] + (gb[1] - ga[1]) * mix,
                ga[2] + (gb[2] - ga[2]) * mix,
            ]
        }
        _ => ga,
    }
}

// --- the math every tier shares -------------------------------------------

/// log2 for positive normal floats (`no_std`: no libm). Within 1e-6 of
/// std's over the range the stack uses.
pub fn log2(x: f32) -> f32 {
    if x <= 0.0 {
        return -126.0;
    }
    let bits = x.to_bits();
    let e = ((bits >> 23) & 0xff) as i32 - 127;
    let m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    // log2(m) = 2/ln 2 · atanh((m − 1)/(m + 1)), m in [1, 2): t ≤ 1/3.
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    let s = t * (1.0 + t2 * (1.0 / 3.0 + t2 * (0.2 + t2 * (1.0 / 7.0 + t2 * (1.0 / 9.0)))));
    e as f32 + s * 2.885_39
}

/// 2^x (`no_std`), within 2e-6 relative over -126..127.
pub fn exp2(x: f32) -> f32 {
    let x = x.clamp(-126.0, 127.0);
    let mut i = x as i32;
    if (i as f32) > x {
        i -= 1;
    }
    let f = (x - i as f32) * core::f32::consts::LN_2;
    // e^f, f in [0, ln 2).
    let p = 1.0
        + f * (1.0
            + f * (0.5 + f * (1.0 / 6.0 + f * (1.0 / 24.0 + f * (1.0 / 120.0 + f / 720.0)))));
    p * f32::from_bits(((i + 127) as u32) << 23)
}

/// `x^y` for x ≥ 0 (0 at 0).
pub fn pow(x: f32, y: f32) -> f32 {
    if x <= 0.0 { 0.0 } else { exp2(y * log2(x)) }
}

/// An sRGB-encoded value to linear light, extended past 1 (what the HDR
/// target holds: the shaders write display values, lights add over 1).
pub fn decode(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        pow((v + 0.055) / 1.055, 2.4)
    }
}

/// Linear light to the sRGB encoding, extended past 1.
pub fn encode(l: f32) -> f32 {
    if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * pow(l, 1.0 / 2.4) - 0.055
    }
}

fn mat(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    let r = |row: &[f32; 3]| row[0] * v[0] + row[1] * v[1] + row[2] * v[2];
    [r(&m[0]), r(&m[1]), r(&m[2])]
}

/// AgX's inset (Rec. 709 → the AgX working space), rows.
pub const AGX_INSET: [[f32; 3]; 3] = [
    [0.842_479_06, 0.078_433_6, 0.079_223_745],
    [0.042_328_242, 0.878_468_6, 0.079_166_13],
    [0.042_375_655, 0.078_433_6, 0.879_143],
];
/// Its outset (the inverse), rows.
pub const AGX_OUTSET: [[f32; 3]; 3] = [
    [1.196_879, -0.098_020_88, -0.099_029_74],
    [-0.052_896_85, 1.151_903_1, -0.098_961_18],
    [-0.052_971_635, -0.098_043_45, 1.151_073_7],
];
/// The log2 range AgX encodes.
pub const AGX_MIN_EV: f32 = -12.473_93;
pub const AGX_MAX_EV: f32 = 4.026_069;

fn agx_contrast(x: f32) -> f32 {
    let x2 = x * x;
    let x4 = x2 * x2;
    15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x
        - 0.00232
}

/// `t` applied to linear light, returning linear light in 0..=1.
pub fn tonemap(t: Tonemap, l: [f32; 3]) -> [f32; 3] {
    let l = [l[0].max(0.0), l[1].max(0.0), l[2].max(0.0)];
    match t {
        Tonemap::None => [l[0].min(1.0), l[1].min(1.0), l[2].min(1.0)],
        Tonemap::Reinhard => l.map(|x| x / (1.0 + x)),
        Tonemap::Aces => {
            l.map(|x| ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0))
        }
        Tonemap::AgX => {
            let v = mat(&AGX_INSET, l).map(|c| {
                let e = log2(c.max(1e-10)).clamp(AGX_MIN_EV, AGX_MAX_EV);
                agx_contrast((e - AGX_MIN_EV) / (AGX_MAX_EV - AGX_MIN_EV))
            });
            mat(&AGX_OUTSET, v).map(|c| pow(c.max(0.0), 2.2).min(1.0))
        }
    }
}

/// The stack's color path for one pixel without bloom, vignette and
/// grain: a display value from the HDR target → linear → exposure →
/// tonemap → display (0..=1), the input of the LUT. The PC shader is held
/// to it.
pub fn tone(post: &PostSettings, display: [f32; 3]) -> [f32; 3] {
    let l = display.map(|v| decode(v.max(0.0)) * post.exposure);
    tonemap(post.tonemap, l).map(|v| encode(v).clamp(0.0, 1.0))
}

/// How much of a pixel at `uv` (0..1, both axes) the vignette keeps.
pub fn vignette(amount: f32, uv: Vec2) -> f32 {
    let dx = (uv.x - 0.5) * 2.0;
    let dy = (uv.y - 0.5) * 2.0;
    // 0 at the center, 1 in the corners.
    let r = crate::math3d::sqrt((dx * dx + dy * dy) * 0.5);
    let t = ((r - 0.35) / 0.6).clamp(0.0, 1.0);
    1.0 - amount * t * t * (3.0 - 2.0 * t)
}

// --- speed blur -------------------------------------------------------------

/// A radial (zoom) blur out of a point on the screen: the feel of speed
/// (a racer's boost, the top speed), as Burnout and Mario Kart 8 do it.
/// Each pixel past [`inner_radius`](Self::inner_radius) is smeared along
/// the line to [`center`](Self::center), over [`strength`](Self::strength)
/// of its distance to it; the middle, where the player looks, stays sharp.
/// Set it each frame with [`crate::Renderer::set_speed_blur`].
///
/// Only the [`PostTier::Full`] tier draws it (the PC's native profile, the
/// Web), in the post stack's composite: in linear light before the
/// tonemap, the HUD untouched. The consoles and the PC's console profiles
/// ignore it.
///
/// The streak is sampled geometrically: the point a fraction `t` along it
/// is `center + (uv - center) * reach^t` ([`SpeedBlur::along`]), so the
/// renderer's two passes tile it exactly: a half-resolution gather over
/// the first `1/COMPOSITE_TAPS` of a full-strength streak from every
/// pixel ([`SpeedBlur::gather_reach`]), then the composite's taps of that,
/// one per sub-streak. In the feather, where streaks are shorter, the
/// sub-streaks overlap a little instead of leaving gaps.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SpeedBlur {
    /// The vanishing point, in screen UVs of the 3D (0..1; x right, y
    /// down): streaks point at it. Usually the middle, or the horizon
    /// ahead of the player.
    pub center: Vec2,
    /// How long a streak is, as a fraction of the pixel's distance to the
    /// center, 0..=1 (0: off, nothing runs; 1: almost to the center).
    pub strength: f32,
    /// The sharp zone around the center, in the vignette's measure: 0 at
    /// the center, 1 in a corner of the screen when the center is the
    /// middle (an ellipse with the screen's aspect).
    pub inner_radius: f32,
    /// The width of the soft edge past `inner_radius` over which the
    /// streaks grow from nothing to their full length (same measure).
    pub feather: f32,
}

impl SpeedBlur {
    /// Off (strength 0), centered, sharp out to a quarter of the way to
    /// the corners, full streaks past 0.6.
    pub const DEFAULT: SpeedBlur = SpeedBlur {
        center: Vec2::new(0.5, 0.5),
        strength: 0.0,
        inner_radius: 0.25,
        feather: 0.35,
    };

    /// Taps per pixel of the half-resolution gather (each covers one
    /// `1/COMPOSITE_TAPS` of a streak).
    pub const GATHER_TAPS: u32 = 12;
    /// Taps per pixel of the composite (each one gathered sub-streak):
    /// `GATHER_TAPS * COMPOSITE_TAPS` samples per streak in all.
    pub const COMPOSITE_TAPS: u32 = 8;
    /// The shortest [`reach`](Self::reach): a streak stops short of the
    /// center, where every pixel's streak would meet.
    pub const MIN_REACH: f32 = 0.05;
    /// Streaks shorter than this (pixels of the picture) fade from the
    /// sharp pixel into the blurred one ([`Self::fade`]): the half-size
    /// gather is a little soft, and a streak of a pixel or two must not
    /// show it as a step. Longer streaks are all blur: the feather grows
    /// the streak's length, never its opacity (a cross-fade of a sharp
    /// and a blurred picture reads as a ghost of every edge).
    pub const FADE_PIXELS: f32 = 4.0;

    /// [`SpeedBlur::DEFAULT`] at `strength`.
    pub const fn new(strength: f32) -> SpeedBlur {
        SpeedBlur {
            strength,
            ..SpeedBlur::DEFAULT
        }
    }

    /// Every field in its valid range: the center within a screen of the
    /// picture, strength 0..=1, the radius 0..=2, the feather 0.01..=2
    /// (NaN takes the default; a NaN strength is off).
    pub fn clamped(&self) -> SpeedBlur {
        let fix = |v: f32, lo: f32, hi: f32, def: f32| {
            if v.is_nan() { def } else { v.clamp(lo, hi) }
        };
        let d = SpeedBlur::DEFAULT;
        SpeedBlur {
            center: Vec2::new(
                fix(self.center.x, -1.0, 2.0, d.center.x),
                fix(self.center.y, -1.0, 2.0, d.center.y),
            ),
            strength: fix(self.strength, 0.0, 1.0, 0.0),
            inner_radius: fix(self.inner_radius, 0.0, 2.0, d.inner_radius),
            feather: fix(self.feather, 0.01, 2.0, d.feather),
        }
    }

    /// Whether it changes the picture (a strength above 0).
    pub fn is_on(&self) -> bool {
        self.strength > 0.0
    }

    /// How much of its streak the pixel at `uv` gets, 0..=1: 0 inside the
    /// inner radius, 1 past the feather, smoothstep between.
    pub fn weight(&self, uv: Vec2) -> f32 {
        let dx = (uv.x - self.center.x) * 2.0;
        let dy = (uv.y - self.center.y) * 2.0;
        let r = crate::math3d::sqrt((dx * dx + dy * dy) * 0.5);
        let t = ((r - self.inner_radius) / self.feather).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// Where the streak of the pixel at `uv` ends, as a scale toward the
    /// center: `1 - strength * weight`, never under [`Self::MIN_REACH`].
    pub fn reach(&self, uv: Vec2) -> f32 {
        (1.0 - self.strength * self.weight(uv)).max(SpeedBlur::MIN_REACH)
    }

    /// The reach of the renderer's gather: the first `1/COMPOSITE_TAPS`
    /// (in scale) of a full-strength streak, the same for every pixel.
    pub fn gather_reach(&self) -> f32 {
        pow(
            (1.0 - self.strength).max(SpeedBlur::MIN_REACH),
            1.0 / SpeedBlur::COMPOSITE_TAPS as f32,
        )
    }

    /// The point a fraction `t` (0..=1) along the streak of the pixel at
    /// `uv`, which ends at `reach` ([`Self::reach`]).
    pub fn along(&self, uv: Vec2, reach: f32, t: f32) -> Vec2 {
        let s = pow(reach, t);
        Vec2::new(
            self.center.x + (uv.x - self.center.x) * s,
            self.center.y + (uv.y - self.center.y) * s,
        )
    }

    /// The reference blur of one pixel: the mean light of `taps` points
    /// spread evenly along its streak (the middle of each step), read
    /// through `sample` (a display value at a UV), returned as a display
    /// value. Inside the inner radius, the pixel itself. Light is the
    /// square of the display value (gamma 2, within a few levels of sRGB's
    /// decode on a blend and exact on a flat color): the GPU pays one
    /// multiply a tap for it instead of three `pow`.
    pub fn gather(
        &self,
        uv: Vec2,
        taps: u32,
        mut sample: impl FnMut(Vec2) -> [f32; 3],
    ) -> [f32; 3] {
        let reach = self.reach(uv);
        if reach >= 1.0 || taps == 0 {
            return sample(uv);
        }
        let mut sum = [0.0f32; 3];
        for i in 0..taps {
            let t = (i as f32 + 0.5) / taps as f32;
            let c = sample(self.along(uv, reach, t));
            for k in 0..3 {
                let v = c[k].max(0.0);
                sum[k] += v * v;
            }
        }
        sum.map(|l| crate::math3d::sqrt(l / taps as f32))
    }

    /// How much of the blurred value the pixel at `uv` takes in a
    /// `size`-pixel picture: its streak's length over
    /// [`Self::FADE_PIXELS`], at most 1 (0 inside the inner radius).
    pub fn fade(&self, uv: Vec2, size: Vec2) -> f32 {
        let dx = (uv.x - self.center.x) * size.x;
        let dy = (uv.y - self.center.y) * size.y;
        let len = (1.0 - self.reach(uv)) * crate::math3d::sqrt(dx * dx + dy * dy);
        (len / SpeedBlur::FADE_PIXELS).clamp(0.0, 1.0)
    }

    /// The pixel the composite writes: the sharp value faded into the
    /// blurred one by [`Self::fade`] (both display values).
    pub fn mix(&self, uv: Vec2, size: Vec2, sharp: [f32; 3], blurred: [f32; 3]) -> [f32; 3] {
        let m = self.fade(uv, size);
        [0, 1, 2].map(|k| sharp[k] + (blurred[k] - sharp[k]) * m)
    }
}

impl Default for SpeedBlur {
    fn default() -> Self {
        SpeedBlur::DEFAULT
    }
}

// --- LUTs -----------------------------------------------------------------

/// The magic of a baked LUT blob.
pub const TLUT_MAGIC: [u8; 4] = *b"TLUT";
/// Its format version.
pub const TLUT_VERSION: u8 = 1;
/// Header bytes before the texels.
pub const TLUT_HEADER: usize = 8;
/// Sizes a LUT may have (texels per axis).
pub const LUT_MIN: u8 = 2;
pub const LUT_MAX: u8 = 64;

/// A parsed `TLUT` blob: `size`³ RGB8 texels, red fastest, then green,
/// then blue (the authored strip: `size` slices of blue side by side, red
/// along x, green down y).
#[derive(Clone, Copy, Debug)]
pub struct Lut<'a> {
    size: u8,
    texels: &'a [u8],
}

/// Why a `TLUT` blob doesn't parse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LutError {
    Magic,
    Version(u8),
    Size(u8),
    Length { want: usize, got: usize },
}

impl<'a> Lut<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Lut<'a>, LutError> {
        if bytes.len() < TLUT_HEADER || bytes[..4] != TLUT_MAGIC {
            return Err(LutError::Magic);
        }
        if bytes[4] != TLUT_VERSION {
            return Err(LutError::Version(bytes[4]));
        }
        let size = bytes[5];
        if !(LUT_MIN..=LUT_MAX).contains(&size) {
            return Err(LutError::Size(size));
        }
        let want = TLUT_HEADER + Self::texel_bytes(size);
        if bytes.len() != want {
            return Err(LutError::Length {
                want,
                got: bytes.len(),
            });
        }
        Ok(Lut {
            size,
            texels: &bytes[TLUT_HEADER..],
        })
    }

    /// Bytes of `size`³ RGB8 texels.
    pub const fn texel_bytes(size: u8) -> usize {
        let n = size as usize;
        n * n * n * 3
    }

    /// The header of a `size` LUT (the texels follow).
    pub const fn header(size: u8) -> [u8; TLUT_HEADER] {
        [b'T', b'L', b'U', b'T', TLUT_VERSION, size, 0, 0]
    }

    /// The neutral LUT's texel at grid point `i` of `size`: no change.
    pub const fn neutral(size: u8, i: u32) -> u8 {
        let n = size as u32 - 1;
        ((i * 255 + n / 2) / n) as u8
    }

    pub fn size(&self) -> u8 {
        self.size
    }

    /// The RGB8 texels (red fastest, then green, then blue).
    pub fn texels(&self) -> &'a [u8] {
        self.texels
    }

    /// The texel at grid point (r, g, b).
    pub fn texel(&self, r: u32, g: u32, b: u32) -> [u8; 3] {
        let n = self.size as u32;
        let i = (((b * n + g) * n + r) * 3) as usize;
        [self.texels[i], self.texels[i + 1], self.texels[i + 2]]
    }

    /// Grade a display color (0..=1 per channel) by trilinear filtering,
    /// as the GPU samples it.
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let n = self.size as u32 - 1;
        let axis = |v: f32| {
            let x = v.clamp(0.0, 1.0) * n as f32;
            let i = (x as u32).min(n.saturating_sub(1));
            (i, x - i as f32)
        };
        let (r0, fr) = axis(c[0]);
        let (g0, fg) = axis(c[1]);
        let (b0, fb) = axis(c[2]);
        let mut out = [0.0f32; 3];
        for (db, wb) in [(0, 1.0 - fb), (1, fb)] {
            for (dg, wg) in [(0, 1.0 - fg), (1, fg)] {
                for (dr, wr) in [(0, 1.0 - fr), (1, fr)] {
                    let w = wr * wg * wb;
                    if w == 0.0 {
                        continue;
                    }
                    let t = self.texel(r0 + dr, g0 + dg, b0 + db);
                    for k in 0..3 {
                        out[k] += w * t[k] as f32;
                    }
                }
            }
        }
        out.map(|v| v / 255.0)
    }

    /// Grade an 8-bit color (alpha kept), rounded: what the console bake
    /// writes into vertex colors and scene colors.
    pub fn apply_color(&self, c: Color) -> Color {
        let g = self.apply([c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0]);
        let q = |v: f32| (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        Color::rgba(q(g[0]), q(g[1]), q(g[2]), c.a)
    }
}

// --- glow, the consoles' bloom ---------------------------------------------

/// Where a [`Glow`] halo draws.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum GlowMode {
    /// Only below [`PostTier::Full`]: the PC and the Web bloom instead.
    #[default]
    Baked,
    /// On every target (the PC adds it to its bloom).
    Always,
}

/// A halo around an emissive object: a camera-facing billboard of a soft
/// sprite (the engine's `sprites/glow`, or the game's), tinted, z-tested
/// and fogged like the 3D around it. The cheap bloom of the N64 and 3DS.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Glow {
    pub sprite: SpriteId,
    /// Half the quad's side, world units.
    pub radius: f32,
    /// Multiplies the sprite; alpha sets the strength.
    pub color: Color,
    pub mode: GlowMode,
}

impl Glow {
    /// The engine's halo sprite (`assets/shared/sprites/glow.png`).
    pub const SPRITE: SpriteId = SpriteId(crate::asset::asset_id("sprites/glow"));

    pub const fn new(radius: f32, color: Color) -> Glow {
        Glow {
            sprite: Glow::SPRITE,
            radius,
            color,
            mode: GlowMode::Baked,
        }
    }

    /// Whether the halo draws on a target with `tier`.
    pub fn draws_on(&self, tier: PostTier) -> bool {
        self.mode == GlowMode::Always || tier != PostTier::Full
    }
}

/// Draw `glow` centered at `pos` where its mode says (one billboard).
/// Returns whether it drew.
pub fn draw_glow(r: &mut dyn Renderer, glow: &Glow, pos: Vec3) -> bool {
    if !glow.draws_on(r.caps().post_tier) || glow.radius <= 0.0 || glow.color.a == 0 {
        return false;
    }
    let side = glow.radius * 2.0;
    r.draw_billboard(
        glow.sprite,
        pos,
        &BillboardParams {
            size: Vec2::new(side, side),
            tint: glow.color,
            ..BillboardParams::DEFAULT
        },
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore = "Miri adds error to std's float functions on purpose")]
    fn log2_exp2_and_pow_match_std() {
        for x in [1e-6f32, 0.001, 0.18, 0.5, 1.0, 1.5, 2.0, 7.3, 100.0, 4000.0] {
            assert!((log2(x) - x.log2()).abs() < 2e-6, "log2 {x}");
        }
        for x in [-20.0f32, -3.3, -0.5, 0.0, 0.25, 1.0, 5.7, 20.0] {
            let want = x.exp2();
            assert!((exp2(x) - want).abs() / want < 3e-6, "exp2 {x}");
        }
        for (x, y) in [(0.5f32, 2.4f32), (0.9, 1.0 / 2.4), (3.0, 2.2), (0.01, 0.5)] {
            let want = x.powf(y);
            assert!((pow(x, y) - want).abs() / want < 1e-5, "pow {x} {y}");
        }
        assert_eq!(pow(0.0, 2.2), 0.0);
    }

    #[test]
    fn srgb_round_trips_and_extends_past_one() {
        for i in 0..=255u32 {
            let v = i as f32 / 255.0;
            let l = decode(v);
            assert!(
                (l - crate::access::color::to_linear(i as u8)).abs() < 2e-5,
                "{i}"
            );
            assert!((encode(l) - v).abs() < 2e-5, "{i}");
        }
        assert!(decode(1.5) > 2.5 && decode(1.5) < 2.7);
        assert!((encode(decode(1.5)) - 1.5).abs() < 1e-5);
    }

    #[test]
    fn tonemappers_match_their_reference_values() {
        let one = |t, x: f32| tonemap(t, [x, x, x])[0];
        // Clamp.
        assert_eq!(one(Tonemap::None, 0.4), 0.4);
        assert_eq!(one(Tonemap::None, 3.0), 1.0);
        // Reinhard: x / (1 + x).
        assert!((one(Tonemap::Reinhard, 1.0) - 0.5).abs() < 1e-6);
        assert!((one(Tonemap::Reinhard, 3.0) - 0.75).abs() < 1e-6);
        // Narkowicz ACES: (2.51 + 0.03) / (2.43 + 0.59 + 0.14) at 1.
        assert!((one(Tonemap::Aces, 1.0) - 2.54 / 3.16).abs() < 1e-6);
        assert_eq!(one(Tonemap::Aces, 0.0), 0.03 * 0.0);
        assert_eq!(one(Tonemap::Aces, 1000.0), 1.0);
        // AgX: middle gray 0.18 lands near 0.2 linear (≈ 0.48 sRGB, the
        // reference implementation's), black near black, highlights
        // roll off below white and grays stay gray.
        let g = tonemap(Tonemap::AgX, [0.18; 3]);
        assert!(
            (g[0] - g[1]).abs() < 1e-4 && (g[1] - g[2]).abs() < 1e-4,
            "{g:?}"
        );
        assert!((encode(g[0]) - 0.48).abs() < 0.03, "{}", encode(g[0]));
        assert!(one(Tonemap::AgX, 0.0) < 0.002);
        assert!(one(Tonemap::AgX, 16.0) > 0.95);
        // Every curve is monotonic and stays in 0..=1.
        for t in Tonemap::ALL {
            let mut last = -1.0;
            for i in 0..200 {
                let v = one(t, i as f32 * 0.05);
                assert!((0.0..=1.0).contains(&v), "{t:?}");
                assert!(v >= last - 1e-6, "{t:?} at {i}");
                last = v;
            }
        }
        // AgX desaturates a bright saturated red toward white; the clamp
        // keeps it pure red.
        let red = tonemap(Tonemap::AgX, [8.0, 0.2, 0.2]);
        assert!(red[1] > 0.3 && red[2] > 0.3, "{red:?}");
        assert!(tonemap(Tonemap::None, [8.0, 0.2, 0.2])[1] < 0.21);
    }

    #[test]
    fn the_default_settings_change_nothing() {
        for i in 0..=255u32 {
            let v = i as f32 / 255.0;
            let t = tone(&PostSettings::DEFAULT, [v, v * 0.5, 1.0 - v]);
            assert!((t[0] - v).abs() < 0.5 / 255.0, "{i}");
            assert!((t[1] - v * 0.5).abs() < 0.5 / 255.0, "{i}");
            assert!((t[2] - (1.0 - v)).abs() < 0.5 / 255.0, "{i}");
        }
        // Exposure is linear light: two stops on 0.25 linear is 1.0.
        let e = PostSettings {
            exposure: 4.0,
            ..PostSettings::DEFAULT
        };
        let v = encode(0.25);
        assert!((tone(&e, [v; 3])[0] - 1.0).abs() < 1e-4);
    }

    fn neutral(size: u8) -> std::vec::Vec<u8> {
        let mut v = Lut::header(size).to_vec();
        for b in 0..size as u32 {
            for g in 0..size as u32 {
                for r in 0..size as u32 {
                    v.extend([
                        Lut::neutral(size, r),
                        Lut::neutral(size, g),
                        Lut::neutral(size, b),
                    ]);
                }
            }
        }
        v
    }

    #[test]
    fn a_neutral_lut_is_the_identity_within_one_step() {
        for size in [2u8, 16, 17, 32, 33] {
            let bytes = neutral(size);
            let lut = Lut::parse(&bytes).unwrap();
            for r in (0..=255u8).step_by(3) {
                for g in (0..=255u8).step_by(17) {
                    let c = Color::rgba(r, g, 255 - r, 40);
                    let out = lut.apply_color(c);
                    assert!(out.r.abs_diff(c.r) <= 1, "size {size} {c:?} {out:?}");
                    assert!(out.g.abs_diff(c.g) <= 1, "size {size} {c:?} {out:?}");
                    assert!(out.b.abs_diff(c.b) <= 1, "size {size} {c:?} {out:?}");
                    assert_eq!(out.a, 40);
                }
            }
        }
    }

    #[test]
    fn luts_interpolate_trilinearly_and_reject_bad_blobs() {
        // A 2³ LUT that inverts: trilinear between its corners is exact.
        let mut b = Lut::header(2).to_vec();
        for i in 0..8u32 {
            let (r, g, bl) = (i & 1, (i >> 1) & 1, (i >> 2) & 1);
            b.extend([
                255 * (1 - r) as u8,
                255 * (1 - g) as u8,
                255 * (1 - bl) as u8,
            ]);
        }
        let lut = Lut::parse(&b).unwrap();
        let out = lut.apply([0.25, 0.5, 1.0]);
        assert!((out[0] - 0.75).abs() < 1e-6 && (out[1] - 0.5).abs() < 1e-6 && out[2] < 1e-6);
        assert_eq!(lut.texel(1, 0, 1), [0, 255, 0]);

        assert_eq!(Lut::parse(b"nope").unwrap_err(), LutError::Magic);
        let mut v = b.clone();
        v[4] = 9;
        assert_eq!(Lut::parse(&v).unwrap_err(), LutError::Version(9));
        let mut v = b.clone();
        v[5] = 1;
        assert_eq!(Lut::parse(&v).unwrap_err(), LutError::Size(1));
        let mut v = b.clone();
        v.pop();
        assert_eq!(
            Lut::parse(&v).unwrap_err(),
            LutError::Length { want: 32, got: 31 }
        );
    }

    #[test]
    fn settings_clamp_and_bloom_levels_follow_the_cap() {
        let wild = PostSettings {
            exposure: f32::NAN,
            vignette: 3.0,
            grain: -1.0,
            bloom: Some(Bloom {
                threshold: -1.0,
                intensity: 9.0,
                radius: f32::NAN,
            }),
            ..PostSettings::DEFAULT
        }
        .clamped();
        assert_eq!(wild.exposure, 1.0);
        assert_eq!((wild.vignette, wild.grain), (1.0, 0.0));
        let b = wild.bloom.unwrap();
        assert_eq!((b.threshold, b.intensity, b.radius), (0.0, 4.0, 0.6));
        assert_eq!(PostSettings::DEFAULT.clamped(), PostSettings::DEFAULT);
        let full = Bloom {
            radius: 1.0,
            ..Bloom::DEFAULT
        };
        assert_eq!(full.levels(6), 6);
        assert_eq!(full.levels(3), 3);
        assert_eq!(
            Bloom {
                radius: 0.0,
                ..full
            }
            .levels(6),
            1
        );
        assert_eq!(Tonemap::parse("AgX"), Some(Tonemap::AgX));
        assert_eq!(Tonemap::parse("filmic"), None);
    }

    #[test]
    fn the_vignette_spares_the_center_and_darkens_the_corners() {
        assert_eq!(vignette(1.0, Vec2::new(0.5, 0.5)), 1.0);
        assert!((vignette(0.4, Vec2::new(0.0, 0.0)) - 0.6).abs() < 1e-3);
        assert_eq!(vignette(0.0, Vec2::new(0.0, 1.0)), 1.0);
        assert!(vignette(0.5, Vec2::new(0.1, 0.5)) > vignette(0.5, Vec2::new(0.0, 0.0)));
    }

    #[test]
    fn speed_blur_is_off_by_default_and_clamps_every_field() {
        assert!(!SpeedBlur::DEFAULT.is_on());
        assert!(!SpeedBlur::default().clamped().is_on());
        assert!(SpeedBlur::new(0.3).is_on());
        let wild = SpeedBlur {
            center: Vec2::new(f32::NAN, 9.0),
            strength: 3.0,
            inner_radius: -1.0,
            feather: 0.0,
        }
        .clamped();
        assert_eq!(wild.center, Vec2::new(0.5, 2.0));
        assert_eq!(wild.strength, 1.0);
        assert_eq!(wild.inner_radius, 0.0);
        assert_eq!(wild.feather, 0.01);
        let nan = SpeedBlur {
            strength: f32::NAN,
            inner_radius: f32::NAN,
            feather: f32::NAN,
            ..SpeedBlur::DEFAULT
        }
        .clamped();
        assert!(!nan.is_on(), "a NaN strength is off");
        assert_eq!(nan.inner_radius, SpeedBlur::DEFAULT.inner_radius);
        assert_eq!(nan.feather, SpeedBlur::DEFAULT.feather);
        // Valid settings pass unchanged.
        let ok = SpeedBlur {
            center: Vec2::new(0.5, 0.4),
            strength: 0.3,
            inner_radius: 0.2,
            feather: 0.5,
        };
        assert_eq!(ok.clamped(), ok);
        assert!((ok.reach(Vec2::new(0.5, 0.4)) - 1.0).abs() < 1e-6);
        // Even a full strength stops short of the center.
        assert_eq!(
            SpeedBlur::new(1.0).reach(Vec2::new(0.0, 0.0)),
            SpeedBlur::MIN_REACH
        );
    }

    #[test]
    fn speed_blur_spares_the_inner_radius_and_grows_to_full_past_the_feather() {
        let b = SpeedBlur {
            center: Vec2::new(0.5, 0.45),
            ..SpeedBlur::new(0.4)
        };
        assert_eq!(b.weight(b.center), 0.0);
        // Out along a ray to the right: 0 inside the radius, rising, then 1.
        let mut last = 0.0;
        for i in 0..=50 {
            let uv = Vec2::new(0.5 + i as f32 * 0.01, 0.45);
            let w = b.weight(uv);
            // r = |2 dx| / sqrt(2).
            let r = (uv.x - 0.5) * 2.0 / core::f32::consts::SQRT_2;
            if r <= b.inner_radius {
                assert_eq!(w, 0.0, "inside the radius at {r}");
                assert_eq!(b.reach(uv), 1.0);
            }
            if r >= b.inner_radius + b.feather {
                assert_eq!(w, 1.0, "past the feather at {r}");
            }
            assert!(w >= last, "monotonic");
            last = w;
        }
        // The ellipse has the screen's aspect: a corner is r = 1.
        let c = SpeedBlur {
            inner_radius: 0.98,
            feather: 0.01,
            ..SpeedBlur::new(0.5)
        };
        assert_eq!(c.weight(Vec2::new(0.0, 0.0)), 1.0);
        assert_eq!(c.weight(Vec2::new(0.0, 0.5)), 0.0);
    }

    #[test]
    fn a_streak_runs_from_the_pixel_toward_the_center_over_strength_of_the_way() {
        let b = SpeedBlur {
            center: Vec2::new(0.4, 0.6),
            inner_radius: 0.0,
            feather: 0.01,
            ..SpeedBlur::new(0.3)
        };
        for uv in [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.2),
            Vec2::new(0.9, 0.95),
        ] {
            let reach = b.reach(uv);
            assert!((reach - 0.7).abs() < 1e-6);
            let start = b.along(uv, reach, 0.0);
            let end = b.along(uv, reach, 1.0);
            assert!((start.x - uv.x).abs() < 1e-6 && (start.y - uv.y).abs() < 1e-6);
            // 30% of the way to the center, on the line through it.
            let want = Vec2::new(
                uv.x + (b.center.x - uv.x) * 0.3,
                uv.y + (b.center.y - uv.y) * 0.3,
            );
            assert!(
                (end.x - want.x).abs() < 1e-5 && (end.y - want.y).abs() < 1e-5,
                "{uv:?}: {end:?} vs {want:?}"
            );
            for t in [0.25f32, 0.5, 0.75] {
                let p = b.along(uv, reach, t);
                let cross = (p.x - b.center.x) * (uv.y - b.center.y)
                    - (p.y - b.center.y) * (uv.x - b.center.x);
                assert!(cross.abs() < 1e-6, "on the line to the center");
            }
        }
    }

    /// The renderer's two passes: the composite's tap `k` reads the
    /// gather at the point `k / COMPOSITE_TAPS` along the streak, which
    /// gathered the first `1 / COMPOSITE_TAPS` of a full-strength streak.
    /// Where the streaks are full length, the taps are exactly the
    /// one-pass reference's `GATHER_TAPS * COMPOSITE_TAPS`, at every
    /// strength; in the feather they overlap past the shorter streak's
    /// end, by less than one sub-streak, and never cross the center.
    #[test]
    fn the_two_passes_tile_the_streak_like_one_long_gather() {
        let (n1, n2) = (SpeedBlur::GATHER_TAPS, SpeedBlur::COMPOSITE_TAPS);
        let two_pass = |b: &SpeedBlur, uv: Vec2| {
            let reach = b.reach(uv);
            let mut taps = std::vec::Vec::new();
            for k in 0..n2 {
                let q = b.along(uv, reach, k as f32 / n2 as f32);
                let sub = b.gather_reach();
                for j in 0..n1 {
                    taps.push(b.along(q, sub, (j as f32 + 0.5) / n1 as f32));
                }
            }
            taps
        };
        let dist = |p: Vec2, c: Vec2| ((p.x - c.x).powi(2) + (p.y - c.y).powi(2)).sqrt();
        for strength in [0.1f32, 0.3, 0.8, 1.0] {
            let b = SpeedBlur {
                center: Vec2::new(0.5, 0.42),
                inner_radius: 0.05,
                feather: 0.1,
                ..SpeedBlur::new(strength)
            };
            // From a corner, past the feather: the very taps of one long
            // gather.
            let uv = Vec2::new(0.02, 0.97);
            let reach = b.reach(uv);
            assert_eq!(b.weight(uv), 1.0);
            let n = n1 * n2;
            for (i, p) in two_pass(&b, uv).into_iter().enumerate() {
                let k = i as u32 / n1;
                let j = i as u32 % n1;
                let t = (k as f32 + (j as f32 + 0.5) / n1 as f32) / n2 as f32;
                let want = b.along(uv, reach, t);
                assert!(
                    dist(p, want) < 2e-5,
                    "strength {strength}, tap {i} of {n}: {p:?} vs {want:?}"
                );
            }
            // In the feather: from the pixel to less than a sub-streak past
            // its streak's end, on the pixel's side of the center.
            for x in [0.54f32, 0.56, 0.58, 0.6] {
                let uv = Vec2::new(x, 0.42);
                assert!(b.weight(uv) > 0.0 && b.weight(uv) < 1.0);
                let reach = b.reach(uv);
                let end = b.along(uv, reach, 1.0);
                let past = b.along(end, b.gather_reach(), 1.0);
                for p in two_pass(&b, uv) {
                    assert!(p.x > b.center.x, "the center not crossed");
                    assert!(dist(p, b.center) <= dist(uv, b.center) + 1e-6);
                    assert!(dist(p, b.center) >= dist(past, b.center) - 1e-6);
                }
            }
        }
    }

    #[test]
    fn the_reference_gather_averages_light_along_the_streak() {
        let b = SpeedBlur {
            inner_radius: 0.2,
            feather: 0.1,
            ..SpeedBlur::new(0.5)
        };
        // A flat picture stays flat.
        let flat = b.gather(Vec2::new(0.9, 0.9), 64, |_| [0.3, 0.6, 0.9]);
        for (got, want) in flat.iter().zip([0.3, 0.6, 0.9]) {
            assert!((got - want).abs() < 1e-4, "{flat:?}");
        }
        // Inside the radius: the pixel itself, whatever is around it.
        let inside = b.gather(Vec2::new(0.55, 0.5), 64, |p| {
            if p.x == 0.55 { [0.2; 3] } else { [1.0; 3] }
        });
        assert_eq!(inside, [0.2; 3]);
        // A white half toward the center and black outside: the pixel just
        // outside the edge (its streak half in the white) mixes them in
        // linear light, and one whose streak never reaches it stays black.
        let edge = |p: Vec2| if p.x < 0.8 { [1.0; 3] } else { [0.0; 3] };
        let near = b.gather(Vec2::new(0.9, 0.5), 256, edge)[0];
        // The streak runs 0.9 → 0.7 (reach 0.5); x = 0.5 + 0.4 · 0.5^t
        // passes 0.8 at t = log2(0.75) / log2(0.5): the rest is white
        // (light, the square of the display value).
        let lit = crate::math3d::sqrt(1.0 - log2(0.75) / log2(0.5));
        // Within a few levels of the sRGB blend.
        assert!((lit - encode(1.0 - log2(0.75) / log2(0.5))).abs() < 0.03);
        assert!((near - lit).abs() < 0.01, "{near} vs {lit}");
        let short = SpeedBlur { strength: 0.2, ..b };
        assert_eq!(
            short.gather(Vec2::new(0.99, 0.5), 256, edge),
            [0.0; 3],
            "the streak 0.99 → 0.89 never reaches the white"
        );
        // The composite takes the blurred value once the streak is a few
        // pixels long, fading in from the sharp one over the first ones.
        let sharp = [0.1; 3];
        let blurred = [0.9; 3];
        let size = Vec2::new(640.0, 480.0);
        assert_eq!(b.mix(Vec2::new(0.5, 0.5), size, sharp, blurred), sharp);
        assert_eq!(b.mix(Vec2::new(0.0, 0.0), size, sharp, blurred), blurred);
        let mut last = 0.0;
        for i in 0..200 {
            let uv = Vec2::new(0.5 + i as f32 * 0.0025, 0.5);
            let m = b.fade(uv, size);
            assert!(m >= last && m <= 1.0);
            if m > 0.0 && m < 1.0 {
                // Partly blurred only while the streak is under 4 pixels.
                let len = (1.0 - b.reach(uv)) * (uv.x - 0.5) * size.x;
                assert!(len < SpeedBlur::FADE_PIXELS + 1e-3, "{len}");
            }
            last = m;
        }
        assert_eq!(last, 1.0);
    }

    /// A size-2 LUT that maps every color to `to` (a flat look).
    fn flat(to: [u8; 3]) -> std::vec::Vec<u8> {
        let mut v = Lut::header(2).to_vec();
        for _ in 0..8 {
            v.extend(to);
        }
        v
    }

    /// `grade_mix` (Bongo-Kart #181): no second LUT or a zero weight is the
    /// first grading alone; a weight walks linearly from the first grading
    /// to the second; without a first LUT it mixes from the ungraded color.
    #[test]
    fn two_luts_mix_by_the_weight() {
        let (ba, bb) = (flat([255, 0, 0]), flat([0, 0, 255]));
        let (a, b) = (Lut::parse(&ba).unwrap(), Lut::parse(&bb).unwrap());
        let c = [0.25, 0.5, 0.75];
        assert_eq!(grade_mix(Some(&a), None, 0.7, c), [1.0, 0.0, 0.0]);
        assert_eq!(grade_mix(Some(&a), Some(&b), 0.0, c), [1.0, 0.0, 0.0]);
        assert_eq!(grade_mix(Some(&a), Some(&b), 1.0, c), [0.0, 0.0, 1.0]);
        let half = grade_mix(Some(&a), Some(&b), 0.5, c);
        assert!((half[0] - 0.5).abs() < 1e-6 && (half[2] - 0.5).abs() < 1e-6);
        assert_eq!(grade_mix(None, None, 0.5, c), c);
        let from_plain = grade_mix(None, Some(&b), 0.5, c);
        assert!((from_plain[0] - 0.125).abs() < 1e-6, "{from_plain:?}");
        assert!((from_plain[2] - 0.875).abs() < 1e-6, "{from_plain:?}");
        // The weight is clamped with the rest of the settings.
        let wild = PostSettings {
            grading_mix: 3.0,
            ..PostSettings::DEFAULT
        };
        assert_eq!(wild.clamped().grading_mix, 1.0);
        let nan = PostSettings {
            grading_mix: f32::NAN,
            ..PostSettings::DEFAULT
        };
        assert_eq!(nan.clamped().grading_mix, 0.0);
    }

    #[test]
    fn glows_draw_below_the_full_tier_unless_always() {
        let g = Glow::new(0.5, Color::WHITE);
        assert!(g.draws_on(PostTier::Baked));
        assert!(!g.draws_on(PostTier::Full));
        let a = Glow {
            mode: GlowMode::Always,
            ..g
        };
        assert!(a.draws_on(PostTier::Full));
        assert_eq!(
            LutId::of("luts/dusk").blob().0,
            crate::asset_id("luts/dusk")
        );
    }
}
