//! Impostors: a model pre-rendered from `K` yaws around it into one sprite
//! atlas, drawn far away as **one camera-facing billboard — 2 triangles**
//! instead of the mesh. The frame shown is the one rendered from the yaw
//! closest to where the camera stands relative to the object.
//!
//! The atlas and its table come from the impostor bake (`cargo xtask
//! impostor bake`, or the editor's model inspector), which renders the
//! baked model with this engine's own T&L. It writes a sprite and a
//! `TIMP` blob; [`Impostor::parse`] reads the blob, and scene LOD levels
//! carry the same descriptor inline (`LodLevel::impostor`).
//!
//! ```ignore
//! let tree = Impostor::parse(platform.blob(BlobId::from_path("blobs/impostor-tree"))?)?;
//! tree.draw(renderer, &matrix, camera.eye, Color::WHITE);   // after set_camera
//! ```
//!
//! **Cost.** Two textured triangles and one texture load. On the 3DS and PC
//! that is far cheaper than a mesh. On the N64 a textured billboard is
//! CPU-submitted (~22 µs per triangle, `perf::N64_COST::us_per_billboard`),
//! which can cost more than the RSP time a small mesh takes — measure, and
//! give the N64 its own LOD distances (or none) in the scene.

use crate::math::{Color, Rect, Vec2, Vec3};
use crate::math3d::{Mat34, atan2, sqrt};
use crate::render::{BillboardParams, Renderer, SpriteId};

pub const TIMP_MAGIC: &[u8; 4] = b"TIMP";
pub const TIMP_VERSION: u16 = 1;
/// Fixed part of a `TIMP` blob (the region table follows: 8 bytes a view).
pub const TIMP_HEADER: usize = 40;
/// The most views an atlas holds.
pub const MAX_VIEWS: u8 = 64;

/// A pre-rendered model: which atlas, how its views are laid out, and the
/// quad that stands in for the model.
///
/// View `i` was rendered from the model's local yaw `i · 2π / views`
/// (around +Y, measured like `atan2(x, z)`), at the bake's pitch, onto the
/// square tile at column `i % cols`, row `i / cols` of the atlas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Impostor {
    /// The atlas sprite.
    pub sprite: SpriteId,
    pub views: u8,
    /// Tiles per atlas row.
    pub cols: u8,
    /// Tile edge in texels (square tiles).
    pub tile: u16,
    /// Edge of the quad in model units (the bounding sphere's diameter, so
    /// every view fits).
    pub size: f32,
    /// Center of the quad in model space (the bounding sphere's center).
    pub center: Vec3,
}

/// Why a `TIMP` blob was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImpostorError {
    BadMagic,
    UnsupportedVersion(u16),
    Truncated,
    /// Zero views, more than [`MAX_VIEWS`], no columns or a zero tile.
    BadLayout,
    /// The region table disagrees with the grid the header describes.
    BadRegions,
}

#[inline]
fn u16_at(d: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([d[at], d[at + 1]])
}

#[inline]
fn u32_at(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
}

#[inline]
fn f32_at(d: &[u8], at: usize) -> f32 {
    f32::from_bits(u32_at(d, at))
}

impl Impostor {
    /// Texel rectangle of view `i` in the atlas (views past the last wrap).
    pub fn region(&self, i: u8) -> Rect {
        let i = if self.views == 0 { 0 } else { i % self.views };
        let cols = self.cols.max(1);
        let t = self.tile as f32;
        Rect::new((i % cols) as f32 * t, (i / cols) as f32 * t, t, t)
    }

    /// Atlas size in texels: `cols` tiles wide, enough rows for every view.
    pub fn atlas_size(&self) -> (u32, u32) {
        let cols = self.cols.max(1) as u32;
        let rows = (self.views as u32).div_ceil(cols);
        (cols * self.tile as u32, rows * self.tile as u32)
    }

