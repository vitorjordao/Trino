//! The rendering contract between games and platform backends.
//!
//! Games never touch wgpu, libdragon or citro directly — they call these
//! methods with handles. Backends translate to their native API.
//!
//! Materials are **presets** (an enum), not free shaders: the N64 RDP cannot
//! run arbitrary shaders, so neither can Trino. `Material::Named` points at a
//! preset declared in `platforms/*.toml`; a platform missing a definition for
//! a named material is a build error in the asset pipeline, never a runtime
//! fallback.

use crate::caps::Caps;
use crate::math::{Color, Vec2, Vec3};

/// Handle to a baked sprite/texture. Stable across live reloads: the ID is
/// derived from the asset's logical path, so a rebake swaps content in place.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SpriteId(pub u32);

/// Handle to a baked 3D model (Fase 7).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ModelId(pub u32);

/// Handle to a material preset declared in `platforms/*.toml`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MaterialId(pub u16);

/// Rendering preset. The whole material system of the engine.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Material {
    /// Textured 2D quad, alpha-blended.
    #[default]
    Sprite,
    /// 3D geometry with per-vertex lighting (Fase 7).
    VertexLit,
    /// A preset defined per-platform in `platforms/*.toml`.
    Named(MaterialId),
}

/// Per-draw sprite parameters. `Default` draws the sprite as-is.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SpriteParams {
    pub scale: Vec2,
    /// Rotation in radians around the sprite center.
    pub rotation: f32,
    pub tint: Color,
    pub flip_x: bool,
    pub flip_y: bool,
}

impl Default for SpriteParams {
    fn default() -> Self {
        SpriteParams {
            scale: Vec2::ONE,
            rotation: 0.0,
            tint: Color::WHITE,
            flip_x: false,
            flip_y: false,
        }
    }
}

/// 3D placement. Euler rotation in radians (XYZ order). Fase 7 material.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Transform3 {
    pub position: Vec3,
    pub rotation: Vec3,
    pub scale: Vec3,
}

impl Default for Transform3 {
    fn default() -> Self {
        Transform3 {
            position: Vec3::ZERO,
            rotation: Vec3::ZERO,
            scale: Vec3::ONE,
        }
    }
}

/// What every platform backend implements.
///
/// Call order per frame: `begin_frame` → any number of draws → `end_frame`.
pub trait Renderer {
    /// The hardware budget of this backend (or of the simulated console when
    /// the PC backend runs a console profile).
    fn caps(&self) -> &Caps;

    fn begin_frame(&mut self, clear: Color);

    /// Draw a 2D sprite with its top-left corner at `pos` (pixels, internal
    /// resolution).
    fn draw_sprite(&mut self, sprite: SpriteId, pos: Vec2, params: &SpriteParams);

    /// Draw a 3D model. Signature fixed now; backends implement in Fase 7.
    fn draw_model(&mut self, model: ModelId, transform: &Transform3, material: Material);

    fn end_frame(&mut self);
}
