//! Shadows: every caster grounded on every target, each the way its
//! hardware affords ([`ShadowTier`], from [`crate::Caps::shadow_tier`], ADR
//! 0025). One component on the draw ([`ModelParams::shadow`],
//! [`ShadowCast`]), one setting per frame ([`ShadowSettings`],
//! [`crate::Renderer::set_shadows`]) and the surfaces shadows land on
//! ([`ShadowPlane`], [`crate::Renderer::set_shadow_receivers`]):
//!
//! - [`ShadowTier::Blob`] (N64): a soft disc under the caster, on the
//!   receiver below it — a textured quad (two triangles, the cheapest
//!   thing the RDP draws), shrinking and fading as the caster rises. The
//!   N64 also projects the shape of up to [`crate::Caps::max_planar_shadows`]
//!   [`ShadowCast::Mesh`] casters (its hero).
//! - [`ShadowTier::Planar`] (3DS): the caster's mesh flattened onto the
//!   receiver along the light ([`planar_matrix`]), drawn once per pixel (the
//!   stencil) in the shadow color.
//! - [`ShadowTier::Map`] (PC native, the Web): a depth map from the light
//!   over the camera's first [`ShadowSettings::distance`] meters, filtered
//!   (PCF), on every surface. The PC's quality preset sizes it.
//!
//! Blob and planar shadows lie on their receiver: a depth offset of a few
//! steps of the target's depth buffer ([`crate::Caps::depth_step`], the
//! N64's 18-bit compressed z included) keeps them from z-fighting it at
//! any distance, without lifting them visibly off it. Both blend the
//! shadow color over the surface, fade out toward the shadow distance and
//! are fogged like the 3D around them, so the three tiers agree on what a
//! shadow looks like and differ in how exact its shape is.

use crate::caps::Caps;
use crate::math::{Color, Vec2, Vec3};
use crate::math3d::{Mat34, sqrt};
use crate::render::{ModelParams, Shading};
use crate::render3d::{Mesh, NEAR_PLANE, ScreenTri, TnlContext, TnlScratch, TnlStats};

/// How a target draws shadows ([`crate::Caps::shadow_tier`]), from the
/// cheapest. Ordered: a game's [`ShadowSettings::tier_max`] and the PC's
/// quality preset cap it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord)]
pub enum ShadowTier {
    /// No shadows.
    None,
    /// A soft disc under each caster (N64).
    #[default]
    Blob,
    /// The caster's shape projected onto its receiver (3DS).
    Planar,
    /// A shadow map from the light (PC native, the Web).
    Map,
}

impl ShadowTier {
    pub const ALL: [ShadowTier; 4] = [
        ShadowTier::None,
        ShadowTier::Blob,
        ShadowTier::Planar,
        ShadowTier::Map,
    ];

    /// `none` / `blob` / `planar` / `map` (files, `--json`).
    pub const fn key(self) -> &'static str {
        ["none", "blob", "planar", "map"][self as usize]
    }

    pub const fn label(self) -> &'static str {
        ["None", "Blob", "Planar", "Map"][self as usize]
    }

    pub fn parse(s: &str) -> Option<ShadowTier> {
        ShadowTier::ALL
            .into_iter()
            .find(|t| s.trim().eq_ignore_ascii_case(t.key()))
    }
}

/// The frame's shadows ([`crate::Renderer::set_shadows`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ShadowSettings {
    /// The best tier the game wants; the target's
    /// [`crate::Caps::shadow_tier`] (and on the PC the quality preset) caps
    /// it further. [`ShadowTier::Blob`] gives every target the N64's look.
    pub tier_max: ShadowTier,
    /// Shadow map texels per side ([`ShadowTier::Map`]), a power of two
    /// from 256 to 4096; 0 takes the quality preset's (1024 on Low, 2048
    /// above).
    pub map_size: u16,
    /// How far from the camera shadows reach, meters: they fade out over
    /// its last fifth on every tier, and the map covers no farther.
    pub distance: f32,
    /// 0 (hard) to 255 (softest): the blob's falloff and the map's filter
    /// radius. Planar shadows are hard.
    pub softness: u8,
    /// What a shadowed surface blends toward; alpha is the strength.
    pub color: Color,
    /// Contact hardening on the [`ShadowTier::Map`] tier: how much wider a
    /// shadow's soft edge grows per unit of distance between the caster
    /// and the surface it lands on, measured along the light. Unitless (the
    /// light's apparent size), so it holds at any drawing scale: 0.1 widens
    /// the edge by 0.1 m for a caster 1 m above the ground. A shadow is as
    /// sharp as [`softness`](Self::softness) says where its caster touches
    /// the ground and softer the higher it is, up to a filter of twice the
    /// quality preset's taps per side. 0 (the default) keeps ADR 0025's
    /// even filter. Blob and planar shadows ignore it.
    pub soft_growth: f32,
    /// How far above the surface it lands on a caster's shadow fades out
    /// completely, world units (so a game drawn at a reduced scale scales
    /// it, as it does [`distance`](Self::distance)): along the light on the
    /// [`ShadowTier::Map`] tier, straight up from its receiver for a blob.
    /// The shadow is at full strength at contact and fades linearly. 0 (the
    /// default) keeps ADR 0025's: a blob fades over `4 × its radius + 1`
    /// m, the map never; planar shadows never fade with height.
    pub fade_distance: f32,
}

impl ShadowSettings {
    pub const DEFAULT: ShadowSettings = ShadowSettings {
        tier_max: ShadowTier::Map,
        map_size: 0,
        distance: 40.0,
        softness: 160,
        color: Color::rgba(0, 0, 0, 140),
        soft_growth: 0.0,
        fade_distance: 0.0,
    };

    /// Largest [`soft_growth`](Self::soft_growth) [`clamped`](Self::clamped)
    /// keeps.
    pub const MAX_SOFT_GROWTH: f32 = 4.0;

    /// Every field in its valid range: `map_size` 0 or a power of two in
    /// 256..=4096, a distance of 1 to 1000 m (NaN takes the default), a
    /// `soft_growth` of 0 to [`MAX_SOFT_GROWTH`](Self::MAX_SOFT_GROWTH)
    /// and a `fade_distance` of 0 or more (NaN and negatives take 0, the
    /// old look; infinity never fades).
    pub fn clamped(&self) -> ShadowSettings {
        let d = ShadowSettings::DEFAULT;
        let map_size = match self.map_size {
            0 => 0,
            s => s.clamp(256, 4096).next_power_of_two().min(4096),
        };
        // `!(x > 0)` also catches NaN.
        let soft_growth = if self.soft_growth > 0.0 {
            self.soft_growth.min(Self::MAX_SOFT_GROWTH)
        } else {
            0.0
        };
        let fade_distance = if self.fade_distance > 0.0 {
            self.fade_distance
        } else {
            0.0
        };
        ShadowSettings {
            map_size,
            distance: if self.distance.is_nan() {
                d.distance
            } else {
                self.distance.clamp(1.0, 1000.0)
            },
            soft_growth,
            fade_distance,
            ..*self
        }
    }

    /// Whether the map draws contact hardening or a height fade (the
    /// composite's blocker search runs only then).
    pub fn contact(&self) -> bool {
        self.soft_growth > 0.0 || self.fade_distance > 0.0
    }

    /// The height over which a blob of `blob_radius` fades out:
    /// [`fade_distance`](Self::fade_distance), or ADR 0025's
    /// `4 × blob_radius + 1` when it is 0.
    pub fn blob_fade(&self, blob_radius: f32) -> f32 {
        if self.fade_distance > 0.0 {
            self.fade_distance
        } else {
            4.0 * blob_radius + 1.0
        }
    }

    /// The strength left at view depth `zv`: full up to four fifths of the
    /// distance, then down to none at it.
    pub fn fade(&self, zv: f32) -> f32 {
        let d = self.distance;
        ((d - zv) / (d * 0.2)).clamp(0.0, 1.0)
    }
}

impl Default for ShadowSettings {
    fn default() -> Self {
        ShadowSettings::DEFAULT
    }
}

