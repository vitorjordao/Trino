//! Scene data for games, in two interchangeable forms:
//!
//! - **Exported code** (`cargo xtask scene export`, the editor's *Export to
//!   game code*): a `static` slice of flattened [`SceneNode`]s plus the
//!   camera, light and clear color as consts — compile-time checked.
//! - **Baked blob** (`[scenes.*]` in the manifest → `TSCN`, read by
//!   [`BakedScene`]): the same nodes loaded at runtime through
//!   `Platform::blob` — data-driven levels with no rebuild.
//!
//! Pure data, `no_std`, zero allocation: editor-made levels run on PC, N64
//! and 3DS through the same code.
//!
//! ```ignore
//! mod level { include!("level_gen.rs"); }
//! trino_core::scene::draw_nodes(renderer, level::NODES, &Mat34::IDENTITY, t);
//! let spawn = trino_core::scene::find(level::NODES, "spawn").unwrap().position();
//! ```

use crate::audio::{SoundId, SoundSource};
use crate::impostor::Impostor;
use crate::math::{Color, Vec3};
use crate::math3d::Mat34;
use crate::physics::{Aabb, Character, Collider, Shape, Solid};
use crate::render::{AnimRef, ClipId, Material, ModelId, ModelParams, Renderer, Shading};
use crate::render3d::{Camera3, Fog, Light};
use crate::shadow::{ShadowCast, ShadowPlane, ShadowSettings, ShadowTier};
use crate::vis::VisVolume;

/// An entity's tags: a static list (exported code) or a packed,
/// `\n`-separated string borrowed from a baked scene blob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tags {
    List(&'static [&'static str]),
    Packed(&'static str),
}

impl Tags {
    pub const NONE: Tags = Tags::List(&[]);

    pub fn contains(&self, tag: &str) -> bool {
        match self {
            Tags::List(l) => l.contains(&tag),
            Tags::Packed(p) => p.split('\n').any(|t| !t.is_empty() && t == tag),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &'static str> {
        let (list, packed) = match *self {
            Tags::List(l) => (l, ""),
            Tags::Packed(p) => (&[][..], p),
        };
        list.iter()
            .copied()
            .chain(packed.split('\n').filter(|t| !t.is_empty()))
    }

    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }
}

/// One distance level of a model: from `distance` (world units from the
/// camera eye) on, draw `model` instead — `None` draws nothing (culled) —
/// or, with `impostor`, a pre-rendered billboard of it (2 triangles).
/// Fake objects make natural far levels: a 6-sided cone for a tree, a box
/// for a house; an impostor is the farthest step before culling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodLevel {
    pub distance: f32,
    pub model: Option<ModelId>,
    /// Draw this impostor instead of a mesh (wins over `model`).
    pub impostor: Option<Impostor>,
}

impl LodLevel {
    /// A mesh level (`None` = culled).
    pub const fn model(distance: f32, model: Option<ModelId>) -> LodLevel {
        LodLevel {
            distance,
            model,
            impostor: None,
        }
    }

    /// An impostor level.
    pub const fn impostor(distance: f32, impostor: Impostor) -> LodLevel {
        LodLevel {
            distance,
            model: None,
            impostor: Some(impostor),
        }
    }
}

/// What a LOD chain picked for one distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LodPick {
    Model(ModelId),
    Impostor(Impostor),
    /// Nothing to draw.
    None,
}

impl LodPick {
    /// The mesh, when the pick is one.
    pub fn model(self) -> Option<ModelId> {
        match self {
            LodPick::Model(m) => Some(m),
            _ => None,
        }
    }

    /// Draw the pick for an object placed by `matrix` (the impostor's view
    /// comes from `eye`): a mesh with `params`, or a billboard tinted by
    /// `params.tint`.
    pub fn draw(
        self,
        renderer: &mut dyn Renderer,
        matrix: &Mat34,
        params: &ModelParams,
        eye: Vec3,
    ) {
        match self {
            LodPick::Model(m) => renderer.draw_model_matrix(m, matrix, Material::VertexLit, params),
            LodPick::Impostor(imp) => imp.draw(renderer, matrix, eye, params.tint),
            LodPick::None => {}
        }
    }
}

/// Bytes of one baked LOD record (TSCN v2): `f32 distance, u32 model`
/// (0 = none), then the impostor — `u32 sprite` (0 = not an impostor),
/// `u8 views, u8 cols, u16 tile, f32 size, f32*3 center`.
pub const LOD_RECORD_BYTES: usize = 32;

