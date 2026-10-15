//! Collision for gameplay: 3D boxes (`Aabb`, `Collider` shapes,
//! `move_aabb`, `raycast`, `overlapping`) and the 2D tilemap path (ASCII
//! level → `Tilemap` → `move_and_collide`), driven like a game loop drives
//! them: fixed steps, gravity, landing, walls and ceilings.

use trino_core::physics::{self, MoveOut, RayHit, SKIN};
use trino_core::tilemap::{TILE_SIZE, Tilemap, TilemapError, tile};
use trino_core::{Aabb, Collider, Mat34, MoveResult, Shape, Vec2, Vec3, move_and_collide};

fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

#[test]
fn aabb_operations() {
    let a = Aabb::from_center_half(v(1.0, 2.0, 3.0), v(1.0, 1.0, 1.0));
    assert_eq!(a, Aabb::new(v(0.0, 1.0, 2.0), v(2.0, 3.0, 4.0)));
    assert_eq!((a.center(), a.half()), (v(1.0, 2.0, 3.0), v(1.0, 1.0, 1.0)));
    let touching = Aabb::new(v(2.0, 1.0, 2.0), v(3.0, 3.0, 4.0));
    assert!(!a.overlaps(&touching), "touching faces do not overlap");
    let inside = Aabb::new(v(1.5, 1.5, 2.5), v(2.5, 2.5, 3.5));
    assert!(a.overlaps(&inside) && inside.overlaps(&a));
    assert!(a.contains(v(2.0, 3.0, 4.0)), "contains is inclusive");
    assert!(!a.contains(v(2.1, 3.0, 4.0)));
    assert_eq!(
        a.union(&touching),
        Aabb::new(v(0.0, 1.0, 2.0), v(3.0, 3.0, 4.0))
    );
    // Scale and translation are exact; a quarter turn about Y swaps X/Z.
    let m = Mat34::from_rotation_scale_translation(Vec3::ZERO, v(2.0, 1.0, 1.0), v(5.0, 0.0, 0.0));
    let unit = Aabb::from_center_half(Vec3::ZERO, v(1.0, 0.5, 0.25));
    assert_eq!(
        unit.transformed(&m),
        Aabb::from_center_half(v(5.0, 0.0, 0.0), v(2.0, 0.5, 0.25))
    );
    let turn = Mat34::from_rotation_scale_translation(
        v(0.0, core::f32::consts::FRAC_PI_2, 0.0),
        Vec3::ONE,
        Vec3::ZERO,
    );
    let t = unit.transformed(&turn).half();
    assert!(
        (t.x - 0.25).abs() < 1e-3 && (t.z - 1.0).abs() < 1e-3,
        "{t:?}"
    );
}

#[test]
fn collider_shapes_become_world_boxes() {
    let ball = Collider {
        shape: Shape::Sphere { radius: 0.5 },
        offset: v(0.0, 0.5, 0.0),
        trigger: false,
    };
    assert_eq!(
        ball.local_aabb(),
        Aabb::new(v(-0.5, 0.0, -0.5), v(0.5, 1.0, 0.5))
    );
    let tall = Collider {
        shape: Shape::Capsule {
            radius: 0.3,
            height: 2.0,
        },
        offset: Vec3::ZERO,
        trigger: true,
    };
    assert_eq!(tall.local_aabb().half(), v(0.3, 1.0, 0.3));
    // A capsule shorter than its diameter is a sphere.
    let squat = Collider {
        shape: Shape::Capsule {
            radius: 0.5,
            height: 0.2,
        },
        ..tall
    };
    assert_eq!(squat.local_aabb().half(), v(0.5, 0.5, 0.5));
    let crate_ = Collider::solid_box(v(1.0, 1.0, 1.0));
    assert!(!crate_.trigger);
    let world =
        Mat34::from_rotation_scale_translation(Vec3::ZERO, v(2.0, 2.0, 2.0), v(0.0, 10.0, 0.0));
    assert_eq!(
        crate_.world_aabb(&world),
        Aabb::new(v(-2.0, 8.0, -2.0), v(2.0, 12.0, 2.0))
    );
}

