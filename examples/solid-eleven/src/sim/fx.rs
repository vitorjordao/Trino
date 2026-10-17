//! Fixed-point numbers for the match simulation.
//!
//! `Fx` is a Q16.16 signed number (1.0 = 65536): meters, seconds, meters
//! per second. Integer math gives the same bits on every target with no
//! float rounding rules to agree on, and a float-to-int conversion on the
//! N64 costs ~70 cycles (AGENTS.md › Pitfalls), so the sim never converts.
//! Products go through `i64` (`mult` on MIPS, one instruction); the few
//! divisions are kept out of per-pair loops.
//!
//! The render and UI layers read `Fx` through [`Fx::to_f32`] only.

use core::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// Q16.16 fixed point.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Fx(pub i32);

impl Fx {
    pub const FRAC: u32 = 16;
    pub const ZERO: Fx = Fx(0);
    pub const ONE: Fx = Fx(1 << 16);
    pub const HALF: Fx = Fx(1 << 15);
    pub const MAX: Fx = Fx(i32::MAX);

    /// A whole number.
    #[inline]
    pub const fn int(i: i32) -> Fx {
        Fx(i << 16)
    }

    /// `n / d`, rounded toward zero (const: tuning tables).
    #[inline]
    pub const fn ratio(n: i32, d: i32) -> Fx {
        Fx((((n as i64) << 16) / d as i64) as i32)
    }

    /// Thousandths: `Fx::milli(1500)` = 1.5.
    #[inline]
    pub const fn milli(m: i32) -> Fx {
        Fx::ratio(m, 1000)
    }

    /// The whole part, rounded toward negative infinity.
    #[inline]
    pub const fn floor_int(self) -> i32 {
        self.0 >> 16
    }

    /// Rounded to the nearest whole number.
    #[inline]
    pub const fn round_int(self) -> i32 {
        (self.0 + (1 << 15)) >> 16
    }

    /// For the renderer and UI only — the sim never leaves fixed point.
    #[inline]
    pub fn to_f32(self) -> f32 {
        self.0 as f32 * (1.0 / 65536.0)
    }

    /// From a float — tools and tests only (rounds to nearest).
    pub fn from_f32(v: f32) -> Fx {
        let s = v * 65536.0;
        Fx(if s >= 0.0 {
            (s + 0.5) as i32
        } else {
            (s - 0.5) as i32
        })
    }

    #[inline]
    pub const fn abs(self) -> Fx {
        Fx(if self.0 < 0 { -self.0 } else { self.0 })
    }

    #[inline]
    pub fn min(self, o: Fx) -> Fx {
        if self.0 < o.0 { self } else { o }
    }

    #[inline]
    pub fn max(self, o: Fx) -> Fx {
        if self.0 > o.0 { self } else { o }
    }

    #[inline]
    pub fn clamp(self, lo: Fx, hi: Fx) -> Fx {
        self.max(lo).min(hi)
    }

    /// -1, 0 or 1 (as `Fx`).
    #[inline]
    pub const fn signum(self) -> Fx {
        if self.0 > 0 {
            Fx::ONE
        } else if self.0 < 0 {
            Fx(-(1 << 16))
        } else {
            Fx::ZERO
        }
    }

    /// Fixed-point product.
    #[inline]
    pub const fn mul(self, o: Fx) -> Fx {
        Fx(((self.0 as i64 * o.0 as i64) >> 16) as i32)
    }

    /// Fixed-point quotient; dividing by zero saturates with the
    /// dividend's sign (the sim never relies on it, but never traps).
    #[inline]
    pub const fn div(self, o: Fx) -> Fx {
        if o.0 == 0 {
            return if self.0 >= 0 {
                Fx(i32::MAX)
            } else {
                Fx(-i32::MAX)
            };
        }
        let q = ((self.0 as i64) << 16) / o.0 as i64;
        Fx(if q > i32::MAX as i64 {
            i32::MAX
        } else if q < -(i32::MAX as i64) {
            -i32::MAX
        } else {
            q as i32
        })
    }

