//! Physics 2.0 phases 2–4 (#66): the character controller (slopes, steps,
//! moving platforms, one-way platforms, coyote time, jump buffering), mesh
//! colliders (`TCOL` + BVH) and the fixed-point mode, through the public API.

use trino_core::physics::{
    self, Box3, CharacterController, ControllerConfig, Fx, MeshCollider, MeshInstance, MeshQuery,
    MoveOpts, RampDir, Scalar, Surface, SurfaceKind, TcolError, V3, move_body, tcol_bytes,
    write_tcol,
};
use trino_core::{Aabb, LAYER_ALL, Mat34, Solid, StaticWorld, Vec3, World};

fn v(x: f32, y: f32, z: f32) -> V3<f32> {
    V3::new(x, y, z)
}

fn bx(min: V3<f32>, max: V3<f32>) -> Box3<f32> {
    Box3::new(min, max)
}

fn floor() -> Surface<f32> {
    Surface::solid(bx(v(-50.0, -1.0, -50.0), v(50.0, 0.0, 50.0)))
}

/// castle64's player.
fn body() -> ControllerConfig<f32> {
    ControllerConfig::new(v(0.3, 0.55, 0.3), 25.0, 20.0)
}

fn run(
    c: &mut CharacterController<f32>,
    frames: usize,
    vx: f32,
    vz: f32,
    surfaces: &[Surface<f32>],
) -> physics::StepReport<f32> {
    let mut rep = None;
    for _ in 0..frames {
        c.vel.x = vx;
        c.vel.z = vz;
        rep = Some(c.step(1.0 / 60.0, surfaces, &[]));
    }
    rep.unwrap()
}

