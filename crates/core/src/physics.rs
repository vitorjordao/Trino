//! 3D collision for gameplay: axis-aligned boxes, an axis-resolved mover
//! and raycasts — the N64-era model (Mario 64 resolved against boxes and
//! floors, not meshes). Pure deterministic `f32`, no allocation: identical
//! results on PC, N64 and 3DS.
//!
//! - [`Aabb`]: world-space box; [`Collider`] is the authored shape (box,
//!   sphere, capsule) a scene entity carries, turned into its world
//!   [`Aabb`] with [`Collider::world_aabb`]. Rotated boxes and round shapes
//!   collide as their bounding box — the honest cost model for a 93 MHz CPU.
//! - [`move_aabb`]: moves a box by `vel * dt` against solids one axis at a
//!   time (X, Z, then Y, so ground contact is stable), with a contact skin
//!   so landing never registers as a wall hit. Callers substep (small fixed
//!   `dt`) for fast movers.
//! - [`raycast`]: slab test; nearest hit with its normal (cameras, line of
//!   sight, ground probes).
//!
//! Static level collision usually comes from scenes: an entity's
//! `collider` component is exported into `SceneNode::collider`, and
//! [`crate::entity::EntityStore`] gathers the solids each frame.

use crate::math::Vec3;
use crate::math3d::Mat34;

mod controller;
mod mesh;
pub mod scalar;
mod world;
pub use controller::*;
pub use mesh::*;
pub use scalar::{Box3, Fx, MeshQuery, MeshSupport, Scalar, V3};
pub use world::*;

/// Axis-aligned box in world space (Y-up).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb {
        min: Vec3::ZERO,
        max: Vec3::ZERO,
    };

    pub const fn new(min: Vec3, max: Vec3) -> Self {
        Aabb { min, max }
    }

    pub fn from_center_half(center: Vec3, half: Vec3) -> Self {
        Aabb {
            min: center - half,
            max: center + half,
        }
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn half(&self) -> Vec3 {
        (self.max - self.min) * 0.5
    }

    /// Strict overlap (touching faces do not overlap).
    #[inline]
    pub fn overlaps(&self, o: &Aabb) -> bool {
        self.min.x < o.max.x
            && o.min.x < self.max.x
            && self.min.y < o.max.y
            && o.min.y < self.max.y
            && self.min.z < o.max.z
            && o.min.z < self.max.z
    }

    pub fn contains(&self, p: Vec3) -> bool {
        p.x >= self.min.x
            && p.x <= self.max.x
            && p.y >= self.min.y
            && p.y <= self.max.y
            && p.z >= self.min.z
            && p.z <= self.max.z
    }

    /// Smallest box containing both.
    pub fn union(&self, o: &Aabb) -> Aabb {
        Aabb {
            min: Vec3::new(
                self.min.x.min(o.min.x),
                self.min.y.min(o.min.y),
                self.min.z.min(o.min.z),
            ),
            max: Vec3::new(
                self.max.x.max(o.max.x),
                self.max.y.max(o.max.y),
                self.max.z.max(o.max.z),
            ),
        }
    }

    /// World box of this (local) box under `m` — exact for translation
    /// and scale, the bounding box of the rotated box otherwise.
    pub fn transformed(&self, m: &Mat34) -> Aabb {
        let c = m.transform_point(self.center());
        let h = self.half();
        // |M| * half: the classic Arvo bound.
        let ext = |r: usize| m.m[r][0].abs() * h.x + m.m[r][1].abs() * h.y + m.m[r][2].abs() * h.z;
        let e = Vec3::new(ext(0), ext(1), ext(2));
        Aabb::from_center_half(c, e)
    }
}

/// Authored collision shape, centered on the entity origin + `offset`
/// (local space; the entity's transform scales/rotates it).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shape {
    /// Box with half extents.
    Box {
        half: Vec3,
    },
    Sphere {
        radius: f32,
    },
    /// Y-aligned capsule: `height` is the total height including the caps.
    Capsule {
        radius: f32,
        height: f32,
    },
    /// A box (half extents) whose top slopes from its bottom on one side
    /// to its top toward `dir` (local axes; a turned entity turns it).
    Ramp {
        half: Vec3,
        dir: RampDir,
    },
    /// A triangle mesh: the `TCOL` blob `mesh` (`collision/<model>`, from
    /// the manifest's `collision` key; [`MeshCollider`]) under the
    /// entity's matrix, whose local bounds are `half` around `offset`.
    Mesh {
        mesh: u32,
        half: Vec3,
    },
}

