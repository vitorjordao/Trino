//! Runtime entities for gameplay: a fixed-capacity store instantiated from
//! exported scenes (`SceneNode`s) and grown/shrunk by game code — spawn,
//! despawn, move, query by name/tag, gather colliders, draw. No allocator
//! (`no_std` games on every target), O(N) queries (N is small on purpose:
//! an N64 level has tens to a few hundred live things).
//!
//! ```ignore
//! let mut world: EntityStore<128> = EntityStore::new();
//! world.spawn_nodes(level::NODES);
//! let door = world.find("castle/door").unwrap();
//! world.get_mut(door).unwrap().translate(Vec3::new(0.0, 0.1, 0.0));
//! let mut solids = [Aabb::EMPTY; 128];
//! let n = world.solids(&mut solids, None);
//! let out = move_aabb(pos, half, vel, dt, &solids[..n]);
//! world.draw(renderer, &Mat34::IDENTITY, time, camera.eye);
//! ```
//!
//! Handles are generational ([`EntityId`]): a despawned slot reused by a
//! new entity does not answer to the old handle.
//!
//! Memory: each slot is a couple hundred bytes; keep the store inside the
//! game state (which apps keep for the whole run) and size `N` to the level.

use crate::math::Vec3;
use crate::math3d::Mat34;
use crate::physics::{Aabb, Collider};
use crate::render::{AnimRef, ClipId, ModelId, ModelParams, Renderer};
use crate::scene::{LodPick, Lods, SceneNode, Tags};

/// One live entity. Plain data; game logic lives in the game, keyed by
/// `user` (a kind or an index into game-side arrays) or by tags.
#[derive(Clone, Copy, Debug)]
pub struct Entity {
    /// `/`-joined name path (scene entities) or any static name.
    pub name: &'static str,
    pub tags: Tags,
    /// World matrix.
    pub matrix: Mat34,
    pub model: Option<ModelId>,
    pub params: ModelParams,
    /// Looping clip and playback speed (scene semantics); game code can
    /// also set `params.anim` directly for full control (crossfades).
    pub anim: Option<(ClipId, f32)>,
    pub collider: Option<Collider>,
    /// Distance levels (see [`crate::scene::LodLevel`]).
    pub lod: Lods,
    /// Positional looping sound (see [`crate::audio::Ambience`]).
    pub sound: Option<crate::audio::SoundSource>,
    /// Interior cell (see [`crate::vis`]); [`crate::vis::NO_CELL`] = none.
    pub cell: u8,
    pub visible: bool,
    /// A halo at its position ([`crate::post::Glow`], issue #64): the
    /// consoles' bloom, drawn with it.
    pub glow: Option<crate::post::Glow>,
    /// Game-defined payload.
    pub user: u32,
}

impl Entity {
    pub const EMPTY: Entity = Entity {
        name: "",
        tags: Tags::NONE,
        matrix: Mat34::IDENTITY,
        model: None,
        params: ModelParams::DEFAULT,
        anim: None,
        collider: None,
        lod: Lods::NONE,
        sound: None,
        cell: crate::vis::NO_CELL,
        visible: true,
        glow: None,
        user: 0,
    };

    /// A live copy of an exported scene node.
    pub fn from_node(node: &SceneNode) -> Self {
        Entity {
            name: node.name,
            tags: node.tags,
            matrix: node.matrix,
            model: node.model,
            params: node.params,
            anim: node.anim,
            collider: node.collider,
            lod: node.lod,
            sound: node.sound,
            glow: node.glow,
            cell: node.cell,
            visible: true,
            user: 0,
        }
    }

    pub fn position(&self) -> Vec3 {
        Vec3::new(
            self.matrix.m[0][3],
            self.matrix.m[1][3],
            self.matrix.m[2][3],
        )
    }

    pub fn set_position(&mut self, p: Vec3) {
        self.matrix.m[0][3] = p.x;
        self.matrix.m[1][3] = p.y;
        self.matrix.m[2][3] = p.z;
    }

