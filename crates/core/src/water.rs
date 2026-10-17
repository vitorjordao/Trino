//! Water surfaces: swell, ripples, flowing textures, Fresnel and glint
//! (ADR 0015).
//!
//! A water surface is a material, not a shader: [`Water`] in
//! [`crate::ModelParams::water`] says how the surface moves (swell height
//! and frequency, texture flow, how strongly ripples push it), and each
//! target draws it the way its hardware affords ([`WaterTier`], from
//! [`crate::Caps::water_tier`]):
//!
//! - [`WaterTier::PerPixel`] (PC native): the core displaces the vertices;
//!   the fragment shader evaluates the swell and the ripples' normals per
//!   pixel, Schlick Fresnel, a Blinn-Phong sun glint, and soft foam where
//!   the water meets the ground (scene depth).
//! - [`WaterTier::VertexProgram`] (3DS): the PICA200's vertex unit displaces
//!   and shades the mesh (`water.v.pica`); the TexEnv stages blend the two
//!   flowing layers.
//! - [`WaterTier::PerVertex`] (N64, and the PC's console profiles): the
//!   CPU T&L displaces and shades per vertex; the RDP's 2-cycle combiner
//!   blends the two flowing layers (`TILE0`, `TILE1`).
//!
//! Every tier evaluates the same functions ([`WaveSet::sample`],
//! [`ripple_sample`], [`shade`]), so the surface has the same shape
//! everywhere; the PC adds detail between the vertices. Ripples are frame
//! state ([`crate::Renderer::set_water`]): a [`RippleBuffer`] in the game
//! spawns them where things touch the water and ages them.
//!
//! All of it is deterministic `f32` with the module's own sine
//! ([`sin_turns`]: no range reduction by division, a handful of multiplies),
//! identical on PC, N64 and 3DS.

use crate::math::{Color, Vec2, Vec3};
use crate::math3d::to_i32;

/// Most ripples a target evaluates (the 3DS vertex program's uniform
/// table; [`crate::Caps::max_ripples`]).
pub const MAX_RIPPLES: usize = 4;

/// Speed a ripple ring grows at, world units per second.
pub const RIPPLE_SPEED: f32 = 1.6;
/// Distance between a ripple's crests, world units.
pub const RIPPLE_WAVELENGTH: f32 = 0.55;
/// Half-width of the ring band, world units: the surface moves only
/// within this distance of the ring's radius.
pub const RIPPLE_WIDTH: f32 = 0.7;
/// Seconds a ripple lasts; its height fades to zero over them.
pub const RIPPLE_LIFE: f32 = 2.2;

/// Gravity of the swell's dispersion (deep water: a crest at spatial
/// frequency `k` cycles per unit moves at `sqrt(g k / 2π)` cycles per
/// second).
const GRAVITY: f32 = 9.81;
const TAU: f32 = core::f32::consts::TAU;

/// How a target draws water ([`crate::Caps::water_tier`], ADR 0015).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum WaterTier {
    /// Per-pixel normals, Fresnel, glint and depth foam (PC native).
    PerPixel,
    /// The GPU's vertex unit displaces and shades; a fixed-function
    /// combiner blends the layers (3DS: PICA200 + TexEnv).
    VertexProgram,
    /// The CPU T&L displaces and shades per vertex; a fixed-function
    /// combiner blends the layers (N64: RDP 2-cycle; the PC's console
    /// profiles).
    PerVertex,
}

/// A water surface ([`crate::ModelParams::water`]; `water:` in material
/// files). The mesh is a horizontal surface (a river ribbon, a pond): the
/// engine displaces it along world Y. Its texture flows at `flow_speed`
/// and its [`crate::ModelParams::detail`] layer, when it has one, at
/// `detail_flow`; the two are blended half and half (not multiplied, as a
/// plain detail layer is). Water is translucent where its vertex alpha (or
/// tint alpha) is below 255, and more opaque at grazing angles (Fresnel).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Water {
    /// Texture scroll, in repeats per second (u, v).
    pub flow_speed: Vec2,
    /// The detail layer's scroll, in its own repeats per second.
    pub detail_flow: Vec2,
    /// Height of the swell above and below the surface, world units.
    pub wave_amplitude: f32,
    /// Swell crests per world unit along the flow.
    pub wave_frequency: f32,
    /// How far ripples push this surface: their strength times this
    /// (0 ignores ripples).
    pub ripple_responsiveness: f32,
}