/// A node's LOD levels, sorted by distance: a static list (exported code)
/// or [`LOD_RECORD_BYTES`]-byte little-endian records borrowed from a baked
/// scene blob.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lods {
    List(&'static [LodLevel]),
    Packed(&'static [u8]),
}

impl Lods {
    pub const NONE: Lods = Lods::List(&[]);

    pub fn len(&self) -> usize {
        match self {
            Lods::List(l) => l.len(),
            Lods::Packed(b) => b.len() / LOD_RECORD_BYTES,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, i: usize) -> Option<LodLevel> {
        match self {
            Lods::List(l) => l.get(i).copied(),
            Lods::Packed(b) => {
                let r = b.get(i * LOD_RECORD_BYTES..(i + 1) * LOD_RECORD_BYTES)?;
                let u = |at: usize| u32::from_le_bytes([r[at], r[at + 1], r[at + 2], r[at + 3]]);
                let f = |at: usize| f32::from_bits(u(at));
                let id = u(4);
                let sprite = u(8);
                Some(LodLevel {
                    distance: f(0),
                    model: (id != 0).then_some(ModelId(id)),
                    impostor: (sprite != 0).then(|| Impostor {
                        sprite: crate::render::SpriteId(sprite),
                        views: r[12],
                        cols: r[13],
                        tile: u16::from_le_bytes([r[14], r[15]]),
                        size: f(16),
                        center: Vec3::new(f(20), f(24), f(28)),
                    }),
                })
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = LodLevel> + '_ {
        (0..self.len()).filter_map(|i| self.get(i))
    }

    /// What to draw at squared distance `dist_sq` from the eye: the last
    /// level whose distance is reached, else `base`. Squared distances
    /// keep `sqrt` (software on consoles) out of the per-node cost.
    pub fn pick(&self, base: Option<ModelId>, dist_sq: f32) -> LodPick {
        self.pick_biased(base, dist_sq, 1.0)
    }

    /// [`pick`](Self::pick) with every level's start distance multiplied by
    /// `bias` ([`crate::QualitySettings::lod_bias`], what
    /// [`crate::Renderer::lod_bias`] reports): above 1 detail stays
    /// farther out. A bias of 1 picks exactly what `pick` does.
    pub fn pick_biased(&self, base: Option<ModelId>, dist_sq: f32, bias: f32) -> LodPick {
        let mut pick = base.map_or(LodPick::None, LodPick::Model);
        for level in self.iter() {
            let start = level.distance * bias;
            if dist_sq >= start * start {
                pick = match (level.impostor, level.model) {
                    (Some(imp), _) => LodPick::Impostor(imp),
                    (None, Some(m)) => LodPick::Model(m),
                    (None, None) => LodPick::None,
                };
            } else {
                break;
            }
        }
        pick
    }

    /// The mesh [`pick`](Self::pick) chose (`None` for an impostor or a
    /// cull).
    pub fn select(&self, base: Option<ModelId>, dist_sq: f32) -> Option<ModelId> {
        self.pick(base, dist_sq).model()
    }
}

/// One flattened entity (prefabs expanded, world matrix precomputed).
#[derive(Clone, Copy, Debug)]
pub struct SceneNode {
    /// `/`-joined name path (`house/door`).
    pub name: &'static str,
    pub tags: Tags,
    pub matrix: Mat34,
    /// None = marker (spawn points, triggers, waypoints).
    pub model: Option<ModelId>,
    pub params: ModelParams,
    /// Clip to loop and its playback speed.
    pub anim: Option<(ClipId, f32)>,
    /// Collision shape (local to `matrix`); see [`crate::physics`].
    pub collider: Option<Collider>,
    /// Distance levels replacing `model` far from the camera.
    pub lod: Lods,
    /// Positional looping sound (see [`crate::audio::Ambience`]).
    pub sound: Option<SoundSource>,
    /// Interior cell this node belongs to ([`crate::vis::NO_CELL`] = none:
    /// drawn regardless of portal visibility).
    pub cell: u8,
    /// Cell or portal volume (see [`crate::vis`]).
    pub vis: Option<VisVolume>,
    /// A halo around it ([`crate::post::Glow`], issue #64): the consoles'
    /// bloom, drawn by [`draw_nodes`] where its mode says.
    pub glow: Option<crate::post::Glow>,
    /// A character's body (spawn point of a player or an NPC, issue #66):
    /// [`Character::config`] makes its controller.
    pub character: Option<Character>,
    /// Its accessibility block (issue #96), `None` for most nodes: the
    /// semantic colors of its model's slots ([`Self::semantic`]) and its
    /// high-contrast outline ([`Self::outline`]), as the [`NODE_ACCESS_BYTES`]
    /// bytes of a TSCN v8 node (`node_access` builds them for exported
    /// code). A reference, so a node pays 4 bytes for it on the consoles
    /// (an `EntityStore` pays them per slot: the N64 base tier's heap
    /// noticed the 44 a decoded copy cost).
    pub access: Option<&'static [u8; NODE_ACCESS_BYTES]>,
}

/// Bytes of a node's accessibility block ([`SceneNode::access`], TSCN v8):
/// `u32 × 4` semantic slot ids (0 = none), the palette `rgba × 4` (unused
/// in exported code: `params.palette` has it), the outline's color `rgba`,
/// center `f32 × 3`, radius `f32` (0 = no outline) and pixels `f32`.
pub const NODE_ACCESS_BYTES: usize = 56;

/// A node's accessibility block from its semantic slot ids and outline
/// (`const`: exported scenes build it in their statics).
pub const fn node_access(
    semantic: [u32; 4],
    outline: Option<NodeOutline>,
) -> [u8; NODE_ACCESS_BYTES] {
    let mut b = [0u8; NODE_ACCESS_BYTES];
    let mut i = 0;
    while i < 4 {
        let v = semantic[i].to_le_bytes();
        let mut k = 0;
        while k < 4 {
            b[i * 4 + k] = v[k];
            k += 1;
        }
        i += 1;
    }
    if let Some(o) = outline {
        b[32] = o.color.r;
        b[33] = o.color.g;
        b[34] = o.color.b;
        b[35] = o.color.a;
        let f = [o.center.x, o.center.y, o.center.z, o.radius, o.pixels];
        let mut j = 0;
        while j < 5 {
            let v = f[j].to_bits().to_le_bytes();
            let mut k = 0;
            while k < 4 {
                b[36 + j * 4 + k] = v[k];
                k += 1;
            }
            j += 1;
        }
    }
    b
}

/// A node's high-contrast outline ([`crate::access::Outline`], issue #96):
/// its color and width, and the model's bounding sphere in the node's own
/// space (the tools fill it from the model's bounds unless a radius was
/// authored).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct NodeOutline {
    pub color: Color,
    pub center: Vec3,
    pub radius: f32,
    /// The ring's width at the sphere's depth, in a 240-line picture's
    /// pixels ([`crate::access::Outline::pixels`]).
    pub pixels: f32,
}

impl NodeOutline {
    /// The outline of the node drawn at world matrix `m`: the sphere moved
    /// with it, its radius scaled by the matrix's largest axis.
    pub fn at(&self, m: &Mat34) -> crate::access::Outline {
        crate::access::Outline::new(
            self.color,
            m.transform_point(self.center),
            self.radius * m.max_scale(),
        )
        .with_pixels(self.pixels)
    }
}

/// What the player's accessibility settings change in a scene's drawing
/// (issue #96): semantic slot colors by color-vision mode, outlines in high
/// contrast. [`NodeAccess::NONE`] draws exactly what [`draw_nodes_in`]
/// always drew.
#[derive(Clone, Copy)]
pub struct NodeAccess<'a> {
    /// The project's semantic colors (`access/data`); without them a
    /// semantic slot keeps its base color.
    pub data: Option<&'a crate::access::AccessData<'a>>,
    pub cvd: crate::access::CvdMode,
    pub high_contrast: bool,
    /// The camera an outline is pushed back from: no camera, no outlines.
    pub camera: Option<&'a Camera3>,
}

impl NodeAccess<'_> {
    pub const NONE: NodeAccess<'static> = NodeAccess {
        data: None,
        cvd: crate::access::CvdMode::Off,
        high_contrast: false,
        camera: None,
    };
}

impl<'a> NodeAccess<'a> {
    /// The player's `settings`, the project's colors and the frame's
    /// camera.
    pub fn new(
        data: Option<&'a crate::access::AccessData<'a>>,
        settings: &crate::access::AccessSettings,
        camera: Option<&'a Camera3>,
    ) -> NodeAccess<'a> {
        NodeAccess {
            data,
            cvd: settings.cvd,
            high_contrast: settings.high_contrast,
            camera,
        }
    }
}

