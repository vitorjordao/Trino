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

impl SpriteId {
    /// Handle from a logical asset path, e.g. `SpriteId::from_path("sprites/player")`.
    /// `const` — use it for game constants.
    pub const fn from_path(logical_path: &str) -> Self {
        SpriteId(crate::asset::asset_id(logical_path))
    }
}

/// Handle to a baked 3D model.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ModelId(pub u32);

impl ModelId {
    /// Handle from a logical asset path, e.g. `ModelId::from_path("models/cube")`.
    pub const fn from_path(logical_path: &str) -> Self {
        ModelId(crate::asset::asset_id(logical_path))
    }
}

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

/// Per-draw model parameters. `Default` draws the model as-is.
///
/// Found stress-testing a cube-world game: without a per-draw tint every
/// color variation of a model needs its own baked mesh (five identical
/// doors differing only in slab color). `tint` is the 3D analog of
/// [`SpriteParams::tint`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ModelParams {
    /// Multiplies the mesh's vertex colors (white = unchanged), applied
    /// before lighting. Alpha multiplies vertex alpha.
    pub tint: Color,
}

impl Default for ModelParams {
    fn default() -> Self {
        ModelParams { tint: Color::WHITE }
    }
}

/// 3D placement. Euler rotation in radians (XYZ order).
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

impl Transform3 {
    /// Model matrix for the software T&L pipeline.
    pub fn matrix(&self) -> crate::math3d::Mat34 {
        crate::math3d::Mat34::from_rotation_scale_translation(
            self.rotation,
            self.scale,
            self.position,
        )
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

    /// Set the 3D camera for subsequent `draw_model` calls this frame.
    /// Backends without 3D content may keep the default no-op.
    fn set_camera(&mut self, _camera: &crate::render3d::Camera3) {}

    /// Draw a 3D model (vertex-lit, engine-side T&L — see `render3d`).
    ///
    /// Models drawn back-to-back form a batch: the backend depth-sorts all
    /// triangles of the batch together (painter across meshes) and flushes
    /// it when a sprite is drawn, the camera changes or the frame ends.
    fn draw_model(
        &mut self,
        model: ModelId,
        transform: &Transform3,
        material: Material,
        params: &ModelParams,
    );

    fn end_frame(&mut self);
}
