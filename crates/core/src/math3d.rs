//! 3D math for the software transform & lighting pipeline (Fase 7).
//!
//! `no_std` has no `sin`/`cos`/`sqrt`, so this module ships small
//! polynomial/Newton approximations instead of libm. They are plain f32
//! arithmetic — identical results on PC, N64 and 3DS — and accurate to
//! ~1e-4, far beyond what a 320x240 raster can show.

use crate::math::Vec3;

/// sin(x) for any x (radians). Range-reduced Bhaskara-style minimax.
pub fn sin(x: f32) -> f32 {
    use core::f32::consts::{PI, TAU};
    // Reduce to [-PI, PI].
    let mut r = x % TAU;
    if r > PI {
        r -= TAU;
    } else if r < -PI {
        r += TAU;
    }
    // Parabolic approximation refined with a quartic term
    // (max error ~1e-4 over the range).
    const B: f32 = 4.0 / PI;
    const C: f32 = -4.0 / (PI * PI);
    let y = B * r + C * r * if r < 0.0 { -r } else { r };
    const P: f32 = 0.225;
    P * (y * if y < 0.0 { -y } else { y } - y) + y
}

pub fn cos(x: f32) -> f32 {
    sin(x + core::f32::consts::FRAC_PI_2)
}

/// atan2(y, x) in (-PI, PI], octant-reduced polynomial (max error ~5e-3 —
/// far below what a yaw-from-direction needs at console resolutions).
/// Every 3D game needs it for "face where I'm walking"; keeping it here
/// keeps games deterministic across PC/N64/3DS like the rest of the module.
pub fn atan2(y: f32, x: f32) -> f32 {
    use core::f32::consts::{FRAC_PI_2, PI};
    let abs = |v: f32| if v < 0.0 { -v } else { v };
    let (ax, ay) = (abs(x), abs(y));
    if ax == 0.0 && ay == 0.0 {
        return 0.0;
    }
    let a = if ax >= ay { ay / ax } else { ax / ay };
    let s = a * a;
    let mut r = ((-0.046_496_474 * s + 0.159_314_22) * s - 0.327_622_76) * s * a + a;
    if ay > ax {
        r = FRAC_PI_2 - r;
    }
    if x < 0.0 {
        r = PI - r;
    }
    if y < 0.0 { -r } else { r }
}

/// sqrt via one reciprocal-sqrt bit trick + two Newton iterations.
pub fn sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let i = 0x5f37_59df_u32.wrapping_sub(x.to_bits() >> 1);
    let mut y = f32::from_bits(i); // ~1/sqrt(x)
    y *= 1.5 - 0.5 * x * y * y;
    y *= 1.5 - 0.5 * x * y * y;
    x * y // x * 1/sqrt(x) = sqrt(x)
}

impl Vec3 {
    #[inline]
    pub fn dot(self, rhs: Vec3) -> f32 {
        self.x * rhs.x + self.y * rhs.y + self.z * rhs.z
    }

    #[inline]
    pub fn cross(self, rhs: Vec3) -> Vec3 {
        Vec3::new(
            self.y * rhs.z - self.z * rhs.y,
            self.z * rhs.x - self.x * rhs.z,
            self.x * rhs.y - self.y * rhs.x,
        )
    }

    pub fn normalized(self) -> Vec3 {
        let len = sqrt(self.dot(self));
        if len < 1e-6 {
            return Vec3::ZERO;
        }
        self * (1.0 / len)
    }
}

/// Row-major affine transform (rotation/scale/translation; no projection —
/// perspective happens in the render3d projection step).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Mat34 {
    pub m: [[f32; 4]; 3],
}

