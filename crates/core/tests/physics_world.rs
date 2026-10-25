//! Physics 2.0 phase 1 (#66): the broadphase answers what a scan of the
//! same solids answers, shape casts match analytic contacts, layers filter,
//! triggers report Enter / Stay / Exit.

use trino_core::physics::{self, CastShape, shape_cast_solids};
use trino_core::{
    Aabb, LAYER_ALL, Solid, StaticWorld, TriggerEvent, TriggerPhase, TriggerTracker, Vec3, World,
};

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
}

/// A 500-solid "venue": 400 m × 400 m, boxes of 1–12 m on four layers.
fn venue() -> Vec<Solid> {
    let mut r = Rng(0x1234_5678);
    (0..500)
        .map(|i| {
            let c = v(
                r.range(-200.0, 200.0),
                r.range(0.0, 8.0),
                r.range(-200.0, 200.0),
            );
            let h = v(r.range(0.5, 6.0), r.range(0.5, 4.0), r.range(0.5, 6.0));
            Solid::new(Aabb::from_center_half(c, h), 1 << (i % 4))
        })
        .collect()
}

fn build<'a>(solids: &'a [Solid], s: &'a mut Vec<u16>, r: &'a mut Vec<u16>) -> StaticWorld<'a> {
    let size = StaticWorld::size(solids, 16.0).unwrap();
    s.resize(size.starts, 0);
    r.resize(size.refs, 0);
    StaticWorld::build(solids, 16.0, s, r).unwrap()
}

#[test]
fn grid_overlaps_equal_a_scan() {
    let solids = venue();
    let (mut s, mut r) = (vec![], vec![]);
    let grid = build(&solids, &mut s, &mut r);
    let mut rng = Rng(7);
    let (mut a, mut b) = ([0u16; 600], [0u16; 600]);
    let mut total = 0;
    for _ in 0..400 {
        let c = v(
            rng.range(-220.0, 220.0),
            rng.range(-2.0, 10.0),
            rng.range(-220.0, 220.0),
        );
        let q = Aabb::from_center_half(
            c,
            v(
                rng.range(0.1, 20.0),
                rng.range(0.1, 5.0),
                rng.range(0.1, 20.0),
            ),
        );
        let mask = [LAYER_ALL, 1, 6, 8][(rng.next() * 4.0) as usize % 4];
        let n = grid.overlap_aabb(&q, mask, &mut a);
        let m = physics::overlap_solids(&q, mask, &solids, &mut b);
        assert_eq!(n, m, "query {c:?} mask {mask}");
        assert_eq!(a[..n], b[..m], "query {c:?} mask {mask}");
        // A short buffer keeps the same (lowest) indices as the scan.
        let (mut sa, mut sb) = ([0u16; 3], [0u16; 3]);
        let (n3, m3) = (
            grid.overlap_aabb(&q, mask, &mut sa),
            physics::overlap_solids(&q, mask, &solids, &mut sb),
        );
        assert_eq!(n3, m3);
        assert_eq!(sa[..n3.min(3)], sb[..m3.min(3)], "short buffer {c:?}");
        assert_eq!(grid.any_overlap(&q, mask), m > 0);
        total += m;
    }
    assert!(total > 100, "the queries must hit something: {total}");
}

#[test]
fn grid_rays_equal_a_scan() {
    let solids = venue();
    let (mut s, mut r) = (vec![], vec![]);
    let grid = build(&solids, &mut s, &mut r);
    let mut rng = Rng(99);
    let mut hits = 0;
    for _ in 0..2000 {
        let o = v(
            rng.range(-230.0, 230.0),
            rng.range(-1.0, 9.0),
            rng.range(-230.0, 230.0),
        );
        let d = v(
            rng.range(-1.0, 1.0),
            rng.range(-0.3, 0.3),
            rng.range(-1.0, 1.0),
        );
        let l = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
        let d = v(d.x / l, d.y / l, d.z / l);
        let mask = [LAYER_ALL, 2, 5][(rng.next() * 3.0) as usize % 3];
        let g = grid.raycast(o, d, 150.0, mask);
        let b = physics::raycast_solids(o, d, 150.0, mask, &solids);
        assert_eq!(g, b, "ray {o:?} -> {d:?} mask {mask}");
        hits += g.is_some() as usize;
    }
    assert!(hits > 200, "rays must hit: {hits}");
    // Axis-aligned rays (a zero component) and a vertical one.
    for d in [v(1.0, 0.0, 0.0), v(0.0, 0.0, -1.0), v(0.0, -1.0, 0.0)] {
        let o = v(-210.0, 20.0, 3.0);
        let o = if d.y != 0.0 {
            v(solids[0].aabb.center().x, 30.0, solids[0].aabb.center().z)
        } else {
            o
        };
        assert_eq!(
            grid.raycast(o, d, 500.0, LAYER_ALL),
            physics::raycast_solids(o, d, 500.0, LAYER_ALL, &solids)
        );
    }
}

