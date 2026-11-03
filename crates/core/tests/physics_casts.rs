//! Physics 2.1 (issue #115): shape casts and overlaps against triangle
//! meshes, ramps and one-way platforms, exactly.
//!
//! - A mesh cast through the BVH equals the brute force over every triangle
//!   (same hit, same triangle, same bits), for spheres, Y-capsules and boxes
//!   under rotated and scaled instances.
//! - The contact is the real first contact: no sampled point of the path
//!   before it overlaps, the shape there touches, and a miss never overlaps.
//! - A ramp is its wedge: a cast against the ramp solid lands where a cast
//!   against a mesh of the wedge's triangles lands (two independent routes),
//!   and overlaps above the slope are empty.
//! - The grid still answers what the scan answers with ramps and one-way
//!   boxes among the solids.

use trino_core::physics::{
    self, CastShape, MeshCollider, MeshInstance, RampDir, ShapeHit, overlap_shape_mesh_scan,
    overlap_shape_solids, shape_cast_mesh_scan, shape_cast_solids, tcol_bytes, write_tcol,
};
use trino_core::{Aabb, LAYER_ALL, Mat34, Solid, StaticWorld, Vec3};

fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next() * (hi - lo)
    }
    fn v(&mut self, lo: f32, hi: f32) -> Vec3 {
        v(self.range(lo, hi), self.range(lo, hi), self.range(lo, hi))
    }
}

fn tcol(verts: &[Vec3], tris: &[[u16; 3]]) -> Vec<u8> {
    let mut out = vec![0u8; tcol_bytes(verts.len(), tris.len())];
    let mut order = vec![0u32; tris.len()];
    let n = write_tcol(verts, tris, &mut order, &mut out).unwrap();
    out.truncate(n);
    out
}

/// A bumpy 12×12 terrain (288 triangles) with a few walls standing up.
fn terrain() -> Vec<u8> {
    let mut verts = Vec::new();
    let mut tris = Vec::new();
    let n = 13;
    for z in 0..n {
        for x in 0..n {
            let (fx, fz) = (x as f32 - 6.0, z as f32 - 6.0);
            let y = 0.4 * (fx * 0.9).sin() * (fz * 0.7).cos() + if x == 6 { 1.5 } else { 0.0 };
            verts.push(v(fx, y, fz));
        }
    }
    for z in 0..n - 1 {
        for x in 0..n - 1 {
            let i = (z * n + x) as u16;
            let n16 = n as u16;
            tris.push([i, i + n16, i + 1]);
            tris.push([i + 1, i + n16, i + n16 + 1]);
        }
    }
    tcol(&verts, &tris)
}

fn shapes() -> [CastShape; 4] {
    [
        CastShape::Sphere { radius: 0.35 },
        CastShape::Capsule {
            radius: 0.3,
            height: 1.6,
        },
        CastShape::Box {
            half: v(0.3, 0.55, 0.3),
        },
        CastShape::Sphere { radius: 1.3 },
    ]
}