/// `params` with the semantic slots of `semantic` (ids, 0 = keep) taken
/// from `data` in `mode` (issue #96). Unchanged when no slot is semantic;
/// a name `data` lacks keeps the slot's color in `params.palette` (its
/// base).
pub fn semantic_params(
    params: &ModelParams,
    semantic: &[u32; 4],
    data: Option<&crate::access::AccessData<'_>>,
    mode: crate::access::CvdMode,
) -> ModelParams {
    if *semantic == [0; 4] {
        return *params;
    }
    let mut palette = params.palette.unwrap_or([Color::WHITE; 4]);
    if let Some(data) = data {
        for (c, &id) in palette.iter_mut().zip(semantic) {
            if id != 0
                && let Some(v) = data.color_id(id, mode)
            {
                *c = v;
            }
        }
    }
    ModelParams {
        palette: Some(palette),
        ..*params
    }
}

impl SceneNode {
    /// The semantic color ids of its model's slots 1..=4 (issue #96): the
    /// `asset_id` of a `palettes.ron` name, 0 = that slot keeps
    /// `params.palette`'s color. `params.palette` holds each name's base
    /// color, so a game without the access data draws those;
    /// [`draw_nodes_with`] swaps them by the player's color-vision mode.
    pub fn semantic(&self) -> [u32; 4] {
        match self.access {
            Some(b) => [0, 4, 8, 12].map(|o| rd_u32(b, o)),
            None => [0; 4],
        }
    }

    /// Its high-contrast outline (issue #96), drawn by [`draw_nodes_with`]
    /// while the player's settings ask for high contrast.
    pub fn outline(&self) -> Option<NodeOutline> {
        let b = self.access?;
        let radius = rd_f32(b, 48);
        (radius > 0.0).then(|| NodeOutline {
            color: rd_rgba(b, 32),
            center: rd_vec3(b, 36),
            radius,
            pixels: rd_f32(b, 52),
        })
    }

    /// World position (the matrix translation).
    pub fn position(&self) -> Vec3 {
        Vec3::new(
            self.matrix.m[0][3],
            self.matrix.m[1][3],
            self.matrix.m[2][3],
        )
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.contains(tag)
    }

    /// World box of the collider, if any.
    pub fn world_aabb(&self) -> Option<Aabb> {
        self.collider.map(|c| c.world_aabb(&self.matrix))
    }

    /// Model to draw seen from `eye` (LOD applied; `None` for an impostor
    /// level too — see [`lod_from`](Self::lod_from)).
    pub fn model_from(&self, eye: Vec3) -> Option<ModelId> {
        self.lod_from(eye).model()
    }

    /// What to draw seen from `eye`: the mesh, an impostor or nothing.
    pub fn lod_from(&self, eye: Vec3) -> LodPick {
        self.lod_from_biased(eye, 1.0)
    }

    /// [`lod_from`](Self::lod_from) with the LOD starts multiplied by `bias`
    /// ([`Lods::pick_biased`]; `renderer.lod_bias()` is the quality's).
    pub fn lod_from_biased(&self, eye: Vec3, bias: f32) -> LodPick {
        if self.lod.is_empty() {
            return self.model.map_or(LodPick::None, LodPick::Model);
        }
        let d = self.position() - eye;
        self.lod.pick_biased(self.model, d.dot(d), bias)
    }
}

// ---------------------------------------------------------------- TSCN ----

/// Baked scene blob ("TSCN", little-endian), written by
/// `trino_scene::bake::to_tscn`:
///
/// ```text
/// magic "TSCN"  u16 version (2)  u16 reserved  u32 node_count
/// environment: clear rgb u8*3 + pad u8, camera eye f32*3 target f32*3
///   fov_y f32 (radians), light dir f32*3 ambient f32 color rgba
///   ambient_color rgba, fog color rgba near f32 far f32,
///   shadows (v5): u8 on, u8 tier_max (0 none, 1 blob, 2 planar, 3 map),
///   u16 map_size, f32 distance, u8 softness, u8 reserved*3, color rgba
///                                                             (= 84 bytes)
/// node_count records of NODE_BYTES:
///   name (u32 offset, u32 len) and tags (u32 offset, u32 len) into the
///   string table — tags are `\n`-separated
///   matrix f32*12 (row-major 3x4)
///   flags u32: bit0 model, bit1 anim, bit2 double_sided, bit3 texture,
///     bit4 sound, bit5 detail layer, bit6 water,
///     bits 8..10 shading
///     (0 gouraud, 1 flat, 2 unlit), bits 12..14 collider shape (0 none,
///     1 box, 2 sphere, 3 capsule), bit15 collider trigger
///   model u32, tint rgba, emissive rgba, clip u32, speed f32
///   collider a f32 b f32 c f32 (box half xyz / sphere r / capsule r,h)
///   collider offset f32*3, texture u32 (sprite handle),
///   LOD levels (u32 offset, u32 count) into the data table — 32-byte
///   records (`LOD_RECORD_BYTES`: distance, model, impostor), sorted by
///   distance,
///   sound source: u32 sound (flag bit4), f32 radius, f32 volume,
///   visibility: u8 cell (0xFF none), u8 volume kind (0 none, 1 cell,
///   2 portal), u8 portal a, u8 portal b, f32*3 world half extents,
///   surface (v3): u8 gloss, u8 filter (0 default, 1 point, 2 bilinear),
///   u8 detail scale_log2, u8 reserved, u32 detail sprite (flag bit5)
///   water (v4, flag bit6): f32 flow_speed x y, f32 detail_flow x y,
///   f32 wave_amplitude, f32 wave_frequency, f32 ripple_responsiveness
///   shadow (v5): u8 cast (0 none, 1 blob, 2 mesh, 3 auto), u8 receiver,
///   u16 reserved, f32 blob radius, receiver top f32*3 center and f32*2
///   half sizes (x, z)
///   glow (v6, flag bit7): u32 sprite, f32 radius, color rgba, u8 mode
///   (0 baked, 1 always), u8 reserved*3
///   physics (v7): u16 collider layer, u8 one_way, u8 ramp direction
///   (`RampDir::code`), u32 mesh blob (shape 5), then the character
///   (flag bit16): f32 radius, f32 height, f32 max_slope_deg,
///   f32 step_height; shapes 4 (ramp: a b c = half extents) and 5 (mesh:
///   a b c = half extents of its local bounds)
///   access (v8, issue #96): u32*4 semantic slot ids (0 = none), the
///   palette rgba*4 (flag bit18; the semantic slots' base colors), then
///   the outline (flag bit17): color rgba, center f32*3, radius f32,
///   pixels f32
/// data table: UTF-8 strings and LOD records, to the end of the blob: UTF-8 bytes to the end of the blob
/// ```
///
/// [`BakedScene::parse`] validates everything up front (bounds, UTF-8), so
/// node access never fails afterwards.
pub const TSCN_MAGIC: &[u8; 4] = b"TSCN";
/// v2 (October 2026): LOD records grew from 8 to 32 bytes (impostor
/// levels). v3: nodes end with the surface's gloss, texture filter and
/// detail layer (8 bytes). v4: then a water surface (28 bytes, ADR 0015).
/// v5: the environment ends with the shadows (16 bytes) and nodes with
/// what they cast and the receiver they are (28 bytes, ADR 0025). v6:
/// nodes end with their glow (16 bytes, issue #64). v7: then their
/// physics (24 bytes, issue #66): collider layer, one-way, ramps, mesh
/// colliders and the character component. v8: then their semantic slot
/// colors, palette and high-contrast outline (56 bytes, issue #96).
/// Older blobs are refused: rebake the scene.
pub const TSCN_VERSION: u16 = 8;
pub const TSCN_HEADER: usize = 12;
pub const TSCN_ENV_BYTES: usize = 68 + TSCN_ENV_SHADOW_BYTES;
/// The shadows block at the end of the environment (v5).
pub const TSCN_ENV_SHADOW_BYTES: usize = 16;
pub const TSCN_NODE_BYTES: usize = 16
    + 48
    + 4
    + 4
    + 4
    + 4
    + 4
    + 4
    + 12
    + 12
    + 4
    + 8
    + 12
    + 16
    + 8
    + TSCN_WATER_BYTES
    + TSCN_SHADOW_BYTES
    + TSCN_GLOW_BYTES
    + TSCN_PHYSICS_BYTES
    + TSCN_ACCESS_BYTES;
