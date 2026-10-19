//! # trino-scene
//!
//! Trino's authored-content formats — **scenes, prefabs and materials** —
//! as versioned, human-readable RON. Text on purpose: every file diffs and
//! merges in PRs, an AI agent can write it, and the editor hot-reloads it.
//!
//! | File | Logical path | Type |
//! |---|---|---|
//! | `scenes/<name>.scene.ron` | — | [`Scene`] |
//! | `prefabs/<name>.prefab.ron` | `prefabs/<name>` | [`Prefab`] |
//! | `assets/materials/<name>.material.ron` | `materials/<name>` | [`MaterialDef`] |
//!
//! Scenes are entity trees (`children` nest, like Blender's outliner).
//! An entity can draw a sprite, a model (+ material), and/or **instance a
//! prefab** — a linked copy: editing the prefab file updates every instance
//! live; per-instance [`Override`]s tweak children without unlinking;
//! [`Library::unpack`] makes an instance real (Blender's "make instances
//! real").
//!
//! [`resolve`] flattens a scene + library into a draw list (what the editor
//! viewport renders); [`codegen`] turns it into `no_std` Rust data so console
//! games can ship editor-made scenes, and [`bake`] into a `TSCN` blob that
//! games load at runtime (`trino_core::scene::BakedScene`).
//!
//! ## Versioning contract
//!
//! - `version` is the first field of every file.
//! - Loaders accept the current version and migrate older ones
//!   ([`Scene::from_ron`] migrates v1 → v2).
//! - Removing/renaming a field or changing its meaning bumps the version
//!   and adds a migration. Adding an optional field with a default does not.

pub mod bake;
pub mod budget;
pub mod codegen;
pub mod library;
pub mod material;
pub mod platform;
pub mod prefab;
pub mod resolve;

pub use library::{Library, ModelBounds};
pub use material::{DetailDef, FilterDef, MaterialDef, ShadingDef, WaterDef};
pub use platform::{PlatformVariants, Target, Variant};
pub use prefab::{Override, Prefab, PrefabInstance};
pub use resolve::{DrawItem, LodDraw, LodItem, Resolved, resolve, resolve_for};

use serde::{Deserialize, Serialize};
use trino_core::SpriteId;

/// v7: shadows (issue #57, ADR 0024): `environment.shadows` and an entity's
/// `shadow` (what its model casts, whether its top receives). v6: asset
/// groups (issue #56): `requires`, the groups the scene needs
/// loaded (`["boot"]` for every older file), and a cell's `groups`, what an
/// interior streams in as the cell comes into view. v5: the consoles'
/// hardware tiers (issue #54): `n64_expanded` and
/// `n3ds_new` variants and LOD starts, which follow their console's when
/// unset. v4: the Web (issue #47) as a fourth platform: a `web` variant and a
/// `web` LOD start, which follow the PC's when unset. v3 added per-platform
/// variants (`platforms`, see [`platform`]); v2 added 3D transforms
/// (degrees), nested children, models, prefab instances and the
/// environment. v1 (2D-only, flat) to v6 are migrated on load.
///
/// v3 to v6 only add fields, but they change what a scene *means* per
/// target: an engine that ignored `platforms` would ship the PC model to a
/// console without a word, and one that ignored `web` would drop the Web's
/// own choices when it saves (as one that ignored the tiers would drop
/// theirs, or one that ignored `requires` a level's groups). The bump makes
/// such an engine refuse the file instead.
pub const SCENE_VERSION: u32 = 7;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Scene {
    pub version: u32,
    pub name: String,
    /// The asset groups the scene needs loaded (issue #56, ADR 0022):
    /// `boot` (always resident) plus a level's own. `cargo xtask assets
    /// check` holds every asset the scene draws to one of them; the memory
    /// report and strict mode count them together.
    #[serde(default = "boot_only")]
    pub requires: Vec<String>,
    #[serde(default)]
    pub environment: Environment,
    #[serde(default)]
    pub entities: Vec<Entity>,
}

/// Camera, light, fog and ambient occlusion of a scene — maps 1:1 onto
/// `trino_core::{Camera3, Light, Fog, AmbientOcclusion}`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Environment {
    #[serde(default = "default_clear")]
    pub clear: [u8; 3],
    #[serde(default)]
    pub camera: CameraDef,
    #[serde(default)]
    pub light: LightDef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fog: Option<FogDef>,
    /// Screen-space ambient occlusion (PC native; consoles use the AO
    /// baked into model vertex colors — `trino_core::ao`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ao: Option<AmbientOcclusionDef>,
    /// Shadows (`trino_core::shadow`, ADR 0024): each target draws its own
    /// tier (a blob on the N64, the caster's shape on the 3DS, a shadow
    /// map on the PC and the Web), capped by `tier_max`. Entities cast and
    /// receive through their `shadow` component.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadows: Option<ShadowsDef>,
}

fn default_clear() -> [u8; 3] {
    [24, 26, 40]
}

impl Default for Environment {
    fn default() -> Self {
        Environment {
            clear: default_clear(),
            camera: CameraDef::default(),
            light: LightDef::default(),
            fog: None,
            ao: None,
            shadows: None,
        }
    }
}

/// The game camera the scene starts with (the editor orbits freely).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct CameraDef {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    /// Vertical field of view, degrees.
    pub fov: f32,
}