fn instances(mesh: MeshCollider<'_>) -> Vec<MeshInstance<'_>> {
    vec![
        MeshInstance::new(mesh, Mat34::IDENTITY),
        MeshInstance::new(
            mesh,
            Mat34::from_rotation_scale_translation(
                v(0.2, 0.7, -0.1),
                v(1.5, 0.8, 1.2),
                v(3.0, -1.0, 2.0),
            ),
        ),
    ]
}

#[test]
fn mesh_casts_through_the_bvh_equal_the_brute_force() {
    let blob = terrain();
    let mesh = MeshCollider::parse(&blob).unwrap();
    let mut r = Rng(0xC0FFEE);
    let mut hits = 0;
    let mut insides = 0;
    for inst in instances(mesh) {
        for _ in 0..400 {
            let from = r.v(-8.0, 8.0) + v(0.0, 2.0, 0.0);
            let to = r.v(-8.0, 8.0) - v(0.0, 1.0, 0.0);
            for shape in shapes() {
                let fast = inst.shape_cast(shape, from, to);
                let slow = shape_cast_mesh_scan(shape, from, to, &inst);
                assert_eq!(fast, slow, "{shape:?} {from:?} -> {to:?}");
                if let Some(h) = fast {
                    hits += 1;
                    insides += h.inside as usize;
                }
                let (mut a, mut b) = ([0u32; 24], [0u32; 24]);
                let (na, nb) = (
                    inst.overlap_shape(shape, from, &mut a),
                    overlap_shape_mesh_scan(shape, from, &inst, &mut b),
                );
                assert_eq!(na, nb);
                assert_eq!(a[..na.min(24)], b[..nb.min(24)]);
            }
        }
    }
    assert!(hits > 1500 && insides > 50, "hits {hits} inside {insides}");
}

/// `shape` grown a little (a contact must overlap it).
fn grown(shape: CastShape, e: f32) -> CastShape {
    match shape {
        CastShape::Sphere { radius } => CastShape::Sphere { radius: radius + e },
        CastShape::Capsule { radius, height } => CastShape::Capsule {
            radius: radius + e,
            height: height + 2.0 * e,
        },
        CastShape::Box { half } => CastShape::Box {
            half: half + Vec3::ONE * e,
        },
    }
}

fn overlaps(inst: &MeshInstance, shape: CastShape, c: Vec3) -> bool {
    inst.overlap_shape(shape, c, &mut [0u32; 4]) > 0
}

#[test]
fn mesh_casts_find_the_first_contact() {
    let blob = terrain();
    let mesh = MeshCollider::parse(&blob).unwrap();
    let mut r = Rng(0xBADC0DE);
    let mut checked = 0;
    for inst in instances(mesh) {
        for _ in 0..150 {
            let from = r.v(-7.0, 7.0) + v(0.0, 3.5, 0.0);
            let to = r.v(-7.0, 7.0) - v(0.0, 2.0, 0.0);
            let d = to - from;
            let len = d.dot(d).sqrt();
            let at = |t: f32| from + d * (t / len);
            for shape in shapes() {
                let hit = inst.shape_cast(shape, from, to);
                if hit.is_some_and(|h| h.inside) {
                    assert!(overlaps(&inst, shape, from));
                    continue;
                }
                let end = hit.map_or(len, |h: ShapeHit| h.t);
                // Nothing before the contact overlaps.
                for k in 0..200 {
                    let t = end * k as f32 / 200.0 - 2e-3;
                    if t > 0.0 {
                        assert!(
                            !overlaps(&inst, shape, at(t)),
                            "{shape:?} overlaps at {t} before the contact at {end}"
                        );
                    }
                }
                if let Some(h) = hit {
                    // At the contact, a slightly larger shape overlaps.
                    assert!(overlaps(&inst, grown(shape, 2e-3), at(h.t)), "{shape:?}");
                    assert!((h.normal.dot(h.normal) - 1.0).abs() < 1e-3);
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 300, "{checked}");
}

/// The wedge of a ramp box rising toward `dir` as eight triangles.
fn wedge_mesh(b: &Aabb, dir: RampDir) -> Vec<u8> {
    let (lo, hi) = (b.min, b.max);
    // Rising toward +X; other directions turn the corners.
    let p = |a: f32, y: f32, o: f32| match dir {
        RampDir::PosX => v(lo.x + a * (hi.x - lo.x), y, lo.z + o * (hi.z - lo.z)),
        RampDir::NegX => v(hi.x - a * (hi.x - lo.x), y, lo.z + o * (hi.z - lo.z)),
        RampDir::PosZ => v(lo.x + o * (hi.x - lo.x), y, lo.z + a * (hi.z - lo.z)),
        RampDir::NegZ => v(lo.x + o * (hi.x - lo.x), y, hi.z - a * (hi.z - lo.z)),
    };
    let verts = [
        p(0.0, lo.y, 0.0),
        p(0.0, lo.y, 1.0),
        p(1.0, lo.y, 0.0),
        p(1.0, lo.y, 1.0),
        p(1.0, hi.y, 0.0),
        p(1.0, hi.y, 1.0),
    ];
    let tris = [
        [0, 2, 3],
        [0, 3, 1],
        [0, 1, 5],
        [0, 5, 4],
        [2, 4, 5],
        [2, 5, 3],
        [0, 4, 2],
        [1, 3, 5],
    ];
    tcol(&verts, &tris)
}

#[test]
fn ramps_are_cast_against_as_their_wedge() {
    let mut r = Rng(0x5EED);
    let mut compared = 0;
    for dir in [RampDir::PosX, RampDir::NegX, RampDir::PosZ, RampDir::NegZ] {
        let b = Aabb::new(v(-2.0, 0.0, -1.5), v(2.0, 1.5, 1.5));
        let ramp = [Solid::ramp(b, 1, dir)];
        let blob = wedge_mesh(&b, dir);
        let inst = MeshInstance::new(MeshCollider::parse(&blob).unwrap(), Mat34::IDENTITY);
        for _ in 0..300 {
            let from = r.v(-4.0, 4.0) + v(0.0, 2.5, 0.0);
            let to = r.v(-3.0, 3.0) * 0.6;
            for shape in shapes() {
                let a = shape_cast_solids(shape, from, to, LAYER_ALL, &ramp);
                let m = inst.shape_cast(shape, from, to);
                if a.is_some_and(|a| a.inside) && !m.is_some_and(|m| m.inside) {
                    // Wholly inside the solid wedge: the mesh (a surface)
                    // doesn't touch it yet.
                    assert!(!overlaps(&inst, shape, from));
                    continue;
                }
                assert_eq!(a.is_some(), m.is_some(), "{dir:?} {shape:?} {from:?}");
                if let (Some(a), Some(m)) = (a, m) {
                    assert_eq!(a.inside, m.inside);
                    assert!(
                        (a.t - m.t).abs() < 1e-3,
                        "{dir:?} {shape:?}: {} vs {}",
                        a.t,
                        m.t
                    );
                    compared += 1;
                }
                // Overlaps agree with the wedge's triangles... unless the
                // shape is wholly inside the solid wedge (a mesh is a
                // surface).
                let solid = overlap_shape_solids(shape, from, LAYER_ALL, &ramp, &mut [0; 1]) > 0;
                if overlaps(&inst, shape, from) {
                    assert!(solid);
                }
            }
        }
    }
    assert!(compared > 500, "{compared}");
}

#[test]
fn ramp_overlaps_follow_the_slope() {
    let b = Aabb::new(v(0.0, 0.0, 0.0), v(4.0, 2.0, 2.0));
    let ramp = Solid::ramp(b, 1, RampDir::PosX);
    // Above the low end, inside the box: empty space over the slope.
    let above = Aabb::from_center_half(v(0.5, 1.6, 1.0), v(0.2, 0.2, 0.2));
    assert!(ramp.aabb.overlaps(&above));
    assert!(!ramp.overlaps_aabb(&above));
    // Under the slope near the high end: inside the wedge.
    let under = Aabb::from_center_half(v(3.5, 1.0, 1.0), v(0.2, 0.2, 0.2));
    assert!(ramp.overlaps_aabb(&under));
    let ball = CastShape::Sphere { radius: 0.3 };
    assert!(!ramp.overlaps_shape(ball, v(0.5, 1.6, 1.0)));
    assert!(ramp.overlaps_shape(ball, v(3.5, 1.0, 1.0)));
    // Just over the slope's surface at x = 2 (height 1): touching misses.
    assert!(!ramp.overlaps_shape(ball, v(2.0, 1.0 + 0.3 * 1.118_034 + 1e-3, 1.0)));
}

#[test]
fn one_way_boxes_stop_casts_from_above_only() {
    let deck = [Solid::one_way(
        Aabb::new(v(-2.0, 1.0, -2.0), v(2.0, 1.2, 2.0)),
        1,
    )];
    let ball = CastShape::Sphere { radius: 0.25 };
    // Falling onto it: lands on the top.
    let down = shape_cast_solids(ball, v(0.0, 3.0, 0.0), v(0.0, 0.0, 0.0), LAYER_ALL, &deck)
        .expect("lands");
    assert!((down.t - (3.0 - 1.2 - 0.25)).abs() < 1e-4);
    assert!((down.normal - v(0.0, 1.0, 0.0)).dot(Vec3::ONE).abs() < 1e-4);
    // Jumping up through it, and walking through its side from below.
    assert!(
        shape_cast_solids(ball, v(0.0, 0.0, 0.0), v(0.0, 3.0, 0.0), LAYER_ALL, &deck).is_none()
    );
    assert!(
        shape_cast_solids(ball, v(-4.0, 1.1, 0.0), v(4.0, 1.1, 0.0), LAYER_ALL, &deck).is_none()
    );
    // A box resting on it, cast down: touching at once (not inside).
    let feet = CastShape::Box {
        half: v(0.3, 0.5, 0.3),
    };
    let rest =
        shape_cast_solids(feet, v(0.0, 1.7, 0.0), v(0.0, 1.0, 0.0), LAYER_ALL, &deck).unwrap();
    assert!(!rest.inside && rest.t == 0.0);
}

fn mixed_venue() -> Vec<Solid> {
    let mut r = Rng(0x1234_5678);
    let dirs = [RampDir::PosX, RampDir::NegX, RampDir::PosZ, RampDir::NegZ];
    (0..500)
        .map(|i| {
            let c = v(
                r.range(-200.0, 200.0),
                r.range(0.0, 8.0),
                r.range(-200.0, 200.0),
            );
            let h = v(r.range(0.5, 6.0), r.range(0.5, 4.0), r.range(0.5, 6.0));
            let b = Aabb::from_center_half(c, h);
            match i % 5 {
                0 => Solid::ramp(b, 1 << (i % 4), dirs[(i / 5) % 4]),
                1 => Solid::one_way(b, 1 << (i % 4)),
                _ => Solid::new(b, 1 << (i % 4)),
            }
        })
        .collect()
}

#[test]
fn the_grid_answers_what_the_scan_answers_with_ramps_and_one_ways() {
    let solids = mixed_venue();
    let size = StaticWorld::size(&solids, 16.0).unwrap();
    let (mut s, mut rf) = (vec![0u16; size.starts], vec![0u16; size.refs]);
    let grid = StaticWorld::build(&solids, 16.0, &mut s, &mut rf).unwrap();
    let mut r = Rng(0xABCD);
    let mut hits = 0;
    for i in 0..600 {
        let from = v(
            r.range(-200.0, 200.0),
            r.range(-2.0, 12.0),
            r.range(-200.0, 200.0),
        );
        let to = from
            + v(
                r.range(-30.0, 30.0),
                r.range(-6.0, 6.0),
                r.range(-30.0, 30.0),
            );
        let mask = [LAYER_ALL, 1, 6][i % 3];
        for shape in shapes() {
            let g = grid.shape_cast(shape, from, to, mask);
            assert_eq!(g, shape_cast_solids(shape, from, to, mask, &solids));
            hits += g.is_some() as usize;
            let (mut a, mut b) = ([0u16; 16], [0u16; 16]);
            let (na, nb) = (
                grid.overlap_shape(shape, from, mask, &mut a),
                overlap_shape_solids(shape, from, mask, &solids, &mut b),
            );
            assert_eq!(na, nb);
            assert_eq!(a[..na.min(16)], b[..nb.min(16)]);
        }
        let q = Aabb::from_center_half(from, v(6.0, 3.0, 6.0));
        let (mut a, mut b) = ([0u16; 16], [0u16; 16]);
        let (na, nb) = (
            grid.overlap_aabb(&q, mask, &mut a),
            physics::overlap_solids(&q, mask, &solids, &mut b),
        );
        assert_eq!(na, nb);
        assert_eq!(a[..na.min(16)], b[..nb.min(16)]);
        // The controller's candidates are a superset.
        let mut c = [0u16; 64];
        assert!(grid.overlap_bounds(&q, mask, &mut c) >= na);
    }
    assert!(hits > 200, "{hits}");
}

#[test]
fn bodies_are_boxes_on_every_tier_and_capsules_are_exact_in_queries() {
    // ADR 0035 §11: one body model on every target (replays, lockstep).
    for caps in [
        trino_core::Caps::N64,
        trino_core::Caps::N64_EXPANDED,
        trino_core::Caps::N3DS,
        trino_core::Caps::N3DS_NEW,
        trino_core::Caps::PC,
        trino_core::Caps::WEB,
    ] {
        assert!(!caps.capsule_bodies);
    }
    // A capsule cast passes the corner of a box its bounding box would hit.
    let block = [Solid::new(
        Aabb::new(v(1.0, 0.0, -1.0), v(2.0, 1.0, 1.0)),
        1,
    )];
    let capsule = CastShape::Capsule {
        radius: 0.5,
        height: 2.0,
    };
    // Passing beside the block's top edge (y 1, z 1): the capsule's box
    // (bottom 0.9, near side z 0.9) clips the edge, its round cap (center
    // y 1.4, z 1.4: 0.566 m from the edge) clears it.
    let (from, to) = (v(-1.0, 1.9, 1.4), v(4.0, 1.9, 1.4));
    assert!(shape_cast_solids(capsule, from, to, LAYER_ALL, &block).is_none());
    let boxed = CastShape::Box {
        half: v(0.5, 1.0, 0.5),
    };
    assert!(shape_cast_solids(boxed, from, to, LAYER_ALL, &block).is_some());
}