/// A scene entity's collider: solid geometry, or a **trigger** (overlap
/// queries only — coins, doors, kill zones, level exits). `layer` is its
/// layer bit (`assets/physics.ron` names them); `one_way` makes a box a
/// platform bodies jump up through.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Collider {
    pub shape: Shape,
    pub offset: Vec3,
    pub trigger: bool,
    pub layer: u16,
    pub one_way: bool,
}

/// The layer every collider is on unless its scene says otherwise.
pub const LAYER_DEFAULT: u16 = 1;

impl Collider {
    pub const fn solid_box(half: Vec3) -> Self {
        Collider {
            shape: Shape::Box { half },
            offset: Vec3::ZERO,
            trigger: false,
            layer: LAYER_DEFAULT,
            one_way: false,
        }
    }

    /// The shape's local bounding box.
    pub fn local_aabb(&self) -> Aabb {
        let half = match self.shape {
            Shape::Box { half } | Shape::Ramp { half, .. } | Shape::Mesh { half, .. } => half,
            Shape::Sphere { radius } => Vec3::new(radius, radius, radius),
            Shape::Capsule { radius, height } => {
                Vec3::new(radius, (height * 0.5).max(radius), radius)
            }
        };
        Aabb::from_center_half(self.offset, half)
    }

    /// World box under the entity's world matrix.
    pub fn world_aabb(&self, world: &Mat34) -> Aabb {
        self.local_aabb().transformed(world)
    }

    /// The [`Solid`] this collider is in the world (box, one-way box or
    /// ramp, its slope turned with the entity), or `None` for a mesh (a
    /// [`MeshInstance`] of its blob) or a trigger.
    pub fn world_solid(&self, world: &Mat34) -> Option<Solid> {
        if self.trigger {
            return None;
        }
        let aabb = self.world_aabb(world);
        let kind = match self.shape {
            Shape::Mesh { .. } => return None,
            Shape::Ramp { dir, .. } => SurfaceKind::Ramp(turn_ramp(dir, world)),
            _ if self.one_way => SurfaceKind::OneWay,
            _ => SurfaceKind::Solid,
        };
        Some(Solid {
            aabb,
            layer: self.layer,
            kind,
        })
    }

    /// The trigger volume of a trigger collider.
    pub fn world_trigger(&self, world: &Mat34) -> Option<Solid> {
        self.trigger
            .then(|| Solid::new(self.world_aabb(world), self.layer))
    }
}

/// A local ramp direction under `m`: the world axis it ends up nearest
/// (exact for the quarter turns ramps are placed at).
pub fn turn_ramp(dir: RampDir, m: &Mat34) -> RampDir {
    let local = match dir {
        RampDir::PosX => Vec3::new(1.0, 0.0, 0.0),
        RampDir::NegX => Vec3::new(-1.0, 0.0, 0.0),
        RampDir::PosZ => Vec3::new(0.0, 0.0, 1.0),
        RampDir::NegZ => Vec3::new(0.0, 0.0, -1.0),
    };
    let w = m.transform_dir(local);
    if w.x.abs() >= w.z.abs() {
        if w.x >= 0.0 {
            RampDir::PosX
        } else {
            RampDir::NegX
        }
    } else if w.z >= 0.0 {
        RampDir::PosZ
    } else {
        RampDir::NegZ
    }
}

/// Result of one [`move_aabb`] step.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MoveOut {
    pub pos: Vec3,
    pub vel: Vec3,
    pub on_ground: bool,
    pub hit_ceiling: bool,
    /// Hit something on X or Z this step.
    pub hit_wall: bool,
    /// Index (into `solids`) of the solid the box stands on.
    pub standing_on: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    X,
    Y,
    Z,
}

#[inline]
fn axis_get(v: Vec3, a: Axis) -> f32 {
    match a {
        Axis::X => v.x,
        Axis::Y => v.y,
        Axis::Z => v.z,
    }
}

#[inline]
fn axis_set(v: &mut Vec3, a: Axis, val: f32) {
    match a {
        Axis::X => v.x = val,
        Axis::Y => v.y = val,
        Axis::Z => v.z = val,
    }
}

