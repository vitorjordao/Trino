//! The number type of the character controller (issue #66 phase 4).
//!
//! [`CharacterController`](super::CharacterController) and its geometry are
//! generic over [`Scalar`]: `f32` (the default, what castle64 runs) or
//! [`Fx`], Q16.16 fixed point. Fixed point gives the same bits on every
//! target by construction (integer adds, `i64` products) — for sports sims
//! and lockstep netcode (#67) that cannot rely on every compiler and FPU
//! rounding `f32` alike. Only the instantiations a game uses are compiled.
//!
//! Range: Q16.16 holds ±32 767 with 1/65 536 steps, so a fixed-point level
//! stays within a few thousand units of the origin (products of two
//! coordinates must stay below 32 767: keep levels under ~150 units across,
//! or scale the world down as Solid Eleven does). Triangle-mesh colliders
//! are `f32` only ([`MeshQuery`] is implemented for `f32`); a fixed-point
//! game describes its level with boxes, one-way boxes and ramps.

use core::fmt::Debug;
use core::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

use crate::math::Vec3;
use crate::math3d;

/// A number the controller computes with.
pub trait Scalar:
    Copy
    + PartialEq
    + PartialOrd
    + Debug
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + Neg<Output = Self>
    + AddAssign
    + SubAssign
{
    const ZERO: Self;
    const ONE: Self;
    /// Nearest value (f32: itself).
    fn from_f32(v: f32) -> Self;
    fn to_f32(self) -> f32;
    fn from_int(v: i32) -> Self;
    /// Truncation toward zero (saturating).
    fn trunc_i32(self) -> i32;
    fn abs(self) -> Self;
    fn min(self, o: Self) -> Self;
    fn max(self, o: Self) -> Self;
    fn sqrt(self) -> Self;
    #[inline]
    fn clamp(self, lo: Self, hi: Self) -> Self {
        self.max(lo).min(hi)
    }
    /// The controller's step for this scalar, compiled once in this crate
    /// ([`CharacterController::step`](super::CharacterController::step)
    /// calls it).
    #[doc(hidden)]
    fn controller_step(
        c: &mut super::CharacterController<Self>,
        dt: Self,
        surfaces: &[super::Surface<Self>],
        meshes: &[&dyn MeshQuery<Self>],
    ) -> super::StepReport<Self>;
}

impl Scalar for f32 {
    const ZERO: f32 = 0.0;
    const ONE: f32 = 1.0;
    #[inline]
    fn from_f32(v: f32) -> f32 {
        v
    }
    #[inline]
    fn to_f32(self) -> f32 {
        self
    }
    #[inline]
    fn from_int(v: i32) -> f32 {
        v as f32
    }
    #[inline]
    fn trunc_i32(self) -> i32 {
        math3d::to_i32(self)
    }
    #[inline]
    fn abs(self) -> f32 {
        f32::abs(self)
    }
    #[inline]
    fn min(self, o: f32) -> f32 {
        f32::min(self, o)
    }
    #[inline]
    fn max(self, o: f32) -> f32 {
        f32::max(self, o)
    }
    #[inline]
    fn sqrt(self) -> f32 {
        math3d::sqrt(self)
    }
    fn controller_step(
        c: &mut super::CharacterController<f32>,
        dt: f32,
        surfaces: &[super::Surface<f32>],
        meshes: &[&dyn MeshQuery<f32>],
    ) -> super::StepReport<f32> {
        c.step_impl(dt, surfaces, meshes)
    }
}

/// Q16.16 fixed point: `Fx(65536)` is 1.0. Products round toward −∞
/// (arithmetic shift), quotients toward zero; division saturates at
/// ±`i32::MAX` (symmetric, so a saturated value negates safely) — the same
/// bits as Solid Eleven's own `Fx` (tested there).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Fx(pub i32);

impl Debug for Fx {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Fx({})", self.to_f32())
    }
}

impl Fx {
    pub const FRAC_BITS: u32 = 16;
    pub const MAX: Fx = Fx(i32::MAX);
    /// The most negative value division saturates to (`-MAX`).
    pub const MIN: Fx = Fx(-i32::MAX);
    /// The smallest step (1/65 536).
    pub const EPSILON: Fx = Fx(1);

    pub const fn from_bits(b: i32) -> Fx {
        Fx(b)
    }

    pub const fn to_bits(self) -> i32 {
        self.0
    }

    /// `a / b` as a constant (`Fx::ratio(1, 50)` = 0.02).
    pub const fn ratio(a: i32, b: i32) -> Fx {
        Fx((((a as i64) << 16) / b as i64) as i32)
    }
}

impl Add for Fx {
    type Output = Fx;
    #[inline]
    fn add(self, o: Fx) -> Fx {
        Fx(self.0.wrapping_add(o.0))
    }
}