#[test]
fn grid_shape_casts_equal_a_scan() {
    let solids = venue();
    let (mut s, mut r) = (vec![], vec![]);
    let grid = build(&solids, &mut s, &mut r);
    let mut rng = Rng(31337);
    let shapes = [
        CastShape::Sphere { radius: 0.4 },
        CastShape::Capsule {
            radius: 0.4,
            height: 1.8,
        },
        CastShape::Box {
            half: v(0.4, 0.9, 0.4),
        },
    ];
    let mut hits = 0;
    for k in 0..600 {
        let from = v(
            rng.range(-200.0, 200.0),
            rng.range(0.0, 9.0),
            rng.range(-200.0, 200.0),
        );
        let to = from
            + v(
                rng.range(-30.0, 30.0),
                rng.range(-4.0, 4.0),
                rng.range(-30.0, 30.0),
            );
        let shape = shapes[k % 3];
        let g = grid.shape_cast(shape, from, to, LAYER_ALL);
        let b = shape_cast_solids(shape, from, to, LAYER_ALL, &solids);
        assert_eq!(g, b, "{shape:?} {from:?} -> {to:?}");
        hits += g.is_some() as usize;
    }
    assert!(hits > 100, "casts must hit: {hits}");
}

#[test]
fn dynamics_follow_the_statics() {
    let statics = [Solid::new(
        Aabb::new(v(0.0, -1.0, 0.0), v(4.0, 0.0, 4.0)),
        1,
    )];
    let dynamics = [Solid::new(Aabb::new(v(1.0, 0.0, 1.0), v(2.0, 1.0, 2.0)), 2)];
    let (mut s, mut r) = (vec![], vec![]);
    let size = StaticWorld::size(&statics, 2.0).unwrap();
    s.resize(size.starts, 0);
    r.resize(size.refs, 0);
    let world = World {
        statics: StaticWorld::build(&statics, 2.0, &mut s, &mut r).unwrap(),
        dynamics: &dynamics,
    };
    let down = world
        .raycast(v(1.5, 5.0, 1.5), v(0.0, -1.0, 0.0), 10.0, LAYER_ALL)
        .unwrap();
    assert_eq!((down.index, down.t), (1, 4.0), "the crate is hit first");
    let floor = world
        .raycast(v(1.5, 5.0, 1.5), v(0.0, -1.0, 0.0), 10.0, 1)
        .unwrap();
    assert_eq!((floor.index, floor.t), (0, 5.0), "the mask skips the crate");
    let mut out = [0u16; 4];
    let q = Aabb::new(v(1.2, -0.5, 1.2), v(1.8, 0.5, 1.8));
    assert_eq!(world.overlap_aabb(&q, LAYER_ALL, &mut out), 2);
    assert_eq!(out[..2], [0, 1]);
}