impl Default for CameraDef {
    fn default() -> Self {
        CameraDef {
            eye: [0.0, 4.0, -8.0],
            target: [0.0, 0.5, 0.0],
            fov: 60.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct LightDef {
    /// Direction the light travels (world space, Y-up).
    pub dir: [f32; 3],
    pub color: [u8; 3],
    pub ambient: f32,
    pub ambient_color: [u8; 3],
}

impl Default for LightDef {
    fn default() -> Self {
        let d = trino_core::DEFAULT_LIGHT;
        LightDef {
            dir: [d.dir.x, d.dir.y, d.dir.z],
            color: [255, 255, 255],
            ambient: d.ambient,
            ambient_color: [255, 255, 255],
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct FogDef {
    pub color: [u8; 3],
    pub near: f32,
    pub far: f32,
}

/// Ambient occlusion settings of a scene ([`trino_core::AmbientOcclusion`]).
/// Every field has a default, so `ao: ()` is a valid start; `enabled:
/// false` keeps the tuning while turning the pass off.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct AmbientOcclusionDef {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// How far around a point occluders count, meters.
    #[serde(default = "default_ao_radius")]
    pub radius: f32,
    /// Occlusion strength, 0.0 to 2.0.
    #[serde(default = "default_ao_intensity")]
    pub intensity: f32,
    /// Depth bias, meters: keeps surfaces from shadowing themselves.
    #[serde(default = "default_ao_bias")]
    pub bias: f32,
    /// Samples per pixel: `Low` (8), `Medium` (16), `High` (32).
    #[serde(default)]
    pub quality: AoQualityDef,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AoQualityDef {
    Low,
    #[default]
    Medium,
    High,
}

fn default_ao_radius() -> f32 {
    trino_core::AmbientOcclusion::DEFAULT.radius
}
fn default_ao_intensity() -> f32 {
    trino_core::AmbientOcclusion::DEFAULT.intensity
}
fn default_ao_bias() -> f32 {
    trino_core::AmbientOcclusion::DEFAULT.bias
}

impl Default for AmbientOcclusionDef {
    fn default() -> Self {
        AmbientOcclusionDef {
            enabled: true,
            radius: default_ao_radius(),
            intensity: default_ao_intensity(),
            bias: default_ao_bias(),
            quality: AoQualityDef::Medium,
        }
    }
}

impl AmbientOcclusionDef {
    /// The engine settings, enabled or not.
    pub fn core(&self) -> trino_core::AmbientOcclusion {
        trino_core::AmbientOcclusion {
            radius: self.radius,
            intensity: self.intensity,
            bias: self.bias,
            quality: match self.quality {
                AoQualityDef::Low => trino_core::AoQuality::Low,
                AoQualityDef::Medium => trino_core::AoQuality::Medium,
                AoQualityDef::High => trino_core::AoQuality::High,
            },
        }
        .clamped()
    }
}

/// A scene's shadows ([`trino_core::shadow::ShadowSettings`]). Every field
/// has a default, so `shadows: ()` is a valid start; `enabled: false`
/// keeps the tuning while turning them off.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct ShadowsDef {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// The best tier wanted (`Blob` gives every target the N64's look).
    #[serde(default)]
    pub tier_max: ShadowTierDef,
    /// Shadow map texels per side (a power of two, 256 to 4096); 0 takes
    /// the quality preset's.
    #[serde(default)]
    pub map_size: u16,
    /// How far from the camera shadows reach, meters.
    #[serde(default = "default_shadow_distance")]
    pub distance: f32,
    /// 0 (hard) to 255: the blob's falloff and the map's filter radius.
    #[serde(default = "default_shadow_softness")]
    pub softness: u8,
    /// What a shadowed surface blends toward (RGBA: alpha is the
    /// strength).
    #[serde(default = "default_shadow_color")]
    pub color: [u8; 4],
}

/// [`trino_core::shadow::ShadowTier`] in files.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ShadowTierDef {
    None,
    Blob,
    Planar,
    #[default]
    Map,
}

impl ShadowTierDef {
    pub const ALL: [ShadowTierDef; 4] = [
        ShadowTierDef::None,
        ShadowTierDef::Blob,
        ShadowTierDef::Planar,
        ShadowTierDef::Map,
    ];

    pub fn core(self) -> trino_core::shadow::ShadowTier {
        use trino_core::shadow::ShadowTier as T;
        match self {
            ShadowTierDef::None => T::None,
            ShadowTierDef::Blob => T::Blob,
            ShadowTierDef::Planar => T::Planar,
            ShadowTierDef::Map => T::Map,
        }
    }
}

fn default_shadow_distance() -> f32 {
    trino_core::shadow::ShadowSettings::DEFAULT.distance
}
fn default_shadow_softness() -> u8 {
    trino_core::shadow::ShadowSettings::DEFAULT.softness
}
fn default_shadow_color() -> [u8; 4] {
    let c = trino_core::shadow::ShadowSettings::DEFAULT.color;
    [c.r, c.g, c.b, c.a]
}

impl Default for ShadowsDef {
    fn default() -> Self {
        ShadowsDef {
            enabled: true,
            tier_max: ShadowTierDef::Map,
            map_size: 0,
            distance: default_shadow_distance(),
            softness: default_shadow_softness(),
            color: default_shadow_color(),
        }
    }
}

impl ShadowsDef {
    /// The engine settings, enabled or not (clamped to their ranges).
    pub fn core(&self) -> trino_core::shadow::ShadowSettings {
        let [r, g, b, a] = self.color;
        trino_core::shadow::ShadowSettings {
            tier_max: self.tier_max.core(),
            map_size: self.map_size,
            distance: self.distance,
            softness: self.softness,
            color: trino_core::Color::rgba(r, g, b, a),
        }
        .clamped()
    }
}

/// What an entity's model casts and whether shadows land on it
/// (`trino_core::shadow`, ADR 0024). `shadow: ()` casts the tier's pick
/// with a half-meter blob.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct ShadowComponent {
    #[serde(default)]
    pub cast: ShadowCastDef,
    /// The blob's radius, meters (`Blob`, and `Auto` on the blob tier: 0
    /// draws none there, for a character's limbs whose body casts it).
    #[serde(default = "default_blob_radius")]
    pub radius: f32,
    /// Blob and planar shadows land on the top of this entity's box (its
    /// collider, else its model's bounds): a floor, a table, a platform.
    /// The map tier shades every surface regardless.
    #[serde(default, skip_serializing_if = "is_false")]
    pub receive: bool,
}

/// [`trino_core::ShadowCast`] in files.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ShadowCastDef {
    /// Nothing (a receiver that casts nothing).
    None,
    /// A blob on every tier.
    Blob,
    /// The model's shape (projected, or in the map); on the blob tier the
    /// first few of a frame (the N64's two).
    Mesh,
    /// The shape where the tier draws shapes, a blob on the blob tier.
    #[default]
    Auto,
}

impl ShadowCastDef {
    pub const ALL: [ShadowCastDef; 4] = [
        ShadowCastDef::None,
        ShadowCastDef::Blob,
        ShadowCastDef::Mesh,
        ShadowCastDef::Auto,
    ];
}

fn default_blob_radius() -> f32 {
    0.5
}

impl Default for ShadowComponent {
    fn default() -> Self {
        ShadowComponent {
            cast: ShadowCastDef::Auto,
            radius: default_blob_radius(),
            receive: false,
        }
    }
}

impl ShadowComponent {
    /// What the draw casts ([`trino_core::ModelParams::shadow`]).
    pub fn core(&self) -> trino_core::ShadowCast {
        let radius = if self.radius.is_finite() {
            self.radius.max(0.0)
        } else {
            0.0
        };
        match self.cast {
            ShadowCastDef::None => trino_core::ShadowCast::None,
            ShadowCastDef::Blob => trino_core::ShadowCast::Blob { radius },
            ShadowCastDef::Mesh => trino_core::ShadowCast::Mesh,
            ShadowCastDef::Auto => trino_core::ShadowCast::auto(radius),
        }
    }
}

impl Environment {
    /// What `Renderer::set_ambient_occlusion` takes: `None` without an
    /// `ao` block or with `enabled: false`.
    pub fn core_ao(&self) -> Option<trino_core::AmbientOcclusion> {
        self.ao.filter(|a| a.enabled).map(|a| a.core())
    }

    /// What `Renderer::set_shadows` takes: `None` without a `shadows`
    /// block or with `enabled: false`.
    pub fn core_shadows(&self) -> Option<trino_core::shadow::ShadowSettings> {
        self.shadows.filter(|s| s.enabled).map(|s| s.core())
    }

    pub fn core_camera(&self) -> trino_core::Camera3 {
        let c = &self.camera;
        trino_core::Camera3 {
            eye: v3(c.eye),
            target: v3(c.target),
            fov_y: c.fov.to_radians(),
        }
    }

    pub fn core_light(&self) -> trino_core::Light {
        let l = &self.light;
        let rgb = |c: [u8; 3]| trino_core::Color::rgb(c[0], c[1], c[2]);
        trino_core::Light {
            dir: v3(l.dir),
            ambient: l.ambient,
            color: rgb(l.color),
            ambient_color: rgb(l.ambient_color),
            fog: match self.fog {
                Some(f) => trino_core::Fog {
                    color: rgb(f.color),
                    near: f.near,
                    far: f.far,
                },
                None => trino_core::Fog::NONE,
            },
        }
    }
}

pub(crate) fn v3(a: [f32; 3]) -> trino_core::Vec3 {
    trino_core::Vec3::new(a[0], a[1], a[2])
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entity {
    pub name: String,
    #[serde(default)]
    pub transform: Transform,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub visible: bool,
    /// Free-form labels game code can query (`spawn`, `enemy`, ...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprite: Option<SpriteComponent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelComponent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefab: Option<PrefabInstance>,
    /// Collision shape (solid or trigger) — exported to
    /// `SceneNode::collider` for `trino_core::physics`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collider: Option<ColliderComponent>,
    /// Positional looping sound (waterfalls, machines, torches) — played by
    /// `trino_core::audio::Ambience` near the listener.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound: Option<SoundComponent>,
    /// Interior visibility: this entity is a **cell** (a room; everything
    /// under it belongs to it) or a **portal** (a doorway between two
    /// cells) — see `trino_core::vis`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vis: Option<VisComponent>,
    /// What its model casts, whether shadows land on it (ADR 0024; drawn
    /// while `environment.shadows` is on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<ShadowComponent>,
    /// What changes on PC, N64 and 3DS (model, material, scale,
    /// visibility, or not there at all) — see [`platform`].
    #[serde(default, skip_serializing_if = "PlatformVariants::is_empty")]
    pub platforms: PlatformVariants,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Entity>,
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

impl Entity {
    pub fn new(name: impl Into<String>) -> Self {
        Entity {
            name: name.into(),
            transform: Transform::default(),
            visible: true,
            tags: Vec::new(),
            sprite: None,
            model: None,
            prefab: None,
            collider: None,
            sound: None,
            vis: None,
            shadow: None,
            platforms: PlatformVariants::default(),
            children: Vec::new(),
        }
    }

    /// Builder: a model entity.
    pub fn with_model(mut self, model: ModelComponent) -> Self {
        self.model = Some(model);
        self
    }

    pub fn at(mut self, pos: [f32; 3]) -> Self {
        self.transform.pos = pos;
        self
    }
}

/// Local transform relative to the parent. 3D world is Y-up (meters);
/// sprites read `pos.x/pos.y` as screen pixels (Y-down) and `rot.z`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub pos: [f32; 3],
    /// Euler XYZ, **degrees** (human- and AI-friendly).
    pub rot: [f32; 3],
    pub scale: [f32; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Transform {
            pos: [0.0; 3],
            rot: [0.0; 3],
            scale: [1.0; 3],
        }
    }
}

impl Transform {
    pub fn matrix(&self) -> trino_core::Mat34 {
        trino_core::Mat34::from_rotation_scale_translation(
            trino_core::Vec3::new(
                self.rot[0].to_radians(),
                self.rot[1].to_radians(),
                self.rot[2].to_radians(),
            ),
            v3(self.scale),
            v3(self.pos),
        )
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SpriteComponent {
    /// Logical asset path, e.g. `"sprites/player"`.
    pub path: String,
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub flip_y: bool,
    #[serde(default = "white")]
    pub tint: [u8; 4],
}

pub(crate) fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

pub(crate) fn white() -> [u8; 4] {
    [255, 255, 255, 255]
}

impl SpriteComponent {
    pub fn new(path: impl Into<String>) -> Self {
        SpriteComponent {
            path: path.into(),
            flip_x: false,
            flip_y: false,
            tint: white(),
        }
    }

    pub fn sprite_id(&self) -> SpriteId {
        SpriteId(trino_core::asset_id(&self.path))
    }
}

/// Draws a baked model (`models/<name>` — glTF or a placeholder primitive).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ModelComponent {
    /// Logical asset path, e.g. `"models/crate"`.
    pub path: String,
    /// Material file, e.g. `"materials/wood"`. None = default surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
    /// Per-entity tint multiplied over the material's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tint: Option<[u8; 4]>,
    /// Animation clip to play (models with a baked skeleton).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anim: Option<AnimComponent>,
    /// Distance levels: from `distance` on, draw `path` (a cheaper model —
    /// fake objects make good far levels), an `impostor` (a pre-rendered
    /// billboard, 2 triangles) or nothing. Sorted by distance on resolve;
    /// each level can move or drop out per platform.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lod: Vec<LodDef>,
}

/// One LOD level of a [`ModelComponent`]: a chain full mesh → light mesh →
/// impostor → nothing, with distances per platform.
///
/// ```ron
/// lod: [
///     (distance: 12.0, path: Some("models/tree-light")),
///     (distance: 30.0, impostor: Some("impostors/tree"), platforms: (n64: Some(Never))),
///     (distance: 60.0, platforms: (n64: Some(At(35.0)))),
/// ]
/// ```
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct LodDef {
    /// Camera distance (world units) where this level starts.
    pub distance: f32,
    /// Model logical path (`models/tree-far`); `None` (and no impostor) =
    /// culled.
    #[serde(default)]
    pub path: Option<String>,
    /// An impostor (`impostors/<name>`, baked by `cargo xtask impostor
    /// bake`) drawn instead of a mesh. Wins over `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impostor: Option<String>,
    /// Where this level starts on each platform, when not at `distance`
    /// (`At(d)`), or that it is skipped there (`Never`).
    #[serde(default, skip_serializing_if = "LodPlatforms::is_empty")]
    pub platforms: LodPlatforms,
}

impl LodDef {
    /// The level's start distance on `target` (`None` = not used there).
    pub fn distance_on(&self, target: Target) -> Option<f32> {
        match self.platforms.get(target) {
            Some(LodAt::At(d)) => Some(*d),
            Some(LodAt::Never) => None,
            None => Some(self.distance),
        }
    }
}

/// A LOD level's distance on one platform.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum LodAt {
    /// Starts at this distance there.
    At(f32),
    /// Not used there (the previous level keeps drawing).
    Never,
}

/// Per-platform [`LodAt`]s of a level (`None` = the level's `distance`;
/// the Web takes the PC's when it has none, a hardware tier its console's,
/// like [`PlatformVariants::get`]).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct LodPlatforms {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pc: Option<LodAt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n64: Option<LodAt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n3ds: Option<LodAt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web: Option<LodAt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n64_expanded: Option<LodAt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n3ds_new: Option<LodAt>,
}

impl LodPlatforms {
    pub fn is_empty(&self) -> bool {
        Target::EVERY.iter().all(|t| self.own(*t).is_none())
    }

    /// The start `target` uses: its own, else its fallback's.
    pub fn get(&self, target: Target) -> Option<&LodAt> {
        self.own(target)
            .or_else(|| target.fallback().and_then(|f| self.own(f)))
    }

    /// The start written for `target` itself (no fallback).
    pub fn own(&self, target: Target) -> Option<&LodAt> {
        match target {
            Target::Pc => self.pc.as_ref(),
            Target::N64 => self.n64.as_ref(),
            Target::N3ds => self.n3ds.as_ref(),
            Target::Web => self.web.as_ref(),
            Target::N64Expanded => self.n64_expanded.as_ref(),
            Target::N3dsNew => self.n3ds_new.as_ref(),
        }
    }

    pub fn get_mut(&mut self, target: Target) -> &mut Option<LodAt> {
        match target {
            Target::Pc => &mut self.pc,
            Target::N64 => &mut self.n64,
            Target::N3ds => &mut self.n3ds,
            Target::Web => &mut self.web,
            Target::N64Expanded => &mut self.n64_expanded,
            Target::N3dsNew => &mut self.n3ds_new,
        }
    }
}

impl ModelComponent {
    pub fn new(path: impl Into<String>) -> Self {
        ModelComponent {
            path: path.into(),
            material: None,
            tint: None,
            anim: None,
            lod: Vec::new(),
        }
    }

    pub fn model_id(&self) -> trino_core::ModelId {
        trino_core::ModelId(trino_core::asset_id(&self.path))
    }
}

/// Collision shape of an entity, local to its transform (the transform
/// scales/rotates it; collision uses the resulting world box — see
/// `trino_core::physics`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ColliderComponent {
    #[serde(default)]
    pub shape: ShapeDef,
    /// Center offset from the entity origin (local units).
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub offset: [f32; 3],
    /// Overlap-only (coins, doors, kill zones) instead of solid.
    #[serde(default, skip_serializing_if = "is_false")]
    pub trigger: bool,
}

impl Default for ColliderComponent {
    fn default() -> Self {
        ColliderComponent {
            shape: ShapeDef::Auto,
            offset: [0.0; 3],
            trigger: false,
        }
    }
}

fn is_zero3(v: &[f32; 3]) -> bool {
    *v == [0.0; 3]
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Interior visibility volume. Sizes are **full** box sizes in local
/// units, centered on the entity origin (rotation/scale give the world box
/// that encloses the result).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum VisComponent {
    /// A room. Its descendants are drawn only while it is visible.
    /// `groups` (v6): the asset groups its contents come from, streamed in
    /// while it is in view (`trino_core::vis::CellGroups`).
    Cell {
        size: [f32; 3],
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        groups: Vec<String>,
    },
    /// A doorway joining the cells named `a` and `b` (entity names or
    /// name paths, like `house/kitchen`). Make it cover the opening.
    Portal {
        size: [f32; 3],
        a: String,
        b: String,
    },
}

/// A positional sound source (a looping sound effect; on the N64 declare
/// it with `loop = true` in the manifest).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SoundComponent {
    /// Logical path, e.g. `"sounds/water"`.
    pub path: String,
    /// Heard within this distance (world units), fading to silence.
    #[serde(default = "default_sound_radius")]
    pub radius: f32,
    #[serde(default = "one")]
    pub volume: f32,
}