impl Water {
    /// A calm surface: a slow flow, a low swell, ripples at full strength.
    pub const DEFAULT: Water = Water {
        flow_speed: Vec2::new(0.0, 0.25),
        detail_flow: Vec2::new(0.06, 0.4),
        wave_amplitude: 0.05,
        wave_frequency: 0.5,
        ripple_responsiveness: 1.0,
    };

    /// The texture's and the detail layer's scroll at `time`, each wrapped
    /// to one repeat (`[0, 1)`): seamless, and exact however long the game
    /// runs.
    pub fn scroll(&self, time: f32) -> (Vec2, Vec2) {
        let f = |v: Vec2| Vec2::new(frac(v.x * time), frac(v.y * time));
        (f(self.flow_speed), f(self.detail_flow))
    }

    /// What the detail layer adds to its coordinates at `time`, for a
    /// layer tiling `scale` times per base repeat: the T&L already scrolled
    /// the UVs by the texture's flow, and the layer reads them times
    /// `scale`, so this is its own scroll minus that (wrapped to `[0, 1)`).
    /// The detail texel is at `uv * scale + offset` on every target (the
    /// N64's `TILE1` translate, the 3DS's texcoord1 uniform, the PC's
    /// water sets).
    pub fn detail_offset(&self, time: f32, scale: f32) -> Vec2 {
        let (base, detail) = self.scroll(time);
        Vec2::new(
            frac(detail.x - base.x * scale),
            frac(detail.y - base.y * scale),
        )
    }
}

impl Default for Water {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// One ripple ring ([`crate::Renderer::set_water`]): a disturbance where
/// something touched the water, spreading at [`RIPPLE_SPEED`] and fading
/// over [`RIPPLE_LIFE`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Ripple {
    /// World position of the impact on the water plane (x, z).
    pub pos: Vec2,
    /// Seconds since the impact.
    pub age: f32,
    /// Height of the first crest, world units (times the surface's
    /// [`Water::ripple_responsiveness`]).
    pub strength: f32,
}

impl Ripple {
    pub const NONE: Ripple = Ripple {
        pos: Vec2::new(0.0, 0.0),
        age: RIPPLE_LIFE,
        strength: 0.0,
    };

    /// Still moving the water?
    pub fn alive(&self) -> bool {
        self.age < RIPPLE_LIFE && self.strength != 0.0
    }

    /// The ring's radius now.
    pub fn radius(&self) -> f32 {
        self.age * RIPPLE_SPEED
    }

    /// How much is left of its height: `(1 - age / life)²`.
    pub fn decay(&self) -> f32 {
        let k = 1.0 - self.age / RIPPLE_LIFE;
        if k <= 0.0 { 0.0 } else { k * k }
    }
}

/// A game's ripples: at most `N` live ones (a new ripple replaces the
/// oldest when full), aged by [`RippleBuffer::update`]. No allocation.
/// Hand [`RippleBuffer::as_slice`] to [`crate::Renderer::set_water`] every
/// frame; targets use the first [`crate::Caps::max_ripples`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RippleBuffer<const N: usize> {
    ripples: [Ripple; N],
    len: usize,
}

impl<const N: usize> RippleBuffer<N> {
    pub const fn new() -> Self {
        RippleBuffer {
            ripples: [Ripple::NONE; N],
            len: 0,
        }
    }

    /// Start a ripple at world `(x, z)` = `pos` with crest height
    /// `strength`. Full: the oldest one makes room.
    pub fn spawn(&mut self, pos: Vec2, strength: f32) {
        if N == 0 {
            return;
        }
        let r = Ripple {
            pos,
            age: 0.0,
            strength,
        };
        if self.len < N {
            self.ripples[self.len] = r;
            self.len += 1;
        } else {
            // Kept oldest first: drop the head.
            self.ripples.copy_within(1.., 0);
            self.ripples[N - 1] = r;
        }
    }

    /// Age every ripple by `dt` seconds and drop the finished ones (order
    /// kept: oldest first).
    pub fn update(&mut self, dt: f32) {
        let mut n = 0;
        for i in 0..self.len {
            let mut r = self.ripples[i];
            r.age += dt;
            if r.alive() {
                self.ripples[n] = r;
                n += 1;
            }
        }
        self.len = n;
    }