impl Sub for Fx {
    type Output = Fx;
    #[inline]
    fn sub(self, o: Fx) -> Fx {
        Fx(self.0.wrapping_sub(o.0))
    }
}

impl Mul for Fx {
    type Output = Fx;
    #[inline]
    fn mul(self, o: Fx) -> Fx {
        Fx(((self.0 as i64 * o.0 as i64) >> 16) as i32)
    }
}

impl Div for Fx {
    type Output = Fx;
    #[inline]
    fn div(self, o: Fx) -> Fx {
        if o.0 == 0 {
            return if self.0 >= 0 { Fx::MAX } else { Fx::MIN };
        }
        let q = ((self.0 as i64) << 16) / o.0 as i64;
        Fx(q.clamp(-(i32::MAX as i64), i32::MAX as i64) as i32)
    }
}

impl Neg for Fx {
    type Output = Fx;
    #[inline]
    fn neg(self) -> Fx {
        Fx(self.0.wrapping_neg())
    }
}

impl AddAssign for Fx {
    #[inline]
    fn add_assign(&mut self, o: Fx) {
        *self = *self + o;
    }
}

impl SubAssign for Fx {
    #[inline]
    fn sub_assign(&mut self, o: Fx) {
        *self = *self - o;
    }
}

impl Scalar for Fx {
    const ZERO: Fx = Fx(0);
    const ONE: Fx = Fx(1 << 16);
    fn from_f32(v: f32) -> Fx {
        let s = v * 65536.0;
        Fx(math3d::to_i32(if s >= 0.0 { s + 0.5 } else { s - 0.5 }))
    }
    fn to_f32(self) -> f32 {
        self.0 as f32 * (1.0 / 65536.0)
    }
    fn from_int(v: i32) -> Fx {
        Fx(v.saturating_mul(1 << 16))
    }
    fn trunc_i32(self) -> i32 {
        // Toward zero, like `as`.
        if self.0 >= 0 {
            self.0 >> 16
        } else {
            -((-(self.0 as i64)) >> 16) as i32
        }
    }
    #[inline]
    fn abs(self) -> Fx {
        Fx(self.0.wrapping_abs())
    }
    #[inline]
    fn min(self, o: Fx) -> Fx {
        if o.0 < self.0 { o } else { self }
    }
    #[inline]
    fn max(self, o: Fx) -> Fx {
        if o.0 > self.0 { o } else { self }
    }
    /// Integer square root of the Q32.32 value (exact floor).
    fn sqrt(self) -> Fx {
        if self.0 <= 0 {
            return Fx(0);
        }
        let n = (self.0 as u64) << 16;
        // Newton from above: converges to floor(sqrt(n)).
        let mut x = 1u64 << ((64 - n.leading_zeros()).div_ceil(2));
        loop {
            let y = (x + n / x) >> 1;
            if y >= x {
                return Fx(x as i32);
            }
            x = y;
        }
    }
    fn controller_step(
        c: &mut super::CharacterController<Fx>,
        dt: Fx,
        surfaces: &[super::Surface<Fx>],
        meshes: &[&dyn MeshQuery<Fx>],
    ) -> super::StepReport<Fx> {
        c.step_impl(dt, surfaces, meshes)
    }
}

/// A 3-vector of `S` (the controller's; `Vec3` converts with `from`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct V3<S> {
    pub x: S,
    pub y: S,
    pub z: S,
}

impl<S: Scalar> V3<S> {
    pub const fn new(x: S, y: S, z: S) -> Self {
        V3 { x, y, z }
    }

    pub const ZERO: V3<S> = V3 {
        x: S::ZERO,
        y: S::ZERO,
        z: S::ZERO,
    };

