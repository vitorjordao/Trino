//! Minimal math primitives shared by all platforms.
//!
//! Deliberately small: only what 2D gameplay and the renderer contract need.
//! Grows in Fase 7 (3D) — see PLANO_EXECUCAO_TRINO.md.

use core::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

/// 2D vector, `f32`. Screen space is X-right, Y-down, origin top-left.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };
    pub const ONE: Vec2 = Vec2 { x: 1.0, y: 1.0 };

    #[inline]
    pub const fn new(x: f32, y: f32) -> Self {
        Vec2 { x, y }
    }

    #[inline]
    pub fn dot(self, rhs: Vec2) -> f32 {
        self.x * rhs.x + self.y * rhs.y
    }

    #[inline]
    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    #[inline]
    fn add(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl AddAssign for Vec2 {
    #[inline]
    fn add_assign(&mut self, rhs: Vec2) {
        *self = *self + rhs;
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    #[inline]
    fn sub(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl SubAssign for Vec2 {
    #[inline]
    fn sub_assign(&mut self, rhs: Vec2) {
        *self = *self - rhs;
    }
}

impl Mul<f32> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn mul(self, rhs: f32) -> Vec2 {
        Vec2::new(self.x * rhs, self.y * rhs)
    }
}

impl Neg for Vec2 {
    type Output = Vec2;
    #[inline]
    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}

/// 3D vector, `f32`. Present from day one so 3D trait signatures exist;
/// gains operators alongside Fase 7.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    pub const ONE: Vec3 = Vec3 {
        x: 1.0,
        y: 1.0,
        z: 1.0,
    };

    #[inline]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Vec3 { x, y, z }
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    #[inline]
    fn add(self, rhs: Vec3) -> Vec3 {
        Vec3::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl Sub for Vec3 {
    type Output = Vec3;
    #[inline]
    fn sub(self, rhs: Vec3) -> Vec3 {
        Vec3::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

impl Mul<f32> for Vec3 {
    type Output = Vec3;
    #[inline]
    fn mul(self, rhs: f32) -> Vec3 {
        Vec3::new(self.x * rhs, self.y * rhs, self.z * rhs)
    }
}

impl AddAssign for Vec3 {
    #[inline]
    fn add_assign(&mut self, rhs: Vec3) {
        *self = *self + rhs;
    }
}

impl SubAssign for Vec3 {
    #[inline]
    fn sub_assign(&mut self, rhs: Vec3) {
        *self = *self - rhs;
    }
}

impl Neg for Vec3 {
    type Output = Vec3;
    #[inline]
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

/// Axis-aligned rectangle: position of the top-left corner plus size.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Rect {
    pub pos: Vec2,
    pub size: Vec2,
}

impl Rect {
    #[inline]
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect {
            pos: Vec2::new(x, y),
            size: Vec2::new(w, h),
        }
    }

    #[inline]
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.pos.x
            && p.y >= self.pos.y
            && p.x < self.pos.x + self.size.x
            && p.y < self.pos.y + self.size.y
    }

    #[inline]
    pub fn intersects(&self, other: &Rect) -> bool {
        self.pos.x < other.pos.x + other.size.x
            && other.pos.x < self.pos.x + self.size.x
            && self.pos.y < other.pos.y + other.size.y
            && other.pos.y < self.pos.y + self.size.y
    }
}

/// 8-bit RGBA color. Backends quantize as needed (e.g. RGBA5551 on N64).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);

    #[inline]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color { r, g, b, a: 255 }
    }

    #[inline]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Color { r, g, b, a }
    }

    /// Quantize to N64 framebuffer format (5 bits per channel, 1-bit alpha).
    /// Used by the PC "N64 look" mode and by pipeline tests.
    #[inline]
    pub const fn to_rgba5551(self) -> u16 {
        let r = (self.r >> 3) as u16;
        let g = (self.g >> 3) as u16;
        let b = (self.b >> 3) as u16;
        let a = (self.a >> 7) as u16;
        (r << 11) | (g << 6) | (b << 1) | a
    }
}

impl Default for Color {
    fn default() -> Self {
        Color::WHITE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec2_ops() {
        let a = Vec2::new(1.0, 2.0);
        let b = Vec2::new(3.0, -1.0);
        assert_eq!(a + b, Vec2::new(4.0, 1.0));
        assert_eq!(a - b, Vec2::new(-2.0, 3.0));
        assert_eq!(a * 2.0, Vec2::new(2.0, 4.0));
        assert_eq!(-a, Vec2::new(-1.0, -2.0));
        assert_eq!(a.dot(b), 1.0);
    }

    #[test]
    fn vec3_ops() {
        let mut a = Vec3::new(1.0, 2.0, 3.0);
        a += Vec3::new(1.0, 1.0, 1.0);
        assert_eq!(a, Vec3::new(2.0, 3.0, 4.0));
        a -= Vec3::new(2.0, 2.0, 2.0);
        assert_eq!(a, Vec3::new(0.0, 1.0, 2.0));
        assert_eq!(-a, Vec3::new(0.0, -1.0, -2.0));
    }

    #[test]
    fn rect_contains_is_half_open() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(Vec2::ZERO));
        assert!(r.contains(Vec2::new(9.9, 9.9)));
        assert!(!r.contains(Vec2::new(10.0, 5.0)));
        assert!(!r.contains(Vec2::new(-0.1, 5.0)));
    }

    #[test]
    fn rect_intersects() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(a.intersects(&Rect::new(5.0, 5.0, 10.0, 10.0)));
        assert!(!a.intersects(&Rect::new(10.0, 0.0, 5.0, 5.0)));
        assert!(!a.intersects(&Rect::new(0.0, 20.0, 5.0, 5.0)));
    }

    #[test]
    fn color_rgba5551_quantization() {
        assert_eq!(Color::WHITE.to_rgba5551(), 0xFFFF);
        assert_eq!(Color::TRANSPARENT.to_rgba5551(), 0x0000);
        // Pure red: r=31, g=0, b=0, a=1 -> 0b11111_00000_00000_1
        assert_eq!(Color::rgb(255, 0, 0).to_rgba5551(), 0xF801);
    }
}
