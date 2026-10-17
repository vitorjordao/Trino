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
pub use material::{DetailDef, FilterDef, MaterialDef, ShadingDef};
pub use platform::{PlatformVariants, Target, Variant};
pub use prefab::{Override, Prefab, PrefabInstance};
pub use resolve::{DrawItem, LodDraw, LodItem, Resolved, resolve, resolve_for};

use serde::{Deserialize, Serialize};
use trino_core::SpriteId;

/// v3: per-platform variants (`platforms`, see [`platform`]). v2 added 3D
/// transforms (degrees), nested children, models, prefab instances and the
/// environment. v1 (2D-only, flat) and v2 are migrated on load.
///
/// v3 only adds a field, but it changes what a scene *means* per target:
/// an engine that ignored `platforms` would ship the PC model to a console
/// without a word. The bump makes such an engine refuse the file instead.
pub const SCENE_VERSION: u32 = 3;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Scene {
    pub version: u32,
    pub name: String,
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

impl Environment {
    /// What `Renderer::set_ambient_occlusion` takes: `None` without an
    /// `ao` block or with `enabled: false`.
    pub fn core_ao(&self) -> Option<trino_core::AmbientOcclusion> {
        self.ao.filter(|a| a.enabled).map(|a| a.core())
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

/// Per-platform [`LodAt`]s of a level (`None` = the level's `distance`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct LodPlatforms {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pc: Option<LodAt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n64: Option<LodAt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n3ds: Option<LodAt>,
}

impl LodPlatforms {
    pub fn is_empty(&self) -> bool {
        self.pc.is_none() && self.n64.is_none() && self.n3ds.is_none()
    }

    pub fn get(&self, target: Target) -> Option<&LodAt> {
        match target {
            Target::Pc => self.pc.as_ref(),
            Target::N64 => self.n64.as_ref(),
            Target::N3ds => self.n3ds.as_ref(),
        }
    }

    pub fn get_mut(&mut self, target: Target) -> &mut Option<LodAt> {
        match target {
            Target::Pc => &mut self.pc,
            Target::N64 => &mut self.n64,
            Target::N3ds => &mut self.n3ds,
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
    Cell { size: [f32; 3] },
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

impl Scene {
    pub fn new(name: impl Into<String>) -> Self {
        Scene {
            version: SCENE_VERSION,
            name: name.into(),
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
            // v2 is v3 without variants: same fields, defaults fill the rest.
            2 => {
                let mut scene: Scene =
                    ron::from_str(text).map_err(|e| format!("scene parse error (v2 file): {e}"))?;
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