#[test]
fn sphere_casts_match_the_analytic_contacts() {
    let unit = [Solid::new(Aabb::new(v(0.0, 0.0, 0.0), v(1.0, 1.0, 1.0)), 1)];
    let ball = CastShape::Sphere { radius: 0.5 };
    let cast = |from, to| shape_cast_solids(ball, from, to, LAYER_ALL, &unit).unwrap();
    // Face: straight at x = 0 → contact when the center is at x = -0.5.
    let f = cast(v(-3.0, 0.5, 0.5), v(3.0, 0.5, 0.5));
    assert!((f.t - 2.5).abs() < 1e-3 && !f.inside, "{f:?}");
    assert!((f.normal.x + 1.0).abs() < 1e-3);
    assert!((f.fraction - 2.5 / 6.0).abs() < 1e-3);
    // Edge (x = 0, z = 0, along Y): offset 0.3 in z → x² + 0.09 = 0.25.
    let e = cast(v(-3.0, 0.5, -0.3), v(3.0, 0.5, -0.3));
    assert!((e.t - 2.6).abs() < 1e-3, "{e:?}");
    assert!(
        (e.normal.x + 0.8).abs() < 1e-3 && (e.normal.z + 0.6).abs() < 1e-3,
        "{e:?}"
    );
    // Corner (0, 0, 0): x² + 0.09 + 0.09 = 0.25.
    let c = cast(v(-3.0, -0.3, -0.3), v(3.0, -0.3, -0.3));
    assert!((c.t - (3.0 - 0.07f32.sqrt())).abs() < 1e-3, "{c:?}");
    // A ball passing wide misses; a short cast stops before contact.
    assert!(
        shape_cast_solids(
            ball,
            v(-3.0, 0.5, -0.6),
            v(3.0, 0.5, -0.6),
            LAYER_ALL,
            &unit
        )
        .is_none()
    );
    assert!(
        shape_cast_solids(ball, v(-3.0, 0.5, 0.5), v(-1.0, 0.5, 0.5), LAYER_ALL, &unit).is_none()
    );
    // Starting inside reports it, with the way out.
    let i = cast(v(-0.2, 0.5, 0.5), v(3.0, 0.5, 0.5));
    assert!(i.inside && i.t == 0.0 && i.normal.x < -0.99, "{i:?}");
}

#[test]
fn capsule_and_box_casts_land_on_a_floor() {
    let floor = [Solid::new(
        Aabb::new(v(-5.0, -1.0, -5.0), v(5.0, 0.0, 5.0)),
        1,
    )];
    let from = v(0.0, 3.0, 0.0);
    let to = v(0.0, -1.0, 0.0);
    let cap = CastShape::Capsule {
        radius: 0.5,
        height: 2.0,
    };
    let h = shape_cast_solids(cap, from, to, LAYER_ALL, &floor).unwrap();
    assert!(
        (h.t - 2.0).abs() < 1e-3 && h.normal.y > 0.99,
        "capsule {h:?}"
    );
    let b = CastShape::Box {
        half: v(0.3, 0.9, 0.3),
    };
    let h = shape_cast_solids(b, from, to, LAYER_ALL, &floor).unwrap();
    assert!(
        (h.t - 2.1).abs() < 1e-3 && h.normal == v(0.0, 1.0, 0.0),
        "box {h:?}"
    );
    // Falling past a ledge's corner: the box's corner still clips it, the
    // round shape (0.57 from the edge, radius 0.5) slips by.
    let ledge = [Solid::new(Aabb::new(v(0.0, 0.0, 0.0), v(2.0, 1.0, 2.0)), 1)];
    let round = CastShape::Sphere { radius: 0.5 };
    let sq = CastShape::Box {
        half: v(0.5, 0.5, 0.5),
    };
    let (f, t) = (v(-0.4, 2.0, -0.4), v(-0.4, 0.0, -0.4));
    assert!(shape_cast_solids(round, f, t, LAYER_ALL, &ledge).is_none());
    assert!(shape_cast_solids(sq, f, t, LAYER_ALL, &ledge).is_some());
}

#[test]
fn layers_filter_every_query() {
    let solids = [
        Solid::new(Aabb::new(v(0.0, 0.0, 0.0), v(1.0, 1.0, 1.0)), 1),
        Solid::new(Aabb::new(v(3.0, 0.0, 0.0), v(4.0, 1.0, 1.0)), 2),
    ];
    let (mut s, mut r) = (vec![], vec![]);
    let g = build(&solids, &mut s, &mut r);
    let o = v(-1.0, 0.5, 0.5);
    let d = v(1.0, 0.0, 0.0);
    assert_eq!(g.raycast(o, d, 10.0, LAYER_ALL).unwrap().index, 0);
    assert_eq!(g.raycast(o, d, 10.0, 2).unwrap().index, 1);
    assert!(g.raycast(o, d, 10.0, 4).is_none());
    assert!(
        g.raycast(o, d, 10.0, 0).is_none(),
        "an empty mask sees nothing"
    );
}