    pub fn translate(&mut self, d: Vec3) {
        let p = self.position();
        self.set_position(p + d);
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
    /// ([`crate::Lods::pick_biased`]).
    pub fn lod_from_biased(&self, eye: Vec3, bias: f32) -> LodPick {
        if self.lod.is_empty() {
            return self.model.map_or(LodPick::None, LodPick::Model);
        }
        let d = self.position() - eye;
        self.lod.pick_biased(self.model, d.dot(d), bias)
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.contains(tag)
    }

    /// World box of the collider, if any.
    pub fn world_aabb(&self) -> Option<Aabb> {
        self.collider.map(|c| c.world_aabb(&self.matrix))
    }

    /// Name is `name` or ends with `/name` (same rule as `scene::find`).
    pub fn is_named(&self, name: &str) -> bool {
        self.name == name
            || (self.name.len() > name.len()
                && self.name.ends_with(name)
                && self.name.as_bytes()[self.name.len() - name.len() - 1] == b'/')
    }
}

/// Generational handle into an [`EntityStore`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EntityId {
    index: u16,
    generation: u16,
}

impl EntityId {
    /// A handle no entity ever answers to (fills buffers such as the `ids`
    /// of [`EntityStore::solids`]).
    pub const NONE: EntityId = EntityId {
        index: u16::MAX,
        generation: u16::MAX,
    };

    pub fn index(self) -> usize {
        self.index as usize
    }
}

/// Fixed-capacity entity store (see the module docs).
pub struct EntityStore<const N: usize> {
    slots: [Option<Entity>; N],
    generations: [u16; N],
    len: usize,
}

impl<const N: usize> Default for EntityStore<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> EntityStore<N> {
    pub const fn new() -> Self {
        assert!(N <= u16::MAX as usize, "EntityStore capacity must fit u16");
        EntityStore {
            slots: [None; N],
            generations: [0; N],
            len: 0,
        }
    }

    /// [`Self::new`] written into `p` one slot at a time, so a store far
    /// bigger than the N64's 64 KB stack never sits on it whole.
    ///
    /// # Safety
    /// `p` must be valid for writes and aligned for `Self`.
    #[inline(never)]
    pub unsafe fn write_new(p: *mut Self) {
        // SAFETY: the caller's contract; every field is written below.
        unsafe {
            let slots = core::ptr::addr_of_mut!((*p).slots) as *mut Option<Entity>;
            for i in 0..N {
                slots.add(i).write(None);
            }
            core::ptr::addr_of_mut!((*p).generations).write([0; N]);
            core::ptr::addr_of_mut!((*p).len).write(0);
        }
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Add an entity; `None` when the store is full.
    pub fn spawn(&mut self, entity: Entity) -> Option<EntityId> {
        let index = self.slots.iter().position(Option::is_none)?;
        self.slots[index] = Some(entity);
        self.len += 1;
        Some(EntityId {
            index: index as u16,
            generation: self.generations[index],
        })
    }

    /// Spawn every node of an exported scene; returns how many fit.
    pub fn spawn_nodes(&mut self, nodes: &[SceneNode]) -> usize {
        self.spawn_iter(nodes.iter().copied())
    }

    /// Spawn nodes from any source — e.g. `BakedScene::nodes()` for a scene
    /// loaded at runtime. Returns how many fit.
    pub fn spawn_iter(&mut self, nodes: impl IntoIterator<Item = SceneNode>) -> usize {
        nodes
            .into_iter()
            .take_while(|n| self.spawn(Entity::from_node(n)).is_some())
            .count()
    }

    /// Remove an entity; false if the handle is stale.
    pub fn despawn(&mut self, id: EntityId) -> bool {
        if self.get(id).is_none() {
            return false;
        }
        let i = id.index();
        self.slots[i] = None;
        self.generations[i] = self.generations[i].wrapping_add(1);
        self.len -= 1;
        true
    }

    /// Despawn everything (generations advance: old handles go stale).
    pub fn clear(&mut self) {
        for i in 0..N {
            if self.slots[i].take().is_some() {
                self.generations[i] = self.generations[i].wrapping_add(1);
            }
        }
        self.len = 0;
    }

    pub fn get(&self, id: EntityId) -> Option<&Entity> {
        let i = id.index();
        if i < N && self.generations[i] == id.generation {
            self.slots[i].as_ref()
        } else {
            None
        }
    }

    pub fn get_mut(&mut self, id: EntityId) -> Option<&mut Entity> {
        let i = id.index();
        if i < N && self.generations[i] == id.generation {
            self.slots[i].as_mut()
        } else {
            None
        }
    }

    fn id_of(&self, index: usize) -> EntityId {
        EntityId {
            index: index as u16,
            generation: self.generations[index],
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (EntityId, &Entity)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|e| (self.id_of(i), e)))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (EntityId, &mut Entity)> {
        let generations = &self.generations;
        self.slots.iter_mut().enumerate().filter_map(move |(i, s)| {
            s.as_mut().map(|e| {
                (
                    EntityId {
                        index: i as u16,
                        generation: generations[i],
                    },
                    e,
                )
            })
        })
    }

    /// First entity named `name` (or whose path ends with `/name`).
    pub fn find(&self, name: &str) -> Option<EntityId> {
        self.iter()
            .find(|(_, e)| e.is_named(name))
            .map(|(id, _)| id)
    }

    pub fn with_tag<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = EntityId> + 'a {
        self.iter()
            .filter(move |(_, e)| e.has_tag(tag))
            .map(|(id, _)| id)
    }

