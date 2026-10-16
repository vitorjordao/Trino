//! Local lights: point and spot lights on top of the global light.
//!
//! The global light ([`crate::render3d::Light`]: one directional light, an
//! ambient floor and fog) lights every draw on every target. Local lights
//! are positioned in the world and reach only a radius around them: a lamp,
//! a lantern, a spotlight on a stage. How a target evaluates them is its
//! [`LightTier`], from [`crate::Caps`] (ADR 0010):
//!
//! - [`LightTier::PerPixel`] (PC): per pixel, with specular highlights
//!   ([`crate::ModelParams::gloss`]) and wet surfaces
//!   ([`crate::Renderer::set_wetness`]).
//! - [`LightTier::PerVertex`] (3DS, N64): the engine's software T&L adds
//!   each light's diffuse term to the vertex colors (Gouraud), for the draws
//!   whose bounding sphere the light reaches. No specular, no wetness. On
//!   the N64 such a draw leaves the RSP for the CPU T&L
//!   ([`crate::RspReject::LocalLight`]): each lit draw costs its full CPU
//!   transform, so console games keep lights few and small, or draw a
//!   [`ground_pool`] decal instead.
//!
//! Every target uses at most [`crate::Caps::max_local_lights`] lights (the
//! first ones given). No local light set: every target draws exactly what
//! it drew before local lights existed.

use crate::math::{Color, Vec3};

/// Most local lights any target evaluates (the per-frame table's size).
pub const MAX_LOCAL_LIGHTS: usize = 8;

/// Most local lights the per-vertex tier evaluates (the T&L context's
/// table; per-vertex caps stay at or under it).
pub const MAX_VERTEX_LIGHTS: usize = 4;

/// How a target evaluates local lights ([`crate::Caps::light_tier`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LightTier {
    /// Per pixel, with specular and wetness (PC native).
    PerPixel,
    /// Per vertex in the software T&L, diffuse only (consoles and the PC's
    /// console profiles).
    PerVertex,
}

/// The shape of a local light.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LightKind {
    /// Shines every way from its position.
    Point,
    /// A cone along `dir` (world space, normalized by the engine). Full
    /// strength inside `inner` (half-angle, radians), fading to nothing at
    /// `outer`.
    Spot { dir: Vec3, inner: f32, outer: f32 },
}

/// A point or spot light ([`crate::Renderer::set_local_lights`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LocalLight {
    pub kind: LightKind,
    /// World position.
    pub pos: Vec3,
    pub color: Color,
    /// Multiplies `color` (1 = the color at full strength at the light).
    pub intensity: f32,
    /// Distance where the light reaches zero, world units. The falloff is
    /// `(1 - d²/radius²)²`: smooth, and exactly zero past the radius, so a
    /// light never touches a draw outside it.
    pub radius: f32,
}

impl LocalLight {
    /// A point light of `color` reaching `radius`.
    pub const fn point(pos: Vec3, color: Color, radius: f32) -> LocalLight {
        LocalLight {
            kind: LightKind::Point,
            pos,
            color,
            intensity: 1.0,
            radius,
        }
    }

    /// A spot light at `pos` shining along `dir`, full inside `inner` and
    /// fading out at `outer` (half-angles, radians).
    pub const fn spot(
        pos: Vec3,
        dir: Vec3,
        inner: f32,
        outer: f32,
        color: Color,
        radius: f32,
    ) -> LocalLight {
        LocalLight {
            kind: LightKind::Spot { dir, inner, outer },
            pos,
            color,
            intensity: 1.0,
            radius,
        }
    }

    /// The same light at another intensity.
    pub const fn with_intensity(self, intensity: f32) -> LocalLight {
        LocalLight { intensity, ..self }
    }

