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
use crate::math::{Color, Rect, Vec2, Vec3};
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
    /// Sub-rectangle of the sprite to draw, in sprite pixels (atlases,
    /// glyphs, animation strips). `None` draws the whole sprite. The drawn
    /// size is `region.size * scale`. Every backend maps it natively (rdpq
    /// blit `s0/t0/width/height` on N64, a sub-texture on 3DS, UVs on PC).
    pub region: Option<Rect>,
}

impl SpriteParams {
    pub const DEFAULT: SpriteParams = SpriteParams {
        scale: Vec2::ONE,
        rotation: 0.0,
        tint: Color::WHITE,
        flip_x: false,
        flip_y: false,
        region: None,
    };
}

impl Default for SpriteParams {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Handle to a baked bitmap font (`[fonts.*]` in the manifest; logical path
/// `fonts/<name>`). The font's atlas is registered as a sprite under the
/// same numeric handle, so `SpriteId(font.0)` draws the raw atlas.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FontId(pub u32);

impl FontId {
    pub const fn from_path(logical_path: &str) -> Self {
        FontId(crate::asset::asset_id(logical_path))
    }
}

/// Horizontal alignment of each text line relative to the draw position.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Per-draw text parameters. `Default` draws white, 1:1, left-aligned.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TextParams {
    /// Pixel scale (integer scales stay crisp on every target).
    pub scale: f32,
    pub tint: Color,
    pub align: TextAlign,
}

impl TextParams {
    pub const DEFAULT: TextParams = TextParams {
        scale: 1.0,
        tint: Color::WHITE,
        align: TextAlign::Left,
    };