impl Mat34 {
    pub const IDENTITY: Mat34 = Mat34 {
        m: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ],
    };

    /// Model matrix from Euler XYZ rotation (radians), scale and translation
    /// (applied in that order).
    pub fn from_rotation_scale_translation(rot: Vec3, scale: Vec3, pos: Vec3) -> Mat34 {
        // Most draws leave some axes unrotated (blocks all three, coins and
        // boars all but yaw): skip the software sin/cos there. `+0.0` only,
        // where sin and cos return exactly 0 and 1 (pinned by a test), so
        // the matrix is bit-identical.
        let sin_cos = |a: f32| {
            if a.to_bits() == 0 {
                (0.0, 1.0)
            } else {
                (sin(a), cos(a))
            }
        };
        let (sx, cx) = sin_cos(rot.x);
        let (sy, cy) = sin_cos(rot.y);
        let (sz, cz) = sin_cos(rot.z);
        // R = Rz * Ry * Rx, then columns scaled.
        let r = [
            [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
            [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
            [-sy, cy * sx, cy * cx],
        ];
        Mat34 {
            m: [
                [
                    r[0][0] * scale.x,
                    r[0][1] * scale.y,
                    r[0][2] * scale.z,
                    pos.x,
                ],
                [
                    r[1][0] * scale.x,
                    r[1][1] * scale.y,
                    r[1][2] * scale.z,
                    pos.y,
                ],
                [
                    r[2][0] * scale.x,
                    r[2][1] * scale.y,
                    r[2][2] * scale.z,
                    pos.z,
                ],
            ],
        }
    }

    /// View matrix: world -> camera space. +Z is in front of the camera;
    /// +X view maps to +X world when looking down world +Z (no mirroring).
    pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Mat34 {
        let f = (target - eye).normalized(); // forward
        let r = up.cross(f).normalized(); // right
        let u = f.cross(r); // true up
        Mat34 {
            m: [
                [r.x, r.y, r.z, -r.dot(eye)],
                [u.x, u.y, u.z, -u.dot(eye)],
                [f.x, f.y, f.z, -f.dot(eye)],
            ],
        }
    }

    #[inline]
    pub fn transform_point(&self, p: Vec3) -> Vec3 {
        Vec3::new(
            self.m[0][0] * p.x + self.m[0][1] * p.y + self.m[0][2] * p.z + self.m[0][3],
            self.m[1][0] * p.x + self.m[1][1] * p.y + self.m[1][2] * p.z + self.m[1][3],
            self.m[2][0] * p.x + self.m[2][1] * p.y + self.m[2][2] * p.z + self.m[2][3],
        )
    }

    /// Rotate a direction (ignores translation). Correct for
    /// rotation+uniform-scale matrices (normals get re-normalized anyway).
    #[inline]
    pub fn transform_dir(&self, d: Vec3) -> Vec3 {
        Vec3::new(
            self.m[0][0] * d.x + self.m[0][1] * d.y + self.m[0][2] * d.z,
            self.m[1][0] * d.x + self.m[1][1] * d.y + self.m[1][2] * d.z,
            self.m[2][0] * d.x + self.m[2][1] * d.y + self.m[2][2] * d.z,
        )
    }

    /// self * rhs (apply rhs first).
    pub fn mul(&self, rhs: &Mat34) -> Mat34 {
        let mut out = [[0.0f32; 4]; 3];
        for (i, row) in out.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = self.m[i][0] * rhs.m[0][j]
                    + self.m[i][1] * rhs.m[1][j]
                    + self.m[i][2] * rhs.m[2][j]
                    + if j == 3 { self.m[i][3] } else { 0.0 };
            }
        }
        Mat34 { m: out }
    }
}

/// Unit quaternion (x, y, z, w) — the rotation format of baked animation
/// clips (glTF native). Only what rigid-joint animation needs: normalized
/// lerp and conversion to a matrix.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Quat = Quat {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Quat { x, y, z, w }
    }

    pub fn dot(self, o: Quat) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z + self.w * o.w
    }

    pub fn normalized(self) -> Quat {
        let len = sqrt(self.dot(self));
        if len > 1e-12 {
            let inv = 1.0 / len;
            Quat::new(self.x * inv, self.y * inv, self.z * inv, self.w * inv)
        } else {
            Quat::IDENTITY
        }
    }

    /// Normalized lerp along the shortest arc. For the small per-sample
    /// steps of a baked clip it is indistinguishable from slerp, and needs no
    /// trigonometry (deterministic and cheap on every target).
    pub fn nlerp(self, o: Quat, t: f32) -> Quat {
        let o = if self.dot(o) < 0.0 {
            Quat::new(-o.x, -o.y, -o.z, -o.w)
        } else {
            o
        };
        Quat::new(
            self.x + (o.x - self.x) * t,
            self.y + (o.y - self.y) * t,
            self.z + (o.z - self.z) * t,
            self.w + (o.w - self.w) * t,
        )
        .normalized()
    }
}

impl Mat34 {
    /// Matrix from translation, rotation quaternion and scale (applied
    /// scale → rotate → translate, the glTF node convention).
    pub fn from_trs(t: Vec3, q: Quat, s: Vec3) -> Mat34 {
        let Quat { x, y, z, w } = q;
        let (xx, yy, zz) = (x * x, y * y, z * z);
        let (xy, xz, yz) = (x * y, x * z, y * z);
        let (wx, wy, wz) = (w * x, w * y, w * z);
        Mat34 {
            m: [
                [
                    (1.0 - 2.0 * (yy + zz)) * s.x,
                    2.0 * (xy - wz) * s.y,
                    2.0 * (xz + wy) * s.z,
                    t.x,
                ],
                [
                    2.0 * (xy + wz) * s.x,
                    (1.0 - 2.0 * (xx + zz)) * s.y,
                    2.0 * (yz - wx) * s.z,
                    t.y,
                ],
                [
                    2.0 * (xz - wy) * s.x,
                    2.0 * (yz + wx) * s.y,
                    (1.0 - 2.0 * (xx + yy)) * s.z,
                    t.z,
                ],
            ],
        }
    }