/// Contact skin: each axis' overlap test shrinks the OTHER axes by this
/// margin. Without it a box resting with its feet 1e-7 inside the floor
/// (f32 error in `top + half - half`) registered as a horizontal hit and
/// got pushed to the block's side face — found by castle64's playtest bot
/// landing on an island at a run.
pub const SKIN: f32 = 0.02;

/// Move a box centered at `pos` (half extents `half`) by `vel * dt`
/// against `solids`, one axis at a time (X, Z, then Y for stable ground
/// contact). Velocity components into a contact are zeroed.
pub fn move_aabb(pos: Vec3, half: Vec3, vel: Vec3, dt: f32, solids: &[Aabb]) -> MoveOut {
    let mut out = MoveOut {
        pos,
        vel,
        on_ground: false,
        hit_ceiling: false,
        hit_wall: false,
        standing_on: None,
    };

    for axis in [Axis::X, Axis::Z, Axis::Y] {
        let dv = axis_get(vel, axis) * dt;
        if dv == 0.0 && axis != Axis::Y {
            continue;
        }
        let new = axis_get(out.pos, axis) + dv;
        axis_set(&mut out.pos, axis, new);
        let skin = match axis {
            Axis::Y => Vec3::new(SKIN, 0.0, SKIN),
            _ => Vec3::new(0.0, SKIN, 0.0),
        };
        let me = Aabb::new(out.pos - half + skin, out.pos + half - skin);
        for (i, s) in solids.iter().enumerate() {
            if !me.overlaps(s) {
                continue;
            }
            match axis {
                Axis::Y => {
                    if dv <= 0.0 {
                        // Falling/resting: snap onto the solid's top.
                        axis_set(&mut out.pos, Axis::Y, s.max.y + half.y);
                        out.vel.y = 0.0;
                        out.on_ground = true;
                        out.standing_on = Some(i);
                    } else {
                        axis_set(&mut out.pos, Axis::Y, s.min.y - half.y);
                        out.vel.y = 0.0;
                        out.hit_ceiling = true;
                    }
                }
                _ => {
                    if dv > 0.0 {
                        axis_set(
                            &mut out.pos,
                            axis,
                            axis_get(s.min, axis) - axis_get(half, axis),
                        );
                    } else if dv < 0.0 {
                        axis_set(
                            &mut out.pos,
                            axis,
                            axis_get(s.max, axis) + axis_get(half, axis),
                        );
                    }
                    out.hit_wall = true;
                    match axis {
                        Axis::X => out.vel.x = 0.0,
                        Axis::Z => out.vel.z = 0.0,
                        Axis::Y => {}
                    }
                }
            }
        }
    }
    out
}

/// A ray hit: distance along the (normalized) direction, which box, and
/// the face normal at the hit point (zero when the ray starts inside).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RayHit {
    pub t: f32,
    pub index: usize,
    pub normal: Vec3,
}

/// Slab test of one box: the entry distance and face normal when the ray
/// `origin + dir * t` enters `b` within `0 <= t <= limit` (zero normal when
/// it starts inside).
pub(crate) fn ray_box(origin: Vec3, dir: Vec3, limit: f32, b: &Aabb) -> Option<(f32, Vec3)> {
    let mut t_enter = 0.0f32;
    let mut t_exit = limit;
    let mut normal = Vec3::ZERO;
    for axis in 0..3 {
        let (o, d, min, max) = match axis {
            0 => (origin.x, dir.x, b.min.x, b.max.x),
            1 => (origin.y, dir.y, b.min.y, b.max.y),
            _ => (origin.z, dir.z, b.min.z, b.max.z),
        };
        if d.abs() < 1e-6 {
            if o < min || o > max {
                return None;
            }
            continue;
        }
        let (mut t0, mut t1) = ((min - o) / d, (max - o) / d);
        // Entering through the min face means the normal points -axis.
        let mut sign = -1.0;
        if t0 > t1 {
            core::mem::swap(&mut t0, &mut t1);
            sign = 1.0;
        }
        if t0 > t_enter {
            t_enter = t0;
            normal = match axis {
                0 => Vec3::new(sign, 0.0, 0.0),
                1 => Vec3::new(0.0, sign, 0.0),
                _ => Vec3::new(0.0, 0.0, sign),
            };
        }
        t_exit = t_exit.min(t1);
        if t_enter > t_exit {
            return None;
        }
    }
    (t_enter <= limit).then_some((t_enter.max(0.0), normal))
}