/// What a draw casts ([`ModelParams::shadow`]). The default casts nothing.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum ShadowCast {
    #[default]
    None,
    /// A blob of this radius (meters) on every tier: small props, pickups,
    /// a ball whose shadow tells where it will land.
    Blob { radius: f32 },
    /// The mesh's own shape: projected on the Planar tier, in the map on
    /// the Map tier. On the Blob tier the first
    /// [`crate::Caps::max_planar_shadows`] of a frame are projected (the
    /// N64's hero), the rest cast nothing.
    Mesh,
    /// The mesh's shape where the tier draws shapes, a blob of
    /// `blob_radius` on the Blob tier (0 = none there: a character's limbs,
    /// whose body casts the blob).
    Auto { blob_radius: f32 },
}

impl ShadowCast {
    /// `Auto` with a blob of `blob_radius` on the Blob tier.
    pub const fn auto(blob_radius: f32) -> ShadowCast {
        ShadowCast::Auto { blob_radius }
    }

    pub const fn casts(&self) -> bool {
        !matches!(self, ShadowCast::None)
    }
}

/// A surface shadows land on ([`crate::Renderer::set_shadow_receivers`]):
/// a rectangle on a plane. Blob and planar shadows go onto the receiver
/// under their caster and are cut to its rectangle; the Map tier shades
/// every surface and ignores them.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ShadowPlane {
    /// The rectangle's center.
    pub point: Vec3,
    /// Unit normal, toward the side shadows land on.
    pub normal: Vec3,
    /// Unit direction in the plane of the first extent.
    pub axis: Vec3,
    /// Half sizes along `axis` and along `normal × axis`;
    /// `f32::INFINITY` for an endless ground.
    pub half: Vec2,
}

impl ShadowPlane {
    /// An endless horizontal ground at height `y`.
    pub const fn ground(y: f32) -> ShadowPlane {
        ShadowPlane {
            point: Vec3::new(0.0, y, 0.0),
            normal: Vec3::new(0.0, 1.0, 0.0),
            axis: Vec3::new(1.0, 0.0, 0.0),
            half: Vec2::new(f32::INFINITY, f32::INFINITY),
        }
    }

    /// A horizontal rectangle facing up, `half_x` by `half_z` around
    /// `center`.
    pub const fn horizontal(center: Vec3, half_x: f32, half_z: f32) -> ShadowPlane {
        ShadowPlane {
            point: center,
            normal: Vec3::new(0.0, 1.0, 0.0),
            axis: Vec3::new(1.0, 0.0, 0.0),
            half: Vec2::new(half_x, half_z),
        }
    }

    /// The top face of the box `min..max` (a platform, a block).
    pub fn top_of(min: Vec3, max: Vec3) -> ShadowPlane {
        ShadowPlane::horizontal(
            Vec3::new((min.x + max.x) * 0.5, max.y, (min.z + max.z) * 0.5),
            (max.x - min.x).abs() * 0.5,
            (max.z - min.z).abs() * 0.5,
        )
    }

    /// The second in-plane direction (`normal × axis`).
    pub fn axis_v(&self) -> Vec3 {
        self.normal.cross(self.axis)
    }

    /// Signed height of `p` above the plane.
    pub fn height(&self, p: Vec3) -> f32 {
        (p - self.point).dot(self.normal)
    }

    /// `p` in the rectangle's coordinates (along `axis`, along
    /// [`axis_v`](Self::axis_v)).
    pub fn local(&self, p: Vec3) -> Vec2 {
        let d = p - self.point;
        Vec2::new(d.dot(self.axis), d.dot(self.axis_v()))
    }

    /// The point at rectangle coordinates `q`.
    pub fn at(&self, q: Vec2) -> Vec3 {
        self.point + self.axis * q.x + self.axis_v() * q.y
    }

    /// Whether `p`, dropped onto the plane, falls inside the rectangle.
    pub fn covers(&self, p: Vec3) -> bool {
        let q = self.local(p);
        q.x.abs() <= self.half.x && q.y.abs() <= self.half.y
    }

    fn bounded(&self) -> bool {
        self.half.x.is_finite() && self.half.y.is_finite()
    }
}

/// `v` normalized; a unit vector already (the common case) is kept
/// exact: the core's software sqrt would move it by an ulp or two.
fn unit(v: Vec3) -> Vec3 {
    if (v.dot(v) - 1.0).abs() < 1e-6 {
        v
    } else {
        v.normalized()
    }
}

/// Most receivers a frame takes (the first ones given to
/// `set_shadow_receivers`).
pub const MAX_SHADOW_PLANES: usize = 32;

/// How far below a receiver a caster's center may sink and still cast on
/// it (feet through a floor), meters.
const SINK: f32 = 0.05;

/// Side of the blob texture ([`blob_texels`]), texels.
pub const BLOB_TEXTURE_SIZE: usize = 32;

/// Depth-buffer steps a blob or planar shadow sits in front of its
/// receiver ([`shadow_z`]): the receiver's and the shadow's rounding are a
/// step each, the rest is margin.
pub const SHADOW_Z_STEPS: f32 = 4.0;

/// The blob's falloff: [`BLOB_TEXTURE_SIZE`]² 8-bit values, row-major, 255
/// in the middle to 0 at the rim (the outermost texels are 0, so a clamped
/// sampler ends the disc there). Radially symmetric: no orientation to get
/// wrong per platform. `softness` 0 is a hard disc (a texel of smoothing),
/// 255 fades from the very center. Every backend uploads this (intensity
/// on the N64, alpha on the 3DS and the PC) as its blob texture.
pub fn blob_texels(softness: u8, out: &mut [u8; BLOB_TEXTURE_SIZE * BLOB_TEXTURE_SIZE]) {
    let n = BLOB_TEXTURE_SIZE;
    let half = n as f32 * 0.5;
    let rim = half - 1.0;
    // The fully dark core, as a fraction of the radius.
    let core = (1.0 - softness as f32 / 255.0) * (1.0 - 1.5 / rim);
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = (x as f32 + 0.5 - half, y as f32 + 0.5 - half);
            let r = sqrt(dx * dx + dy * dy) / rim;
            let t = ((1.0 - r) / (1.0 - core)).clamp(0.0, 1.0);
            let a = t * t * (3.0 - 2.0 * t);
            out[y * n + x] = (a * 255.0 + 0.5) as u8;
        }
    }
}

/// The projection along `light` (the direction it travels) onto `plane`,
/// as a model matrix: a point `x` lands where the ray `x + t·light` meets
/// the plane, `x - light · (n·(x - p)) / (n·light)`. `None` when the light
/// doesn't come down onto the plane's front (grazing within ~5°, or from
/// behind it).
pub fn planar_matrix(plane: &ShadowPlane, light: Vec3) -> Option<Mat34> {
    let l = light.normalized();
    let n = plane.normal;
    let nl = n.dot(l);
    if nl > -0.08 {
        return None;
    }
    let d = n.dot(plane.point);
    let k = 1.0 / nl;
    let (l, n) = ([l.x, l.y, l.z], [n.x, n.y, n.z]);
    let mut m = [[0.0f32; 4]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().take(3).enumerate() {
            *cell = (i == j) as u32 as f32 - l[i] * n[j] * k;
        }
        row[3] = l[i] * d * k;
    }
    Some(Mat34 { m })
}

/// `z` (a [`ScreenTri::z`]) moved [`SHADOW_Z_STEPS`] steps of the target's
/// depth buffer toward the camera: in front of the receiver it was
/// projected onto, at any distance, by no more than the buffer can tell.
pub fn shadow_z(caps: &Caps, z: f32) -> f32 {
    (z - SHADOW_Z_STEPS * caps.depth_step(z)).max(0.0)
}

/// What the frame's shadows cost ([`crate::FrameStats::shadows`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ShadowStats {
    /// Casters drawn as a blob.
    pub blobs: u16,
    /// Casters projected onto their receiver.
    pub planar: u16,
    /// Casters drawn into the shadow map.
    pub mapped: u16,
    /// [`ShadowCast::Mesh`] casters past the Blob tier's planar budget
    /// ([`crate::Caps::max_planar_shadows`]): no shadow.
    pub over_budget: u16,
    /// Blob and planar triangles rasterized.
    pub tris: u32,
}

/// What a caster draws this frame ([`ShadowFrame::plan`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ShadowPlan {
    None,
    Blob { radius: f32 },
    Planar,
    Map,
}

/// A blob ready to draw: a quad on its receiver, cut to the receiver's
/// rectangle, with the blob texture's coordinates and the shadow color
/// (alpha: the strength left after the height and distance fades).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BlobQuad {
    pub corners: [Vec3; 4],
    pub uv: [Vec2; 4],
    pub color: Color,
}