    /// The live ripples, oldest first.
    pub fn as_slice(&self) -> &[Ripple] {
        &self.ripples[..self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl<const N: usize> Default for RippleBuffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// A renderer's water state for the frame ([`crate::Renderer::set_water`]):
/// the clock and the first [`MAX_RIPPLES`] live ripples.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WaterState {
    pub time: f32,
    ripples: [Ripple; MAX_RIPPLES],
    n: u8,
}

impl WaterState {
    pub const NONE: WaterState = WaterState {
        time: 0.0,
        ripples: [Ripple::NONE; MAX_RIPPLES],
        n: 0,
    };

    /// `time` and the first `cap` (at most [`MAX_RIPPLES`]) live ripples.
    pub fn new(time: f32, ripples: &[Ripple], cap: u8) -> WaterState {
        let mut s = WaterState { time, ..Self::NONE };
        let cap = (cap as usize).min(MAX_RIPPLES);
        for r in ripples.iter().filter(|r| r.alive()) {
            if s.n as usize == cap {
                break;
            }
            s.ripples[s.n as usize] = *r;
            s.n += 1;
        }
        s
    }

    /// The ripples this state evaluates.
    pub fn ripples(&self) -> &[Ripple] {
        &self.ripples[..self.n as usize]
    }
}

impl Default for WaterState {
    fn default() -> Self {
        Self::NONE
    }
}

/// `x - floor(x)`, in `[0, 1)`.
#[inline]
pub fn frac(x: f32) -> f32 {
    let i = to_i32(x) as f32;
    let f = if i > x { i - 1.0 } else { i };
    let r = x - f;
    if r >= 1.0 { 0.0 } else { r }
}

/// `sin(2π t)`: `t` in turns. The fractional part is the only range
/// reduction (no division), then the parabola + quartic refinement of
/// [`crate::math3d::sin`] (error ~1e-3). The water's own sine: dozens per
/// vertex on the N64, where `math3d::sin`'s `%` is a software `fmodf`.
#[inline]
pub fn sin_turns(t: f32) -> f32 {
    // u in [-1, 1): sin(πu).
    let u = 2.0 * frac(t + 0.5) - 1.0;
    let au = if u < 0.0 { -u } else { u };
    let y = 4.0 * u * (1.0 - au);
    const P: f32 = 0.225;
    let ay = if y < 0.0 { -y } else { y };
    P * (y * ay - y) + y
}

/// `cos(2π t)`, `t` in turns.
#[inline]
pub fn cos_turns(t: f32) -> f32 {
    sin_turns(t + 0.25)
}

/// The swell of one draw at one time: two directional sine waves, the
/// first along the flow, the second 70° off it at 1.7 times the frequency
/// and 45 % of the height (precomputed once per draw).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WaveSet {
    /// Unit directions on the water plane (x, z).
    pub dir: [Vec2; 2],
    /// Spatial frequency, cycles per world unit.
    pub k: [f32; 2],
    /// Phase at the draw's time, turns (`[0, 1)`).
    pub phase: [f32; 2],
    /// Heights, world units.
    pub amp: [f32; 2],
}

impl WaveSet {
    pub const FLAT: WaveSet = WaveSet {
        dir: [Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)],
        k: [0.0; 2],
        phase: [0.0; 2],
        amp: [0.0; 2],
    };

    pub fn new(water: &Water, time: f32) -> WaveSet {
        let a = water.wave_amplitude;
        let k = water.wave_frequency;
        if a == 0.0 || k <= 0.0 {
            return Self::FLAT;
        }
        let f = water.flow_speed;
        let l2 = f.dot(f);
        let d0 = if l2 > 1e-12 {
            f * (1.0 / crate::math3d::sqrt(l2))
        } else {
            Vec2::new(1.0, 0.0)
        };
        // 70° off the flow.
        const C: f32 = 0.342_020_14;
        const S: f32 = 0.939_692_6;
        let d1 = Vec2::new(d0.x * C - d0.y * S, d0.x * S + d0.y * C);
        let ks = [k, k * 1.7];
        let phase = core::array::from_fn(|i| {
            let hz = crate::math3d::sqrt(GRAVITY * ks[i] / TAU);
            frac(hz * time)
        });
        WaveSet {
            dir: [d0, d1],
            k: ks,
            phase,
            amp: [a, a * 0.45],
        }
    }