    /// Write the world boxes of visible **solid** colliders into `out`
    /// (and their owners into `ids`, same order); returns how many. Stops
    /// at `out.len()` — size the buffer like the store.
    pub fn solids(&self, out: &mut [Aabb], mut ids: Option<&mut [EntityId]>) -> usize {
        let mut n = 0;
        for (id, e) in self.iter() {
            let Some(c) = e.collider.filter(|c| !c.trigger && e.visible) else {
                continue;
            };
            if n == out.len() {
                break;
            }
            out[n] = c.world_aabb(&e.matrix);
            if let Some(ids) = ids.as_deref_mut()
                && n < ids.len()
            {
                ids[n] = id;
            }
            n += 1;
        }
        n
    }

    /// The visible solid colliders as [`crate::physics::Solid`]s (boxes,
    /// one-way boxes and ramps with their layers; issue #66), owners in
    /// `ids`; returns how many. Mesh colliders are not boxes: build a
    /// [`crate::physics::MeshInstance`] from their blob.
    pub fn level_solids(
        &self,
        out: &mut [crate::physics::Solid],
        mut ids: Option<&mut [EntityId]>,
    ) -> usize {
        let mut n = 0;
        for (id, e) in self.iter() {
            let Some(s) = e
                .collider
                .filter(|_| e.visible)
                .and_then(|c| c.world_solid(&e.matrix))
            else {
                continue;
            };
            if n == out.len() {
                break;
            }
            out[n] = s;
            if let Some(ids) = ids.as_deref_mut()
                && n < ids.len()
            {
                ids[n] = id;
            }
            n += 1;
        }
        n
    }