    /// Square root (0 for negatives).
    pub fn sqrt(self) -> Fx {
        if self.0 <= 0 {
            return Fx::ZERO;
        }
        // sqrt(v / 2^16) * 2^16 = sqrt(v * 2^16)
        Fx(isqrt64((self.0 as u64) << 16) as i32)
    }

    /// Linear interpolation `self + (to - self) * t`.
    #[inline]
    pub fn lerp(self, to: Fx, t: Fx) -> Fx {
        self + (to - self).mul(t)
    }
}

/// Integer square root of a `u64` (floor).
pub fn isqrt64(v: u64) -> u64 {
    if v < 2 {
        return v;
    }
    // Digit-by-digit (base 4): no division, exact floor.
    let mut op = v;
    let mut res = 0u64;
    let mut one = 1u64 << 62;
    while one > op {
        one >>= 2;
    }
    while one != 0 {
        if op >= res + one {
            op -= res + one;
            res = (res >> 1) + one;
        } else {
            res >>= 1;
        }
        one >>= 2;
    }
    res
}

impl Add for Fx {
    type Output = Fx;
    #[inline]
    fn add(self, o: Fx) -> Fx {
        Fx(self.0 + o.0)
    }
}

impl AddAssign for Fx {
    #[inline]
    fn add_assign(&mut self, o: Fx) {
        self.0 += o.0;
    }
}

impl Sub for Fx {
    type Output = Fx;
    #[inline]
    fn sub(self, o: Fx) -> Fx {
        Fx(self.0 - o.0)
    }
}

impl SubAssign for Fx {
    #[inline]
    fn sub_assign(&mut self, o: Fx) {
        self.0 -= o.0;
    }
}

impl Neg for Fx {
    type Output = Fx;
    #[inline]
    fn neg(self) -> Fx {
        Fx(-self.0)
    }
}

impl Mul for Fx {
    type Output = Fx;
    #[inline]
    fn mul(self, o: Fx) -> Fx {
        Fx::mul(self, o)
    }
}

impl Mul<i32> for Fx {
    type Output = Fx;
    #[inline]
    fn mul(self, o: i32) -> Fx {
        Fx(self.0 * o)
    }
}

impl Div for Fx {
    type Output = Fx;
    #[inline]
    fn div(self, o: Fx) -> Fx {
        Fx::div(self, o)
    }
}

impl Div<i32> for Fx {
    type Output = Fx;
    #[inline]
    fn div(self, o: i32) -> Fx {
        Fx(self.0 / o)
    }
}

/// A ground-plane vector: `x` along the pitch's length, `y` across it (the
/// world's z). Heights live in [`V3`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct V2 {
    pub x: Fx,
    pub y: Fx,
}

impl V2 {
    pub const ZERO: V2 = V2 {
        x: Fx::ZERO,
        y: Fx::ZERO,
    };

    #[inline]
    pub const fn new(x: Fx, y: Fx) -> V2 {
        V2 { x, y }
    }

    /// From thousandths of a meter (tests, tables).
    #[inline]
    pub const fn milli(x: i32, y: i32) -> V2 {
        V2::new(Fx::milli(x), Fx::milli(y))
    }

    /// Dot product (computed in `i64`, so long vectors don't overflow
    /// before the shift).
    #[inline]
    pub fn dot(self, o: V2) -> Fx {
        Fx(((self.x.0 as i64 * o.x.0 as i64 + self.y.0 as i64 * o.y.0 as i64) >> 16) as i32)
    }

    /// 2D cross product (z of the 3D cross): > 0 when `o` is
    /// counter-clockwise from `self` (x right, y up).
    #[inline]
    pub fn cross(self, o: V2) -> Fx {
        Fx(((self.x.0 as i64 * o.y.0 as i64 - self.y.0 as i64 * o.x.0 as i64) >> 16) as i32)
    }