/// The water block at the end of a node (v4).
pub const TSCN_WATER_BYTES: usize = 28;
/// The shadow block near the end of a node (v5).
pub const TSCN_SHADOW_BYTES: usize = 28;
/// The glow block at the end of a node (v6).
pub const TSCN_GLOW_BYTES: usize = 16;
/// The physics block near the end of a node (v7).
pub const TSCN_PHYSICS_BYTES: usize = 24;
/// The accessibility block at the end of a node (v8).
pub const TSCN_ACCESS_BYTES: usize = NODE_ACCESS_BYTES;
/// Where a node's accessibility block starts.
const ACCESS_AT: usize = TSCN_NODE_BYTES - TSCN_ACCESS_BYTES;
/// Where a node's physics block starts.
const PHYSICS_AT: usize = ACCESS_AT - TSCN_PHYSICS_BYTES;
/// Where a node's glow block starts.
const GLOW_AT: usize = PHYSICS_AT - TSCN_GLOW_BYTES;
/// Where a node's shadow block starts.
const SHADOW_AT: usize = GLOW_AT - TSCN_SHADOW_BYTES;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SceneError {
    BadMagic,
    UnsupportedVersion(u16),
    Truncated,
    /// A string reference points outside the table or is not UTF-8.
    BadString,
}

/// A parsed baked scene, borrowing a `'static` blob (`Platform::blob`).
#[derive(Clone, Copy, Debug)]
pub struct BakedScene {
    bytes: &'static [u8],
    node_count: usize,
    strings: usize,
}

#[inline]
fn rd_u32(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
}

#[inline]
fn rd_f32(d: &[u8], at: usize) -> f32 {
    f32::from_bits(rd_u32(d, at))
}

#[inline]
fn rd_vec3(d: &[u8], at: usize) -> Vec3 {
    Vec3::new(rd_f32(d, at), rd_f32(d, at + 4), rd_f32(d, at + 8))
}

#[inline]
fn rd_rgba(d: &[u8], at: usize) -> Color {
    Color::rgba(d[at], d[at + 1], d[at + 2], d[at + 3])
}

impl BakedScene {
    pub fn parse(bytes: &'static [u8]) -> Result<Self, SceneError> {
        if bytes.len() < TSCN_HEADER + TSCN_ENV_BYTES {
            return Err(SceneError::Truncated);
        }
        if &bytes[0..4] != TSCN_MAGIC {
            return Err(SceneError::BadMagic);
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != TSCN_VERSION {
            return Err(SceneError::UnsupportedVersion(version));
        }
        let node_count = rd_u32(bytes, 8) as usize;
        let strings = node_count
            .checked_mul(TSCN_NODE_BYTES)
            .and_then(|n| n.checked_add(TSCN_HEADER + TSCN_ENV_BYTES))
            .ok_or(SceneError::Truncated)?;
        if bytes.len() < strings {
            return Err(SceneError::Truncated);
        }
        let scene = BakedScene {
            bytes,
            node_count,
            strings,
        };
        for i in 0..node_count {
            let at = scene.node_at(i);
            for field in [at, at + 8] {
                scene.string(rd_u32(bytes, field), rd_u32(bytes, field + 4))?;
            }
            let lod = at + 64 + 52;
            scene.data(
                rd_u32(bytes, lod),
                rd_u32(bytes, lod + 4).saturating_mul(LOD_RECORD_BYTES as u32),
            )?;
        }
        Ok(scene)
    }

    pub fn node_count(&self) -> usize {
        self.node_count
    }

    fn node_at(&self, i: usize) -> usize {
        TSCN_HEADER + TSCN_ENV_BYTES + i * TSCN_NODE_BYTES
    }

    /// Raw bytes `offset..offset+len` of the data table.
    fn data(&self, offset: u32, len: u32) -> Result<&'static [u8], SceneError> {
        let start = self
            .strings
            .checked_add(offset as usize)
            .ok_or(SceneError::BadString)?;
        let end = start
            .checked_add(len as usize)
            .ok_or(SceneError::BadString)?;
        let bytes: &'static [u8] = self.bytes;
        bytes.get(start..end).ok_or(SceneError::BadString)
    }