#[test]
fn grid_refuses_what_does_not_fit() {
    let solids = venue();
    assert!(StaticWorld::size(&[], 4.0).is_none());
    assert!(StaticWorld::size(&solids, 0.0).is_none());
    let size = StaticWorld::size(&solids, 16.0).unwrap();
    let (mut s, mut r) = (vec![0u16; size.starts], vec![0u16; size.refs - 1]);
    assert!(StaticWorld::build(&solids, 16.0, &mut s, &mut r).is_none());
}

#[test]
fn triggers_report_enter_stay_exit_in_order() {
    let zones = [
        Solid::new(Aabb::new(v(0.0, 0.0, 0.0), v(2.0, 2.0, 2.0)), 1),
        Solid::new(Aabb::new(v(10.0, 0.0, 0.0), v(12.0, 2.0, 2.0)), 1),
    ];
    let mut t = TriggerTracker::<8>::new();
    let mut ev = [TriggerEvent {
        body: 0,
        trigger: 0,
        phase: TriggerPhase::Stay,
    }; 8];
    let at = |x: f32| Aabb::from_center_half(v(x, 1.0, 1.0), v(0.25, 0.25, 0.25));
    let mut frame = |t: &mut TriggerTracker<8>, bodies: &[(u16, f32)]| {
        t.begin_frame();
        for &(b, x) in bodies {
            t.track(b, &at(x), &zones, LAYER_ALL);
        }
        let n = t.end_frame(&mut ev);
        ev[..n].to_vec()
    };
    let e = |body, trigger, phase| TriggerEvent {
        body,
        trigger,
        phase,
    };
    assert_eq!(frame(&mut t, &[(7, 5.0)]), []);
    assert_eq!(frame(&mut t, &[(7, 1.0)]), [e(7, 0, TriggerPhase::Enter)]);
    assert_eq!(frame(&mut t, &[(7, 1.5)]), [e(7, 0, TriggerPhase::Stay)]);
    // Body 7 walks to zone 1 while body 3 enters zone 0: ordered by body.
    assert_eq!(
        frame(&mut t, &[(7, 11.0), (3, 0.5)]),
        [
            e(3, 0, TriggerPhase::Enter),
            e(7, 0, TriggerPhase::Exit),
            e(7, 1, TriggerPhase::Enter)
        ]
    );
    // A body that vanishes exits everywhere.
    assert_eq!(
        frame(&mut t, &[(3, 0.5)]),
        [e(3, 0, TriggerPhase::Stay), e(7, 1, TriggerPhase::Exit)]
    );
    // A full tracker counts what it drops and keeps the rest consistent.
    let mut small = TriggerTracker::<1>::new();
    small.begin_frame();
    small.touch(1, 0);
    small.touch(2, 0);
    small.touch(1, 0);
    assert_eq!(small.dropped, 1);
    let mut out = [e(0, 0, TriggerPhase::Stay); 4];
    assert_eq!(small.end_frame(&mut out), 1);
}

#[test]
fn trigger_grids_find_zones_like_a_scan() {
    let zones: Vec<Solid> = (0..40)
        .map(|i| {
            let x = i as f32 * 5.0;
            Solid::new(Aabb::new(v(x, 0.0, 0.0), v(x + 3.0, 2.0, 3.0)), 1)
        })
        .collect();
    let (mut s, mut r) = (vec![], vec![]);
    let g = build(&zones, &mut s, &mut r);
    let body = Aabb::from_center_half(v(52.0, 1.0, 1.0), v(1.0, 1.0, 1.0));
    let (mut a, mut b) = (TriggerTracker::<8>::new(), TriggerTracker::<8>::new());
    a.begin_frame();
    b.begin_frame();
    a.track(1, &body, &zones, LAYER_ALL);
    b.track_in(1, &body, &g, LAYER_ALL);
    let (mut ea, mut eb) = (
        [TriggerEvent {
            body: 0,
            trigger: 0,
            phase: TriggerPhase::Stay,
        }; 8],
        [TriggerEvent {
            body: 0,
            trigger: 0,
            phase: TriggerPhase::Stay,
        }; 8],
    );
    let (na, nb) = (a.end_frame(&mut ea), b.end_frame(&mut eb));
    assert_eq!(ea[..na], eb[..nb]);
    assert_eq!(na, 1);
}