    /// Squared length, raw Q32.32 (no overflow for any on-pitch vector).
    #[inline]
    pub fn len_sq_raw(self) -> i64 {
        self.x.0 as i64 * self.x.0 as i64 + self.y.0 as i64 * self.y.0 as i64
    }

    #[inline]
    pub fn len(self) -> Fx {
        Fx(isqrt64(self.len_sq_raw() as u64) as i32)
    }

    /// `|self| < r` without a square root.
    #[inline]
    pub fn shorter_than(self, r: Fx) -> bool {
        self.len_sq_raw() < r.0 as i64 * r.0 as i64
    }

    /// Unit vector (zero stays zero).
    pub fn norm(self) -> V2 {
        let l = self.len();
        if l.0 == 0 {
            return V2::ZERO;
        }
        V2::new(self.x.div(l), self.y.div(l))
    }

    /// Unit vector and length together.
    pub fn norm_len(self) -> (V2, Fx) {
        let l = self.len();
        if l.0 == 0 {
            return (V2::ZERO, l);
        }
        (V2::new(self.x.div(l), self.y.div(l)), l)
    }

    /// Rotated a quarter turn counter-clockwise.
    #[inline]
    pub fn perp(self) -> V2 {
        V2::new(-self.y, self.x)
    }

    #[inline]
    pub fn scale(self, k: Fx) -> V2 {
        V2::new(self.x.mul(k), self.y.mul(k))
    }

    /// Length capped at `max`.
    pub fn clamp_len(self, max: Fx) -> V2 {
        if self.shorter_than(max) {
            return self;
        }
        self.norm().scale(max)
    }

    /// Distance to the segment `a..b`.
    pub fn dist_to_segment(self, a: V2, b: V2) -> Fx {
        let ab = b - a;
        let l2 = ab.len_sq_raw();
        if l2 == 0 {
            return (self - a).len();
        }
        let t = ((self - a).x.0 as i64 * ab.x.0 as i64 + (self - a).y.0 as i64 * ab.y.0 as i64)
            .clamp(0, l2);
        // closest = a + ab * (t / l2); t / l2 in Q16 (t <= l2 < 2^47 for
        // on-pitch vectors, so the shift stays inside i64).
        let k = Fx(((t << 16) / l2) as i32);
        (self - (a + ab.scale(k))).len()
    }

    /// Turn the unit vector `self` toward the unit vector `to` by at most
    /// `max_cos_step` worth (a normalized lerp: cheap, monotone, and exact
    /// when already aligned).
    pub fn turn_toward(self, to: V2, rate: Fx) -> V2 {
        if to == V2::ZERO {
            return self;
        }
        if self == V2::ZERO {
            return to;
        }
        let mixed = self + (to - self).scale(rate.clamp(Fx::ZERO, Fx::ONE));
        let n = mixed.norm();
        if n == V2::ZERO { to.perp() } else { n }
    }

    pub fn to_f32(self) -> (f32, f32) {
        (self.x.to_f32(), self.y.to_f32())
    }
}

impl Add for V2 {
    type Output = V2;
    #[inline]
    fn add(self, o: V2) -> V2 {
        V2::new(self.x + o.x, self.y + o.y)
    }
}

impl AddAssign for V2 {
    #[inline]
    fn add_assign(&mut self, o: V2) {
        self.x += o.x;
        self.y += o.y;
    }
}

impl Sub for V2 {
    type Output = V2;
    #[inline]
    fn sub(self, o: V2) -> V2 {
        V2::new(self.x - o.x, self.y - o.y)
    }
}

impl SubAssign for V2 {
    #[inline]
    fn sub_assign(&mut self, o: V2) {
        self.x -= o.x;
        self.y -= o.y;
    }
}