    fn string(&self, offset: u32, len: u32) -> Result<&'static str, SceneError> {
        core::str::from_utf8(self.data(offset, len)?).map_err(|_| SceneError::BadString)
    }

    pub fn clear(&self) -> Color {
        let e = TSCN_HEADER;
        Color::rgb(self.bytes[e], self.bytes[e + 1], self.bytes[e + 2])
    }

    pub fn camera(&self) -> Camera3 {
        let e = TSCN_HEADER + 4;
        Camera3 {
            eye: rd_vec3(self.bytes, e),
            target: rd_vec3(self.bytes, e + 12),
            fov_y: rd_f32(self.bytes, e + 24),
        }
    }

    pub fn light(&self) -> Light {
        let e = TSCN_HEADER + 32;
        Light {
            dir: rd_vec3(self.bytes, e),
            ambient: rd_f32(self.bytes, e + 12),
            color: rd_rgba(self.bytes, e + 16),
            ambient_color: rd_rgba(self.bytes, e + 20),
            fog: Fog {
                color: rd_rgba(self.bytes, e + 24),
                near: rd_f32(self.bytes, e + 28),
                far: rd_f32(self.bytes, e + 32),
            },
        }
    }

    /// The scene's shadows (`None`: off), for `Renderer::set_shadows`.
    pub fn shadows(&self) -> Option<ShadowSettings> {
        let e = TSCN_HEADER + 68;
        let d = self.bytes;
        if d[e] == 0 {
            return None;
        }
        Some(
            ShadowSettings {
                tier_max: match d[e + 1] {
                    0 => ShadowTier::None,
                    1 => ShadowTier::Blob,
                    2 => ShadowTier::Planar,
                    _ => ShadowTier::Map,
                },
                map_size: u16::from_le_bytes([d[e + 2], d[e + 3]]),
                distance: rd_f32(d, e + 4),
                softness: d[e + 8],
                color: rd_rgba(d, e + 12),
                // TSCN v5 predates contact hardening (#86): a baked scene
                // gets ADR 0025's look; a game sets them over it.
                ..ShadowSettings::DEFAULT
            }
            .clamped(),
        )
    }

    /// The top of node `i`'s box, when shadows land on it.
    pub fn receiver(&self, i: usize) -> Option<ShadowPlane> {
        let s = self.node_at(i) + SHADOW_AT;
        let d = self.bytes;
        (d[s + 1] != 0).then(|| {
            ShadowPlane::horizontal(rd_vec3(d, s + 8), rd_f32(d, s + 20), rd_f32(d, s + 24))
        })
    }

    /// Every receiver, in node order (`Renderer::set_shadow_receivers`).
    pub fn receivers(&self) -> impl Iterator<Item = ShadowPlane> + '_ {
        (0..self.node_count).filter_map(|i| self.receiver(i))
    }

    /// Node `i` (`i < node_count()`), decoded (strings borrowed).
    pub fn node(&self, i: usize) -> SceneNode {
        let d = self.bytes;
        let at = self.node_at(i);
        let name = self
            .string(rd_u32(d, at), rd_u32(d, at + 4))
            .expect("validated in parse");
        let tags = self
            .string(rd_u32(d, at + 8), rd_u32(d, at + 12))
            .expect("validated in parse");
        let mut matrix = Mat34::IDENTITY;
        for r in 0..3 {
            for c in 0..4 {
                matrix.m[r][c] = rd_f32(d, at + 16 + (r * 4 + c) * 4);
            }
        }
        let b = at + 64;
        let flags = rd_u32(d, b);
        let shading = match (flags >> 8) & 7 {
            1 => Shading::Flat,
            2 => Shading::Unlit,
            _ => Shading::Gouraud,
        };
        let surf = b + 88;
        let params = ModelParams {
            tint: rd_rgba(d, b + 8),
            emissive: rd_rgba(d, b + 12),
            shading,
            double_sided: flags & 4 != 0,
            anim: None,
            texture: (flags & 8 != 0).then(|| crate::render::SpriteId(rd_u32(d, b + 48))),
            gloss: d[surf],
            filter: match d[surf + 1] {
                1 => crate::render::TexFilter::Point,
                2 => crate::render::TexFilter::Bilinear,
                _ => crate::render::TexFilter::Default,
            },
            detail: (flags & 32 != 0).then(|| {
                crate::render::DetailTexture::new(
                    crate::render::SpriteId(rd_u32(d, surf + 4)),
                    d[surf + 2],
                )
            }),
            water: (flags & 64 != 0).then(|| {
                let w = surf + 8;
                crate::water::Water {
                    flow_speed: crate::math::Vec2::new(rd_f32(d, w), rd_f32(d, w + 4)),
                    detail_flow: crate::math::Vec2::new(rd_f32(d, w + 8), rd_f32(d, w + 12)),
                    wave_amplitude: rd_f32(d, w + 16),
                    wave_frequency: rd_f32(d, w + 20),
                    ripple_responsiveness: rd_f32(d, w + 24),
                }
            }),
            shadow: {
                let s = at + SHADOW_AT;
                let radius = rd_f32(d, s + 4);
                match d[s] {
                    1 => ShadowCast::Blob { radius },
                    2 => ShadowCast::Mesh,
                    3 => ShadowCast::Auto {
                        blob_radius: radius,
                    },
                    _ => ShadowCast::None,
                }
            },
            ..ModelParams::DEFAULT
        };
        // The palette (v8, flag bit 18): the semantic slots' base colors.
        let params = ModelParams {
            palette: (flags & (1 << 18) != 0).then(|| {
                let p = at + ACCESS_AT + 16;
                [0, 4, 8, 12].map(|o| rd_rgba(d, p + o))
            }),
            ..params
        };
        let (x, y, z) = (rd_f32(d, b + 24), rd_f32(d, b + 28), rd_f32(d, b + 32));
        let shape = match (flags >> 12) & 7 {
            1 => Some(Shape::Box {
                half: Vec3::new(x, y, z),
            }),
            2 => Some(Shape::Sphere { radius: x }),
            3 => Some(Shape::Capsule {
                radius: x,
                height: y,
            }),
            4 => Some(Shape::Ramp {
                half: Vec3::new(x, y, z),
                dir: crate::physics::RampDir::from_code(d[at + PHYSICS_AT + 3]),
            }),
            5 => Some(Shape::Mesh {
                mesh: rd_u32(d, at + PHYSICS_AT + 4),
                half: Vec3::new(x, y, z),
            }),
            _ => None,
        };
        let ph = at + PHYSICS_AT;
        let character = (flags & (1 << 16) != 0).then(|| Character {
            radius: rd_f32(d, ph + 8),
            height: rd_f32(d, ph + 12),
            max_slope_deg: rd_f32(d, ph + 16),
            step_height: rd_f32(d, ph + 20),
        });
        let lod_at = b + 52;
        let lod = self
            .data(
                rd_u32(d, lod_at),
                rd_u32(d, lod_at + 4) * LOD_RECORD_BYTES as u32,
            )
            .expect("validated in parse");
        let sound = (flags & 16 != 0).then(|| SoundSource {
            sound: SoundId(rd_u32(d, b + 60)),
            radius: rd_f32(d, b + 64),
            volume: rd_f32(d, b + 68),
        });
        let v = b + 72;
        let half = rd_vec3(d, v + 4);
        let vis = match d[v + 1] {
            1 => Some(VisVolume::Cell { half }),
            2 => Some(VisVolume::Portal {
                half,
                a: d[v + 2],
                b: d[v + 3],
            }),
            _ => None,
        };
        let g = at + GLOW_AT;
        let glow = (flags & 128 != 0).then(|| crate::post::Glow {
            sprite: crate::render::SpriteId(rd_u32(d, g)),
            radius: rd_f32(d, g + 4),
            color: rd_rgba(d, g + 8),
            mode: if d[g + 12] == 1 {
                crate::post::GlowMode::Always
            } else {
                crate::post::GlowMode::Baked
            },
        });
        // The accessibility block, borrowed from the blob when it holds a
        // semantic slot or an outline (flag bit17).
        let a = at + ACCESS_AT;
        let block: &'static [u8; NODE_ACCESS_BYTES] = d[a..a + NODE_ACCESS_BYTES]
            .try_into()
            .expect("validated in parse");
        let access =
            (flags & (1 << 17) != 0 || block[..16].iter().any(|&b| b != 0)).then_some(block);
        SceneNode {
            name,
            tags: Tags::Packed(tags),
            access,
            glow,
            cell: d[v],
            vis,
            lod: Lods::Packed(lod),
            sound,
            matrix,
            model: (flags & 1 != 0).then(|| ModelId(rd_u32(d, b + 4))),
            params,
            anim: (flags & 2 != 0).then(|| (ClipId(rd_u32(d, b + 16)), rd_f32(d, b + 20))),
            collider: shape.map(|shape| Collider {
                shape,
                offset: rd_vec3(d, b + 36),
                trigger: flags & (1 << 15) != 0,
                layer: u16::from_le_bytes([d[ph], d[ph + 1]]),
                one_way: d[ph + 2] != 0,
            }),
            character,
        }
    }

    pub fn nodes(&self) -> impl Iterator<Item = SceneNode> + '_ {
        (0..self.node_count).map(|i| self.node(i))
    }

    /// First node named `name` (or whose path ends with `/name`).
    pub fn find(&self, name: &str) -> Option<SceneNode> {
        self.nodes().find(|n| name_matches(n.name, name))
    }
}