/// Fixed-step falling onto a floor, walking into a wall, jumping into a
/// ceiling — the loop a 3D game runs every frame.
#[test]
fn move_aabb_lands_slides_and_bonks() {
    let floor = Aabb::new(v(-10.0, -1.0, -10.0), v(10.0, 0.0, 10.0));
    let wall = Aabb::new(v(3.0, 0.0, -10.0), v(4.0, 5.0, 10.0));
    let ceiling = Aabb::new(v(-10.0, 3.0, -10.0), v(-2.0, 4.0, 10.0));
    let solids = [floor, wall, ceiling];
    let half = v(0.25, 0.5, 0.25);
    let dt = 1.0 / 60.0;

    let mut pos = v(0.0, 2.0, 0.0);
    let mut vel = Vec3::ZERO;
    let mut out: MoveOut = physics::move_aabb(pos, half, vel, dt, &solids);
    for _ in 0..120 {
        vel.y -= 20.0 * dt;
        out = physics::move_aabb(pos, half, vel, dt, &solids);
        pos = out.pos;
        vel = out.vel;
    }
    assert!(out.on_ground);
    assert_eq!(out.standing_on, Some(0));
    assert_eq!(pos.y, 0.5, "snapped exactly onto the floor top");
    assert_eq!(vel.y, 0.0);

    // Walk right into the wall: stop flush against it, keep standing.
    vel = v(6.0, 0.0, 0.0);
    let mut hit = false;
    for _ in 0..60 {
        vel.y -= 20.0 * dt;
        out = physics::move_aabb(pos, half, vel, dt, &solids);
        hit |= out.hit_wall;
        pos = out.pos;
        vel = Vec3::new(6.0, out.vel.y, out.vel.z);
    }
    assert!(hit && out.on_ground);
    assert_eq!(pos.x, 3.0 - half.x, "flush against the wall");
    // Walk left into nothing: free movement, no wall.
    let free = physics::move_aabb(pos, half, v(-6.0, 0.0, 0.0), dt, &solids);
    assert!(!free.hit_wall && (free.pos.x - (pos.x - 0.1)).abs() < 1e-5);
    // Moving -X into the wall's right face from beyond it.
    let back = physics::move_aabb(v(4.3, 1.0, 0.0), half, v(-60.0, 0.0, 0.0), dt, &[wall]);
    assert!(back.hit_wall);
    assert_eq!(back.pos.x, 4.0 + half.x);
    assert_eq!(back.vel.x, 0.0);
    // Z walls too.
    let zwall = Aabb::new(v(-5.0, 0.0, 2.0), v(5.0, 5.0, 3.0));
    let z = physics::move_aabb(v(0.0, 1.0, 1.5), half, v(0.0, 0.0, 60.0), dt, &[zwall]);
    assert!(z.hit_wall && z.vel.z == 0.0);
    assert_eq!(z.pos.z, 2.0 - half.z);

    // Jump under the ceiling: bonk, vertical speed cancelled.
    let bonk = physics::move_aabb(v(-5.0, 2.4, 0.0), half, v(0.0, 10.0, 0.0), dt, &solids);
    assert!(bonk.hit_ceiling && !bonk.on_ground);
    assert_eq!(bonk.pos.y, 3.0 - half.y);
    assert_eq!(bonk.vel.y, 0.0);

    // The contact skin: a box resting a hair inside the floor is not a
    // wall hit when it moves sideways.
    let resting = v(0.0, 0.5 - SKIN * 0.5, 0.0);
    let slide = physics::move_aabb(resting, half, v(1.0, 0.0, 0.0), dt, &[floor]);
    assert!(!slide.hit_wall);
}