    /// Sound sources of visible entities, for [`crate::audio::Ambience::update`]
    /// (keyed by slot index — stable while the entity lives).
    pub fn sound_sources(
        &self,
    ) -> impl Iterator<Item = (u32, Vec3, crate::audio::SoundSource)> + '_ {
        self.iter().filter_map(|(id, e)| {
            let s = e.sound.filter(|_| e.visible)?;
            Some((id.index as u32, e.position(), s))
        })
    }

    /// Visible **trigger** entities whose collider overlaps `query`.
    pub fn triggers_overlapping<'a>(
        &'a self,
        query: &'a Aabb,
    ) -> impl Iterator<Item = EntityId> + 'a {
        self.iter()
            .filter(move |(_, e)| {
                e.visible
                    && e.collider
                        .is_some_and(|c| c.trigger && c.world_aabb(&e.matrix).overlaps(query))
            })
            .map(|(id, _)| id)
    }

    /// Draw every visible model entity under `root`, looping clips at
    /// `time` seconds (entities with `params.anim` set keep it), with LOD
    /// levels chosen for a camera at `eye`.
    pub fn draw(&self, renderer: &mut dyn Renderer, root: &Mat34, time: f32, eye: Vec3) {
        self.draw_in(renderer, root, time, eye, u64::MAX);
    }

    /// [`Self::draw`] limited to the cells set in `visible` (see
    /// [`crate::vis::Cells::visible`]).
    pub fn draw_in(
        &self,
        renderer: &mut dyn Renderer,
        root: &Mat34,
        time: f32,
        eye: Vec3,
        visible: u64,
    ) {
        let bias = renderer.lod_bias();
        for (_, e) in self.iter() {
            if !crate::vis::in_view(e.cell, visible) {
                continue;
            }
            let pick = e.lod_from_biased(eye, bias);
            // Culled by distance; a marker without a model may still glow.
            if !e.visible || (pick == LodPick::None && (e.model.is_some() || e.glow.is_none())) {
                continue;
            }
            let mut params = e.params;
            if let Some((clip, speed)) = e.anim {
                params.anim = Some(AnimRef::new(clip, time * speed));
            }
            let matrix = root.mul(&e.matrix);
            if pick != LodPick::None {
                pick.draw(renderer, &matrix, &params, eye);
            }
            if let Some(glow) = &e.glow {
                let at = Vec3::new(matrix.m[0][3], matrix.m[1][3], matrix.m[2][3]);
                crate::post::draw_glow(renderer, glow, at);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::Shape;

    fn node(name: &'static str, tags: &'static [&'static str], x: f32) -> SceneNode {
        let mut matrix = Mat34::IDENTITY;
        matrix.m[0][3] = x;
        SceneNode {
            name,
            tags: Tags::List(tags),
            matrix,
            model: None,
            params: ModelParams::DEFAULT,
            anim: None,
            collider: None,
            lod: Lods::NONE,
            sound: None,
            cell: crate::vis::NO_CELL,
            vis: None,
            glow: None,
            character: None,
            semantic: [0; 4],
            outline: None,
        }
    }

    #[test]
    fn spawn_despawn_and_generations() {
        let mut s: EntityStore<2> = EntityStore::new();
        let a = s.spawn(Entity::EMPTY).unwrap();
        let b = s.spawn(Entity::EMPTY).unwrap();
        assert!(s.spawn(Entity::EMPTY).is_none(), "full");
        assert_eq!(s.len(), 2);
        assert!(s.despawn(a));
        assert!(!s.despawn(a), "stale handle");
        let c = s
            .spawn(Entity {
                user: 7,
                ..Entity::EMPTY
            })
            .unwrap();
        assert_eq!(c.index(), a.index(), "slot reused");
        assert!(s.get(a).is_none(), "old handle does not see the new entity");
        assert_eq!(s.get(c).unwrap().user, 7);
        assert!(s.get(b).is_some());
        s.clear();
        assert!(s.is_empty() && s.get(b).is_none());
    }

    #[test]
    fn scene_nodes_queries_and_movement() {
        let nodes = [
            node("house/door", &["door"], 1.0),
            node("spawn", &["spawn"], 3.0),
            node("coin", &["coin"], 5.0),
        ];
        let mut s: EntityStore<8> = EntityStore::new();
        assert_eq!(s.spawn_nodes(&nodes), 3);
        let door = s.find("door").unwrap();
        assert_eq!(s.get(door).unwrap().name, "house/door");
        assert!(s.find("oor").is_none());
        assert_eq!(s.with_tag("coin").count(), 1);
        s.get_mut(door).unwrap().translate(Vec3::new(0.0, 2.0, 0.0));
        assert_eq!(s.get(door).unwrap().position(), Vec3::new(1.0, 2.0, 0.0));
        // Capacity limits how many nodes spawn.
        let mut small: EntityStore<2> = EntityStore::new();
        assert_eq!(small.spawn_nodes(&nodes), 2);
    }

    #[test]
    fn solids_and_triggers() {
        let mut s: EntityStore<8> = EntityStore::new();
        let wall = s
            .spawn(Entity {
                collider: Some(Collider::solid_box(Vec3::ONE)),
                ..Entity::EMPTY
            })
            .unwrap();
        let coin = s
            .spawn(Entity {
                collider: Some(Collider {
                    shape: Shape::Sphere { radius: 0.5 },
                    offset: Vec3::ZERO,
                    trigger: true,
                    layer: crate::physics::LAYER_DEFAULT,
                    one_way: false,
                }),
                ..Entity::EMPTY
            })
            .unwrap();
        s.get_mut(coin)
            .unwrap()
            .set_position(Vec3::new(5.0, 0.0, 0.0));
        let _hidden = s.spawn(Entity {
            collider: Some(Collider::solid_box(Vec3::ONE)),
            visible: false,
            ..Entity::EMPTY
        });
        let mut boxes = [Aabb::EMPTY; 8];
        let mut ids = [wall; 8];
        let n = s.solids(&mut boxes, Some(&mut ids));
        assert_eq!(n, 1, "triggers and hidden entities are not solid");
        assert_eq!(ids[0], wall);
        assert_eq!(boxes[0].max, Vec3::ONE);
        let probe = Aabb::from_center_half(Vec3::new(5.2, 0.0, 0.0), Vec3::new(0.1, 0.1, 0.1));
        let hits: std::vec::Vec<EntityId> = s.triggers_overlapping(&probe).collect();
        assert_eq!(hits, [coin]);
        // A short buffer truncates instead of panicking.
        assert_eq!(s.solids(&mut boxes[..0], None), 0);
    }
}