impl Neg for V2 {
    type Output = V2;
    #[inline]
    fn neg(self) -> V2 {
        V2::new(-self.x, -self.y)
    }
}

/// A 3D vector: `x` along the pitch, `y` up, `z` across (the world's
/// axes; [`V2`]'s `y` is this `z`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct V3 {
    pub x: Fx,
    pub y: Fx,
    pub z: Fx,
}

impl V3 {
    pub const ZERO: V3 = V3 {
        x: Fx::ZERO,
        y: Fx::ZERO,
        z: Fx::ZERO,
    };

    #[inline]
    pub const fn new(x: Fx, y: Fx, z: Fx) -> V3 {
        V3 { x, y, z }
    }

    /// The ground point under this one.
    #[inline]
    pub const fn ground(self) -> V2 {
        V2::new(self.x, self.z)
    }

    /// A ground point at height `h`.
    #[inline]
    pub const fn from_ground(p: V2, h: Fx) -> V3 {
        V3::new(p.x, h, p.y)
    }

    #[inline]
    pub fn scale(self, k: Fx) -> V3 {
        V3::new(self.x.mul(k), self.y.mul(k), self.z.mul(k))
    }

    #[inline]
    pub fn len_sq_raw(self) -> i64 {
        self.x.0 as i64 * self.x.0 as i64
            + self.y.0 as i64 * self.y.0 as i64
            + self.z.0 as i64 * self.z.0 as i64
    }

    #[inline]
    pub fn len(self) -> Fx {
        Fx(isqrt64(self.len_sq_raw() as u64) as i32)
    }
}