    /// The view to show for an object placed by `matrix`, seen from `eye`:
    /// the rendered yaw nearest to the eye's direction in model space.
    /// Deterministic (the core's own `atan2`), so every target picks the
    /// same frame.
    pub fn view_for(&self, matrix: &Mat34, eye: Vec3) -> u8 {
        if self.views <= 1 {
            return 0;
        }
        let m = &matrix.m;
        let p = matrix.transform_point(self.center);
        let d = eye - p;
        // The model's yaw: where its local +Z points, around +Y.
        let yaw = atan2(m[0][2], m[2][2]);
        let step = core::f32::consts::TAU / self.views as f32;
        let x = (atan2(d.x, d.z) - yaw) / step + 0.5;
        // floor() without std: truncate, then step down below zero.
        let mut i = crate::math3d::to_i32(x);
        if (i as f32) > x {
            i -= 1;
        }
        i.rem_euclid(self.views as i32) as u8
    }

    /// The billboard that stands in for the model: its world center and
    /// parameters (the quad scales with the matrix's X axis).
    pub fn billboard(&self, matrix: &Mat34, eye: Vec3, tint: Color) -> (Vec3, BillboardParams) {
        let m = &matrix.m;
        let scale = sqrt(m[0][0] * m[0][0] + m[1][0] * m[1][0] + m[2][0] * m[2][0]);
        let s = self.size * scale;
        (
            matrix.transform_point(self.center),
            BillboardParams {
                size: Vec2::new(s, s),
                rotation: 0.0,
                tint,
                region: Some(self.region(self.view_for(matrix, eye))),
            },
        )
    }

    /// Draw it (after `set_camera`): one `draw_billboard`, 2 triangles.
    pub fn draw(&self, renderer: &mut dyn Renderer, matrix: &Mat34, eye: Vec3, tint: Color) {
        let (pos, params) = self.billboard(matrix, eye, tint);
        renderer.draw_billboard(self.sprite, pos, &params);
    }

    /// Read a `TIMP` blob (little endian):
    ///
    /// ```text
    /// magic "TIMP"  u16 version (1)  u8 views  u8 cols
    /// u32 sprite  u16 tile  u16 reserved  f32 size  f32*3 center
    /// u32 model hash (FNV-1a of the TMDL it was rendered from)
    /// f32 pitch (radians, informational)
    /// views × (u16 x, u16 y, u16 w, u16 h): the region table
    /// ```
    ///
    /// The table must match the grid (`region`), so tools and game code
    /// that read either agree.
    pub fn parse(bytes: &[u8]) -> Result<Impostor, ImpostorError> {
        if bytes.len() < TIMP_HEADER {
            return Err(ImpostorError::Truncated);
        }
        if &bytes[0..4] != TIMP_MAGIC {
            return Err(ImpostorError::BadMagic);
        }
        let version = u16_at(bytes, 4);
        if version != TIMP_VERSION {
            return Err(ImpostorError::UnsupportedVersion(version));
        }
        let imp = Impostor {
            views: bytes[6],
            cols: bytes[7],
            sprite: SpriteId(u32_at(bytes, 8)),
            tile: u16_at(bytes, 12),
            size: f32_at(bytes, 16),
            center: Vec3::new(f32_at(bytes, 20), f32_at(bytes, 24), f32_at(bytes, 28)),
        };
        if imp.views == 0 || imp.views > MAX_VIEWS || imp.cols == 0 || imp.tile == 0 {
            return Err(ImpostorError::BadLayout);
        }
        if bytes.len() < TIMP_HEADER + imp.views as usize * 8 {
            return Err(ImpostorError::Truncated);
        }
        for i in 0..imp.views {
            let at = TIMP_HEADER + i as usize * 8;
            let r = imp.region(i);
            let want = [r.pos.x, r.pos.y, r.size.x, r.size.y];
            for (k, w) in want.iter().enumerate() {
                if u16_at(bytes, at + k * 2) as f32 != *w {
                    return Err(ImpostorError::BadRegions);
                }
            }
        }
        Ok(imp)
    }