/// A planar shadow ready to draw: the caster's mesh under `matrix` (the
/// caster's, flattened onto receiver `plane`) with `params`
/// ([`shadow_params`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PlanarDraw {
    pub matrix: Mat34,
    pub params: ModelParams,
    pub plane: usize,
}

/// A backend's shadow state for the frame: the settings, the receivers,
/// the tier they come to and the planar budget left. Cleared at
/// `begin_frame` ([`ShadowFrame::clear`]).
#[derive(Clone, Copy, Debug)]
pub struct ShadowFrame {
    settings: Option<ShadowSettings>,
    tier: ShadowTier,
    planes: [ShadowPlane; MAX_SHADOW_PLANES],
    n_planes: u8,
    planar_left: u8,
    pub stats: ShadowStats,
}

impl ShadowFrame {
    pub const OFF: ShadowFrame = ShadowFrame {
        settings: None,
        tier: ShadowTier::None,
        planes: [ShadowPlane::ground(0.0); MAX_SHADOW_PLANES],
        n_planes: 0,
        planar_left: 0,
        stats: ShadowStats {
            blobs: 0,
            planar: 0,
            mapped: 0,
            over_budget: 0,
            tris: 0,
        },
    };

    /// No shadows, no receivers (`begin_frame`).
    pub fn clear(&mut self) {
        self.settings = None;
        self.tier = ShadowTier::None;
        self.n_planes = 0;
        self.planar_left = 0;
        self.stats = ShadowStats::default();
    }

    /// `set_shadows`: the tier is the least of the target's, the game's
    /// and `cap` (the PC's quality preset; [`ShadowTier::Map`] elsewhere).
    pub fn set(&mut self, settings: Option<&ShadowSettings>, caps: &Caps, cap: ShadowTier) {
        self.settings = settings.map(ShadowSettings::clamped);
        self.tier = match &self.settings {
            Some(s) => caps.shadow_tier.min(s.tier_max).min(cap),
            None => ShadowTier::None,
        };
        self.planar_left = caps.max_planar_shadows;
    }

    /// `set_shadow_receivers`: the first [`MAX_SHADOW_PLANES`] (planes
    /// with a degenerate normal are skipped).
    pub fn set_receivers(&mut self, planes: &[ShadowPlane]) {
        let mut n = 0;
        for p in planes {
            if n == MAX_SHADOW_PLANES {
                break;
            }
            let normal = unit(p.normal);
            if normal == Vec3::ZERO {
                continue;
            }
            // An axis along the normal has no in-plane part: pick another.
            let mut axis = unit(p.axis - normal * p.axis.dot(normal));
            if axis == Vec3::ZERO {
                let any = if normal.x.abs() < 0.9 {
                    Vec3::new(1.0, 0.0, 0.0)
                } else {
                    Vec3::new(0.0, 0.0, 1.0)
                };
                axis = (any - normal * any.dot(normal)).normalized();
            }
            self.planes[n] = ShadowPlane {
                normal,
                axis,
                half: Vec2::new(p.half.x.abs(), p.half.y.abs()),
                ..*p
            };
            n += 1;
        }
        self.n_planes = n as u8;
    }

    pub fn settings(&self) -> Option<&ShadowSettings> {
        self.settings.as_ref()
    }

    /// The tier this frame draws at ([`ShadowTier::None`] without
    /// settings).
    pub fn tier(&self) -> ShadowTier {
        self.tier
    }

    pub fn receivers(&self) -> &[ShadowPlane] {
        &self.planes[..self.n_planes as usize]
    }

    /// What `cast` draws this frame, spending the planar budget on the
    /// Blob tier. Doesn't look at where the caster is.
    pub fn plan(&mut self, cast: ShadowCast) -> ShadowPlan {
        let shapes = self.tier >= ShadowTier::Planar;
        let shape = if self.tier == ShadowTier::Map {
            ShadowPlan::Map
        } else {
            ShadowPlan::Planar
        };
        match cast {
            _ if self.tier == ShadowTier::None => ShadowPlan::None,
            ShadowCast::None => ShadowPlan::None,
            ShadowCast::Blob { radius } if radius > 0.0 => ShadowPlan::Blob { radius },
            ShadowCast::Blob { .. } => ShadowPlan::None,
            ShadowCast::Mesh | ShadowCast::Auto { .. } if shapes => shape,
            ShadowCast::Mesh if self.planar_left > 0 => {
                self.planar_left -= 1;
                ShadowPlan::Planar
            }
            ShadowCast::Mesh => {
                self.stats.over_budget += 1;
                ShadowPlan::None
            }
            ShadowCast::Auto { blob_radius } if blob_radius > 0.0 => ShadowPlan::Blob {
                radius: blob_radius,
            },
            ShadowCast::Auto { .. } => ShadowPlan::None,
        }
    }

    /// The receiver under the world point `c`: the nearest plane below it
    /// (along each plane's normal) whose rectangle it is over, with the
    /// height above it.
    pub fn receiver_under(&self, c: Vec3) -> Option<(usize, f32)> {
        let mut best: Option<(usize, f32)> = None;
        for (i, p) in self.receivers().iter().enumerate() {
            let h = p.height(c);
            if h < -SINK || !p.covers(c) {
                continue;
            }
            if best.is_none_or(|(_, b)| h < b) {
                best = Some((i, h));
            }
        }
        best
    }

    /// The blob of a caster whose bounding sphere is `center`, `radius`
    /// (world): `blob_radius` wide on the receiver under it, shrinking to
    /// 60% and fading out as the sphere's bottom rises to
    /// [`ShadowSettings::blob_fade`] above it (`4·blob_radius + 1` m unless
    /// the game sets [`ShadowSettings::fade_distance`]), faded with
    /// distance, cut to the receiver. `None` with nothing to draw.
    pub fn blob(
        &self,
        ctx: &TnlContext,
        center: Vec3,
        radius: f32,
        blob_radius: f32,
    ) -> Option<BlobQuad> {
        let s = self.settings.as_ref()?;
        let (i, h) = self.receiver_under(center)?;
        let plane = &self.planes[i];
        if plane.height(ctx.camera().eye) <= 0.0 {
            return None; // seen from behind
        }
        let zv = ctx.view_point(center).z;
        let fade = s.fade(zv);
        let rise = ((h - radius).max(0.0) / s.blob_fade(blob_radius)).min(1.0);
        let alpha = s.color.a as f32 * fade * (1.0 - rise);
        if alpha < 1.0 {
            return None;
        }
        let r = blob_radius * (1.0 - 0.4 * rise);
        let q = plane.local(center);
        let (lo, hi) = (q - Vec2::new(r, r), q + Vec2::new(r, r));
        let clo = Vec2::new(lo.x.max(-plane.half.x), lo.y.max(-plane.half.y));
        let chi = Vec2::new(hi.x.min(plane.half.x), hi.y.min(plane.half.y));
        if chi.x <= clo.x || chi.y <= clo.y {
            return None;
        }
        let uv = |p: Vec2| Vec2::new((p.x - lo.x) / (2.0 * r), (p.y - lo.y) / (2.0 * r));
        let corners2 = [clo, Vec2::new(chi.x, clo.y), chi, Vec2::new(clo.x, chi.y)];
        Some(BlobQuad {
            corners: corners2.map(|p| plane.at(p)),
            uv: corners2.map(uv),
            color: Color::rgba(s.color.r, s.color.g, s.color.b, (alpha + 0.5) as u8),
        })
    }

    /// The planar shadow of a caster drawn at `matrix` with `params`, its
    /// bounding sphere `center`, `radius` (world): its receiver, the
    /// flattening matrix and the surface. `None` with no receiver under it,
    /// a light that doesn't reach the receiver's front, the camera behind
    /// it, or past the shadow distance.
    pub fn planar(
        &self,
        ctx: &TnlContext,
        center: Vec3,
        matrix: &Mat34,
        params: &ModelParams,
    ) -> Option<PlanarDraw> {
        let s = self.settings.as_ref()?;
        let (i, _) = self.receiver_under(center)?;
        let plane = &self.planes[i];
        if plane.height(ctx.camera().eye) <= 0.0 {
            return None;
        }
        let fade = s.fade(ctx.view_point(center).z);
        let alpha = s.color.a as f32 * fade;
        if alpha < 1.0 {
            return None;
        }
        let p = planar_matrix(plane, ctx.light().dir)?;
        Some(PlanarDraw {
            matrix: p.mul(matrix),
            params: shadow_params(params, s.color, (alpha + 0.5) as u8),
            plane: i,
        })
    }