    /// Distance falloff at squared distance `d2`: `(1 - d²/r²)²`, zero
    /// past the radius.
    #[inline]
    pub fn falloff(&self, d2: f32) -> f32 {
        let r2 = self.radius * self.radius;
        if r2 <= 0.0 || d2 >= r2 {
            return 0.0;
        }
        let k = 1.0 - d2 / r2;
        k * k
    }
}

/// A renderer's current local lights: a fixed table (no allocation on
/// consoles) holding at most a target's cap.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LightList {
    lights: [LocalLight; MAX_LOCAL_LIGHTS],
    n: u8,
}

impl LightList {
    pub const EMPTY: LightList = LightList {
        lights: [LocalLight::point(Vec3::ZERO, Color::BLACK, 0.0); MAX_LOCAL_LIGHTS],
        n: 0,
    };

    /// Keep the first `cap` (at most [`MAX_LOCAL_LIGHTS`]) of `lights`.
    /// Returns whether the list changed.
    pub fn set(&mut self, lights: &[LocalLight], cap: u8) -> bool {
        let n = lights.len().min(cap as usize).min(MAX_LOCAL_LIGHTS);
        if self.as_slice() == &lights[..n] {
            return false;
        }
        self.lights[..n].copy_from_slice(&lights[..n]);
        self.n = n as u8;
        true
    }