    /// The model hash a `TIMP` blob records (0 if it is too short) — tools
    /// compare it with the current model to spot a stale impostor.
    pub fn model_hash(bytes: &[u8]) -> u32 {
        if bytes.len() < TIMP_HEADER {
            0
        } else {
            u32_at(bytes, 32)
        }
    }

    /// The pitch a `TIMP` blob was rendered at (radians).
    pub fn pitch(bytes: &[u8]) -> f32 {
        if bytes.len() < TIMP_HEADER {
            0.0
        } else {
            f32_at(bytes, 36)
        }
    }

    /// Write `self` as a `TIMP` blob into `out` (at least
    /// `TIMP_HEADER + 8 · views` bytes); returns the bytes written.
    pub fn write(&self, model_hash: u32, pitch: f32, out: &mut [u8]) -> usize {
        let n = TIMP_HEADER + self.views as usize * 8;
        assert!(out.len() >= n, "TIMP buffer too small");
        out[0..4].copy_from_slice(TIMP_MAGIC);
        out[4..6].copy_from_slice(&TIMP_VERSION.to_le_bytes());
        out[6] = self.views;
        out[7] = self.cols;
        out[8..12].copy_from_slice(&self.sprite.0.to_le_bytes());
        out[12..14].copy_from_slice(&self.tile.to_le_bytes());
        out[14..16].copy_from_slice(&[0, 0]);
        for (k, v) in [self.size, self.center.x, self.center.y, self.center.z]
            .iter()
            .enumerate()
        {
            out[16 + k * 4..20 + k * 4].copy_from_slice(&v.to_le_bytes());
        }
        out[32..36].copy_from_slice(&model_hash.to_le_bytes());
        out[36..40].copy_from_slice(&pitch.to_le_bytes());
        for i in 0..self.views {
            let r = self.region(i);
            let at = TIMP_HEADER + i as usize * 8;
            for (k, v) in [r.pos.x, r.pos.y, r.size.x, r.size.y].iter().enumerate() {
                out[at + k * 2..at + k * 2 + 2].copy_from_slice(&(*v as u16).to_le_bytes());
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TREE: Impostor = Impostor {
        sprite: SpriteId(7),
        views: 8,
        cols: 4,
        tile: 32,
        size: 2.0,
        center: Vec3::new(0.0, 1.0, 0.0),
    };

    #[test]
    fn regions_follow_the_grid() {
        assert_eq!(TREE.region(0), Rect::new(0.0, 0.0, 32.0, 32.0));
        assert_eq!(TREE.region(5), Rect::new(32.0, 32.0, 32.0, 32.0));
        assert_eq!(TREE.region(8), TREE.region(0), "wraps");
        assert_eq!(TREE.atlas_size(), (128, 64));
    }

    #[test]
    fn blobs_round_trip_and_reject_damage() {
        let mut buf = [0u8; TIMP_HEADER + 64];
        let n = TREE.write(0xABCD, 0.5, &mut buf);
        assert_eq!(n, TIMP_HEADER + 64);
        assert_eq!(Impostor::parse(&buf[..n]), Ok(TREE));
        assert_eq!(Impostor::model_hash(&buf), 0xABCD);
        assert_eq!(Impostor::pitch(&buf), 0.5);
        let mut bad = buf;
        bad[0] = b'X';
        assert_eq!(Impostor::parse(&bad), Err(ImpostorError::BadMagic));
        let mut bad = buf;
        bad[4] = 9;
        assert_eq!(
            Impostor::parse(&bad),
            Err(ImpostorError::UnsupportedVersion(9))
        );
        assert_eq!(
            Impostor::parse(&buf[..n - 1]),
            Err(ImpostorError::Truncated)
        );
        let mut bad = buf;
        bad[TIMP_HEADER + 8] = 1; // view 1's x
        assert_eq!(Impostor::parse(&bad), Err(ImpostorError::BadRegions));
        let mut bad = buf;
        bad[6] = 0;
        assert_eq!(Impostor::parse(&bad), Err(ImpostorError::BadLayout));
    }
}