    pub fn is_flat(&self) -> bool {
        self.amp[0] == 0.0 && self.amp[1] == 0.0
    }

    /// Height above the surface at world `(x, z)` and its gradient
    /// `(dh/dx, dh/dz)`.
    #[inline]
    pub fn sample(&self, x: f32, z: f32) -> (f32, Vec2) {
        let mut h = 0.0;
        let mut g = Vec2::new(0.0, 0.0);
        for i in 0..2 {
            let d = self.dir[i];
            let t = self.k[i] * (d.x * x + d.y * z) - self.phase[i];
            h += self.amp[i] * sin_turns(t);
            let s = self.amp[i] * TAU * self.k[i] * cos_turns(t);
            g += d * s;
        }
        (h, g)
    }
}

/// The ripples' height at world `(x, z)` and its gradient, each ripple
/// scaled by `response`. A ring moves the surface only within
/// [`RIPPLE_WIDTH`] of its radius: points outside every band cost a
/// squared distance per ripple, no square root.
#[inline]
pub fn ripple_sample(ripples: &[Ripple], response: f32, x: f32, z: f32) -> (f32, Vec2) {
    let mut h = 0.0;
    let mut g = Vec2::new(0.0, 0.0);
    for r in ripples {
        let dx = x - r.pos.x;
        let dz = z - r.pos.y;
        let d2 = dx * dx + dz * dz;
        let rad = r.radius();
        let outer = rad + RIPPLE_WIDTH;
        if d2 >= outer * outer {
            continue;
        }
        let inner = rad - RIPPLE_WIDTH;
        if inner > 0.0 && d2 <= inner * inner {
            continue;
        }
        let d = crate::math3d::sqrt(d2);
        let q = (d - rad) * (1.0 / RIPPLE_WIDTH);
        let e = 1.0 - q * q;
        if e <= 0.0 {
            continue;
        }
        let s = r.strength * response * r.decay();
        let t = (d - rad) * (1.0 / RIPPLE_WAVELENGTH);
        let (sn, cs) = (sin_turns(t), cos_turns(t));
        h += s * e * e * cs;
        if d > 1e-4 {
            // d/dd of e² cos(2π t): 2e(-2q/W) cos - e² (2π/L) sin.
            let de = 2.0 * e * (-2.0 * q * (1.0 / RIPPLE_WIDTH));
            let dh = s * (de * cs - e * e * (TAU / RIPPLE_WAVELENGTH) * sn);
            let k = dh / d;
            g += Vec2::new(dx * k, dz * k);
        }
    }
    (h, g)
}

/// The world normal of a horizontal surface with height gradient `g`.
#[inline]
pub fn normal_of(g: Vec2) -> Vec3 {
    Vec3::new(-g.x, 1.0, -g.y).normalized()
}

/// Schlick's Fresnel term for water (F0 = 0.02) at `cos_theta` (normal ·
/// direction to the eye).
#[inline]
pub fn fresnel(cos_theta: f32) -> f32 {
    let c = if cos_theta < 0.0 { 0.0 } else { cos_theta };
    let m = 1.0 - c;
    let m2 = m * m;
    0.02 + 0.98 * m2 * m2 * m
}

/// What the per-vertex tiers add to a lit water vertex: the sky reflected
/// by the Fresnel term (more at grazing angles, where water also turns
/// opaque) and the sun's glint. `lit` is the vertex color after the usual
/// lighting (before fog), `n` the displaced world normal, `to_eye` the unit
/// direction from the vertex to the eye, `sun` the unit direction toward
/// the light, `sky` the reflected color, `sun_color` the light's,
/// `gloss` the material's shininess (0 = no glint).
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn shade(
    lit: Color,
    n: Vec3,
    to_eye: Vec3,
    sun: Vec3,
    sky: Color,
    sun_color: Color,
    gloss: u8,
) -> Color {
    let f = |c: Color| [c.r as f32, c.g as f32, c.b as f32, c.a as f32];
    let out = shade_f(f(lit), n, to_eye, sun, f(sky), f(sun_color), gloss);
    Color::rgba(to_u8(out[0]), to_u8(out[1]), to_u8(out[2]), to_u8(out[3]))
}