fn default_sound_radius() -> f32 {
    10.0
}

impl SoundComponent {
    pub fn new(path: impl Into<String>) -> Self {
        SoundComponent {
            path: path.into(),
            radius: default_sound_radius(),
            volume: 1.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub enum ShapeDef {
    /// The bounding box of the entity's model (fake objects collide as
    /// their shape's box) — resolved from [`Library::model_bounds`].
    #[default]
    Auto,
    /// Full size (not half extents), local units.
    Box {
        size: [f32; 3],
    },
    Sphere {
        radius: f32,
    },
    /// Y-aligned; `height` includes the caps.
    Capsule {
        radius: f32,
        height: f32,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AnimComponent {
    pub clip: String,
    #[serde(default = "one")]
    pub speed: f32,
}

fn one() -> f32 {
    1.0
}

fn boot_only() -> Vec<String> {
    vec!["boot".to_string()]
}

impl Scene {
    pub fn new(name: impl Into<String>) -> Self {
        Scene {
            version: SCENE_VERSION,
            name: name.into(),
            requires: boot_only(),
            environment: Environment::default(),
            entities: Vec::new(),
        }
    }

    /// Parse + migrate. The single entry point for reading scenes.
    pub fn from_ron(text: &str) -> Result<Scene, String> {
        // Peek the version before full parsing so old schemas route to
        // their migration.
        let versioned: VersionOnly =
            ron::from_str(text).map_err(|e| format!("scene parse error: {e}"))?;
        match versioned.version {
            SCENE_VERSION => ron::from_str(text).map_err(|e| format!("scene parse error: {e}")),
            // v6 is v7 without shadows, v5 is v6 without groups (`requires`
            // defaults to boot), v4 is v5 without the hardware tiers, v3 is
            // v4 without the Web, v2 is v3 without variants: same fields,
            // defaults fill the rest.
            2..=6 => {
                let mut scene: Scene = ron::from_str(text)
                    .map_err(|e| format!("scene parse error (v{} file): {e}", versioned.version))?;
                scene.version = SCENE_VERSION;
                Ok(scene)
            }
            1 => {
                let v1: v1::Scene =
                    ron::from_str(text).map_err(|e| format!("scene (v1) parse error: {e}"))?;
                Ok(v1.migrate())
            }
            older if older < SCENE_VERSION => Err(format!(
                "scene version {older} has no migration path (current {SCENE_VERSION}) — \
                 this is a bug: migrations must exist for every released version"
            )),
            newer => Err(format!(
                "scene version {newer} is newer than this engine understands \
                 ({SCENE_VERSION}) — update Trino"
            )),
        }
    }

    pub fn to_ron(&self) -> String {
        to_ron_pretty(self)
    }

    /// Every asset the scene names, by logical path (`models/boar`,
    /// `sprites/hud`, `sounds/water`), each once, sorted: its entities'
    /// sprites, models (with their LOD levels and platform variants) and
    /// sounds. Prefab instances' assets live in the prefab files.
    pub fn asset_refs(&self) -> Vec<String> {
        fn walk(e: &Entity, out: &mut Vec<String>) {
            if let Some(s) = &e.sprite {
                out.push(s.path.clone());
            }
            if let Some(m) = &e.model {
                out.push(m.path.clone());
                out.extend(m.lod.iter().filter_map(|l| l.path.clone()));
            }
            if let Some(s) = &e.sound {
                out.push(s.path.clone());
            }
            out.extend(e.platforms.models().map(String::from));
            for c in &e.children {
                walk(c, out);
            }
        }
        let mut out = Vec::new();
        for e in &self.entities {
            walk(e, &mut out);
        }
        out.sort();
        out.dedup();
        out
    }

    /// The groups of the scene's cells, by name path, in cell order.
    pub fn cell_groups(&self) -> Vec<(String, Vec<String>)> {
        fn walk(e: &Entity, path: &str, out: &mut Vec<(String, Vec<String>)>) {
            let name = if path.is_empty() {
                e.name.clone()
            } else {
                format!("{path}/{}", e.name)
            };
            if let Some(VisComponent::Cell { groups, .. }) = &e.vis
                && !groups.is_empty()
            {
                out.push((name.clone(), groups.clone()));
            }
            for c in &e.children {
                walk(c, &name, out);
            }
        }
        let mut out = Vec::new();
        for e in &self.entities {
            walk(e, "", &mut out);
        }
        out
    }

    pub fn load(path: &std::path::Path) -> Result<Scene, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::from_ron(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        write_text(path, &self.to_ron())
    }

    /// Entity at an index path (`[2, 0]` = first child of the third root).
    pub fn get(&self, path: &[usize]) -> Option<&Entity> {
        let (first, rest) = path.split_first()?;
        let mut e = self.entities.get(*first)?;
        for &i in rest {
            e = e.children.get(i)?;
        }
        Some(e)
    }

    /// The name the resolved draws give the entity at `path`: its
    /// ancestors' names and its own joined by `/` (`lamp-1/lamp/bulb`), as
    /// in [`crate::DrawItem::name`] and the budget's entity lists.
    pub fn draw_name(&self, path: &[usize]) -> Option<String> {
        let (first, rest) = path.split_first()?;
        let mut e = self.entities.get(*first)?;
        let mut name = e.name.clone();
        for &i in rest {
            e = e.children.get(i)?;
            name.push('/');
            name.push_str(&e.name);
        }
        Some(name)
    }

    /// The index path of the entity whose draws are named `name`
    /// ([`Scene::draw_name`]).
    pub fn find_draw_name(&self, name: &str) -> Option<Vec<usize>> {
        let mut found = None;
        self.walk(|p, _| {
            if found.is_none() && self.draw_name(p).as_deref() == Some(name) {
                found = Some(p.to_vec());
            }
        });
        found
    }

    pub fn get_mut(&mut self, path: &[usize]) -> Option<&mut Entity> {
        let (first, rest) = path.split_first()?;
        let mut e = self.entities.get_mut(*first)?;
        for &i in rest {
            e = e.children.get_mut(i)?;
        }
        Some(e)
    }

    /// Sibling list that contains `path` (roots or a parent's children).
    pub fn siblings_mut(&mut self, path: &[usize]) -> Option<&mut Vec<Entity>> {
        match path.split_last()? {
            (_, []) => Some(&mut self.entities),
            (_, parent) => self.get_mut(parent).map(|p| &mut p.children),
        }
    }

    /// Remove and return the entity at `path`.
    pub fn remove(&mut self, path: &[usize]) -> Option<Entity> {
        let &last = path.last()?;
        let list = self.siblings_mut(path)?;
        (last < list.len()).then(|| list.remove(last))
    }

    /// Insert `entity` as the last child of `parent` (`None` = root).
    /// Returns the new entity's path.
    pub fn insert(&mut self, parent: Option<&[usize]>, entity: Entity) -> Option<Vec<usize>> {
        match parent {
            None => {
                self.entities.push(entity);
                Some(vec![self.entities.len() - 1])
            }
            Some(p) => {
                let parent = self.get_mut(p)?;
                parent.children.push(entity);
                let mut path = p.to_vec();
                path.push(parent.children.len() - 1);
                Some(path)
            }
        }
    }

    /// Move the entity at `from` under `new_parent` (`None` = root),
    /// refusing to move an entity into its own subtree. Returns the new path.
    pub fn reparent(&mut self, from: &[usize], new_parent: Option<&[usize]>) -> Option<Vec<usize>> {
        if let Some(p) = new_parent
            && p.starts_with(from)
        {
            return None;
        }
        // Removing `from` shifts later siblings; fix up the target path.
        let mut target = new_parent.map(|p| p.to_vec());
        if let Some(t) = target.as_mut() {
            let depth = from.len() - 1;
            if t.len() > depth && t[..depth] == from[..depth] && t[depth] > from[depth] {
                t[depth] -= 1;
            }
        }
        let entity = self.remove(from)?;
        self.insert(target.as_deref(), entity)
    }

    /// Depth-first visit of every entity with its index path.
    pub fn walk(&self, mut f: impl FnMut(&[usize], &Entity)) {
        fn rec(list: &[Entity], path: &mut Vec<usize>, f: &mut dyn FnMut(&[usize], &Entity)) {
            for (i, e) in list.iter().enumerate() {
                path.push(i);
                f(path, e);
                rec(&e.children, path, f);
                path.pop();
            }
        }
        rec(&self.entities, &mut Vec::new(), &mut f);
    }

    /// A name not used by any entity: `base`, `base-2`, `base-3`, ...
    pub fn unique_name(&self, base: &str) -> String {
        let mut taken = std::collections::HashSet::new();
        self.walk(|_, e| {
            taken.insert(e.name.clone());
        });
        if !taken.contains(base) {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|n| !taken.contains(n))
            .unwrap()
    }
}

/// Pretty RON with stable formatting (diffable).
pub(crate) fn to_ron_pretty<T: Serialize>(value: &T) -> String {
    let config = ron::ser::PrettyConfig::default().struct_names(false);
    ron::ser::to_string_pretty(value, config).expect("serialization cannot fail")
}

pub(crate) fn write_text(path: &std::path::Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[derive(Deserialize)]
struct VersionOnly {
    version: u32,
}

/// The v1 schema (2D, flat), kept only to migrate old files.
mod v1 {
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub struct Scene {
        pub name: String,
        #[serde(default)]
        pub entities: Vec<Entity>,
    }

    #[derive(Deserialize)]
    pub struct Entity {
        pub name: String,
        #[serde(default)]
        pub transform: Transform2D,
        #[serde(default)]
        pub sprite: Option<super::SpriteComponent>,
    }

    #[derive(Deserialize)]
    pub struct Transform2D {
        pub pos: [f32; 2],
        pub scale: [f32; 2],
        /// Radians.
        pub rotation: f32,
    }

    impl Default for Transform2D {
        fn default() -> Self {
            Transform2D {
                pos: [0.0; 2],
                scale: [1.0; 2],
                rotation: 0.0,
            }
        }
    }

    impl Scene {
        pub fn migrate(self) -> super::Scene {
            let mut scene = super::Scene::new(self.name);
            scene.entities = self
                .entities
                .into_iter()
                .map(|e| {
                    let mut out = super::Entity::new(e.name);
                    out.transform = super::Transform {
                        pos: [e.transform.pos[0], e.transform.pos[1], 0.0],
                        rot: [0.0, 0.0, e.transform.rotation.to_degrees()],
                        scale: [e.transform.scale[0], e.transform.scale[1], 1.0],
                    };
                    out.sprite = e.sprite;
                    out
                })
                .collect();
            scene
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambient_occlusion_is_optional_defaulted_and_round_trips() {
        // Absent: the field is not written, and nothing changes.
        let plain = Scene::new("plain");
        assert!(!plain.to_ron().contains("ao:"));
        assert_eq!(plain.environment.core_ao(), None);
        // `Some(())`: every field takes its default.
        let text = "(version: 3, name: \"a\", environment: (ao: Some(())))";
        let scene = Scene::from_ron(text).unwrap();
        assert_eq!(scene.environment.ao, Some(AmbientOcclusionDef::default()));
        assert_eq!(
            scene.environment.core_ao(),
            Some(trino_core::AmbientOcclusion::DEFAULT)
        );
        // Tuned, then disabled: the tuning stays in the file, the pass goes.
        let mut tuned = Scene::new("tuned");
        tuned.environment.ao = Some(AmbientOcclusionDef {
            enabled: false,
            radius: 1.25,
            intensity: 1.5,
            bias: 0.05,
            quality: AoQualityDef::High,
        });
        let back = Scene::from_ron(&tuned.to_ron()).unwrap();
        assert_eq!(back, tuned);
        assert_eq!(back.environment.core_ao(), None);
        tuned.environment.ao.as_mut().unwrap().enabled = true;
        let ao = tuned.environment.core_ao().unwrap();
        assert_eq!(ao.radius, 1.25);
        assert_eq!(ao.quality, trino_core::AoQuality::High);
        assert_eq!(ao.quality.samples(), 32);
    }

    #[test]
    fn shadows_are_optional_defaulted_and_round_trip() {
        // Absent: neither field is written; nothing casts.
        let plain = Scene::new("plain");
        let text = plain.to_ron();
        assert!(!text.contains("shadows:") && !text.contains("shadow:"));
        assert_eq!(plain.environment.core_shadows(), None);
        // `Some(())` everywhere: the defaults (an Auto caster, a
        // half-meter blob, receiving nothing).
        let text = "(version: 7, name: \"a\", environment: (shadows: Some(())), \
                    entities: [(name: \"e\", shadow: Some(()))])";
        let scene = Scene::from_ron(text).unwrap();
        assert_eq!(scene.environment.shadows, Some(ShadowsDef::default()));
        assert_eq!(
            scene.environment.core_shadows(),
            Some(trino_core::ShadowSettings::DEFAULT)
        );
        let c = scene.entities[0].shadow.unwrap();
        assert_eq!(c, ShadowComponent::default());
        assert_eq!(c.core(), trino_core::ShadowCast::auto(0.5));
        // Tuned, then off: the tuning stays in the file, the shadows go.
        let mut tuned = Scene::new("tuned");
        tuned.environment.shadows = Some(ShadowsDef {
            enabled: false,
            tier_max: ShadowTierDef::Planar,
            map_size: 1024,
            distance: 25.0,
            softness: 90,
            color: [10, 20, 30, 120],
        });
        let mut floor = Entity::new("floor");
        floor.shadow = Some(ShadowComponent {
            cast: ShadowCastDef::None,
            receive: true,
            ..Default::default()
        });
        let mut ball = Entity::new("ball");
        ball.shadow = Some(ShadowComponent {
            cast: ShadowCastDef::Blob,
            radius: 0.3,
            receive: false,
        });
        tuned.entities = vec![floor, ball];
        let text = tuned.to_ron();
        assert!(text.contains("receive: true"));
        assert_eq!(text.matches("receive:").count(), 1, "false is not written");
        let back = Scene::from_ron(&text).unwrap();
        assert_eq!(back, tuned);
        assert_eq!(back.environment.core_shadows(), None);
        tuned.environment.shadows.as_mut().unwrap().enabled = true;
        let s = tuned.environment.core_shadows().unwrap();
        assert_eq!(s.tier_max, trino_core::ShadowTier::Planar);
        assert_eq!((s.map_size, s.distance, s.softness), (1024, 25.0, 90));
        assert_eq!(s.color, trino_core::Color::rgba(10, 20, 30, 120));
        let casts: Vec<_> = tuned
            .entities
            .iter()
            .map(|e| e.shadow.unwrap().core())
            .collect();
        assert_eq!(
            casts,
            [
                trino_core::ShadowCast::None,
                trino_core::ShadowCast::Blob { radius: 0.3 }
            ]
        );
        // Every kind maps to its engine cast.
        let cast = |cast| ShadowComponent {
            cast,
            radius: 0.0,
            receive: false,
        };
        assert_eq!(
            cast(ShadowCastDef::Mesh).core(),
            trino_core::ShadowCast::Mesh
        );
        assert_eq!(
            cast(ShadowCastDef::Auto).core(),
            trino_core::ShadowCast::Auto { blob_radius: 0.0 },
            "0: no blob on the blob tier"
        );
        for t in ShadowTierDef::ALL {
            assert_eq!(t.core().key(), format!("{t:?}").to_lowercase());
        }
    }

    #[test]
    fn shadow_settings_out_of_range_are_clamped() {
        let def = ShadowsDef {
            map_size: 300,
            distance: 0.0,
            ..ShadowsDef::default()
        };
        let s = def.core();
        assert_eq!(s.map_size, 512, "a power of two");
        assert_eq!(s.distance, 1.0);
        let big = ShadowsDef {
            map_size: 60000,
            distance: f32::NAN,
            ..ShadowsDef::default()
        };
        assert_eq!(big.core().map_size, 4096);
        assert_eq!(
            big.core().distance,
            trino_core::ShadowSettings::DEFAULT.distance
        );
    }

    #[test]
    fn v6_scenes_load_as_v7_without_shadows() {
        let text = r#"(version: 6, name: "old", environment: (ao: Some(())),
            entities: [(name: "rock", model: Some((path: "models/rock")))])"#;
        let scene = Scene::from_ron(text).unwrap();
        assert_eq!(scene.version, SCENE_VERSION);
        assert_eq!(SCENE_VERSION, 7);
        assert_eq!(scene.environment.shadows, None);
        assert!(scene.entities[0].shadow.is_none());
        assert!(scene.environment.ao.is_some(), "the rest kept");
        // Saved as v7, it reloads the same.
        let saved = scene.to_ron();
        assert!(saved.contains("version: 7"));
        assert_eq!(Scene::from_ron(&saved).unwrap(), scene);
    }

    #[test]
    fn ambient_occlusion_out_of_range_values_are_clamped() {
        let def = AmbientOcclusionDef {
            intensity: 7.0,
            radius: 0.0,
            ..AmbientOcclusionDef::default()
        };
        let ao = def.core();
        assert_eq!(ao.intensity, 2.0);
        assert!(ao.radius > 0.0);
    }

    fn sample() -> Scene {
        let mut scene = Scene::new("level-1");
        let mut player = Entity::new("player");
        player.transform.pos = [144.0, 104.0, 0.0];
        player.sprite = Some(SpriteComponent::new("sprites/player"));
        scene.entities.push(player);
        let mut house = Entity::new("house").with_model(ModelComponent {
            material: Some("materials/brick".into()),
            ..ModelComponent::new("models/house")
        });
        house.children.push(Entity::new("door").at([0.0, 0.0, 1.0]));
        house.tags.push("building".into());
        scene.entities.push(house);
        scene
    }

    #[test]
    fn round_trip_is_lossless_and_stable() {
        let scene = sample();
        let text = scene.to_ron();
        let back = Scene::from_ron(&text).unwrap();
        assert_eq!(scene, back);
        // And stable: serializing again yields identical text (diffable).
        assert_eq!(text, back.to_ron());
    }

    #[test]
    fn missing_optional_fields_get_defaults() {
        let text = r#"(version: 3, name: "min", entities: [(name: "e")])"#;
        let scene = Scene::from_ron(text).unwrap();
        let e = &scene.entities[0];
        assert_eq!(e.transform, Transform::default());
        assert!(e.visible && e.sprite.is_none() && e.children.is_empty());
        assert_eq!(scene.environment, Environment::default());
    }

    #[test]
    fn v1_scenes_migrate_to_v2() {
        let text = r#"(
            version: 1,
            name: "old",
            entities: [(
                name: "player",
                transform: (pos: (144.0, 104.0), scale: (2.0, 1.0), rotation: 3.14159265),
                sprite: Some((path: "sprites/player", flip_x: true)),
            )],
        )"#;
        let scene = Scene::from_ron(text).unwrap();
        assert_eq!(scene.version, SCENE_VERSION);
        let e = &scene.entities[0];
        assert_eq!(e.transform.pos, [144.0, 104.0, 0.0]);
        assert_eq!(e.transform.scale, [2.0, 1.0, 1.0]);
        assert!((e.transform.rot[2] - 180.0).abs() < 1e-3);
        assert!(e.sprite.as_ref().unwrap().flip_x);
        // Migrated scenes save as v2 and reload identically.
        assert_eq!(Scene::from_ron(&scene.to_ron()).unwrap(), scene);
    }

    #[test]
    fn newer_version_is_rejected_with_guidance() {
        let text = r#"(version: 99, name: "future", entities: [])"#;
        let err = Scene::from_ron(text).unwrap_err();
        assert!(err.contains("update Trino"), "{err}");
    }

    #[test]
    fn sprite_id_matches_core_hash() {
        let sprite = SpriteComponent::new("sprites/player");
        assert_eq!(sprite.sprite_id(), SpriteId::from_path("sprites/player"));
    }

    #[test]
    fn draw_names_join_the_ancestors() {
        let scene = sample();
        let house = scene.get(&[1]).unwrap().name.clone();
        assert_eq!(scene.draw_name(&[1, 0]), Some(format!("{house}/door")));
        assert_eq!(
            scene.find_draw_name(&format!("{house}/door")),
            Some(vec![1, 0])
        );
        assert_eq!(scene.find_draw_name("door"), None);
        assert_eq!(scene.draw_name(&[9]), None);
    }

    #[test]
    fn tree_editing_paths() {
        let mut scene = sample();
        assert_eq!(scene.get(&[1, 0]).unwrap().name, "door");
        // Reparent the player under the house.
        let new = scene.reparent(&[0], Some(&[1])).unwrap();
        assert_eq!(new, vec![0, 1]);
        assert_eq!(scene.get(&[0, 1]).unwrap().name, "player");
        // Cannot move a parent into its own child.
        assert!(scene.reparent(&[0], Some(&[0, 0])).is_none());
        // Back to the root.
        let back = scene.reparent(&[0, 1], None).unwrap();
        assert_eq!(back, vec![1]);
        let mut names = Vec::new();
        scene.walk(|p, e| names.push((p.to_vec(), e.name.clone())));
        assert_eq!(names.len(), 3);
        assert_eq!(scene.unique_name("door"), "door-2");
        assert_eq!(scene.unique_name("tree"), "tree");
    }
}
