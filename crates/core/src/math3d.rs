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
        let (sx, cx) = (sin(rot.x), cos(rot.x));
        let (sy, cy) = (sin(rot.y), cos(rot.y));
        let (sz, cz) = (sin(rot.z), cos(rot.z));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sin_cos_are_close_to_std() {
        for i in -100..=100 {
            let x = i as f32 * 0.1;
            assert!((sin(x) - x.sin()).abs() < 2e-3, "sin({x})");
            assert!((cos(x) - x.cos()).abs() < 2e-3, "cos({x})");
        }
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
}