    /// The bounding sphere of `draw`'s shadow (world) for a caster in
    /// `sphere`: its center dropped onto the receiver along the light, the
    /// radius stretched as far as the light slants.
    pub fn planar_bounds(
        &self,
        ctx: &TnlContext,
        sphere: (Vec3, f32),
        draw: &PlanarDraw,
    ) -> (Vec3, f32) {
        let plane = &self.planes[draw.plane];
        let l = ctx.light().dir.normalized();
        let nl = plane.normal.dot(l);
        let stretch = 1.0 + 1.0 / nl.abs().max(0.08);
        let (c, r) = sphere;
        (c - l * (plane.height(c) / nl), r * stretch)
    }

    /// `draw`'s receiver rectangle as a quad (lying on it, white), for a
    /// GPU that draws the shadow whole and cuts it to the receiver with a
    /// stencil mark instead; `None` for an endless one (nothing to cut).
    pub fn receiver_quad(&self, draw: &PlanarDraw) -> Option<BlobQuad> {
        let plane = &self.planes[draw.plane];
        if !plane.bounded() {
            return None;
        }
        let h = plane.half;
        let q = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        Some(BlobQuad {
            corners: q.map(|(x, y)| plane.at(Vec2::new(x * h.x, y * h.y))),
            uv: q.map(|(x, y)| Vec2::new((x + 1.0) * 0.5, (y + 1.0) * 0.5)),
            color: Color::WHITE,
        })
    }
}

impl Default for ShadowFrame {
    fn default() -> Self {
        ShadowFrame::OFF
    }
}

/// The surface of a planar shadow of a caster drawn with `caster`:
/// unlit, its color constant (`color`'s, at `alpha`), its pose and sides
/// the caster's (a one-sided sheet must keep its only face), nothing else.
pub fn shadow_params(caster: &ModelParams, color: Color, alpha: u8) -> ModelParams {
    ModelParams {
        tint: Color::rgba(0, 0, 0, alpha),
        emissive: Color::rgb(color.r, color.g, color.b),
        shading: Shading::Unlit,
        double_sided: caster.double_sided,
        anim: caster.anim,
        ..ModelParams::DEFAULT
    }
}

/// A caster's bounding sphere in world space: its mesh's `bounds`
/// ([`Mesh::bounds`]; its pivot and half a meter without them) under
/// `matrix`.
pub fn caster_sphere(bounds: Option<(Vec3, f32)>, matrix: &Mat34) -> (Vec3, f32) {
    let (c, r) = bounds.unwrap_or((Vec3::ZERO, 0.5));
    (matrix.transform_point(c), r * matrix.max_scale())
}

/// The light's box over the view for [`ShadowTier::Map`] ([`map_fit`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ShadowMapFit {
    /// World to map: `x` and `y` -1..1 across it (y up), `z` 0..1 away
    /// from the light.
    pub matrix: Mat34,
    /// One texel's width, world units.
    pub texel: f32,
    /// The depth range (map `z` 0..1), world units.
    pub depth: f32,
    /// The box's half width, world units.
    pub radius: f32,
}

/// The orthographic box from `light` (the direction it travels) over the
/// camera's first `distance` meters, for a `size`² map: square, around the
/// bounding sphere of that slice of the view, so it doesn't change size as
/// the camera turns, and its center snapped to whole texels, so its edges
/// don't crawl as the camera moves. It reaches back toward the light to
/// every caster sphere in `casters` (world) over the box, however high
/// above the view. `None` without a light direction.
pub fn map_fit(
    camera: &crate::render3d::Camera3,
    aspect: f32,
    light: Vec3,
    distance: f32,
    size: u16,
    casters: &[(Vec3, f32)],
) -> Option<ShadowMapFit> {
    let f = unit(light);
    if f == Vec3::ZERO || size == 0 {
        return None;
    }
    let up = if f.y.abs() < 0.99 {
        Vec3::new(0.0, 1.0, 0.0)
    } else {
        Vec3::new(0.0, 0.0, 1.0)
    };
    let r = up.cross(f).normalized();
    let u = f.cross(r);
    // The slice's bounding sphere: its far corners are `d·k` off the axis,
    // the center `t` along it as far from the eye as from them.
    let half = camera.fov_y * 0.5;
    let ty = crate::math3d::sin(half) / crate::math3d::cos(half);
    let k2 = ty * ty * (1.0 + aspect * aspect);
    let d = distance.max(1.0);
    let t = d * (1.0 + k2) * 0.5;
    let (t, radius) = if t <= d { (t, t) } else { (d, d * sqrt(k2)) };
    let forward = (camera.target - camera.eye).normalized();
    let c = camera.eye + forward * t;
    let texel = 2.0 * radius / size as f32;
    let snap = |v: f32| {
        let q = v / texel + 0.5;
        let i = q as i32 as f32;
        (if i > q { i - 1.0 } else { i }) * texel
    };
    let (cx, cy, cz) = (snap(r.dot(c)), snap(u.dot(c)), f.dot(c));
    let mut near = -radius;
    for &(p, pr) in casters {
        let (x, y) = (r.dot(p) - cx, u.dot(p) - cy);
        if x.abs() > radius + pr || y.abs() > radius + pr {
            continue;
        }
        // A meter past its top, so it isn't cut off.
        near = near.min(f.dot(p) - cz - pr - 1.0);
    }
    let depth = radius - near;
    let (kr, kz) = (1.0 / radius, 1.0 / depth);
    Some(ShadowMapFit {
        matrix: Mat34 {
            m: [
                [r.x * kr, r.y * kr, r.z * kr, -cx * kr],
                [u.x * kr, u.y * kr, u.z * kr, -cy * kr],
                [f.x * kz, f.y * kz, f.z * kz, -(cz + near) * kz],
            ],
        },
        texel,
        depth,
        radius,
    })
}

/// The triangles of `blob` on screen: clipped to the near plane and the
/// guard band, fogged, depth-offset onto its receiver ([`shadow_z`]).
/// Writes up to 7 triangles into `out`, returns how many.
pub fn blob_tris(
    ctx: &TnlContext,
    caps: &Caps,
    blob: &BlobQuad,
    out: &mut [ScreenTri; 7],
) -> usize {
    let mut poly = [(Vec3::ZERO, Vec2::ZERO); 9];
    for (k, slot) in poly.iter_mut().take(4).enumerate() {
        *slot = (ctx.view_point(blob.corners[k]), blob.uv[k]);
    }
    let mut n = 4;
    let (lx, ly) = ctx.guard();
    // The near plane, then the guard band's sides (inlined: the N64 builds
    // a blob per caster per frame).
    let side = |k: usize, v: Vec3| match k {
        0 => v.z - NEAR_PLANE,
        1 => lx * v.z - v.x,
        2 => lx * v.z + v.x,
        3 => ly * v.z - v.y,
        _ => ly * v.z + v.y,
    };
    for k in 0..5 {
        let dist = |v: Vec3| side(k, v);
        if poly[..n].iter().all(|(v, _)| dist(*v) >= 0.0) {
            continue;
        }
        let mut next = [(Vec3::ZERO, Vec2::ZERO); 9];
        let mut m = 0;
        for i in 0..n {
            let (a, ua) = poly[i];
            let (b, ub) = poly[(i + 1) % n];
            let (da, db) = (dist(a), dist(b));
            if da >= 0.0 && m < 9 {
                next[m] = (a, ua);
                m += 1;
            }
            if (da >= 0.0) != (db >= 0.0) && m < 9 {
                let t = da / (da - db);
                next[m] = (a + (b - a) * t, ua + (ub - ua) * t);
                m += 1;
            }
        }
        poly = next;
        n = m;
        if n < 3 {
            return 0;
        }
    }
    let screen = ctx.screen();
    let (focal, center) = (ctx.focal(), ctx.center());
    let mut pts = [Vec2::ZERO; 9];
    let mut z = [0.0f32; 9];
    let mut col = [blob.color; 9];
    for i in 0..n {
        let v = poly[i].0;
        pts[i] = Vec2::new(center.x + v.x * focal / v.z, center.y - v.y * focal / v.z);
        z[i] = shadow_z(caps, 1.0 - NEAR_PLANE / v.z);
        col[i] = ctx.fogged(blob.color, v.z);
    }
    let (min, max) = pts[..n].iter().fold(
        (Vec2::new(f32::MAX, f32::MAX), Vec2::new(f32::MIN, f32::MIN)),
        |(lo, hi), p| {
            (
                Vec2::new(lo.x.min(p.x), lo.y.min(p.y)),
                Vec2::new(hi.x.max(p.x), hi.y.max(p.y)),
            )
        },
    );
    if max.x < 0.0 || max.y < 0.0 || min.x > screen.x || min.y > screen.y {
        return 0;
    }
    let mut count = 0;
    for k in 1..n - 1 {
        let ids = [0, k, k + 1];
        out[count] = ScreenTri {
            pts: ids.map(|i| pts[i]),
            colors: ids.map(|i| col[i]),
            depth: ids.iter().fold(0.0f32, |d, &i| d.max(poly[i].0.z)),
            z: ids.map(|i| z[i]),
            uv: ids.map(|i| poly[i].1),
            w: ids.map(|i| poly[i].0.z),
        };
        count += 1;
    }
    count
}

