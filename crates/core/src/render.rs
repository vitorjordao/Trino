//! The rendering contract between games and platform backends.
//!
//! Games never touch wgpu, libdragon or citro directly — they call these
//! methods with handles. Backends translate to their native API.
//!
//! Materials are **presets**, not free shaders: the N64 RDP cannot run
//! arbitrary shaders, so neither can Trino. Two layers:
//!
//! - [`Material`] picks the pipeline (`Sprite` / `VertexLit`).
//! - [`ModelParams`] carries the per-draw **surface** of a 3D draw — tint,
//!   emissive, [`Shading`] mode, double-sided, animation pose. All of it is
//!   evaluated by the engine's software T&L (`render3d::tessellate`), so
//!   every property works identically on PC, N64 and 3DS by construction.
//!   Material *files* (`assets/materials/*.material.ron`, editor + text) are
//!   resolved into `ModelParams` by the scene layer (`trino-scene`).
//!
//! `Material::Named` is reserved for per-platform presets and not wired to
//! any backend yet.

use crate::caps::Caps;
use crate::math::{Color, Vec2, Vec3};
use crate::math3d::Mat34;

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
    /// 3D geometry with per-vertex lighting (Fase 7). Surface properties
    /// come from [`ModelParams`].
    VertexLit,
    /// Reserved for per-platform presets (not wired to a backend yet).
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

/// How a 3D surface reacts to light. Evaluated per vertex by the engine's
/// software T&L — identical on every target.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Shading {
    /// Smooth per-vertex lighting from the mesh normals.
    #[default]
    Gouraud,
    /// One light value per triangle (face normal): the faceted low-poly look,
    /// whatever normals the mesh was exported with.
    Flat,
    /// No lighting: vertex color × tint, full bright (signs, sky, glow).
    Unlit,
}

/// Handle to an animation clip baked into a model (`ClipId::from_name("walk")`).
/// Same FNV-1a identity as asset handles, over the clip name.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ClipId(pub u32);

impl ClipId {
    pub const fn from_name(name: &str) -> Self {
        ClipId(crate::asset::asset_id(name))
    }
}

/// Which pose to draw an animated model in: a clip and a time in seconds.
/// Looping clips wrap; one-shot clips clamp to their last frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AnimRef {
    pub clip: ClipId,
    pub time: f32,
}

/// Per-draw model parameters — the **surface material** of a 3D draw.
/// `Default` draws the model as-is (lit, opaque, back faces culled).
///
/// Found stress-testing a cube-world game: without a per-draw tint every
/// color variation of a model needs its own baked mesh (five identical
/// doors differing only in slab color). The other fields grew from the same
/// principle: a material is data applied at draw time, never a new mesh.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ModelParams {
    /// Multiplies the mesh's vertex colors (white = unchanged), applied
    /// before lighting. Alpha multiplies vertex alpha.
    pub tint: Color,
    /// Added after lighting (black = none): glow that ignores the light.
    pub emissive: Color,
    pub shading: Shading,
    /// Draw back faces too (thin geometry: leaves, flags, planes).
    pub double_sided: bool,
    /// Pose for models with a baked skeleton; `None` = bind pose.
    pub anim: Option<AnimRef>,
}

impl ModelParams {
    pub const DEFAULT: ModelParams = ModelParams {
        tint: Color::WHITE,
        emissive: Color::BLACK,
        shading: Shading::Gouraud,
        double_sided: false,
        anim: None,
    };

    /// Default surface with a tint — the common case in game code.
    pub const fn tinted(tint: Color) -> Self {
        ModelParams {
            tint,
            ..Self::DEFAULT
        }
    }
}

impl Default for ModelParams {
    fn default() -> Self {
        Self::DEFAULT
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

    /// Set the light (direction, colors, ambient) and fog used by
    /// subsequent `draw_model` calls. Persists across frames until changed;
    /// starts as [`crate::render3d::DEFAULT_LIGHT`].
    fn set_light(&mut self, _light: &crate::render3d::Light) {}

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

    /// Draw a 3D model with an arbitrary model matrix — what hierarchies
    /// (prefab parts, attached props) need, since composed transforms are
    /// not always expressible as one [`Transform3`]. Backends override this
    /// with the exact path; the default decomposes the matrix (exact for
    /// rotation + scale, approximate only for sheared matrices).
    fn draw_model_matrix(
        &mut self,
        model: ModelId,
        matrix: &Mat34,
        material: Material,
        params: &ModelParams,
    ) {
        let (rotation, scale, position) = matrix.decompose();
        self.draw_model(
            model,
            &Transform3 {
                position,
                rotation,
                scale,
            },
            material,
            params,
        );
    }

    fn end_frame(&mut self);
}