    pub const fn tinted(tint: Color) -> Self {
        TextParams {
            tint,
            ..Self::DEFAULT
        }
    }
}

impl Default for TextParams {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Per-draw billboard parameters (see [`Renderer::draw_billboard`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BillboardParams {
    /// World-space size of the quad.
    pub size: Vec2,
    /// Screen-space roll, radians.
    pub rotation: f32,
    /// Multiplies the texture (alpha too) — fades, color variation.
    pub tint: Color,
    /// Sprite sub-rectangle in pixels (flipbook frames); `None` = whole
    /// sprite.
    pub region: Option<Rect>,
}

impl BillboardParams {
    pub const DEFAULT: BillboardParams = BillboardParams {
        size: Vec2::ONE,
        rotation: 0.0,
        tint: Color::WHITE,
        region: None,
    };
}

impl Default for BillboardParams {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// What the previous frame cost — read by budget HUDs (the editor's Game
/// view) and compared against [`Caps`]. Backends that don't count return
/// zeros.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct FrameStats {
    /// 3D triangles rasterized (after culling/clipping/subdivision).
    pub tris: u32,
    /// Sprite draws (text glyphs and billboards included).
    pub sprites: u32,
    /// `draw_model*` calls that reached the T&L (culled or not).
    pub models: u32,
    /// Largest single texture (bytes, at the target's texel size) bound
    /// this frame — the TMEM pressure on N64.
    pub max_texture_bytes: u32,
    /// Software T&L work behind `tris` (vertex transforms, clipping,
    /// depth splits, culls) — deterministic, so tests pin it per scene.
    pub tnl: crate::render3d::TnlStats,
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
///
/// `blend` crossfades toward a second clip (idle → walk → run without
/// popping): each joint's translation/rotation/scale is interpolated
/// between the two sampled poses by `weight` before the hierarchy is
/// composed — two extra sample reads per joint, no extra vertex work.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AnimRef {
    pub clip: ClipId,
    pub time: f32,
    pub blend: Option<AnimBlend>,
}

/// Second clip of a crossfade (see [`AnimRef::blend`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AnimBlend {
    pub clip: ClipId,
    pub time: f32,
    /// 0 = only [`AnimRef::clip`], 1 = only this clip. Clamped to 0..1.
    pub weight: f32,
}

impl AnimRef {
    /// Play `clip` at `time` seconds.
    pub const fn new(clip: ClipId, time: f32) -> Self {
        AnimRef {
            clip,
            time,
            blend: None,
        }
    }

    /// Crossfade toward `clip` (at its own `time`) by `weight` (0..1).
    pub const fn blend_to(self, clip: ClipId, time: f32, weight: f32) -> Self {
        AnimRef {
            blend: Some(AnimBlend { clip, time, weight }),
            ..self
        }
    }
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
    /// Texture for meshes with UVs (overrides the model's baked default;
    /// `None` = use the default). One texture per draw: on the N64 it must
    /// fit TMEM (4 KB) and be power-of-two sized to tile. Ignored by meshes
    /// without UVs.
    pub texture: Option<SpriteId>,
    /// Per-draw colors for the mesh's color slots (TMDL `SLOT`): a vertex
    /// in slot `s` (1..=4) has its RGB multiplied by `palette[s - 1]`
    /// before tint and lighting; slot 0 keeps its baked color. One model
    /// drawn many times with different skin, hair or clothing colors.
    /// `None`, or a mesh without slots, draws the baked colors. See
    /// [`Palette`].
    pub palette: Option<Palette>,
    /// Shininess, 0 (matte) to 255 (mirror-like): the strength and
    /// tightness of specular highlights from the global light and local
    /// lights. Specular exists only on the per-pixel tier
    /// ([`crate::LightTier::PerPixel`], PC); per-vertex targets ignore it.
    pub gloss: u8,
}

/// Four per-draw colors for the color slots 1..=4 of a mesh
/// ([`ModelParams::palette`]). Each channel multiplies the vertex color as
/// `(vertex * entry + 127) / 255` (white keeps the vertex color exactly,
/// the same rounding as material tint stacking), so artists paint slot
/// regions white with their shading and the palette color comes out
/// shaded. The entries' alpha is ignored: vertex alpha stays as baked
/// (translucency is the tint's job).
pub type Palette = [Color; 4];

impl ModelParams {
    pub const DEFAULT: ModelParams = ModelParams {
        tint: Color::WHITE,
        emissive: Color::BLACK,
        shading: Shading::Gouraud,
        double_sided: false,
        anim: None,
        texture: None,
        palette: None,
        gloss: 0,
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

    /// Set the point and spot lights for the next 3D draws (see
    /// [`crate::light`]): the first [`Caps::max_local_lights`] are used,
    /// per pixel or per vertex as [`Caps::light_tier`] says. **Cleared at
    /// `begin_frame`**: set them every frame (they usually move), before
    /// the draws they light. Draws made before the call keep the lights
    /// they had.
    fn set_local_lights(&mut self, _lights: &[crate::light::LocalLight]) {}

    /// How wet the world looks, 0 (dry) to 1 (soaked): on the per-pixel
    /// tier (PC), surfaces facing up darken and turn glossy, mirroring the
    /// lights in highlights. Per-vertex targets ignore it. Cleared at
    /// `begin_frame`, like the local lights.
    fn set_wetness(&mut self, _wetness: f32) {}

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

    /// Draw `sprite` as a camera-facing quad at world position `pos` (its
    /// center) — z-tested and fogged like the 3D around it, batched with
    /// the current model batch. The texture is clamped (not tiled) and must
    /// fit texture memory as a whole (N64 TMEM: 4 KB); use `region` for
    /// flipbook frames of a small atlas. Unknown sprites draw nothing.
    fn draw_billboard(&mut self, _sprite: SpriteId, _pos: Vec3, _params: &BillboardParams) {}

    /// Fill a rectangle (screen pixels, top-left origin) with a solid
    /// color; alpha < 255 blends. A 2D draw like `draw_sprite` (it flushes
    /// the model batch), but with no texture: interface panels, bars and
    /// highlights without a texture upload per rectangle (on the N64 a
    /// sprite blit loads TMEM every time). Every backend implements it; the
    /// default draws nothing.
    fn fill_rect(&mut self, _rect: Rect, _color: Color) {}

    /// Draw `text` with a baked bitmap font, its top-left corner at `pos`
    /// (alignment shifts each line around `pos.x`). `\n` starts a new line.
    /// Built on sprite regions, so it costs one sprite draw per glyph.
    /// Unknown fonts draw nothing.
    fn draw_text(&mut self, _font: FontId, _pos: Vec2, _text: &str, _params: &TextParams) {}

    /// Size in pixels `text` would cover at `scale` (zero for unknown fonts).
    fn measure_text(&self, _font: FontId, _text: &str, _scale: f32) -> Vec2 {
        Vec2::ZERO
    }

    /// Counters of the last finished frame (see [`FrameStats`]).
    fn stats(&self) -> FrameStats {
        FrameStats::default()
    }

    /// The second screen's size (3DS: the 320×240 touch screen; the PC's
    /// 3DS profile simulates it), or `None` with one screen.
    fn second_screen(&self) -> Option<Vec2> {
        None
    }

    /// Where the next 2D draws go: `0` = the main screen (the 3D world's),
    /// `1` = the [`second_screen`](Self::second_screen) (2D only:
    /// `fill_rect`, sprites, text). Starts at `0` each frame. A no-op on
    /// single-screen targets.
    fn set_screen(&mut self, _screen: u8) {}

    /// **Test hook** (slow, never per frame): color of the last finished
    /// frame at internal pixel `(x, y)`, or `None` if this backend cannot
    /// read back. Lets self-tests check rendering on the real targets (PC
    /// offscreen readback, N64 framebuffer, 3DS LCD framebuffer — on the
    /// 3DS draw the probed frame a few times: the LCD is double-buffered).
    fn read_pixel(&mut self, _x: u32, _y: u32) -> Option<Color> {
        None
    }
}