/// Nearest hit of `origin + dir * t` (`dir` normalized, `0 <= t <= max_t`)
/// against `boxes` (slab method). Ties go to the lowest index.
pub fn raycast(origin: Vec3, dir: Vec3, max_t: f32, boxes: &[Aabb]) -> Option<RayHit> {
    let mut best: Option<RayHit> = None;
    for (index, b) in boxes.iter().enumerate() {
        let limit = best.map_or(max_t, |h| h.t);
        if let Some((t, normal)) = ray_box(origin, dir, limit, b)
            && best.is_none_or(|h| t < h.t)
        {
            best = Some(RayHit { t, index, normal });
        }
    }
    best
}

/// Distance to the nearest hit, or `max_t` (camera-clamp convenience).
pub fn raycast_distance(origin: Vec3, dir: Vec3, max_t: f32, boxes: &[Aabb]) -> f32 {
    raycast(origin, dir, max_t, boxes).map_or(max_t, |h| h.t)
}

/// Indices of `boxes` overlapping `query` (trigger checks).
pub fn overlapping<'a>(query: &'a Aabb, boxes: &'a [Aabb]) -> impl Iterator<Item = usize> + 'a {
    boxes
        .iter()
        .enumerate()
        .filter(move |(_, b)| query.overlaps(b))
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor() -> [Aabb; 1] {
        [Aabb::new(
            Vec3::new(-10.0, -1.0, -10.0),
            Vec3::new(10.0, 0.0, 10.0),
        )]
    }

    #[test]
    fn falls_and_lands_on_floor() {
        let solids = floor();
        let mut pos = Vec3::new(0.0, 3.0, 0.0);
        let mut vel = Vec3::ZERO;
        let half = Vec3::new(0.3, 0.5, 0.3);
        let mut grounded = false;
        for _ in 0..300 {
            vel.y -= 25.0 * (1.0 / 120.0);
            let out = move_aabb(pos, half, vel, 1.0 / 120.0, &solids);
            pos = out.pos;
            vel = out.vel;
            if out.on_ground {
                assert_eq!(out.standing_on, Some(0));
                grounded = true;
                break;
            }
        }
        assert!(grounded);
        assert!((pos.y - 0.5).abs() < 1e-4, "pos.y = {}", pos.y);
    }

    #[test]
    fn wall_blocks_horizontal_movement() {
        let solids = [
            floor()[0],
            Aabb::new(Vec3::new(2.0, 0.0, -1.0), Vec3::new(3.0, 2.0, 1.0)),
        ];
        let mut pos = Vec3::new(0.0, 0.5, 0.0);
        let mut walled = false;
        for _ in 0..240 {
            let out = move_aabb(
                pos,
                Vec3::new(0.3, 0.5, 0.3),
                Vec3::new(5.0, -1.0, 0.0),
                1.0 / 120.0,
                &solids,
            );
            walled |= out.hit_wall;
            pos = out.pos;
        }
        assert!(walled);
        assert!((pos.x - 1.7).abs() < 1e-3, "pos.x = {}", pos.x);
    }

    #[test]
    fn landing_on_top_never_snags_on_the_side() {
        let solids = [Aabb::new(
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::new(10.0, 1.0, 3.0),
        )];
        let half = Vec3::new(0.3, 0.55, 0.3);
        // 1.55 - 0.55 = 0.99999994f32 < 1.0 — the real playtest case.
        let mut pos = Vec3::new(2.0, 1.55, 1.5);
        let mut min_x = pos.x;
        for _ in 0..240 {
            let out = move_aabb(pos, half, Vec3::new(5.0, -2.0, 0.0), 1.0 / 120.0, &solids);
            assert!(
                out.pos.x >= min_x - 1e-4,
                "side snag: {min_x} -> {}",
                out.pos.x
            );
            assert!(!out.hit_wall);
            min_x = out.pos.x;
            pos = out.pos;
            if pos.x > 8.0 {
                break;
            }
        }
        assert!(pos.x > 8.0, "did not cross the block: {}", pos.x);
    }

    #[test]
    fn ceiling_stops_upward_motion() {
        let solids = [Aabb::new(
            Vec3::new(-1.0, 2.0, -1.0),
            Vec3::new(1.0, 3.0, 1.0),
        )];
        let out = move_aabb(
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.3, 0.5, 0.3),
            Vec3::new(0.0, 10.0, 0.0),
            0.1,
            &solids,
        );
        assert!(out.hit_ceiling);
        assert_eq!(out.vel.y, 0.0);
        assert!((out.pos.y - 1.5).abs() < 1e-4);
    }

    #[test]
    fn raycast_hits_the_nearest_box_with_its_normal() {
        let boxes = [
            Aabb::new(Vec3::new(4.0, -1.0, -1.0), Vec3::new(6.0, 1.0, 1.0)),
            Aabb::new(Vec3::new(2.0, -1.0, -1.0), Vec3::new(3.0, 1.0, 1.0)),
        ];
        let hit = raycast(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), 10.0, &boxes).unwrap();
        assert!((hit.t - 2.0).abs() < 1e-4, "t = {}", hit.t);
        assert_eq!(hit.index, 1);
        assert_eq!(hit.normal, Vec3::new(-1.0, 0.0, 0.0));
        // Down onto a floor: normal points up.
        let down = raycast(
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            10.0,
            &floor(),
        )
        .unwrap();
        assert!((down.t - 5.0).abs() < 1e-4);
        assert_eq!(down.normal, Vec3::new(0.0, 1.0, 0.0));
        // No hit: None / max_t.
        assert!(raycast(Vec3::ZERO, Vec3::new(-1.0, 0.0, 0.0), 10.0, &boxes).is_none());
        assert_eq!(
            raycast_distance(Vec3::ZERO, Vec3::new(-1.0, 0.0, 0.0), 10.0, &boxes),
            10.0
        );
        // A zero-direction axis outside the slab: no hit.
        let t = raycast_distance(
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            10.0,
            &boxes,
        );
        assert_eq!(t, 10.0);
        // Beyond max_t: no hit.
        assert!(raycast(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), 1.5, &boxes).is_none());
    }

    #[test]
    fn collider_world_boxes() {
        let c = Collider {
            shape: Shape::Capsule {
                radius: 0.5,
                height: 2.0,
            },
            offset: Vec3::new(0.0, 1.0, 0.0),
            ..Collider::solid_box(Vec3::ONE)
        };
        let m = Mat34::from_rotation_scale_translation(
            Vec3::ZERO,
            Vec3::new(2.0, 1.0, 1.0),
            Vec3::new(10.0, 0.0, 0.0),
        );
        let w = c.world_aabb(&m);
        assert_eq!(w.min, Vec3::new(9.0, 0.0, -0.5));
        assert_eq!(w.max, Vec3::new(11.0, 2.0, 0.5));
        // 90° about Y swaps X/Z extents of a box.
        let b = Collider::solid_box(Vec3::new(2.0, 1.0, 0.5));
        let rot = Mat34::from_rotation_scale_translation(
            Vec3::new(0.0, core::f32::consts::FRAC_PI_2, 0.0),
            Vec3::ONE,
            Vec3::ZERO,
        );
        let r = b.world_aabb(&rot);
        assert!(
            (r.max.x - 0.5).abs() < 1e-3 && (r.max.z - 2.0).abs() < 1e-3,
            "{r:?}"
        );
        let s = Collider {
            shape: Shape::Sphere { radius: 1.0 },
            trigger: true,
            ..Collider::solid_box(Vec3::ONE)
        };
        assert_eq!(s.local_aabb().half(), Vec3::ONE);
    }

    #[test]
    fn overlap_queries_and_union() {
        let boxes = [
            Aabb::new(Vec3::ZERO, Vec3::ONE),
            Aabb::new(Vec3::new(5.0, 5.0, 5.0), Vec3::new(6.0, 6.0, 6.0)),
        ];
        let q = Aabb::from_center_half(Vec3::new(0.5, 0.5, 0.5), Vec3::new(0.1, 0.1, 0.1));
        let hits: std::vec::Vec<usize> = overlapping(&q, &boxes).collect();
        assert_eq!(hits, [0]);
        let u = boxes[0].union(&boxes[1]);
        assert_eq!(u.max, Vec3::new(6.0, 6.0, 6.0));
        assert!(u.contains(Vec3::new(3.0, 3.0, 3.0)));
    }
}