/// [`shade`] on 0..255 floats (r, g, b, a), unclamped: the T&L keeps the
/// vertex color in floats through lighting, this and fog, and converts
/// once ([`to_u8`]) — a saturating `as u8` costs the N64 ~70 cycles.
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn shade_f(
    lit: [f32; 4],
    n: Vec3,
    to_eye: Vec3,
    sun: Vec3,
    sky: [f32; 4],
    sun_color: [f32; 4],
    gloss: u8,
) -> [f32; 4] {
    let f = fresnel(n.dot(to_eye)) * 0.85;
    let mut out = [
        lit[0] + (sky[0] - lit[0]) * f,
        lit[1] + (sky[1] - lit[1]) * f,
        lit[2] + (sky[2] - lit[2]) * f,
        lit[3] + (255.0 - lit[3]) * f,
    ];
    if gloss > 0 {
        // Blinn-Phong per vertex: a broad lobe (the mesh is coarse).
        let h = (to_eye + sun).normalized();
        let ndh = n.dot(h);
        if ndh > 0.0 {
            let mut s = ndh * ndh; // ^2
            s *= s; // ^4
            s *= s; // ^8
            if gloss > 128 {
                s *= s; // ^16
            }
            let k = s * gloss as f32 * (1.0 / 255.0);
            out[0] += sun_color[0] * k;
            out[1] += sun_color[1] * k;
            out[2] += sun_color[2] * k;
        }
    }
    out
}