struct Rng(u32);
impl Rng {
    fn f(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
    fn r(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.f() * (hi - lo)
    }
}

#[test]
fn with_options_off_the_controller_is_move_aabb_bit_for_bit() {
    // castle64's loop, verbatim, against the controller over random block
    // worlds and random input: positions, velocities and contacts equal.
    for seed in 1..40u32 {
        let mut rng = Rng(seed * 7919);
        let mut boxes = vec![Aabb::new(
            Vec3::new(-30.0, -1.0, -30.0),
            Vec3::new(30.0, 0.0, 30.0),
        )];
        for _ in 0..30 {
            let c = Vec3::new(rng.r(-20.0, 20.0), rng.r(0.0, 4.0), rng.r(-20.0, 20.0));
            let h = Vec3::new(rng.r(0.3, 3.0), rng.r(0.2, 1.5), rng.r(0.3, 3.0));
            boxes.push(Aabb::from_center_half(c, h));
        }
        let surfaces: Vec<Surface<f32>> = boxes
            .iter()
            .map(|b| Surface::solid(Box3::from_aabb(b)))
            .collect();
        let half = Vec3::new(0.3, 0.55, 0.3);
        let start = Vec3::new(rng.r(-5.0, 5.0), 8.0, rng.r(-5.0, 5.0));
        let (mut pos, mut vel, mut on_ground, mut standing) = (start, Vec3::ZERO, false, None);
        let mut c = CharacterController::new(body(), start.into());
        for f in 0..900 {
            let dt = [1.0 / 60.0, 1.0 / 30.0, 0.05, 1.0 / 144.0][f % 4];
            let (vx, vz) = (rng.r(-6.0, 6.0), rng.r(-6.0, 6.0));
            let jump = rng.f() < 0.05;
            vel.x = vx;
            vel.z = vz;
            c.vel.x = vx;
            c.vel.z = vz;
            if on_ground && jump {
                vel.y = 9.0;
                on_ground = false;
            }
            c.jump(jump, 9.0);
            let steps = ((dt * 120.0) as i32 + 1).clamp(1, 12);
            let h = dt / steps as f32;
            for _ in 0..steps {
                vel.y = (vel.y - 25.0 * h).max(-20.0);
                let out = physics::move_aabb(pos, half, vel, h, &boxes);
                pos = out.pos;
                vel = out.vel;
                on_ground = out.on_ground;
                standing = out.standing_on;
            }
            let rep = c.step(dt, &surfaces, &[]);
            let p: Vec3 = c.pos.into();
            let cv: Vec3 = c.vel.into();
            assert_eq!(
                (p.x.to_bits(), p.y.to_bits(), p.z.to_bits()),
                (pos.x.to_bits(), pos.y.to_bits(), pos.z.to_bits()),
                "seed {seed} frame {f}"
            );
            assert_eq!(cv, vel, "seed {seed} frame {f}");
            assert_eq!((rep.on_ground, rep.standing_on), (on_ground, standing));
        }
    }
}

fn ramp_deg(deg: f32, len: f32) -> Surface<f32> {
    let rise = len * (deg.to_radians()).tan();
    Surface::ramp(bx(v(0.0, 0.0, -2.0), v(len, rise, 2.0)), RampDir::PosX)
}

#[test]
fn walks_up_and_down_a_30_degree_slope() {
    let ramp = ramp_deg(30.0, 6.0);
    let top = ramp.aabb.max.y;
    let deck = Surface::solid(bx(v(6.0, 0.0, -2.0), v(12.0, top, 2.0)));
    let world = [floor(), ramp, deck];
    let cfg = ControllerConfig {
        snap_down: 0.1,
        ..body().with_max_slope_deg(45.0)
    };
    let mut c = CharacterController::new(cfg, v(-2.0, 0.55, 0.0));
    let mut max_air = 0;
    let mut air = 0;
    for _ in 0..240 {
        c.vel.x = 4.0;
        c.step(1.0 / 60.0, &world, &[]);
        air = if c.on_ground { 0 } else { air + 1 };
        max_air = max_air.max(air);
        if c.pos.x > 8.0 {
            break;
        }
    }
    assert!(c.pos.x > 8.0, "climbed onto the deck: {:?}", c.pos);
    assert!((c.pos.y - (top + 0.55)).abs() < 1e-3, "{:?}", c.pos);
    assert!(
        max_air <= 1,
        "stayed on the slope going up ({max_air} frames in the air)"
    );
    // Mid-slope the normal is the ramp's.
    let mut c = CharacterController::new(cfg, v(3.0, 3.0, 0.0));
    run(&mut c, 60, 0.0, 0.0, &world);
    assert!(c.on_ground && c.standing_on == Some(1));
    assert!(
        (c.ground_normal.y - 30f32.to_radians().cos()).abs() < 1e-3,
        "{:?}",
        c.ground_normal
    );
    // Down again: snapping keeps it grounded every frame.
    let mut c = CharacterController::new(cfg, v(10.0, top + 0.55, 0.0));
    run(&mut c, 2, 0.0, 0.0, &world);
    let mut lost = 0;
    for _ in 0..200 {
        c.vel.x = -4.0;
        let r = c.step(1.0 / 60.0, &world, &[]);
        lost += !r.on_ground as usize;
        if c.pos.x < -1.0 {
            break;
        }
    }
    assert!(
        c.pos.x < -1.0 && (c.pos.y - 0.55).abs() < 1e-3,
        "{:?}",
        c.pos
    );
    assert_eq!(lost, 0, "walking down never left the ground");
    // Without snapping it hops down the slope instead.
    let mut c = CharacterController::new(body(), v(10.0, top + 0.55, 0.0));
    run(&mut c, 2, 0.0, 0.0, &world);
    let mut airborne = 0;
    for _ in 0..200 {
        c.vel.x = -4.0;
        airborne += !c.step(1.0 / 60.0, &world, &[]).on_ground as usize;
    }
    assert!(airborne > 0);
}

#[test]
fn a_50_degree_slope_blocks_and_slides() {
    let world = [floor(), ramp_deg(50.0, 3.0)];
    let cfg = body().with_max_slope_deg(45.0);
    let mut c = CharacterController::new(cfg, v(-2.0, 0.55, 0.0));
    let rep = run(&mut c, 180, 4.0, 0.0, &world);
    assert!(
        c.pos.x < 0.0 && rep.hit_wall,
        "blocked at the foot: {:?}",
        c.pos
    );
    assert!((c.pos.y - 0.55).abs() < 1e-3);
    // Dropped onto it: no footing, slides off downhill.
    let mut c = CharacterController::new(cfg, v(2.0, 6.0, 0.0));
    let mut slid = false;
    for _ in 0..240 {
        let r = c.step(1.0 / 60.0, &world, &[]);
        slid |= r.sliding;
        assert!(!(r.sliding && r.on_ground));
    }
    assert!(slid, "it slid");
    assert!(
        c.pos.x < 0.0 && c.on_ground,
        "ended on the floor below: {:?}",
        c.pos
    );
    // The same ramp is walkable for a body allowed 55°.
    let mut c = CharacterController::new(body().with_max_slope_deg(55.0), v(-2.0, 0.55, 0.0));
    let mut top: f32 = 0.0;
    for _ in 0..180 {
        c.vel.x = 3.0;
        c.step(1.0 / 60.0, &world, &[]);
        top = top.max(c.pos.y);
    }
    assert!(top > 3.5, "climbed to the top: {top}");
}

#[test]
fn climbs_a_0_3_m_step_but_not_a_0_5_m_one() {
    let step = |h: f32| {
        [
            floor(),
            Surface::solid(bx(v(1.0, 0.0, -3.0), v(6.0, h, 3.0))),
        ]
    };
    let cfg = ControllerConfig {
        step_height: 0.35,
        ..body()
    };
    let mut c = CharacterController::new(cfg, v(0.0, 0.55, 0.0));
    let mut stepped = false;
    for _ in 0..90 {
        c.vel.x = 4.0;
        stepped |= c.step(1.0 / 60.0, &step(0.3), &[]).stepped;
    }
    assert!(
        stepped && c.pos.x > 3.0 && (c.pos.y - 0.85).abs() < 1e-3,
        "{:?}",
        c.pos
    );
    let mut c = CharacterController::new(cfg, v(0.0, 0.55, 0.0));
    run(&mut c, 90, 4.0, 0.0, &step(0.5));
    assert!(c.pos.x < 1.0, "0.5 m is a wall: {:?}", c.pos);
    let mut c = CharacterController::new(body(), v(0.0, 0.55, 0.0));
    run(&mut c, 90, 4.0, 0.0, &step(0.3));
    assert!(c.pos.x < 1.0, "steps off: 0.3 m is a wall");
    // A step into a low ceiling is refused.
    let mut low = step(0.3).to_vec();
    low.push(Surface::solid(bx(v(1.0, 1.2, -3.0), v(6.0, 2.0, 3.0))));
    let mut c = CharacterController::new(cfg, v(0.0, 0.55, 0.0));
    run(&mut c, 90, 4.0, 0.0, &low);
    assert!(c.pos.x < 1.0, "no room above the step: {:?}", c.pos);
}

#[test]
fn rides_a_moving_platform_through_step_world() {
    // A lift moving +X at 2 m/s over a pit: the body stands on it and
    // inherits its delta each frame (carry), via the World's dynamics.
    let statics = [Solid::new(
        Aabb::new(Vec3::new(-4.0, -1.0, -4.0), Vec3::new(0.0, 0.0, 4.0)),
        1,
    )];
    let (mut st, mut rf) = (vec![0u16; 64], vec![0u16; 64]);
    let grid = StaticWorld::build(&statics, 4.0, &mut st, &mut rf).unwrap();
    let mut c = CharacterController::new(body(), v(1.0, 2.0, 0.0));
    let lift = |t: f32| {
        Aabb::new(
            Vec3::new(0.0 + 2.0 * t, 0.5, -1.0),
            Vec3::new(2.0 + 2.0 * t, 1.0, 1.0),
        )
    };
    let dt = 1.0 / 60.0;
    let mut t = 0.0;
    let mut offset = None;
    for f in 0..120 {
        let (prev, cur) = (lift(t), lift(t + dt));
        t += dt;
        let dynamics = [Solid::new(cur, 2)];
        let world = World {
            statics: grid,
            dynamics: &dynamics,
        };
        if c.standing_on == Some(1) {
            c.carry(V3::from(cur.min - prev.min));
        }
        let rep = c.step_world(dt, &world, &[]);
        if f > 30 {
            assert_eq!(rep.standing_on, Some(1), "frame {f}: still on the lift");
            // Once aboard, the body keeps its place on the lift.
            let off = c.pos.x - cur.center().x;
            assert!(
                (off - *offset.get_or_insert(off)).abs() < 1e-3,
                "frame {f}: slid {off}"
            );
        }
    }
    assert!(c.pos.x > 4.0, "carried along: {:?}", c.pos);
    assert!((c.pos.y - 1.55).abs() < 1e-3);
}

#[test]
fn passes_up_through_a_one_way_platform_and_lands_on_it() {
    let world = [
        floor(),
        Surface::one_way(bx(v(-2.0, 2.0, -2.0), v(2.0, 2.2, 2.0))),
    ];
    let mut c = CharacterController::new(body(), v(0.0, 0.55, 0.0));
    run(&mut c, 5, 0.0, 0.0, &world);
    assert!(c.on_ground && c.standing_on == Some(0));
    assert!(c.jump(true, 11.0));
    let mut max_y: f32 = 0.0;
    let mut ceiling = false;
    for _ in 0..120 {
        ceiling |= c.step(1.0 / 60.0, &world, &[]).hit_ceiling;
        max_y = max_y.max(c.pos.y);
    }
    assert!(!ceiling && max_y > 2.75, "went through from below: {max_y}");
    assert!(
        c.on_ground && c.standing_on == Some(1),
        "landed on top: {:?}",
        c.pos
    );
    assert!((c.pos.y - 2.75).abs() < 1e-3);
    // Walking sideways through its edge does not bump.
    let mut c = CharacterController::new(body(), v(-4.0, 2.4, 0.0));
    c.vel.y = 0.0;
    let rep = run(
        &mut c,
        3,
        0.0,
        0.0,
        &[Surface::one_way(bx(v(-2.0, 2.0, -2.0), v(2.0, 3.0, 2.0)))],
    );
    assert!(!rep.hit_wall);
}

#[test]
fn coyote_time_and_jump_buffering() {
    let ledge = [
        Surface::solid(bx(v(-5.0, -1.0, -2.0), v(0.0, 0.0, 2.0))),
        Surface::solid(bx(v(-50.0, -9.0, -50.0), v(50.0, -8.0, 50.0))),
    ];
    let cfg = ControllerConfig {
        coyote_time: 0.1,
        jump_buffer: 0.1,
        ..body()
    };
    // Walk off: a jump 3 frames later still works with coyote time.
    for (cfg, works) in [(cfg, true), (body(), false)] {
        let mut c = CharacterController::new(cfg, v(-1.0, 0.55, 0.0));
        run(&mut c, 3, 0.0, 0.0, &ledge);
        let mut frames_off = 0;
        while frames_off < 3 {
            c.vel.x = 4.0;
            c.step(1.0 / 60.0, &ledge, &[]);
            if !c.on_ground {
                frames_off += 1;
            }
        }
        assert_eq!(c.jump(true, 9.0), works, "coyote {works}");
    }
    // Press 4 frames before landing: buffered, jumps on landing.
    for (cfg, works) in [(cfg, true), (body(), false)] {
        let mut c = CharacterController::new(cfg, v(-2.0, 0.9, 0.0));
        let mut jumped = false;
        let mut pressed_at = None;
        for f in 0..60 {
            // Feet 0.35 above the ground at 0 m/s: ~10 frames of fall.
            let press = f == 6;
            if press {
                pressed_at = Some(c.on_ground);
            }
            jumped |= c.jump(press, 9.0);
            c.step(1.0 / 60.0, &ledge, &[]);
        }
        assert_eq!(pressed_at, Some(false), "pressed in the air");
        assert_eq!(jumped, works, "buffer {works}");
    }
}

fn slope_mesh(deg: f32) -> Vec<u8> {
    // A 30° ramp along +X (6 m) then a deck, and a wall at x = 14.
    let rise = 6.0 * deg.to_radians().tan();
    let verts = [
        Vec3::new(0.0, 0.0, -3.0),
        Vec3::new(6.0, rise, -3.0),
        Vec3::new(6.0, rise, 3.0),
        Vec3::new(0.0, 0.0, 3.0),
        Vec3::new(12.0, rise, -3.0),
        Vec3::new(12.0, rise, 3.0),
        Vec3::new(12.0, 0.0, -3.0),
        Vec3::new(12.0, rise + 4.0, -3.0),
        Vec3::new(12.0, rise + 4.0, 3.0),
        Vec3::new(12.0, 0.0, 3.0),
    ];
    let tris = [
        [0u16, 2, 1],
        [0, 3, 2],
        [1, 2, 5],
        [1, 5, 4],
        [6, 7, 8],
        [6, 8, 9],
    ];
    let mut out = vec![0u8; tcol_bytes(verts.len(), tris.len())];
    let mut order = vec![0u32; tris.len()];
    let n = write_tcol(&verts, &tris, &mut order, &mut out).unwrap();
    out.truncate(n);
    out
}

#[test]
fn walks_a_mesh_slope_and_stops_at_a_mesh_wall() {
    let blob = slope_mesh(30.0);
    let mesh = MeshCollider::parse(&blob).unwrap();
    assert_eq!((mesh.vertex_count(), mesh.triangle_count()), (10, 6));
    // Placed 20 m away and turned 90° about Y: its +X is the world's -Z.
    let m = Mat34::from_rotation_scale_translation(
        Vec3::new(0.0, core::f32::consts::FRAC_PI_2, 0.0),
        Vec3::ONE,
        Vec3::new(20.0, 0.0, 0.0),
    );
    let inst = MeshInstance::new(mesh, m);
    let rise = 6.0 * 30f32.to_radians().tan();
    let cfg = ControllerConfig {
        snap_down: 0.1,
        ..body().with_max_slope_deg(45.0)
    };
    let mut c = CharacterController::new(cfg, v(20.0, 0.55, 2.0));
    let world = [floor()];
    let mut walled = false;
    for _ in 0..400 {
        c.vel.z = -4.0;
        walled |= c.step(1.0 / 60.0, &world, &[&inst]).hit_wall;
    }
    let p: Vec3 = c.pos.into();
    assert!((p.y - (rise + 0.55)).abs() < 0.02, "on the deck: {p:?}");
    assert!(
        walled && p.z > -12.0 && p.z < -11.0,
        "stopped by the wall: {p:?}"
    );
    assert!(c.on_ground);
    // A ray from above hits the slope with its normal.
    let hit = inst
        .raycast(Vec3::new(20.0, 10.0, -3.0), Vec3::new(0.0, -1.0, 0.0), 20.0)
        .unwrap();
    assert!(
        (10.0 - hit.t - 3.0 * 30f32.to_radians().tan()).abs() < 1e-3,
        "{hit:?}"
    );
    assert!((hit.normal.y - 30f32.to_radians().cos()).abs() < 1e-3);
    // Support and blocking queries directly.
    let foot = bx(v(19.7, 0.0, -9.3), v(20.3, 1.0, -8.7));
    let s = inst.support(&foot, 10.0, 0.7).unwrap();
    assert!((s.height - rise).abs() < 1e-3);
    assert!(inst.blocks(&bx(v(19.7, 0.5, -12.2), v(20.3, 1.5, -11.8)), 0.7));
    assert!(!inst.blocks(&bx(v(19.7, 4.0, -9.0), v(20.3, 5.0, -8.0)), 0.7));
    let mut tris = [0u32; 8];
    assert_eq!(inst.tris_in(&inst.bounds(), &mut tris), 6);
}

#[test]
fn tcol_blobs_are_validated() {
    let blob = slope_mesh(30.0);
    assert!(MeshCollider::parse(&blob).is_ok());
    assert_eq!(
        MeshCollider::parse(&blob[..10]).unwrap_err(),
        TcolError::Truncated
    );
    let mut bad = blob.clone();
    bad[0] = b'X';
    assert_eq!(MeshCollider::parse(&bad).unwrap_err(), TcolError::Magic);
    let mut bad = blob.clone();
    bad[4] = 9;
    assert_eq!(
        MeshCollider::parse(&bad).unwrap_err(),
        TcolError::Version(9)
    );
    let mut bad = blob.clone();
    // First triangle's first index past the vertices.
    let at = 20 + 10 * 12;
    bad[at] = 200;
    assert_eq!(MeshCollider::parse(&bad).unwrap_err(), TcolError::BadIndex);
    let mut out = [0u8; 8];
    assert!(matches!(
        write_tcol(&[Vec3::ZERO], &[[0, 0, 0]], &mut [0], &mut out),
        Err(TcolError::Buffer(_))
    ));
    // The writer is deterministic, and a big mesh gets a real tree.
    assert_eq!(slope_mesh(30.0), blob);
    let mut verts = vec![];
    let mut tris = vec![];
    for z in 0..20u16 {
        for x in 0..20u16 {
            verts.push(Vec3::new(
                x as f32,
                ((x * 7 + z * 3) % 5) as f32 * 0.1,
                z as f32,
            ));
            if x < 19 && z < 19 {
                let i = z * 20 + x;
                tris.push([i, i + 20, i + 1]);
                tris.push([i + 1, i + 20, i + 21]);
            }
        }
    }
    let mut out = vec![0u8; tcol_bytes(verts.len(), tris.len())];
    let mut order = vec![0u32; tris.len()];
    let n = write_tcol(&verts, &tris, &mut order, &mut out).unwrap();
    let m = MeshCollider::parse(&out[..n]).unwrap();
    assert!(m.node_count() > 100);
    // BVH ray = every-triangle ray.
    let inst = MeshInstance::new(m, Mat34::IDENTITY);
    let mut rng = Rng(5);
    for _ in 0..300 {
        let o = Vec3::new(rng.r(-2.0, 21.0), 3.0, rng.r(-2.0, 21.0));
        let d = Vec3::new(rng.r(-0.5, 0.5), -1.0, rng.r(-0.5, 0.5)).normalized();
        let got = inst.raycast(o, d, 20.0).map(|h| h.t);
        let mut best: Option<f32> = None;
        for t in 0..m.triangle_count() {
            let [a, b, c] = inst.triangle(t);
            // Plane-and-inside reference (both faces).
            let n = (b - a).cross(c - a);
            let den = n.dot(d);
            if den.abs() < 1e-12 {
                continue;
            }
            let tt = n.dot(a - o) / den;
            let p = o + d * tt;
            let inside = [(a, b), (b, c), (c, a)]
                .iter()
                .all(|&(e0, e1)| (e1 - e0).cross(p - e0).dot(n) >= -1e-5);
            if (0.0..=20.0).contains(&tt) && inside && best.is_none_or(|bb| tt < bb) {
                best = Some(tt);
            }
        }
        match (got, best) {
            (Some(g), Some(b)) => assert!((g - b).abs() < 1e-3, "{g} vs {b}"),
            (g, b) => assert_eq!(g.is_some(), b.is_some(), "ray {o:?} {d:?}"),
        }
    }
}

#[test]
fn ramps_are_hit_on_their_slope_by_rays() {
    let ramp = Solid::ramp(
        Aabb::new(Vec3::new(0.0, 0.0, -1.0), Vec3::new(4.0, 2.0, 1.0)),
        1,
        RampDir::PosX,
    );
    let hit = physics::raycast_solids(
        Vec3::new(2.0, 5.0, 0.0),
        Vec3::new(0.0, -1.0, 0.0),
        10.0,
        LAYER_ALL,
        &[ramp],
    )
    .unwrap();
    assert!(
        (hit.t - 4.0).abs() < 1e-4,
        "slope height 1 at x = 2: {hit:?}"
    );
    assert!(hit.normal.y > 0.89 && hit.normal.x < -0.44);
    // Under the high end it is a box's side.
    let side = physics::raycast_solids(
        Vec3::new(6.0, 1.0, 0.0),
        Vec3::new(-1.0, 0.0, 0.0),
        10.0,
        LAYER_ALL,
        &[ramp],
    )
    .unwrap();
    assert!(
        (side.normal.x - 1.0).abs() < 1e-4 && side.normal.y.abs() < 1e-6,
        "{side:?}"
    );
    // Over the low end the ray passes above the slope.
    assert!(
        physics::raycast_solids(
            Vec3::new(-1.0, 0.9, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            1.5,
            LAYER_ALL,
            &[ramp]
        )
        .is_none()
    );
    assert_eq!(ramp.kind, SurfaceKind::Ramp(RampDir::PosX));
}

fn course<S: Scalar>() -> Vec<Surface<S>> {
    let b = |a: [f32; 3], c: [f32; 3]| {
        Box3::new(
            V3::new(S::from_f32(a[0]), S::from_f32(a[1]), S::from_f32(a[2])),
            V3::new(S::from_f32(c[0]), S::from_f32(c[1]), S::from_f32(c[2])),
        )
    };
    vec![
        Surface::solid(b([-20.0, -1.0, -20.0], [20.0, 0.0, 20.0])),
        Surface::ramp(b([2.0, 0.0, -2.0], [6.0, 1.5, 2.0]), RampDir::PosX),
        Surface::solid(b([6.0, 0.0, -2.0], [10.0, 1.5, 2.0])),
        Surface::solid(b([10.0, 1.5, -2.0], [12.0, 1.75, 2.0])),
        Surface::one_way(b([12.0, 3.0, -2.0], [16.0, 3.2, 2.0])),
        Surface::solid(b([-6.0, 0.0, 4.0], [-2.0, 3.0, 8.0])),
    ]
}

/// The course, scripted: walk, climb, step, jump.
fn drive<S: Scalar>(frames: usize) -> Vec<V3<S>> {
    let world = course::<S>();
    let cfg = ControllerConfig {
        step_height: S::from_f32(0.3),
        snap_down: S::from_f32(0.1),
        ..ControllerConfig::new(
            V3::new(S::from_f32(0.3), S::from_f32(0.55), S::from_f32(0.3)),
            S::from_int(25),
            S::from_int(20),
        )
        .with_max_slope_deg(45.0)
    };
    let mut c =
        CharacterController::new(cfg, V3::new(S::from_f32(-1.0), S::from_f32(0.6), S::ZERO));
    let dt = S::from_f32(1.0 / 60.0);
    let mut path = vec![];
    for f in 0..frames {
        c.vel.x = S::from_f32(if f < 290 { 3.0 } else { -2.0 });
        c.vel.z = S::from_f32(if (330..360).contains(&f) { 2.0 } else { 0.0 });
        c.jump(f == 255, S::from_int(9));
        c.step(dt, &world, &[]);
        path.push(c.pos);
    }
    path
}

#[test]
fn fixed_point_follows_f32_within_tolerance() {
    let a = drive::<f32>(400);
    let b = drive::<Fx>(400);
    // Discrete events (a step, a landing) may fall one frame apart: the
    // fixed-point path matches the f32 one within a frame of slack.
    let d = |p: &V3<f32>, q: &V3<Fx>| {
        let q = q.to_f32();
        (p.x - q.x)
            .abs()
            .max((p.y - q.y).abs())
            .max((p.z - q.z).abs())
    };
    let mut worst: f32 = 0.0;
    for (i, p) in a.iter().enumerate() {
        let lo = i.saturating_sub(1);
        let near = (lo..(i + 2).min(b.len()))
            .map(|j| d(p, &b[j]))
            .fold(f32::MAX, f32::min);
        worst = worst.max(near);
    }
    assert!(worst < 0.1, "Fx drifted {worst} m from f32");
    // The f32 run climbed the ramp, stepped the 0.25 m ledge and jumped up
    // through the one-way deck onto it.
    let top = a.iter().map(|p| p.y).fold(0.0, f32::max);
    assert!(
        a.iter().any(|p| (p.y - 2.30).abs() < 1e-3),
        "on the ledge (highest {top})"
    );
}

#[test]
fn fixed_point_is_bit_exact_and_pinned() {
    // The hash of every position bit: the same on every target (the
    // console self-test pins the f32 one).
    let h = |path: &[V3<Fx>]| {
        let mut h: u32 = 0x811c_9dc5;
        for p in path {
            for w in [p.x.0, p.y.0, p.z.0] {
                for b in w.to_le_bytes() {
                    h = (h ^ b as u32).wrapping_mul(0x0100_0193);
                }
            }
        }
        h
    };
    let p = drive::<Fx>(400);
    assert_eq!(h(&p), h(&drive::<Fx>(400)));
    assert_eq!(
        h(&p),
        physics_fx_hash_pin(),
        "Fx course hash moved: {:#010x}",
        h(&p)
    );
}

/// The pinned hash (bless by updating after an intended change).
fn physics_fx_hash_pin() -> u32 {
    0x3fd7_52b5
}

#[test]
fn move_body_handles_layers() {
    let world = [Surface {
        layer: 2,
        ..floor()
    }];
    let o = MoveOpts {
        mask: 1,
        ..MoveOpts::<f32>::plain()
    };
    let out = move_body(
        v(0.0, 0.5, 0.0),
        v(0.3, 0.55, 0.3),
        v(0.0, -5.0, 0.0),
        0.1,
        &world,
        &[],
        &o,
    );
    assert!(!out.on_ground, "layer 2 is not in the mask");
    let out = move_body(
        v(0.0, 0.5, 0.0),
        v(0.3, 0.55, 0.3),
        v(0.0, -5.0, 0.0),
        0.1,
        &world,
        &[],
        &MoveOpts::plain(),
    );
    assert!(out.on_ground);
}