fn name_matches(full: &str, name: &str) -> bool {
    full == name
        || (full.len() > name.len()
            && full.ends_with(name)
            && full.as_bytes()[full.len() - name.len() - 1] == b'/')
}

/// World boxes of every **solid** collider in `nodes`, written to `out`;
/// returns how many (stops at `out.len()`). Static level collision without
/// an [`crate::entity::EntityStore`].
pub fn solids(nodes: &[SceneNode], out: &mut [Aabb]) -> usize {
    let mut n = 0;
    for node in nodes {
        let Some(c) = node.collider.filter(|c| !c.trigger) else {
            continue;
        };
        if n == out.len() {
            break;
        }
        out[n] = c.world_aabb(&node.matrix);
        n += 1;
    }
    n
}

/// The level's collision: each solid collider in `nodes` as a [`Solid`]
/// (boxes, one-way boxes and ramps with their layers; round shapes as
/// their box), written to `out`; returns how many (stops at `out.len()`).
/// Feed it to a [`crate::physics::StaticWorld`] or the controller.
pub fn level_solids(nodes: &[SceneNode], out: &mut [Solid]) -> usize {
    let mut n = 0;
    for node in nodes {
        let Some(s) = node.collider.and_then(|c| c.world_solid(&node.matrix)) else {
            continue;
        };
        if n == out.len() {
            break;
        }
        out[n] = s;
        n += 1;
    }
    n
}

/// The trigger volumes of `nodes` (their index in `out` is the trigger id
/// a [`crate::physics::TriggerTracker`] reports); `names` gets the node
/// index of each. Returns how many.
pub fn level_triggers(nodes: &[SceneNode], out: &mut [Solid], names: &mut [u16]) -> usize {
    let mut n = 0;
    for (i, node) in nodes.iter().enumerate() {
        let Some(t) = node.collider.and_then(|c| c.world_trigger(&node.matrix)) else {
            continue;
        };
        if n == out.len() || n == names.len() {
            break;
        }
        out[n] = t;
        names[n] = i as u16;
        n += 1;
    }
    n
}

/// Draw every model node under `root`, animating clips at `time` seconds,
/// with LOD levels chosen for a camera at `eye` (pass `camera.eye`).
pub fn draw_nodes(
    renderer: &mut dyn Renderer,
    nodes: &[SceneNode],
    root: &Mat34,
    time: f32,
    eye: Vec3,
) {
    draw_nodes_in(renderer, nodes, root, time, eye, u64::MAX);
}

/// [`draw_nodes`] limited to the cells set in `visible` (a mask from
/// [`crate::vis::Cells::visible`]); nodes outside every cell always draw.
pub fn draw_nodes_in(
    renderer: &mut dyn Renderer,
    nodes: &[SceneNode],
    root: &Mat34,
    time: f32,
    eye: Vec3,
    visible: u64,
) {
    draw_nodes_with(renderer, nodes, root, time, eye, visible, &NodeAccess::NONE);
}

/// [`draw_nodes_in`] as the player's accessibility settings see it (issue
/// #96): semantic slots in their color-vision mode, and in high contrast
/// each outlined node's ring before it (one more unlit draw per node).
pub fn draw_nodes_with(
    renderer: &mut dyn Renderer,
    nodes: &[SceneNode],
    root: &Mat34,
    time: f32,
    eye: Vec3,
    visible: u64,
    access: &NodeAccess,
) {
    // The quality's LOD bias (1 on the consoles), read once per call.
    let bias = renderer.lod_bias();
    for node in nodes {
        if !crate::vis::in_view(node.cell, visible) {
            continue;
        }
        let pick = node.lod_from_biased(eye, bias);
        // Culled by distance; a marker without a model may still glow.
        if pick == LodPick::None && (node.model.is_some() || node.glow.is_none()) {
            continue;
        }
        let mut params = match node.access {
            Some(_) => semantic_params(&node.params, &node.semantic(), access.data, access.cvd),
            None => node.params,
        };
        if let Some((clip, speed)) = node.anim {
            params.anim = Some(AnimRef::new(clip, time * speed));
        }
        let matrix = root.mul(&node.matrix);
        if access.high_contrast
            && let (Some(camera), Some(o), LodPick::Model(m)) =
                (access.camera, node.outline(), pick)
        {
            o.at(&matrix)
                .draw(renderer, camera, m, &matrix, Material::VertexLit);
        }
        if pick != LodPick::None {
            pick.draw(renderer, &matrix, &params, eye);
        }
        if let Some(glow) = &node.glow {
            let at = Vec3::new(matrix.m[0][3], matrix.m[1][3], matrix.m[2][3]);
            crate::post::draw_glow(renderer, glow, at);
        }
    }
}