/// A 0..255 float to a byte, clamped, through [`to_i32`] (truncating like
/// `as u8`, without its saturating compares).
#[inline]
pub fn to_u8(v: f32) -> u8 {
    let i = to_i32(v);
    if i <= 0 {
        0
    } else if i >= 255 {
        255
    } else {
        i as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sin_turns_tracks_sine() {
        let mut worst = 0.0f32;
        for i in -2000..2000 {
            let t = i as f32 / 397.0;
            let e = (sin_turns(t) - (t * TAU).sin()).abs();
            worst = worst.max(e);
            let e = (cos_turns(t) - (t * TAU).cos()).abs();
            worst = worst.max(e);
        }
        assert!(worst < 2e-3, "{worst}");
        assert_eq!(frac(-0.25), 0.75);
        assert_eq!(frac(3.5), 0.5);
        assert_eq!(frac(-1.0), 0.0);
    }

    #[test]
    fn wave_gradient_matches_finite_differences() {
        let w = Water {
            wave_amplitude: 0.2,
            wave_frequency: 0.6,
            flow_speed: Vec2::new(0.3, 0.4),
            ..Water::DEFAULT
        };
        let set = WaveSet::new(&w, 1.25);
        let h = 1e-3;
        for &(x, z) in &[(0.3, 0.7), (-2.0, 1.1), (5.5, -3.25)] {
            let (_, g) = set.sample(x, z);
            let dx = (set.sample(x + h, z).0 - set.sample(x - h, z).0) / (2.0 * h);
            let dz = (set.sample(x, z + h).0 - set.sample(x, z - h).0) / (2.0 * h);
            // The sine approximation's own derivative error: a few percent.
            assert!((g.x - dx).abs() < 0.06, "{} vs {dx}", g.x);
            assert!((g.y - dz).abs() < 0.06, "{} vs {dz}", g.y);
        }
        assert!(
            WaveSet::new(
                &Water {
                    wave_amplitude: 0.0,
                    ..w
                },
                3.0
            )
            .is_flat()
        );
    }

    #[test]
    fn swell_moves_with_time_and_stays_bounded() {
        let w = Water::DEFAULT;
        let a = WaveSet::new(&w, 0.0).sample(1.0, 2.0).0;
        let b = WaveSet::new(&w, 0.4).sample(1.0, 2.0).0;
        assert_ne!(a, b);
        for i in 0..200 {
            let (h, _) = WaveSet::new(&w, i as f32 * 0.07).sample(i as f32 * 0.3, 1.0);
            assert!(h.abs() <= w.wave_amplitude * 1.46, "{h}");
        }
    }

    #[test]
    fn ripples_ring_out_and_fade() {
        let r = Ripple {
            pos: Vec2::new(1.0, 1.0),
            age: 0.5,
            strength: 0.1,
        };
        let rad = r.radius();
        // On the ring: moved; well inside or outside the band: untouched.
        let (on, _) = ripple_sample(&[r], 1.0, 1.0 + rad, 1.0);
        assert!(on.abs() > 0.01, "{on}");
        assert_eq!(ripple_sample(&[r], 1.0, 1.0 + rad + 1.0, 1.0).0, 0.0);
        assert_eq!(ripple_sample(&[r], 0.0, 1.0 + rad, 1.0).0, 0.0);
        let old = Ripple { age: 2.0, ..r };
        let (late, _) = ripple_sample(&[old], 1.0, 1.0 + old.radius(), 1.0);
        assert!(late.abs() < on.abs());
        // Gradient by finite differences, off the ring's center.
        let h = 1e-3;
        let (x, z) = (1.0 + rad * 0.8, 1.0 + rad * 0.5);
        let (_, g) = ripple_sample(&[r], 1.0, x, z);
        let dx = (ripple_sample(&[r], 1.0, x + h, z).0 - ripple_sample(&[r], 1.0, x - h, z).0)
            / (2.0 * h);
        assert!((g.x - dx).abs() < 0.03, "{} vs {dx}", g.x);
    }

    #[test]
    fn ripple_buffer_keeps_the_newest_and_expires() {
        let mut b = RippleBuffer::<2>::new();
        b.spawn(Vec2::new(1.0, 0.0), 0.1);
        b.update(0.5);
        b.spawn(Vec2::new(2.0, 0.0), 0.1);
        b.spawn(Vec2::new(3.0, 0.0), 0.1);
        assert_eq!(b.len(), 2);
        assert_eq!(b.as_slice()[0].pos.x, 2.0);
        assert_eq!(b.as_slice()[1].pos.x, 3.0);
        b.update(RIPPLE_LIFE);
        assert!(b.is_empty());
        let s = WaterState::new(
            1.0,
            &[
                Ripple::NONE,
                Ripple {
                    strength: 1.0,
                    ..Ripple::NONE
                },
            ],
            4,
        );
        assert!(s.ripples().is_empty(), "dead ripples are dropped");
        let live = Ripple {
            age: 0.0,
            strength: 1.0,
            pos: Vec2::new(0.0, 0.0),
        };
        assert_eq!(WaterState::new(0.0, &[live; 6], 4).ripples().len(), 4);
        assert_eq!(WaterState::new(0.0, &[live; 6], 2).ripples().len(), 2);
    }

    #[test]
    fn fresnel_and_glint() {
        assert!((fresnel(1.0) - 0.02).abs() < 1e-6);
        assert!((fresnel(0.0) - 1.0).abs() < 1e-6);
        let base = Color::rgba(20, 60, 120, 128);
        let up = Vec3::new(0.0, 1.0, 0.0);
        let sky = Color::rgb(200, 220, 255);
        // Looking straight down: almost the water's own color.
        let top = shade(base, up, up, up, sky, Color::WHITE, 0);
        assert!(top.b < 130 && top.a < 140, "{top:?}");
        // Grazing: the sky, and nearly opaque.
        let graze = shade(
            base,
            up,
            Vec3::new(1.0, 0.05, 0.0).normalized(),
            up,
            sky,
            Color::WHITE,
            0,
        );
        assert!(graze.b > 190 && graze.a > 200, "{graze:?}");
        // The sun mirrored into the eye: a glint, only with gloss.
        let eye = Vec3::new(0.3, 1.0, 0.0).normalized();
        let sun = Vec3::new(-0.3, 1.0, 0.0).normalized();
        let dull = shade(base, up, eye, sun, sky, Color::WHITE, 0);
        let shiny = shade(base, up, eye, sun, sky, Color::WHITE, 200);
        assert!(shiny.r > dull.r + 100, "{dull:?} {shiny:?}");
    }

    #[test]
    fn scroll_wraps_to_one_repeat() {
        let w = Water {
            flow_speed: Vec2::new(0.25, -0.5),
            ..Water::DEFAULT
        };
        let (a, _) = w.scroll(10.0);
        assert_eq!(a.x, 0.5);
        assert_eq!(a.y, 0.0);
        let (b, _) = w.scroll(1.0);
        assert_eq!(b.y, 0.5);
    }
}