#[test]
fn raycast_finds_the_nearest_face_and_its_normal() {
    let boxes = [
        Aabb::new(v(4.0, -1.0, -1.0), v(6.0, 1.0, 1.0)),
        Aabb::new(v(9.0, -1.0, -1.0), v(10.0, 1.0, 1.0)),
        Aabb::new(v(-1.0, -5.0, -1.0), v(1.0, -4.0, 1.0)),
    ];
    let hit: RayHit = physics::raycast(Vec3::ZERO, v(1.0, 0.0, 0.0), 100.0, &boxes).unwrap();
    assert_eq!((hit.t, hit.index, hit.normal), (4.0, 0, v(-1.0, 0.0, 0.0)));
    // Ground probe straight down: the top face of the third box.
    let down = physics::raycast(Vec3::ZERO, v(0.0, -1.0, 0.0), 100.0, &boxes).unwrap();
    assert_eq!(
        (down.t, down.index, down.normal),
        (4.0, 2, v(0.0, 1.0, 0.0))
    );
    // Coming from +X back toward the origin hits the far box's +X face.
    let back = physics::raycast(v(20.0, 0.0, 0.0), v(-1.0, 0.0, 0.0), 100.0, &boxes).unwrap();
    assert_eq!(
        (back.t, back.index, back.normal),
        (10.0, 1, v(1.0, 0.0, 0.0))
    );
    // Too short, parallel and outside, or pointing away: no hit.
    assert!(physics::raycast(Vec3::ZERO, v(1.0, 0.0, 0.0), 3.0, &boxes).is_none());
    assert!(physics::raycast(v(0.0, 3.0, 0.0), v(1.0, 0.0, 0.0), 100.0, &boxes).is_none());
    assert!(physics::raycast(Vec3::ZERO, v(0.0, 0.0, 1.0), 100.0, &boxes).is_none());
    // Starting inside a box: t = 0, no face normal.
    let inside = physics::raycast(v(5.0, 0.0, 0.0), v(1.0, 0.0, 0.0), 100.0, &boxes).unwrap();
    assert_eq!(
        (inside.t, inside.index, inside.normal),
        (0.0, 0, Vec3::ZERO)
    );
    // Camera clamp convenience.
    assert_eq!(
        physics::raycast_distance(Vec3::ZERO, v(1.0, 0.0, 0.0), 100.0, &boxes),
        4.0
    );
    assert_eq!(
        physics::raycast_distance(Vec3::ZERO, v(0.0, 1.0, 0.0), 7.5, &boxes),
        7.5
    );
    // Trigger checks.
    let q = Aabb::new(v(5.5, 0.0, 0.0), v(9.5, 0.5, 0.5));
    assert_eq!(physics::overlapping(&q, &boxes).collect::<Vec<_>>(), [0, 1]);
}

const LEVEL: &str = "\
........\n\
..C.....\n\
.....#..\n\
.P...#.F\n\
########";

#[test]
fn tilemap_parses_ascii_levels() {
    let map = Tilemap::parse(LEVEL).unwrap();
    assert_eq!((map.width, map.height), (8, 5));
    assert_eq!(
        map.pixel_size(),
        Vec2::new(8.0 * TILE_SIZE, 5.0 * TILE_SIZE)
    );
    assert_eq!(map.tile(1, 3), tile::SPAWN);
    assert_eq!(map.tile(2, 1), tile::COIN);
    assert_eq!(map.tile(7, 3), tile::FLAG);
    assert!(map.is_solid(0, 4) && map.is_solid(5, 2) && !map.is_solid(1, 3));
    assert_eq!(map.tile(-1, 0), tile::EMPTY, "outside the map is empty");
    assert_eq!(map.tile(0, 99), tile::EMPTY);
    assert_eq!(map.cell_pos(3, 2), Vec2::new(48.0, 32.0));
    let coins: Vec<_> = map.cells().filter(|c| c.2 == tile::COIN).collect();
    assert_eq!(coins, [(2, 1, tile::COIN)]);
    assert_eq!(map.cells().count(), 40);
    // CRLF line endings and a trailing newline parse the same.
    let crlf = "..\r\n#B\r\n";
    let m = Tilemap::parse(crlf).unwrap();
    assert_eq!((m.width, m.height), (2, 2));
    assert!(m.is_solid(1, 1), "bricks are solid");
    assert_eq!(Tilemap::parse("").unwrap_err(), TilemapError::Empty);
    assert_eq!(Tilemap::parse("\n\n").unwrap_err(), TilemapError::Empty);
    assert_eq!(
        Tilemap::parse("...\n..\n...").unwrap_err(),
        TilemapError::NotRectangular {
            row: 1,
            expected: 3,
            found: 2
        }
    );
    assert_eq!(
        Tilemap::parse("...\n...\n.").unwrap_err(),
        TilemapError::NotRectangular {
            row: 2,
            expected: 3,
            found: 1
        }
    );
}