impl Add for V3 {
    type Output = V3;
    #[inline]
    fn add(self, o: V3) -> V3 {
        V3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl AddAssign for V3 {
    #[inline]
    fn add_assign(&mut self, o: V3) {
        self.x += o.x;
        self.y += o.y;
        self.z += o.z;
    }
}

impl Sub for V3 {
    type Output = V3;
    #[inline]
    fn sub(self, o: V3) -> V3 {
        V3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

/// Deterministic pseudo-random numbers (xorshift32): one stream per match,
/// seeded from the fixture, advanced only by the sim — so a replay with the
/// same inputs is the same match on every target.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Rng(pub u32);

impl Rng {
    pub const fn new(seed: u32) -> Rng {
        // Zero is xorshift's fixed point.
        Rng(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Uniform in `0..n` (`n` > 0).
    #[inline]
    pub fn below(&mut self, n: u32) -> u32 {
        // Multiply-shift: no division, negligible bias for small `n`.
        ((self.next_u32() as u64 * n as u64) >> 32) as u32
    }

    /// Uniform in `[0, 1)`.
    #[inline]
    pub fn unit(&mut self) -> Fx {
        Fx((self.next_u32() >> 16) as i32)
    }

    /// Uniform in `[-1, 1)`.
    #[inline]
    pub fn signed(&mut self) -> Fx {
        Fx((self.next_u32() >> 15) as i32 - (1 << 16))
    }

    /// `true` with probability `p` (0..1).
    #[inline]
    pub fn chance(&mut self, p: Fx) -> bool {
        self.unit() < p
    }
}

/// FNV-1a over 32-bit words: the sim's state hash (compared across PC,
/// ares and Azahar).
#[derive(Clone, Copy, Debug)]
pub struct Hasher(pub u32);

impl Hasher {
    pub const fn new() -> Hasher {
        Hasher(0x811C_9DC5)
    }

    pub fn word(&mut self, w: u32) {
        for b in w.to_le_bytes() {
            self.0 ^= b as u32;
            self.0 = self.0.wrapping_mul(0x0100_0193);
        }
    }

    pub fn fx(&mut self, v: Fx) {
        self.word(v.0 as u32);
    }

    pub fn v2(&mut self, v: V2) {
        self.fx(v.x);
        self.fx(v.y);
    }

    pub fn v3(&mut self, v: V3) {
        self.fx(v.x);
        self.fx(v.y);
        self.fx(v.z);
    }
}

impl Default for Hasher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_is_q16_16() {
        assert_eq!(Fx::int(3).0, 3 << 16);
        assert_eq!(Fx::milli(1500), Fx(98304));
        assert_eq!(Fx::int(3) * Fx::HALF, Fx::milli(1500));
        assert_eq!(Fx::int(3) / Fx::int(2), Fx::milli(1500));
        assert_eq!((-Fx::milli(1500)).abs(), Fx::milli(1500));
        assert_eq!(Fx::milli(2600).round_int(), 3);
        assert_eq!(Fx::milli(-2600).floor_int(), -3);
        assert_eq!(Fx::int(5).div(Fx::ZERO), Fx(i32::MAX));
    }

    #[test]
    fn square_roots_are_exact_floors() {
        for v in [0u64, 1, 2, 3, 4, 15, 16, 17, 1 << 40, u64::MAX] {
            let r = isqrt64(v);
            assert!(r * r <= v, "{v}");
            assert!((r + 1).checked_mul(r + 1).is_none_or(|s| s > v), "{v}");
        }
        assert_eq!(Fx::int(9).sqrt(), Fx::int(3));
        assert_eq!(Fx::int(2).sqrt(), Fx(92681)); // 1.41421 * 65536
        assert_eq!(V2::new(Fx::int(3), Fx::int(4)).len(), Fx::int(5));
    }

    #[test]
    fn long_pitch_vectors_do_not_overflow() {
        // Corner to corner of the pitch (105 x 68): 125 m.
        let d = V2::new(Fx::int(105), Fx::int(68));
        let l = d.len().to_f32();
        assert!((l - 125.096).abs() < 0.01, "{l}");
        assert!(d.dot(d).0 > 0);
    }

    #[test]
    fn norm_turn_and_segment_distance() {
        let n = V2::new(Fx::int(10), Fx::ZERO).norm();
        assert_eq!(n, V2::new(Fx::ONE, Fx::ZERO));
        let up = V2::new(Fx::ZERO, Fx::ONE);
        let t = n.turn_toward(up, Fx::HALF);
        assert!((t.len().to_f32() - 1.0).abs() < 1e-3);
        assert!(t.x.0 > 0 && t.y.0 > 0);
        // Opposite directions still turn (no zero vector).
        assert_ne!(n.turn_toward(-n, Fx::HALF), V2::ZERO);
        let p = V2::new(Fx::int(5), Fx::int(3));
        let d = p.dist_to_segment(V2::ZERO, V2::new(Fx::int(10), Fx::ZERO));
        assert_eq!(d, Fx::int(3));
        // Beyond the ends: distance to the end point.
        let d = V2::new(Fx::int(13), Fx::int(4))
            .dist_to_segment(V2::ZERO, V2::new(Fx::int(10), Fx::ZERO));
        assert_eq!(d, Fx::int(5));
    }

    #[test]
    fn cross_sign_follows_counter_clockwise() {
        let x = V2::new(Fx::ONE, Fx::ZERO);
        let y = V2::new(Fx::ZERO, Fx::ONE);
        assert!(x.cross(y).0 > 0);
        assert!(y.cross(x).0 < 0);
        assert_eq!(x.perp(), y);
    }

    #[test]
    fn rng_is_deterministic_and_spread() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        let mut hits = [0u32; 4];
        for _ in 0..4000 {
            let v = a.below(4);
            assert_eq!(v, b.below(4));
            hits[v as usize] += 1;
            let u = a.unit();
            assert!(u >= Fx::ZERO && u < Fx::ONE);
            b.unit();
        }
        for h in hits {
            assert!((800..1200).contains(&h), "{hits:?}");
        }
        assert_ne!(Rng::new(0).0, 0);
    }
}