    pub fn as_slice(&self) -> &[LocalLight] {
        &self.lights[..self.n as usize]
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
}

impl Default for LightList {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// `cos` of a spot's half-angles, clamped so the fade never divides by
/// zero: `(cos_outer, 1 / (cos_inner - cos_outer))`.
pub fn spot_cone(inner: f32, outer: f32) -> (f32, f32) {
    let outer = outer.max(1e-3);
    let inner = inner.clamp(0.0, outer);
    let (ci, co) = (crate::math3d::cos(inner), crate::math3d::cos(outer));
    (co, 1.0 / (ci - co).max(1e-4))
}

/// The cone factor of a spot (0 outside, 1 inside the inner cone) for the
/// cosine `c` between its axis and the direction to the lit point.
#[inline]
pub fn cone_factor(c: f32, cos_outer: f32, inv_range: f32) -> f32 {
    let t = ((c - cos_outer) * inv_range).clamp(0.0, 1.0);
    // Smoothstep: no visible ring at the inner edge.
    t * t * (3.0 - 2.0 * t)
}

/// The pool of light a [`LocalLight`] throws on a horizontal floor at
/// height `floor_y` ([`ground_pool`]): what a game draws as a floor decal
/// where geometry isn't lit per vertex (the N64, whose lit draws leave the
/// RSP).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GroundPool {
    /// Center on the floor.
    pub center: Vec3,
    /// Radius on the floor where the light still adds at least 1/16 of its
    /// peak.
    pub radius: f32,
    /// The light's color at the pool's center (intensity and falloff
    /// applied, saturated at 255), alpha 255.
    pub color: Color,
}

/// Where and how bright `light` lights the floor plane `y = floor_y` (a
/// floor facing up): `None` when it doesn't reach the floor. A point light
/// makes a disc under it; a spot makes the disc where its axis meets the
/// floor, sized by its outer cone (an upward spot makes none).
pub fn ground_pool(light: &LocalLight, floor_y: f32) -> Option<GroundPool> {
    let h = light.pos.y - floor_y;
    if h <= 0.0 || h >= light.radius {
        return None;
    }
    // On the floor, d² = h² + x², so the falloff is `(below - x²/r²)²`
    // with `below = 1 - h²/r²`: it is 1/16 of its peak where
    // `below - x²/r² = below / 4`.
    let r2 = light.radius * light.radius;
    let below = 1.0 - h * h / r2;
    let reach = |k: f32| crate::math3d::sqrt(((below - k) * r2).max(0.0));
    let peak = light.falloff(h * h) * light.intensity;
    let (center, radius, peak) = match light.kind {
        LightKind::Point => (
            Vec3::new(light.pos.x, floor_y, light.pos.z),
            reach(below * 0.25),
            peak,
        ),
        LightKind::Spot { dir, outer, .. } => {
            let d = dir.normalized();
            if d.y >= -1e-3 {
                return None;
            }
            // The axis meets the floor at t = h / -d.y.
            let t = h / -d.y;
            if t >= light.radius {
                return None;
            }
            let center = Vec3::new(light.pos.x + d.x * t, floor_y, light.pos.z + d.z * t);
            // The outer cone's footprint, no wider than the light reaches.
            let tan = crate::math3d::sin(outer) / crate::math3d::cos(outer).max(1e-3);
            let reach = crate::math3d::sqrt(r2 - t * t);
            let k = light.falloff(t * t) * light.intensity;
            (center, (t * tan).min(reach), k)
        }
    };
    let c = |v: u8| (v as f32 * peak).clamp(0.0, 255.0) as u8;
    Some(GroundPool {
        center,
        radius,
        color: Color::rgb(c(light.color.r), c(light.color.g), c(light.color.b)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_keeps_the_first_lights_up_to_the_cap() {
        let a = LocalLight::point(Vec3::ZERO, Color::WHITE, 1.0);
        let b = LocalLight::point(Vec3::ONE, Color::WHITE, 1.0);
        let mut list = LightList::EMPTY;
        assert!(list.set(&[a, b, a], 2));
        assert_eq!(list.as_slice(), &[a, b]);
        assert!(!list.set(&[a, b], 2), "same lights: no change");
        assert!(list.set(&[], 2));
        assert!(list.is_empty());
        assert!(!list.set(&[a], 0), "cap 0 keeps nothing");
    }

    #[test]
    fn falloff_is_smooth_and_zero_past_the_radius() {
        let l = LocalLight::point(Vec3::ZERO, Color::WHITE, 2.0);
        assert_eq!(l.falloff(0.0), 1.0);
        assert!((l.falloff(1.0) - 0.5625).abs() < 1e-6);
        assert_eq!(l.falloff(4.0), 0.0);
        assert_eq!(l.falloff(9.0), 0.0);
    }

    #[test]
    fn spot_cone_fades_between_the_angles() {
        let (co, inv) = spot_cone(0.2, 0.5);
        assert_eq!(cone_factor(1.0, co, inv), 1.0);
        assert_eq!(cone_factor(crate::math3d::cos(0.6), co, inv), 0.0);
        let mid = cone_factor(crate::math3d::cos(0.35), co, inv);
        assert!(mid > 0.1 && mid < 0.9, "{mid}");
    }

    #[test]
    fn a_point_light_pools_under_itself() {
        let l = LocalLight::point(Vec3::new(2.0, 1.0, -3.0), Color::rgb(255, 128, 0), 4.0);
        let p = ground_pool(&l, 0.0).unwrap();
        assert_eq!(p.center, Vec3::new(2.0, 0.0, -3.0));
        assert!(p.radius > 1.0 && p.radius < 4.0, "{}", p.radius);
        // 1 m below a 4 m light: (1 - 1/16)² of full.
        assert_eq!(p.color, Color::rgb(224, 112, 0));
        // Below the floor, or the floor out of reach: no pool.
        assert!(ground_pool(&l, 1.5).is_none());
        assert!(ground_pool(&l, -3.5).is_none());
    }

    #[test]
    fn a_spot_pools_where_its_axis_meets_the_floor() {
        let l = LocalLight::spot(
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            0.2,
            0.4,
            Color::WHITE,
            6.0,
        );
        let p = ground_pool(&l, 0.0).unwrap();
        assert!(
            (p.center.x - 2.0).abs() < 1e-5 && p.center.z == 0.0,
            "{p:?}"
        );
        let up = LocalLight::spot(
            Vec3::ZERO,
            Vec3::new(0.0, 1.0, 0.0),
            0.2,
            0.4,
            Color::WHITE,
            6.0,
        );
        assert!(ground_pool(&up, -1.0).is_none());
    }
}