#[test]
fn move_and_collide_runs_a_platformer_step_loop() {
    let map = Tilemap::parse(LEVEL).unwrap();
    let size = Vec2::new(12.0, 14.0);
    // Spawn at P, fall onto the floor (row 4 starts at y = 64).
    let mut pos = map.cell_pos(1, 3);
    let mut r = MoveResult::default();
    for _ in 0..30 {
        r = move_and_collide(&map, pos, size, Vec2::new(0.0, 3.0));
        pos = r.pos;
    }
    assert!(r.on_ground && !r.hit_wall);
    assert_eq!(pos.y, 64.0 - 14.0, "flush on the floor");
    // Run right into the pillar at column 5 (x = 80).
    for _ in 0..40 {
        r = move_and_collide(&map, pos, size, Vec2::new(2.5, 1.0));
        pos = r.pos;
    }
    assert!(r.hit_wall && r.on_ground);
    assert_eq!(pos.x, 80.0 - 12.0);
    // Run left into the map edge? There is no wall: the map is open.
    let left = move_and_collide(&map, Vec2::new(17.0, 50.0), size, Vec2::new(-1.0, 0.0));
    assert!(!left.hit_wall && left.pos.x == 16.0);
    // A huge delta is substepped: no tunneling through the floor.
    let fast = move_and_collide(&map, Vec2::new(20.0, 0.0), size, Vec2::new(0.0, 400.0));
    assert!(fast.on_ground);
    assert_eq!(fast.pos.y, 50.0);
    // A ceiling bonk from below a solid tile.
    let roof = Tilemap::parse("###\n...\n...").unwrap();
    let b = move_and_collide(
        &roof,
        Vec2::new(4.0, 20.0),
        Vec2::new(8.0, 8.0),
        Vec2::new(0.0, -8.0),
    );
    assert!(b.hit_ceiling);
    assert_eq!(b.pos.y, 16.0);
    // Moving left into a wall on the left.
    let walled = Tilemap::parse("#..\n#..\n###").unwrap();
    let w = move_and_collide(
        &walled,
        Vec2::new(20.0, 2.0),
        Vec2::new(8.0, 8.0),
        Vec2::new(-8.0, 0.0),
    );
    assert!(w.hit_wall);
    assert_eq!(w.pos.x, 16.0);
    // An absurd delta is clamped (64 substeps of half a tile).
    let clamp = move_and_collide(
        &roof,
        Vec2::new(0.0, 20.0),
        Vec2::new(1.0, 1.0),
        Vec2::new(10_000.0, 0.0),
    );
    assert!((clamp.pos.x - 512.0).abs() < 1e-2, "{clamp:?}");
}

#[test]
fn raycasts_along_z_and_single_row_tilemaps() {
    let wall = [Aabb::new(v(-1.0, -1.0, 3.0), v(1.0, 1.0, 4.0))];
    let hit = physics::raycast(Vec3::ZERO, v(0.0, 0.0, 1.0), 10.0, &wall).unwrap();
    assert_eq!((hit.t, hit.normal), (3.0, v(0.0, 0.0, -1.0)));
    let back = physics::raycast(v(0.0, 0.0, 9.0), v(0.0, 0.0, -1.0), 10.0, &wall).unwrap();
    assert_eq!((back.t, back.normal), (5.0, v(0.0, 0.0, 1.0)));
    // A one-row level without a trailing newline.
    let row = Tilemap::parse("#.#").unwrap();
    assert_eq!((row.width, row.height), (3, 1));
    assert!(row.is_solid(2, 0) && !row.is_solid(1, 0));
}
