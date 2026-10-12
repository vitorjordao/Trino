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

use crate::math::{Color, Vec3};
use crate::math3d::Mat34;
use crate::physics::{Aabb, Collider, Shape};
use crate::render::{AnimRef, ClipId, Material, ModelId, ModelParams, Renderer, Shading};
use crate::render3d::{Camera3, Fog, Light};

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
/// camera eye) on, draw `model` instead — `None` draws nothing (culled).
/// Fake objects make natural far levels: a 6-sided cone for a tree, a box
/// for a house.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodLevel {
    pub distance: f32,
    pub model: Option<ModelId>,
}

/// A node's LOD levels, sorted by distance: a static list (exported code)
/// or 8-byte little-endian records (`f32 distance, u32 model`, 0 = none)
/// borrowed from a baked scene blob.
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
            Lods::Packed(b) => b.len() / 8,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, i: usize) -> Option<LodLevel> {
        match self {
            Lods::List(l) => l.get(i).copied(),
            Lods::Packed(b) => {
                let r = b.get(i * 8..i * 8 + 8)?;
                let id = u32::from_le_bytes([r[4], r[5], r[6], r[7]]);
                Some(LodLevel {
                    distance: f32::from_le_bytes([r[0], r[1], r[2], r[3]]),
                    model: (id != 0).then_some(ModelId(id)),
                })
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = LodLevel> + '_ {
        (0..self.len()).filter_map(|i| self.get(i))
    }

    /// Model to draw at squared distance `dist_sq` from the eye: the last
    /// level whose distance is reached, else `base`. Squared distances
    /// keep `sqrt` (software on consoles) out of the per-node cost.
    pub fn select(&self, base: Option<ModelId>, dist_sq: f32) -> Option<ModelId> {
        let mut model = base;
        for level in self.iter() {
            if dist_sq >= level.distance * level.distance {
                model = level.model;
            } else {
                break;
            }
        }
        model
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
}

impl SceneNode {
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

    /// Model to draw seen from `eye` (LOD applied).
    pub fn model_from(&self, eye: Vec3) -> Option<ModelId> {
        if self.lod.is_empty() {
            return self.model;
        }
        let d = self.position() - eye;
        self.lod.select(self.model, d.dot(d))
    }
}

// ---------------------------------------------------------------- TSCN ----

/// Baked scene blob ("TSCN", little-endian), written by
/// `trino_scene::bake::to_tscn`:
///
/// ```text
/// magic "TSCN"  u16 version (1)  u16 reserved  u32 node_count
/// environment: clear rgb u8*3 + pad u8, camera eye f32*3 target f32*3
///   fov_y f32 (radians), light dir f32*3 ambient f32 color rgba
///   ambient_color rgba, fog color rgba near f32 far f32      (= 68 bytes)
/// node_count records of NODE_BYTES:
///   name (u32 offset, u32 len) and tags (u32 offset, u32 len) into the
///   string table — tags are `\n`-separated
///   matrix f32*12 (row-major 3x4)
///   flags u32: bit0 model, bit1 anim, bit2 double_sided, bit3 texture,
///     bits 8..10 shading
///     (0 gouraud, 1 flat, 2 unlit), bits 12..14 collider shape (0 none,
///     1 box, 2 sphere, 3 capsule), bit15 collider trigger
///   model u32, tint rgba, emissive rgba, clip u32, speed f32
///   collider a f32 b f32 c f32 (box half xyz / sphere r / capsule r,h)
///   collider offset f32*3, texture u32 (sprite handle),
///   LOD levels (u32 offset, u32 count) into the data table — 8-byte
///   records `f32 distance, u32 model` (0 = none), sorted by distance
/// data table: UTF-8 strings and LOD records, to the end of the blob: UTF-8 bytes to the end of the blob
/// ```
///
/// [`BakedScene::parse`] validates everything up front (bounds, UTF-8), so
/// node access never fails afterwards.
pub const TSCN_MAGIC: &[u8; 4] = b"TSCN";
pub const TSCN_VERSION: u16 = 1;
pub const TSCN_HEADER: usize = 12;
pub const TSCN_ENV_BYTES: usize = 68;
pub const TSCN_NODE_BYTES: usize = 16 + 48 + 4 + 4 + 4 + 4 + 4 + 4 + 12 + 12 + 4 + 8;

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
            scene.data(rd_u32(bytes, lod), rd_u32(bytes, lod + 4).saturating_mul(8))?;
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
        let params = ModelParams {
            tint: rd_rgba(d, b + 8),
            emissive: rd_rgba(d, b + 12),
            shading,
            double_sided: flags & 4 != 0,
            anim: None,
            texture: (flags & 8 != 0).then(|| crate::render::SpriteId(rd_u32(d, b + 48))),
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
            _ => None,
        };
        let lod_at = b + 52;
        let lod = self
            .data(rd_u32(d, lod_at), rd_u32(d, lod_at + 4) * 8)
            .expect("validated in parse");
        SceneNode {
            name,
            tags: Tags::Packed(tags),
            lod: Lods::Packed(lod),
            matrix,
            model: (flags & 1 != 0).then(|| ModelId(rd_u32(d, b + 4))),
            params,
            anim: (flags & 2 != 0).then(|| (ClipId(rd_u32(d, b + 16)), rd_f32(d, b + 20))),
            collider: shape.map(|shape| Collider {
                shape,
                offset: rd_vec3(d, b + 36),
                trigger: flags & (1 << 15) != 0,
            }),
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

/// Draw every model node under `root`, animating clips at `time` seconds,
/// with LOD levels chosen for a camera at `eye` (pass `camera.eye`).
pub fn draw_nodes(
    renderer: &mut dyn Renderer,
    nodes: &[SceneNode],
    root: &Mat34,
    time: f32,
    eye: Vec3,
) {
    for node in nodes {
        let Some(model) = node.model_from(eye) else {
            continue;
        };
        let mut params = node.params;
        if let Some((clip, speed)) = node.anim {
            params.anim = Some(AnimRef::new(clip, time * speed));
        }
        renderer.draw_model_matrix(model, &root.mul(&node.matrix), Material::VertexLit, &params);
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
        b.extend_from_slice(b"a/doordoor\nx");
        f(&mut b, &[30.0]);
        b.extend_from_slice(&0u32.to_le_bytes()); // culled beyond 30
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
        // LOD: model 7 up close, nothing beyond 30 units.
        assert_eq!(n.model_from(Vec3::new(3.0, 0.0, 0.0)), Some(ModelId(7)));
        assert_eq!(n.model_from(Vec3::new(3.0, 0.0, 40.0)), None);
    }

    #[test]
    fn lod_selection() {
        const LEVELS: &[LodLevel] = &[
            LodLevel {
                distance: 10.0,
                model: Some(ModelId(2)),
            },
            LodLevel {
                distance: 20.0,
                model: None,
            },
        ];
        let lods = Lods::List(LEVELS);
        assert_eq!(lods.select(Some(ModelId(1)), 5.0 * 5.0), Some(ModelId(1)));
        assert_eq!(lods.select(Some(ModelId(1)), 10.0 * 10.0), Some(ModelId(2)));
        assert_eq!(lods.select(Some(ModelId(1)), 25.0 * 25.0), None);
        assert_eq!(Lods::NONE.select(Some(ModelId(1)), 1e9), Some(ModelId(1)));
        let mut packed = std::vec::Vec::new();
        for l in LEVELS {
            packed.extend_from_slice(&l.distance.to_le_bytes());
            packed.extend_from_slice(&l.model.map_or(0, |m| m.0).to_le_bytes());
        }
        let packed: &'static [u8] = std::boxed::Box::leak(packed.into_boxed_slice());
        let p = Lods::Packed(packed);
        assert_eq!(p.len(), 2);
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