    #[inline]
    pub fn dot(self, o: V3<S>) -> S {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn from_f32(v: Vec3) -> Self {
        V3::new(S::from_f32(v.x), S::from_f32(v.y), S::from_f32(v.z))
    }

    pub fn to_f32(self) -> Vec3 {
        Vec3::new(self.x.to_f32(), self.y.to_f32(), self.z.to_f32())
    }

    #[inline]
    pub(crate) fn get(self, a: usize) -> S {
        match a {
            0 => self.x,
            1 => self.y,
            _ => self.z,
        }
    }

    #[inline]
    pub(crate) fn set(&mut self, a: usize, v: S) {
        match a {
            0 => self.x = v,
            1 => self.y = v,
            _ => self.z = v,
        }
    }
}

impl From<Vec3> for V3<f32> {
    #[inline]
    fn from(v: Vec3) -> Self {
        V3::new(v.x, v.y, v.z)
    }
}

impl From<V3<f32>> for Vec3 {
    #[inline]
    fn from(v: V3<f32>) -> Self {
        Vec3::new(v.x, v.y, v.z)
    }
}

impl<S: Scalar> Add for V3<S> {
    type Output = V3<S>;
    #[inline]
    fn add(self, o: V3<S>) -> V3<S> {
        V3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl<S: Scalar> Sub for V3<S> {
    type Output = V3<S>;
    #[inline]
    fn sub(self, o: V3<S>) -> V3<S> {
        V3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl<S: Scalar> Mul<S> for V3<S> {
    type Output = V3<S>;
    #[inline]
    fn mul(self, k: S) -> V3<S> {
        V3::new(self.x * k, self.y * k, self.z * k)
    }
}

impl<S: Scalar> AddAssign for V3<S> {
    #[inline]
    fn add_assign(&mut self, o: V3<S>) {
        *self = *self + o;
    }
}

/// An axis-aligned box of `S`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Box3<S> {
    pub min: V3<S>,
    pub max: V3<S>,
}

impl<S: Scalar> Box3<S> {
    pub const fn new(min: V3<S>, max: V3<S>) -> Self {
        Box3 { min, max }
    }

    pub fn from_center_half(c: V3<S>, h: V3<S>) -> Self {
        Box3::new(c - h, c + h)
    }

    /// Strict overlap, like [`Aabb::overlaps`](super::Aabb::overlaps).
    #[inline]
    pub fn overlaps(&self, o: &Box3<S>) -> bool {
        self.min.x < o.max.x
            && o.min.x < self.max.x
            && self.min.y < o.max.y
            && o.min.y < self.max.y
            && self.min.z < o.max.z
            && o.min.z < self.max.z
    }

    /// Strict overlap of the XZ footprints.
    #[inline]
    pub fn overlaps_xz(&self, o: &Box3<S>) -> bool {
        self.min.x < o.max.x && o.min.x < self.max.x && self.min.z < o.max.z && o.min.z < self.max.z
    }

    pub fn from_aabb(b: &super::Aabb) -> Self {
        Box3::new(V3::from_f32(b.min), V3::from_f32(b.max))
    }

    pub fn to_aabb(&self) -> super::Aabb {
        super::Aabb::new(self.min.to_f32(), self.max.to_f32())
    }
}

/// Where a mesh holds a body up: the surface height under a footprint and
/// its normal.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MeshSupport<S> {
    pub height: S,
    pub normal: V3<S>,
    /// The triangle (for tools and debug overlays).
    pub tri: u32,
}

/// Collision geometry beyond boxes and ramps, queried by the controller:
/// `f32` triangle meshes ([`MeshInstance`](super::MeshInstance)), handed
/// to the controller as `&[&dyn MeshQuery<S>]` (`&[]` when there are none).
pub trait MeshQuery<S: Scalar> {
    /// The highest **walkable** surface (normal.y ≥ `min_normal_y`) under
    /// the XZ footprint of `foot` whose height is at most `up_to`.
    fn support(&self, foot: &Box3<S>, up_to: S, min_normal_y: f32) -> Option<MeshSupport<S>>;
    /// Does a **steep** triangle (a wall) overlap `body`?
    fn blocks(&self, body: &Box3<S>, min_normal_y: f32) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point_arithmetic() {
        let a = Fx::from_f32(1.5);
        let b = Fx::from_f32(-2.25);
        assert_eq!(a.0, 98304);
        assert_eq!((a * b).to_f32(), -3.375);
        assert_eq!((b / a).to_f32(), -1.5);
        assert_eq!((a + b).to_f32(), -0.75);
        assert_eq!(Fx::from_int(9).sqrt(), Fx::from_int(3));
        assert!((Fx::from_f32(2.0).sqrt().to_f32() - core::f32::consts::SQRT_2).abs() < 2e-5);
        assert_eq!(Fx::from_f32(-2.75).trunc_i32(), -2);
        assert_eq!(Fx::from_f32(2.75).trunc_i32(), 2);
        assert_eq!(Fx::ONE / Fx::ZERO, Fx::MAX);
        assert_eq!(Fx::ratio(1, 50), Fx(1310));
        assert_eq!(Fx::from_f32(0.02), Fx(1311));
        assert!(Fx::from_f32(-1.0) < Fx::ZERO);
        assert_eq!(Scalar::max(a, b), a);
        assert_eq!(Scalar::clamp(Fx::from_f32(3.0), Fx::ZERO, Fx::ONE), Fx::ONE);
    }

    #[test]
    fn fixed_sqrt_is_the_exact_floor() {
        for bits in [1, 2, 3, 65535, 65536, 65537, 1 << 20, 123_456_789, i32::MAX] {
            let r = Fx(bits).sqrt().0 as u64;
            let n = (bits as u64) << 16;
            assert!(r * r <= n && (r + 1) * (r + 1) > n, "{bits}");
        }
    }
}