    /// Length of each basis column (the per-axis scale).
    pub fn axis_scales(&self) -> Vec3 {
        let col = |c: usize| {
            sqrt(
                self.m[0][c] * self.m[0][c]
                    + self.m[1][c] * self.m[1][c]
                    + self.m[2][c] * self.m[2][c],
            )
        };
        Vec3::new(col(0), col(1), col(2))
    }

    /// Squared length of each basis column (no sqrt).
    pub fn axis_scales_sq(&self) -> Vec3 {
        let col = |c: usize| {
            self.m[0][c] * self.m[0][c] + self.m[1][c] * self.m[1][c] + self.m[2][c] * self.m[2][c]
        };
        Vec3::new(col(0), col(1), col(2))
    }

    /// Largest axis scale — what a bounding sphere radius multiplies by.
    pub fn max_scale(&self) -> f32 {
        let s = self.axis_scales();
        s.x.max(s.y).max(s.z)
    }

    /// Split into (Euler XYZ radians, scale, translation) — the inverse of
    /// [`Mat34::from_rotation_scale_translation`]. Exact for rotation +
    /// per-axis scale; a sheared matrix (non-uniform parent scale with a
    /// rotated child) has no such decomposition and comes back approximated.
    pub fn decompose(&self) -> (Vec3, Vec3, Vec3) {
        let s = self.axis_scales();
        let inv = |v: f32| if v > 1e-12 { 1.0 / v } else { 0.0 };
        let (ix, iy, iz) = (inv(s.x), inv(s.y), inv(s.z));
        let r00 = self.m[0][0] * ix;
        let r10 = self.m[1][0] * ix;
        let r20 = self.m[2][0] * ix;
        let r21 = self.m[2][1] * iy;
        let r22 = self.m[2][2] * iz;
        let rot = Vec3::new(
            atan2(r21, r22),
            atan2(-r20, sqrt(r00 * r00 + r10 * r10)),
            atan2(r10, r00),
        );
        let pos = Vec3::new(self.m[0][3], self.m[1][3], self.m[2][3]);
        (rot, s, pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_angle_fast_path_matches_sin_cos() {
        // from_rotation_scale_translation skips sin/cos for +0.0 angles;
        // that is only output-identical while these hold bit for bit.
        assert_eq!(sin(0.0).to_bits(), 0.0f32.to_bits());
        assert_eq!(cos(0.0).to_bits(), 1.0f32.to_bits());
        let full = |r: Vec3, s: Vec3, p: Vec3| {
            let (sx, cx) = (sin(r.x), cos(r.x));
            let (sy, cy) = (sin(r.y), cos(r.y));
            let (sz, cz) = (sin(r.z), cos(r.z));
            [
                cz * cy * s.x,
                (cz * sy * sx - sz * cx) * s.y,
                cy * cx * s.z,
                p.x,
            ]
        };
        let (r, sc, p) = (
            Vec3::new(0.0, 0.7, 0.0),
            Vec3::new(2.0, 1.0, 3.0),
            Vec3::ONE,
        );
        let m = Mat34::from_rotation_scale_translation(r, sc, p);
        let want = full(r, sc, p);
        let got = [m.m[0][0], m.m[0][1], m.m[2][2], m.m[0][3]];
        assert_eq!(got.map(f32::to_bits), want.map(f32::to_bits));
    }

    #[test]
    fn sin_cos_are_close_to_std() {
        for i in -100..=100 {
            let x = i as f32 * 0.1;
            assert!((sin(x) - x.sin()).abs() < 2e-3, "sin({x})");
            assert!((cos(x) - x.cos()).abs() < 2e-3, "cos({x})");
        }
    }

    #[test]
    fn atan2_is_close_to_std() {
        for i in 0..256 {
            let a = i as f32 / 256.0 * core::f32::consts::TAU - core::f32::consts::PI;
            for r in [0.001f32, 1.0, 300.0] {
                let (y, x) = (a.sin() * r, a.cos() * r);
                let d = (atan2(y, x) - y.atan2(x)).abs();
                assert!(d < 6e-3, "atan2({y}, {x}): diff {d}");
            }
        }
        assert_eq!(atan2(0.0, 0.0), 0.0);
        // Axes hit the exact quadrant boundaries.
        assert!((atan2(1.0, 0.0) - core::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((atan2(-1.0, 0.0) + core::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((atan2(0.0, -1.0) - core::f32::consts::PI).abs() < 1e-6);
    }

    #[test]
    fn sqrt_is_close_to_std() {
        for i in 0..1000 {
            let x = i as f32 * 0.37;
            assert!((sqrt(x) - x.sqrt()).abs() < 1e-3 * (x + 1.0), "sqrt({x})");
        }
        assert_eq!(sqrt(0.0), 0.0);
        assert_eq!(sqrt(-4.0), 0.0);
    }

    #[test]
    fn vec3_cross_and_normalize() {
        let x = Vec3::new(1.0, 0.0, 0.0);
        let y = Vec3::new(0.0, 1.0, 0.0);
        let z = x.cross(y);
        assert!((z.z - 1.0).abs() < 1e-6);
        let n = Vec3::new(3.0, 0.0, 4.0).normalized();
        assert!((n.dot(n) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn identity_transforms_nothing() {
        let p = Vec3::new(1.0, 2.0, 3.0);
        let q = Mat34::IDENTITY.transform_point(p);
        assert_eq!(p, q);
    }

    #[test]
    fn look_at_centers_the_target() {
        let view = Mat34::look_at(
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::ZERO,
            Vec3::new(0.0, 1.0, 0.0),
        );
        let t = view.transform_point(Vec3::ZERO);
        // Target lands on the view axis, 5 units in front.
        assert!(t.x.abs() < 1e-4 && t.y.abs() < 1e-4);
        assert!((t.z - 5.0).abs() < 1e-3);
    }

    #[test]
    fn look_at_does_not_mirror() {
        // Camera at -Z looking at the origin: world +X must stay +X in view
        // space (and +Y up stays +Y).
        let view = Mat34::look_at(
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::ZERO,
            Vec3::new(0.0, 1.0, 0.0),
        );
        let px = view.transform_point(Vec3::new(1.0, 0.0, 0.0));
        assert!(px.x > 0.9, "+X world mirrored to {}", px.x);
        let py = view.transform_point(Vec3::new(0.0, 1.0, 0.0));
        assert!(py.y > 0.9, "+Y world flipped to {}", py.y);
    }

    #[test]
    fn model_matrix_scales_and_translates() {
        let m = Mat34::from_rotation_scale_translation(
            Vec3::ZERO,
            Vec3::new(2.0, 2.0, 2.0),
            Vec3::new(10.0, 0.0, 0.0),
        );
        let p = m.transform_point(Vec3::new(1.0, 1.0, 1.0));
        assert_eq!(p, Vec3::new(12.0, 2.0, 2.0));
    }

    #[test]
    fn quat_identity_matrix_is_identity() {
        let m = Mat34::from_trs(Vec3::ZERO, Quat::IDENTITY, Vec3::ONE);
        assert_eq!(m, Mat34::IDENTITY);
    }

    #[test]
    fn quat_matches_euler_for_a_single_axis() {
        // 90 degrees around Y.
        let h = core::f32::consts::FRAC_PI_4;
        let q = Quat::new(0.0, h.sin(), 0.0, h.cos());
        let a = Mat34::from_trs(Vec3::ZERO, q, Vec3::ONE);
        let b = Mat34::from_rotation_scale_translation(
            Vec3::new(0.0, core::f32::consts::FRAC_PI_2, 0.0),
            Vec3::ONE,
            Vec3::ZERO,
        );
        for r in 0..3 {
            for c in 0..4 {
                assert!((a.m[r][c] - b.m[r][c]).abs() < 3e-3, "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn nlerp_takes_the_short_way_and_stays_unit() {
        let a = Quat::IDENTITY;
        let b = Quat::new(0.0, 0.0, 0.0, -1.0); // same rotation, other sign
        let m = a.nlerp(b, 0.5);
        assert!((m.dot(m) - 1.0).abs() < 1e-3);
        assert!(m.w.abs() > 0.99);
    }

    #[test]
    fn decompose_round_trips_euler_scale_translation() {
        let rot = Vec3::new(0.3, -0.7, 1.2);
        let scale = Vec3::new(2.0, 0.5, 1.5);
        let pos = Vec3::new(1.0, -2.0, 3.0);
        let m = Mat34::from_rotation_scale_translation(rot, scale, pos);
        let (r, s, p) = m.decompose();
        let back = Mat34::from_rotation_scale_translation(r, s, p);
        for i in 0..3 {
            for j in 0..4 {
                assert!((m.m[i][j] - back.m[i][j]).abs() < 1e-2, "{m:?} vs {back:?}");
            }
        }
        assert!((s.x - 2.0).abs() < 1e-2 && (s.y - 0.5).abs() < 1e-2);
        assert_eq!(p, pos);
    }
}