/// A receiver's rectangle on screen, to cut planar shadows to it
/// ([`ReceiverClip::emit`]): the rectangle clipped to the near plane,
/// projected (convex, up to 5 corners). Endless receivers cut nothing.
#[derive(Clone, Copy, Debug)]
pub struct ReceiverClip {
    pts: [Vec2; 8],
    n: usize,
    /// The polygon's winding (+1 counter-clockwise on screen, -1 clockwise).
    sign: f32,
}

impl ReceiverClip {
    /// The clip of receiver `plane`; `None` for an endless one (nothing to
    /// cut). A rectangle entirely behind the camera clips everything away.
    pub fn new(ctx: &TnlContext, plane: &ShadowPlane) -> Option<ReceiverClip> {
        if !plane.bounded() {
            return None;
        }
        let (hx, hy) = (plane.half.x, plane.half.y);
        let corners = [
            Vec2::new(-hx, -hy),
            Vec2::new(hx, -hy),
            Vec2::new(hx, hy),
            Vec2::new(-hx, hy),
        ];
        let mut poly = [Vec3::ZERO; 8];
        for k in 0..4 {
            poly[k] = ctx.view_point(plane.at(corners[k]));
        }
        let mut n = 4;
        if poly[..4].iter().any(|v| v.z < NEAR_PLANE) {
            let mut next = [Vec3::ZERO; 8];
            let mut m = 0;
            for i in 0..n {
                let (a, b) = (poly[i], poly[(i + 1) % n]);
                let (da, db) = (a.z - NEAR_PLANE, b.z - NEAR_PLANE);
                if da >= 0.0 {
                    next[m] = a;
                    m += 1;
                }
                if (da >= 0.0) != (db >= 0.0) {
                    next[m] = a + (b - a) * (da / (da - db));
                    m += 1;
                }
            }
            poly = next;
            n = m;
        }
        let (focal, center) = (ctx.focal(), ctx.center());
        let mut pts = [Vec2::ZERO; 8];
        for i in 0..n {
            let v = poly[i];
            pts[i] = Vec2::new(center.x + v.x * focal / v.z, center.y - v.y * focal / v.z);
        }
        let mut area = 0.0;
        for i in 0..n {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            area += a.x * b.y - b.x * a.y;
        }
        Some(ReceiverClip {
            pts,
            n: if n < 3 { 0 } else { n },
            sign: if area >= 0.0 { 1.0 } else { -1.0 },
        })
    }

    /// Hand `emit` the parts of `tri` inside the rectangle (all of it, as
    /// is, when it is inside; nothing when outside). Attributes are
    /// interpolated as the rasterizer would: z and 1/w (and uv/w) linearly
    /// on screen.
    pub fn emit(&self, tri: &ScreenTri, emit: &mut dyn FnMut(ScreenTri)) {
        if self.n == 0 {
            return;
        }
        let inside = |p: Vec2, k: usize| {
            let (a, b) = (self.pts[k], self.pts[(k + 1) % self.n]);
            ((b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)) * self.sign
        };
        if (0..self.n).all(|k| tri.pts.iter().all(|&p| inside(p, k) >= 0.0)) {
            emit(*tri);
            return;
        }
        // (x, y, z, 1/w, u/w, v/w, r, g, b, a)
        type V = [f32; 10];
        let vert = |i: usize| -> V {
            let iw = 1.0 / tri.w[i];
            let c = tri.colors[i];
            [
                tri.pts[i].x,
                tri.pts[i].y,
                tri.z[i],
                iw,
                tri.uv[i].x * iw,
                tri.uv[i].y * iw,
                c.r as f32,
                c.g as f32,
                c.b as f32,
                c.a as f32,
            ]
        };
        let mut poly: [V; 16] = [[0.0; 10]; 16];
        poly[0] = vert(0);
        poly[1] = vert(1);
        poly[2] = vert(2);
        let mut n = 3;
        for k in 0..self.n {
            let d = |v: &V| inside(Vec2::new(v[0], v[1]), k);
            let mut next: [V; 16] = [[0.0; 10]; 16];
            let mut m = 0;
            for i in 0..n {
                let (a, b) = (poly[i], poly[(i + 1) % n]);
                let (da, db) = (d(&a), d(&b));
                if da >= 0.0 && m < 16 {
                    next[m] = a;
                    m += 1;
                }
                if (da >= 0.0) != (db >= 0.0) && m < 16 {
                    let t = da / (da - db);
                    let mut v = [0.0; 10];
                    for j in 0..10 {
                        v[j] = a[j] + (b[j] - a[j]) * t;
                    }
                    next[m] = v;
                    m += 1;
                }
            }
            poly = next;
            n = m;
            if n < 3 {
                return;
            }
        }
        let back = |v: &V| {
            let w = 1.0 / v[3];
            (
                Vec2::new(v[0], v[1]),
                v[2],
                w,
                Vec2::new(v[4] * w, v[5] * w),
                Color::rgba(v[6] as u8, v[7] as u8, v[8] as u8, v[9] as u8),
            )
        };
        for k in 1..n - 1 {
            let (a, b, c) = (back(&poly[0]), back(&poly[k]), back(&poly[k + 1]));
            emit(ScreenTri {
                pts: [a.0, b.0, c.0],
                colors: [a.4, b.4, c.4],
                depth: tri.depth,
                z: [a.1, b.1, c.1],
                uv: [a.3, b.3, c.3],
                w: [a.2, b.2, c.2],
            });
        }
    }
}