/// First node whose name path is `name` or ends with `/name`.
pub fn find<'a>(nodes: &'a [SceneNode], name: &str) -> Option<&'a SceneNode> {
    nodes.iter().find(|n| name_matches(n.name, name))
}

/// Nodes carrying `tag`.
pub fn with_tag<'a>(nodes: &'a [SceneNode], tag: &'a str) -> impl Iterator<Item = &'a SceneNode> {
    nodes.iter().filter(move |n| n.has_tag(tag))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vis::NO_CELL;

    const NODES: &[SceneNode] = &[
        SceneNode {
            name: "house/door",
            tags: Tags::List(&["door"]),
            matrix: Mat34::IDENTITY,
            model: None,
            params: ModelParams::DEFAULT,
            anim: None,
            collider: None,
            lod: Lods::NONE,
            sound: None,
            cell: NO_CELL,
            vis: None,
            glow: None,
            character: None,
            access: None,
        },
        SceneNode {
            name: "spawn",
            tags: Tags::List(&["spawn"]),
            matrix: Mat34 {
                m: [
                    [1.0, 0.0, 0.0, 3.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, -2.0],
                ],
            },
            model: None,
            params: ModelParams::DEFAULT,
            anim: None,
            collider: None,
            lod: Lods::NONE,
            sound: None,
            cell: NO_CELL,
            vis: None,
            glow: None,
            character: None,
            access: None,
        },
    ];

    #[test]
    fn find_by_name_or_leaf_and_by_tag() {
        assert_eq!(find(NODES, "door").unwrap().name, "house/door");
        assert_eq!(find(NODES, "house/door").unwrap().name, "house/door");
        assert!(find(NODES, "oor").is_none(), "leaf match must align on `/`");
        assert_eq!(
            find(NODES, "spawn").unwrap().position(),
            Vec3::new(3.0, 0.0, -2.0)
        );
        assert_eq!(with_tag(NODES, "door").count(), 1);
    }

    #[test]
    fn solids_skip_triggers() {
        let mut nodes = [NODES[0], NODES[1]];
        nodes[0].collider = Some(Collider::solid_box(Vec3::ONE));
        nodes[1].collider = Some(Collider {
            trigger: true,
            ..Collider::solid_box(Vec3::ONE)
        });
        let mut out = [Aabb::EMPTY; 4];
        assert_eq!(solids(&nodes, &mut out), 1);
        assert_eq!(out[0].max, Vec3::ONE);
        assert_eq!(
            nodes[1].world_aabb().unwrap().min,
            Vec3::new(2.0, -1.0, -3.0)
        );
    }

    #[test]
    fn packed_tags() {
        let t = Tags::Packed("coin\nshiny");
        assert!(t.contains("coin") && t.contains("shiny") && !t.contains("coi"));
        assert_eq!(t.iter().count(), 2);
        assert!(Tags::Packed("").is_empty() && Tags::NONE.is_empty());
        assert!(!Tags::Packed("").contains(""));
    }

    /// Hand-built TSCN with one node (the writer lives in `trino-scene`,
    /// whose tests round-trip real scenes through this reader).
    fn tscn() -> &'static [u8] {
        let mut b = std::vec::Vec::new();
        b.extend_from_slice(TSCN_MAGIC);
        b.extend_from_slice(&TSCN_VERSION.to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&[10, 20, 30, 0]);
        let f = |b: &mut std::vec::Vec<u8>, v: &[f32]| {
            for x in v {
                b.extend_from_slice(&x.to_le_bytes());
            }
        };
        f(&mut b, &[0.0, 1.0, -5.0, 0.0, 0.0, 0.0, 1.0]); // camera
        f(&mut b, &[0.0, -1.0, 0.0, 0.5]); // light dir + ambient
        b.extend_from_slice(&[255, 255, 255, 255, 200, 200, 255, 255, 1, 2, 3, 255]);
        f(&mut b, &[5.0, 20.0]);
        // Shadows on: tier_max blob, map 512, 30 m, softness 90, color.
        b.extend_from_slice(&[1, 1]);
        b.extend_from_slice(&512u16.to_le_bytes());
        f(&mut b, &[30.0]);
        b.extend_from_slice(&[90, 0, 0, 0, 10, 20, 30, 120]);
        assert_eq!(b.len(), TSCN_HEADER + TSCN_ENV_BYTES);
        // name "a/door" @0 (6), tags "door\nx" @6 (6)
        for v in [0u32, 6, 6, 6] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        f(
            &mut b,
            &[1.0, 0.0, 0.0, 3.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, -2.0],
        );
        let flags: u32 = 1 | 2 | (2 << 8) | (1 << 12) | (1 << 15);
        b.extend_from_slice(&flags.to_le_bytes());
        b.extend_from_slice(&7u32.to_le_bytes()); // model
        b.extend_from_slice(&[255, 0, 0, 255, 0, 0, 0, 255]); // tint, emissive
        b.extend_from_slice(&9u32.to_le_bytes()); // clip
        f(&mut b, &[2.0, 0.5, 1.0, 0.5, 0.0, 1.0, 0.0]);
        b.extend_from_slice(&0u32.to_le_bytes()); // texture (flag off)
        b.extend_from_slice(&12u32.to_le_bytes()); // LOD records at 12
        b.extend_from_slice(&1u32.to_le_bytes()); // one level
        b.extend_from_slice(&[0; 12]); // sound (flag off)
        b.extend_from_slice(&[3, 2, 1, 4]); // cell 3, portal 1 <-> 4
        f(&mut b, &[0.5, 1.0, 0.25]);
        b.extend_from_slice(&[40, 1, 0, 0, 0, 0, 0, 0]); // gloss 40, point, no detail
        f(&mut b, &[0.5, -0.25, 0.1, 0.2, 0.05, 0.6, 1.5]); // water (flag bit6 off)
        // Casts a 0.4 m blob (auto), receives: a 2 x 1 top at (3, 1, -2).
        b.extend_from_slice(&[3, 1, 0, 0]);
        f(&mut b, &[0.4, 3.0, 1.0, -2.0, 1.0, 0.5]);
        // Glow (v6, flag bit7 off).
        b.extend_from_slice(&[0; TSCN_GLOW_BYTES]);
        // Physics (v7): layer 4, not one-way, no character.
        b.extend_from_slice(&[4, 0, 0, 0]);
        b.extend_from_slice(&[0; TSCN_PHYSICS_BYTES - 4]);
        // Access (v8): slot 2 semantic, no palette (flag bit18 off) nor
        // outline (bit17 off).
        for id in [0u32, 0xAB, 0, 0] {
            b.extend_from_slice(&id.to_le_bytes());
        }
        b.extend_from_slice(&[0; TSCN_ACCESS_BYTES - 16]);
        b.extend_from_slice(b"a/doordoor\nx");
        f(&mut b, &[30.0]);
        b.extend_from_slice(&0u32.to_le_bytes()); // culled beyond 30
        b.extend_from_slice(&[0; LOD_RECORD_BYTES - 8]); // not an impostor
        std::boxed::Box::leak(b.into_boxed_slice())
    }

    #[test]
    fn baked_scene_reads_every_field() {
        let s = BakedScene::parse(tscn()).unwrap();
        assert_eq!(s.node_count(), 1);
        assert_eq!(s.clear(), Color::rgb(10, 20, 30));
        assert_eq!(s.camera().eye, Vec3::new(0.0, 1.0, -5.0));
        assert_eq!(s.light().fog.far, 20.0);
        let n = s.find("door").unwrap();
        assert_eq!(n.name, "a/door");
        assert!(n.has_tag("door") && n.has_tag("x"));
        assert_eq!(n.position(), Vec3::new(3.0, 0.0, -2.0));
        assert_eq!(n.model, Some(ModelId(7)));
        assert_eq!(n.params.shading, Shading::Unlit);
        assert_eq!(n.params.gloss, 40);
        assert_eq!(n.params.filter, crate::render::TexFilter::Point);
        assert_eq!(n.params.detail, None);
        assert_eq!(n.params.water, None, "flag bit6 off");
        assert_eq!(n.anim, Some((ClipId(9), 2.0)));
        let c = n.collider.unwrap();
        assert!(c.trigger);
        assert_eq!(
            c.shape,
            Shape::Box {
                half: Vec3::new(0.5, 1.0, 0.5)
            }
        );
        assert_eq!(c.offset, Vec3::new(0.0, 1.0, 0.0));
        assert_eq!((c.layer, c.one_way, n.character), (4, false, None));
        // v8: one semantic slot, no palette, no outline.
        assert_eq!(
            (n.semantic(), n.params.palette, n.outline()),
            ([0, 0xAB, 0, 0], None, None)
        );
        // LOD: model 7 up close, nothing beyond 30 units.
        assert_eq!(n.model_from(Vec3::new(3.0, 0.0, 0.0)), Some(ModelId(7)));
        assert_eq!(n.model_from(Vec3::new(3.0, 0.0, 40.0)), None);
        assert_eq!(n.cell, 3);
        assert_eq!(
            n.vis,
            Some(VisVolume::Portal {
                half: Vec3::new(0.5, 1.0, 0.25),
                a: 1,
                b: 4
            })
        );
        assert_eq!(n.params.shadow, ShadowCast::Auto { blob_radius: 0.4 });
        let sh = s.shadows().unwrap();
        assert_eq!(sh.tier_max, ShadowTier::Blob);
        assert_eq!((sh.map_size, sh.distance, sh.softness), (512, 30.0, 90));
        assert_eq!(sh.color, Color::rgba(10, 20, 30, 120));
        let r: std::vec::Vec<ShadowPlane> = s.receivers().collect();
        assert_eq!(
            r,
            [ShadowPlane::horizontal(Vec3::new(3.0, 1.0, -2.0), 1.0, 0.5)]
        );
    }

    #[test]
    fn lod_selection() {
        const IMP: Impostor = Impostor {
            sprite: crate::render::SpriteId(5),
            views: 8,
            cols: 4,
            tile: 16,
            size: 3.0,
            center: Vec3::new(0.0, 1.5, 0.0),
        };
        const LEVELS: &[LodLevel] = &[
            LodLevel::model(10.0, Some(ModelId(2))),
            LodLevel::impostor(20.0, IMP),
            LodLevel::model(40.0, None),
        ];
        let lods = Lods::List(LEVELS);
        assert_eq!(lods.select(Some(ModelId(1)), 5.0 * 5.0), Some(ModelId(1)));
        assert_eq!(lods.select(Some(ModelId(1)), 10.0 * 10.0), Some(ModelId(2)));
        assert_eq!(
            lods.pick(Some(ModelId(1)), 25.0 * 25.0),
            LodPick::Impostor(IMP)
        );
        assert_eq!(lods.select(Some(ModelId(1)), 25.0 * 25.0), None);
        assert_eq!(lods.pick(Some(ModelId(1)), 50.0 * 50.0), LodPick::None);
        assert_eq!(Lods::NONE.select(Some(ModelId(1)), 1e9), Some(ModelId(1)));
        let mut packed = std::vec::Vec::new();
        for l in LEVELS {
            packed.extend_from_slice(&l.distance.to_le_bytes());
            packed.extend_from_slice(&l.model.map_or(0, |m| m.0).to_le_bytes());
            let i = l.impostor;
            packed.extend_from_slice(&i.map_or(0, |i| i.sprite.0).to_le_bytes());
            let i = i.unwrap_or(Impostor {
                sprite: crate::render::SpriteId(0),
                views: 0,
                cols: 0,
                tile: 0,
                size: 0.0,
                center: Vec3::ZERO,
            });
            packed.extend_from_slice(&[i.views, i.cols]);
            packed.extend_from_slice(&i.tile.to_le_bytes());
            for v in [i.size, i.center.x, i.center.y, i.center.z] {
                packed.extend_from_slice(&v.to_le_bytes());
            }
        }
        let packed: &'static [u8] = std::boxed::Box::leak(packed.into_boxed_slice());
        let p = Lods::Packed(packed);
        assert_eq!(p.len(), 3);
        assert!(p.iter().eq(lods.iter()));
    }

    #[test]
    fn baked_scene_rejects_bad_blobs() {
        let good = tscn();
        assert_eq!(
            BakedScene::parse(&good[..40]).unwrap_err(),
            SceneError::Truncated
        );
        let mut bad = good.to_vec();
        bad[0] = b'X';
        let bad: &'static [u8] = std::boxed::Box::leak(bad.into_boxed_slice());
        assert_eq!(BakedScene::parse(bad).unwrap_err(), SceneError::BadMagic);
        // Name length past the string table.
        let mut bad = good.to_vec();
        let at = TSCN_HEADER + TSCN_ENV_BYTES + 4;
        bad[at..at + 4].copy_from_slice(&999u32.to_le_bytes());
        let bad: &'static [u8] = std::boxed::Box::leak(bad.into_boxed_slice());
        assert_eq!(BakedScene::parse(bad).unwrap_err(), SceneError::BadString);
    }
}