/// A planar shadow through the CPU T&L: the caster's mesh flattened by
/// `draw`, depth-offset onto its receiver ([`shadow_z`]) and cut to its
/// rectangle, handed to `emit`. The mesh's own bounding-sphere cull is
/// replaced by one around the flattened sphere (the projection stretches
/// the sphere past what the T&L's test assumes).
#[allow(clippy::too_many_arguments)]
pub fn planar_tris(
    frame: &ShadowFrame,
    ctx: &TnlContext,
    scratch: &mut TnlScratch,
    caps: &Caps,
    mesh: &Mesh,
    sphere: (Vec3, f32),
    draw: &PlanarDraw,
    emit: &mut dyn FnMut(ScreenTri),
) -> TnlStats {
    let plane = &frame.planes[draw.plane];
    let (fc, fr) = frame.planar_bounds(ctx, sphere, draw);
    if !ctx.sphere_visible(fc, fr) {
        return TnlStats {
            culled: 1,
            ..TnlStats::default()
        };
    }
    let clip = ReceiverClip::new(ctx, plane);
    let mut out = |mut tri: ScreenTri| {
        for z in &mut tri.z {
            *z = shadow_z(caps, *z);
        }
        match &clip {
            Some(c) => c.emit(&tri, emit),
            None => emit(tri),
        }
    };
    crate::render3d::tessellate_with(
        ctx,
        scratch,
        &mesh.without_bounds(),
        &draw.matrix,
        &draw.params,
        &mut out,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render3d::{Camera3, DEFAULT_LIGHT};

    fn close(a: Vec3, b: Vec3) -> bool {
        let d = a - b;
        d.dot(d) < 1e-8
    }

    /// Where the ray from `x` along `l` meets the plane: the reference the
    /// matrix must match.
    fn ray_plane(plane: &ShadowPlane, l: Vec3, x: Vec3) -> Vec3 {
        let t = -plane.height(x) / plane.normal.dot(l);
        x + l * t
    }

    #[test]
    fn the_planar_matrix_is_the_ray_plane_intersection() {
        let planes = [
            ShadowPlane::ground(0.0),
            ShadowPlane::horizontal(Vec3::new(3.0, 1.25, -2.0), 1.0, 1.0),
            ShadowPlane {
                point: Vec3::new(0.5, -1.0, 0.25),
                normal: Vec3::new(0.3, 0.9, -0.2).normalized(),
                axis: Vec3::new(1.0, 0.0, 0.0),
                half: Vec2::new(4.0, 4.0),
            },
        ];
        let lights = [
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::new(0.4, -0.8, 0.45),
            Vec3::new(-0.9, -0.35, 0.2),
        ];
        let points = [
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(-3.5, 7.25, 1.0),
            Vec3::new(10.0, 0.5, -4.0),
            Vec3::new(0.25, 0.0, 0.0),
        ];
        for plane in &planes {
            for l in lights {
                let m = planar_matrix(plane, l).unwrap();
                for x in points {
                    let got = m.transform_point(x);
                    let want = ray_plane(plane, l.normalized(), x);
                    assert!(
                        close(got, want),
                        "{plane:?} {l:?} {x:?}: {got:?} vs {want:?}"
                    );
                    assert!(plane.height(got).abs() < 1e-4, "on the plane");
                }
            }
        }
        // A light along the plane or from under it projects nothing.
        let g = ShadowPlane::ground(0.0);
        assert!(planar_matrix(&g, Vec3::new(1.0, 0.0, 0.0)).is_none());
        assert!(planar_matrix(&g, Vec3::new(0.0, 1.0, 0.0)).is_none());
    }

    /// The 18-bit RDP z-buffer stores z compressed (3-bit exponent of
    /// leading ones, 11-bit mantissa): what a depth reads back as.
    fn rdp_quantize(z: f32) -> u32 {
        let fixed = (z.clamp(0.0, 1.0) * 262_143.0) as u32;
        let ones = (fixed << 14).leading_ones().min(7);
        let shift = 6u32.saturating_sub(ones);
        (fixed >> shift) << shift
    }

    #[test]
    fn the_blob_offset_beats_every_targets_depth_precision() {
        // A shadow at view depth zv on a receiver at the same depth: after
        // the offset it must quantize strictly in front of the receiver
        // (a z-test "less" passes) at every distance a 1/128-scale world
        // reaches, and stay within a few centimeters of it up close.
        for caps in [Caps::N64, Caps::N3DS, Caps::PC, Caps::WEB] {
            let mut zv = 0.1f32;
            while zv < 2000.0 {
                let z = 1.0 - NEAR_PLANE / zv;
                let s = shadow_z(&caps, z);
                assert!(s < z, "{zv}: no offset");
                if caps.depth_mantissa > 0 {
                    // Quantized like the RDP, with a step of slack for the
                    // interpolation of either surface.
                    let step = caps.depth_step(z);
                    assert!(
                        rdp_quantize(s + step) < rdp_quantize(z - step) || s == 0.0,
                        "{zv} m: shadow {s} vs receiver {z} share an RDP depth"
                    );
                } else {
                    let q = (1u32 << caps.depth_bits) as f32;
                    assert!((s * q).floor() + 2.0 <= (z * q).floor(), "{caps:?} at {zv}");
                }
                // Lifted no more than this along the view ray (meters).
                let back = NEAR_PLANE / (1.0 - s) - zv;
                assert!(back.abs() < zv * zv * 0.02 + zv * 0.01, "{zv}: {back}");
                zv *= 1.3;
            }
        }
        // Up close on the N64: a blob 5 m away sits within two centimeters.
        let z = 1.0 - NEAR_PLANE / 5.0;
        let lift = 5.0 - NEAR_PLANE / (1.0 - shadow_z(&Caps::N64, z));
        assert!(lift > 0.0 && lift < 0.02, "{lift}");
    }

    #[test]
    fn rdp_steps_follow_the_compressed_format() {
        // Near the camera the mantissa's step, far away the 18-bit floor.
        let near = 1.0 - NEAR_PLANE / 0.2;
        assert!((Caps::N64.depth_step(near) - (1.0 - near) / 2048.0).abs() < 1e-9);
        let far = 1.0 - NEAR_PLANE / 500.0;
        assert_eq!(Caps::N64.depth_step(far), 1.0 / 262_144.0);
        assert_eq!(Caps::PC.depth_step(0.5), Caps::PC.depth_step(0.999));
    }

    #[test]
    fn the_blob_texture_is_a_soft_disc_that_ends_at_the_rim() {
        let mut t = [0u8; BLOB_TEXTURE_SIZE * BLOB_TEXTURE_SIZE];
        for softness in [0u8, 160, 255] {
            blob_texels(softness, &mut t);
            let n = BLOB_TEXTURE_SIZE;
            let at = |x: usize, y: usize| t[y * n + x];
            assert!(at(n / 2, n / 2) >= 250, "dark middle");
            for k in 0..n {
                for (x, y) in [(0, k), (k, 0), (n - 1, k), (k, n - 1)] {
                    assert_eq!(at(x, y), 0, "rim {x},{y} at softness {softness}");
                }
            }
            // Symmetric, so no platform's texture orientation matters.
            for y in 0..n {
                for x in 0..n {
                    assert_eq!(at(x, y), at(n - 1 - x, y));
                    assert_eq!(at(x, y), at(x, n - 1 - y));
                }
            }
        }
        // Harder blobs keep more of the disc dark.
        let dark = |s: u8| {
            let mut t = [0u8; BLOB_TEXTURE_SIZE * BLOB_TEXTURE_SIZE];
            blob_texels(s, &mut t);
            t.iter().filter(|&&v| v > 200).count()
        };
        assert!(dark(0) > dark(160) && dark(160) > dark(255));
    }

    fn frame_with(caps: &Caps, settings: ShadowSettings, planes: &[ShadowPlane]) -> ShadowFrame {
        let mut f = ShadowFrame::OFF;
        f.set(Some(&settings), caps, ShadowTier::Map);
        f.set_receivers(planes);
        f
    }

    #[test]
    fn tiers_follow_the_target_the_game_and_the_cap() {
        let s = ShadowSettings::DEFAULT;
        let tier = |caps: &Caps, max: ShadowTier, cap: ShadowTier| {
            let mut f = ShadowFrame::OFF;
            f.set(Some(&ShadowSettings { tier_max: max, ..s }), caps, cap);
            f.tier()
        };
        assert_eq!(
            tier(&Caps::N64, ShadowTier::Map, ShadowTier::Map),
            ShadowTier::Blob
        );
        assert_eq!(
            tier(&Caps::N3DS, ShadowTier::Map, ShadowTier::Map),
            ShadowTier::Planar
        );
        assert_eq!(
            tier(&Caps::PC, ShadowTier::Map, ShadowTier::Map),
            ShadowTier::Map
        );
        assert_eq!(
            tier(&Caps::WEB, ShadowTier::Map, ShadowTier::Map),
            ShadowTier::Map
        );
        assert_eq!(
            tier(&Caps::PC, ShadowTier::Blob, ShadowTier::Map),
            ShadowTier::Blob
        );
        assert_eq!(
            tier(&Caps::PC, ShadowTier::Map, ShadowTier::Planar),
            ShadowTier::Planar
        );
        let mut off = ShadowFrame::OFF;
        off.set(None, &Caps::PC, ShadowTier::Map);
        assert_eq!(off.tier(), ShadowTier::None);
        assert_eq!(off.plan(ShadowCast::Blob { radius: 1.0 }), ShadowPlan::None);
    }

    #[test]
    fn casts_plan_per_tier_and_the_n64_budgets_its_planar_casters() {
        let s = ShadowSettings::DEFAULT;
        let mut n64 = frame_with(&Caps::N64, s, &[]);
        assert_eq!(
            n64.plan(ShadowCast::Blob { radius: 0.3 }),
            ShadowPlan::Blob { radius: 0.3 }
        );
        assert_eq!(
            n64.plan(ShadowCast::auto(0.4)),
            ShadowPlan::Blob { radius: 0.4 }
        );
        assert_eq!(n64.plan(ShadowCast::auto(0.0)), ShadowPlan::None);
        let budget = Caps::N64.max_planar_shadows as usize;
        for _ in 0..budget {
            assert_eq!(n64.plan(ShadowCast::Mesh), ShadowPlan::Planar);
        }
        assert_eq!(n64.plan(ShadowCast::Mesh), ShadowPlan::None);
        assert_eq!(n64.stats.over_budget, 1);
        let mut n3ds = frame_with(&Caps::N3DS, s, &[]);
        assert_eq!(n3ds.plan(ShadowCast::auto(0.4)), ShadowPlan::Planar);
        assert_eq!(n3ds.plan(ShadowCast::Mesh), ShadowPlan::Planar);
        let mut pc = frame_with(&Caps::PC, s, &[]);
        assert_eq!(pc.plan(ShadowCast::auto(0.4)), ShadowPlan::Map);
        assert_eq!(
            pc.plan(ShadowCast::Blob { radius: 0.2 }),
            ShadowPlan::Blob { radius: 0.2 }
        );
        assert_eq!(pc.plan(ShadowCast::None), ShadowPlan::None);
    }

    fn ctx() -> TnlContext {
        let cam = Camera3 {
            eye: Vec3::new(0.0, 4.0, -6.0),
            target: Vec3::ZERO,
            fov_y: 1.0,
        };
        TnlContext::new(&cam, &DEFAULT_LIGHT, Vec2::new(320.0, 240.0))
    }

    #[test]
    fn a_blob_lands_on_the_receiver_under_its_caster_and_is_cut_to_it() {
        let low = ShadowPlane::horizontal(Vec3::new(0.0, 0.0, 0.0), 5.0, 5.0);
        let step = ShadowPlane::horizontal(Vec3::new(2.0, 1.0, 0.0), 0.5, 0.5);
        let f = frame_with(&Caps::N64, ShadowSettings::DEFAULT, &[low, step]);
        // Over the floor only.
        let b = f.blob(&ctx(), Vec3::new(-1.0, 0.5, 0.0), 0.5, 0.4).unwrap();
        assert!(b.corners.iter().all(|c| c.y == 0.0));
        assert_eq!(b.color.a, ShadowSettings::DEFAULT.color.a);
        // Over the step: on its top, cut at its edge (x 1.5..2.5).
        let e = f.blob(&ctx(), Vec3::new(2.4, 1.5, 0.0), 0.5, 0.4).unwrap();
        assert!(e.corners.iter().all(|c| c.y == 1.0 && c.x <= 2.5 + 1e-6));
        let max_u = e.uv.iter().fold(0.0f32, |m, uv| m.max(uv.x));
        assert!(
            (max_u - 0.625).abs() < 1e-4,
            "{max_u}: the cut keeps the texture in place"
        );
        // Higher up: smaller and fainter; out of reach: nothing.
        let up = f.blob(&ctx(), Vec3::new(-1.0, 1.6, 0.0), 0.5, 0.4).unwrap();
        assert!(up.color.a < b.color.a);
        assert!((up.corners[1].x - up.corners[0].x) < (b.corners[1].x - b.corners[0].x));
        assert!(
            f.blob(&ctx(), Vec3::new(-1.0, 9.0, 0.0), 0.5, 0.4)
                .is_none()
        );
        // Past the shadow distance, or with nothing under it: nothing.
        assert!(f.blob(&ctx(), Vec3::new(0.0, 0.5, 4.9), 0.5, 0.4).is_some());
        let near = frame_with(
            &Caps::N64,
            ShadowSettings {
                distance: 3.0,
                ..ShadowSettings::DEFAULT
            },
            &[low],
        );
        assert!(
            near.blob(&ctx(), Vec3::new(0.0, 0.5, 4.9), 0.5, 0.4)
                .is_none()
        );
        assert!(f.blob(&ctx(), Vec3::new(9.0, 0.5, 0.0), 0.5, 0.4).is_none());
    }

    /// Distance attenuation (#86): a blob is darkest and widest at contact
    /// and fades and shrinks as its caster rises, over `fade_distance` when
    /// the game sets one, else ADR 0025's `4·radius + 1` (bit for bit).
    #[test]
    fn a_blob_fades_and_shrinks_with_its_casters_height() {
        let ground = [ShadowPlane::ground(0.0)];
        let blob = |s: ShadowSettings, y: f32| {
            frame_with(&Caps::N64, s, &ground).blob(&ctx(), Vec3::new(0.0, y, 0.0), 0.25, 0.4)
        };
        let width = |b: &BlobQuad| b.corners[1].x - b.corners[0].x;
        let old = ShadowSettings::DEFAULT;
        let fade = ShadowSettings {
            fade_distance: 2.0,
            ..ShadowSettings::DEFAULT
        };
        // At contact (the sphere's bottom on the ground): full strength,
        // full width, whatever the fade.
        for s in [old, fade] {
            let b = blob(s, 0.25).unwrap();
            assert_eq!(b.color.a, s.color.a);
            assert!((width(&b) - 0.8).abs() < 1e-5);
        }
        // Rising: monotonically fainter and narrower, gone at the fade.
        let mut last = (u8::MAX, f32::MAX);
        for k in 1..8 {
            let b = blob(fade, 0.25 + k as f32 * 0.25).unwrap();
            assert!(b.color.a < last.0 && width(&b) < last.1, "{k}: {b:?}");
            last = (b.color.a, width(&b));
        }
        assert!(blob(fade, 0.25 + 2.0).is_none(), "faded out at 2 m");
        // Half way: half the strength, 80% of the width.
        let half = blob(fade, 0.25 + 1.0).unwrap();
        assert_eq!(half.color.a, (140.0f32 * 0.5 + 0.5) as u8);
        assert!((width(&half) - 0.8 * 0.8).abs() < 1e-5);
        // 0 keeps the old reach exactly: 4·0.4 + 1 = 2.6 m.
        assert_eq!(old.blob_fade(0.4), 4.0 * 0.4 + 1.0);
        assert!(blob(old, 0.25 + 2.5).is_some());
        assert!(blob(old, 0.25 + 2.6).is_none());
        let legacy = blob(old, 1.3).unwrap();
        let rise = (1.3f32 - 0.25) / (4.0 * 0.4 + 1.0);
        assert_eq!(legacy.color.a, (140.0 * (1.0 - rise) + 0.5) as u8);
    }

    #[test]
    fn contact_settings_clamp_and_default_to_the_old_look() {
        let d = ShadowSettings::DEFAULT;
        assert!(!d.contact(), "off by default: goldens unchanged");
        assert_eq!((d.soft_growth, d.fade_distance), (0.0, 0.0));
        let c = |g: f32, f: f32| {
            ShadowSettings {
                soft_growth: g,
                fade_distance: f,
                ..d
            }
            .clamped()
        };
        assert_eq!(c(f32::NAN, f32::NAN), d);
        assert_eq!(c(-1.0, -3.0), d);
        assert_eq!(c(99.0, 1.5).soft_growth, ShadowSettings::MAX_SOFT_GROWTH);
        assert_eq!(c(0.2, 1.5).fade_distance, 1.5);
        assert_eq!(c(0.0, f32::INFINITY).fade_distance, f32::INFINITY);
        assert!(c(0.2, 0.0).contact() && c(0.0, 3.0).contact());
    }

    #[test]
    fn blob_triangles_sit_just_in_front_of_their_receiver() {
        let f = frame_with(
            &Caps::N64,
            ShadowSettings::DEFAULT,
            &[ShadowPlane::ground(0.0)],
        );
        let ctx = ctx();
        let b = f.blob(&ctx, Vec3::new(0.0, 0.5, 0.0), 0.5, 0.6).unwrap();
        let mut out = [ScreenTri {
            pts: [Vec2::ZERO; 3],
            colors: [Color::BLACK; 3],
            depth: 0.0,
            z: [0.0; 3],
            uv: [Vec2::ZERO; 3],
            w: [1.0; 3],
        }; 7];
        let n = blob_tris(&ctx, &Caps::N64, &b, &mut out);
        assert_eq!(n, 2, "a quad in view: two triangles");
        for t in &out[..n] {
            for i in 0..3 {
                let floor = 1.0 - NEAR_PLANE / t.w[i];
                assert!(t.z[i] < floor && t.z[i] > floor - 1e-3);
            }
        }
        // Crossing the near plane: clipped, still drawn.
        let close = TnlContext::new(
            &Camera3 {
                eye: Vec3::new(0.0, 0.3, -0.2),
                target: Vec3::new(0.0, 0.0, 1.0),
                fov_y: 1.0,
            },
            &DEFAULT_LIGHT,
            Vec2::new(320.0, 240.0),
        );
        let b = f.blob(&close, Vec3::new(0.0, 0.2, 0.2), 0.2, 1.0).unwrap();
        let n = blob_tris(&close, &Caps::N64, &b, &mut out);
        assert!(n >= 2);
        assert!(
            out[..n]
                .iter()
                .all(|t| t.w.iter().all(|&w| w >= NEAR_PLANE - 1e-5))
        );
    }

    #[test]
    fn receiver_clips_keep_inside_triangles_whole_and_cut_the_rest() {
        let ctx = ctx();
        let plane = ShadowPlane::horizontal(Vec3::ZERO, 1.0, 1.0);
        let clip = ReceiverClip::new(&ctx, &plane).unwrap();
        let proj = |p: Vec3| {
            let v = ctx.view_point(p);
            (
                Vec2::new(
                    ctx.center().x + v.x * ctx.focal() / v.z,
                    ctx.center().y - v.y * ctx.focal() / v.z,
                ),
                v.z,
            )
        };
        let tri = |a: Vec3, b: Vec3, c: Vec3| {
            let [(pa, wa), (pb, wb), (pc, wc)] = [proj(a), proj(b), proj(c)];
            ScreenTri {
                pts: [pa, pb, pc],
                colors: [Color::BLACK; 3],
                depth: wa.max(wb).max(wc),
                z: [wa, wb, wc].map(|w| 1.0 - NEAR_PLANE / w),
                uv: [Vec2::ZERO; 3],
                w: [wa, wb, wc],
            }
        };
        let inside = tri(
            Vec3::new(-0.5, 0.0, -0.5),
            Vec3::new(0.5, 0.0, -0.5),
            Vec3::new(0.0, 0.0, 0.5),
        );
        let mut got = std::vec::Vec::new();
        clip.emit(&inside, &mut |t| got.push(t));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].pts, inside.pts);
        // Half over the edge (x = 1): every piece left of it, and the cut
        // keeps the plane's depth (z is affine on screen).
        let across = tri(
            Vec3::new(0.0, 0.0, -0.5),
            Vec3::new(2.0, 0.0, -0.5),
            Vec3::new(0.0, 0.0, 0.5),
        );
        got.clear();
        clip.emit(&across, &mut |t| got.push(t));
        assert!(!got.is_empty());
        let edge = proj(Vec3::new(1.0, 0.0, -0.5)).0;
        for t in &got {
            for i in 0..3 {
                assert!(t.pts[i].x <= edge.x + 0.5, "{:?} past {edge:?}", t.pts[i]);
                let zw = 1.0 - NEAR_PLANE / t.w[i];
                assert!((zw - t.z[i]).abs() < 1e-5, "depth stays on the plane");
            }
        }
        // Endless receivers cut nothing.
        assert!(ReceiverClip::new(&ctx, &ShadowPlane::ground(0.0)).is_none());
    }

    #[test]
    fn planar_shadows_are_flat_offset_and_cut() {
        let tmdl = crate::render3d::tests::cube_tmdl();
        let mesh = Mesh::from_tmdl(&tmdl).unwrap();
        let ctx = ctx();
        let floor = ShadowPlane::horizontal(Vec3::ZERO, 1.2, 1.2);
        let mut f = frame_with(&Caps::N3DS, ShadowSettings::DEFAULT, &[floor]);
        assert_eq!(f.plan(ShadowCast::Mesh), ShadowPlan::Planar);
        let m = Mat34::from_rotation_scale_translation(
            Vec3::ZERO,
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(0.0, 1.0, 0.0),
        );
        let sphere = caster_sphere(mesh.bounds(), &m);
        let draw = f.planar(&ctx, sphere.0, &m, &ModelParams::DEFAULT).unwrap();
        assert_eq!(draw.params.shading, Shading::Unlit);
        assert_eq!(draw.params.tint.a, ShadowSettings::DEFAULT.color.a);
        let mut scratch = TnlScratch::new();
        let mut tris = std::vec::Vec::new();
        let stats = planar_tris(
            &f,
            &ctx,
            &mut scratch,
            &Caps::N3DS,
            &mesh,
            sphere,
            &draw,
            &mut |t| tris.push(t),
        );
        assert!(stats.emitted > 0 && !tris.is_empty());
        // Light-facing faces only (the back faces of the flattened cube are
        // culled): a convex caster covers each pixel once.
        assert!(stats.backfaced > 0);
        for t in &tris {
            for i in 0..3 {
                let floor_z = 1.0 - NEAR_PLANE / t.w[i];
                assert!(t.z[i] < floor_z, "in front of the floor");
                assert_eq!(t.colors[i].a, ShadowSettings::DEFAULT.color.a);
            }
        }
    }

    #[test]
    fn the_map_covers_the_view_and_the_casters_above_it() {
        let camera = Camera3 {
            eye: Vec3::new(1.0, 4.0, -8.0),
            target: Vec3::new(0.0, 0.5, 2.0),
            fov_y: 1.0,
        };
        let light = Vec3::new(0.45, -0.8, 0.35);
        let high = (Vec3::new(0.0, 30.0, 4.0), 1.0);
        let fit = map_fit(&camera, 16.0 / 9.0, light, 30.0, 2048, &[high]).unwrap();
        let view = camera.view();
        let (half, aspect) = (0.5f32, 16.0f32 / 9.0);
        let ty = crate::math3d::sin(half) / crate::math3d::cos(half);
        // The corners of the view slice up to the distance.
        for z in [0.1f32, 10.0, 30.0] {
            for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let v = Vec3::new(sx * ty * aspect * z, sy * ty * z, z);
                // View to world: the rows of the view matrix are its basis.
                let m = &view.m;
                let row = |i: usize| Vec3::new(m[i][0], m[i][1], m[i][2]);
                let w = camera.eye + row(0) * v.x + row(1) * v.y + row(2) * v.z;
                let p = fit.matrix.transform_point(w);
                assert!(p.x.abs() <= 1.001 && p.y.abs() <= 1.001, "{z} {p:?}");
                assert!((0.0..=1.0).contains(&p.z), "{z} {p:?}");
            }
        }
        // The high caster is inside the depth range, nearer the light.
        let top = fit
            .matrix
            .transform_point(high.0 - light.normalized() * high.1);
        assert!(top.z > 0.0 && top.z < 0.5, "{top:?}");
        assert!((fit.texel - 2.0 * fit.radius / 2048.0).abs() < 1e-6);
    }

    #[test]
    fn the_map_moves_in_whole_texels() {
        let light = Vec3::new(0.3, -0.9, 0.2);
        let p = Vec3::new(2.0, 0.0, 3.0);
        let base = Camera3 {
            eye: Vec3::new(0.0, 3.0, -6.0),
            target: Vec3::new(0.0, 0.0, 0.0),
            fov_y: 1.0,
        };
        let a = map_fit(&base, 1.5, light, 40.0, 1024, &[]).unwrap();
        for dx in [0.013f32, 0.21, 0.37, 1.9] {
            let moved = Camera3 {
                eye: base.eye + Vec3::new(dx, 0.0, dx * 0.5),
                target: base.target + Vec3::new(dx, 0.0, dx * 0.5),
                ..base
            };
            let b = map_fit(&moved, 1.5, light, 40.0, 1024, &[]).unwrap();
            assert!((a.texel - b.texel).abs() < 1e-6, "same size");
            // A fixed point moves across the map by whole texels.
            let (pa, pb) = (a.matrix.transform_point(p), b.matrix.transform_point(p));
            let step = 2.0 / 1024.0;
            for d in [(pa.x - pb.x) / step, (pa.y - pb.y) / step] {
                assert!(
                    (d - (d + 0.5) as i32 as f32).abs() < 0.01
                        || (d - (d - 0.5) as i32 as f32).abs() < 0.01,
                    "{d}"
                );
            }
        }
        assert_eq!(map_fit(&base, 1.5, Vec3::ZERO, 40.0, 1024, &[]), None);
    }
}
