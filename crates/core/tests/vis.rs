//! Portal/cell visibility end to end: cells and portals authored as scene
//! nodes (exported and baked) → `Cells::from_nodes` → `visible()` from a
//! camera → `draw_nodes_in` skipping hidden rooms before the T&L.

mod common;

use common::*;
use trino_core::render3d::{Camera3, DEFAULT_LIGHT};
use trino_core::scene;
use trino_core::vis::{MAX_CELLS, in_view};
use trino_core::{
    Aabb, BakedScene, Cells, Color, Lods, Mat34, ModelId, ModelParams, NO_CELL, Portal, Renderer,
    SceneNode, Tags, Vec3, VisVolume,
};

const PROP: ModelId = ModelId(21);

const fn marker(x: f32, y: f32, z: f32) -> SceneNode {
    SceneNode {
        name: "",
        tags: Tags::NONE,
        matrix: Mat34 {
            m: [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, y], [0.0, 0.0, 1.0, z]],
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
    }
}

const ROOM_HALF: Vec3 = Vec3::new(2.0, 1.5, 2.0);
const DOOR_HALF: Vec3 = Vec3::new(0.1, 1.0, 0.5);

/// Three 4 m rooms in a row along +X (cells 0, 1, 2), doors at x = 4 and
/// x = 8, one prop per room, and a sky node outside every cell.
const HOUSE: &[SceneNode] = &[
    SceneNode {
        vis: Some(VisVolume::Cell { half: ROOM_HALF }),
        ..marker(2.0, 1.5, 2.0)
    },
    SceneNode {
        vis: Some(VisVolume::Cell { half: ROOM_HALF }),
        ..marker(6.0, 1.5, 2.0)
    },
    SceneNode {
        vis: Some(VisVolume::Cell { half: ROOM_HALF }),
        ..marker(10.0, 1.5, 2.0)
    },
    SceneNode {
        vis: Some(VisVolume::Portal {
            half: DOOR_HALF,
            a: 0,
            b: 1,
        }),
        ..marker(4.0, 1.0, 2.0)
    },
    SceneNode {
        vis: Some(VisVolume::Portal {
            half: DOOR_HALF,
            a: 1,
            b: 2,
        }),
        ..marker(8.0, 1.0, 2.0)
    },
    SceneNode {
        name: "prop0",
        model: Some(PROP),
        cell: 0,
        ..marker(1.0, 0.5, 2.0)
    },
    SceneNode {
        name: "prop1",
        model: Some(PROP),
        cell: 1,
        ..marker(6.0, 0.5, 2.0)
    },
    SceneNode {
        name: "prop2",
        model: Some(PROP),
        cell: 2,
        ..marker(10.0, 0.5, 2.0)
    },
    SceneNode {
        name: "sky",
        model: Some(PROP),
        ..marker(0.0, 30.0, 0.0)
    },
];

fn cam(eye: Vec3, target: Vec3) -> Camera3 {
    Camera3 {
        eye,
        target,
        fov_y: 1.0,
    }
}

fn drawn(cells: &Cells<8>, camera: &Camera3) -> Vec<&'static str> {
    let mask = cells.visible(camera, SCREEN);
    let mut r = Recorder::new();
    r.add_mesh(PROP, bounded_cube());
    r.set_camera(camera);
    r.set_light(&DEFAULT_LIGHT);
    r.begin_frame(Color::BLACK);
    scene::draw_nodes_in(&mut r, HOUSE, &Mat34::IDENTITY, 0.0, camera.eye, mask);
    r.end_frame();
    r.models()
        .iter()
        .map(|m| {
            HOUSE
                .iter()
                .find(|n| n.matrix == m.1)
                .map(|n| n.name)
                .unwrap()
        })
        .collect()
}

#[test]
fn cells_and_portals_come_from_scene_nodes() {
    let c: Cells<8> = Cells::from_nodes(HOUSE);
    assert_eq!((c.cell_count(), c.portal_count()), (3, 2));
    assert_eq!(c.cell_of(Vec3::new(1.0, 1.0, 1.0)), Some(0));
    assert_eq!(c.cell_of(Vec3::new(9.0, 1.0, 3.0)), Some(2));
    assert_eq!(c.cell_of(Vec3::new(-1.0, 1.0, 1.0)), None);
    // Works over an iterator of owned nodes too (baked scenes).
    let owned: Cells<8> = Cells::from_nodes(HOUSE.iter().copied());
    assert_eq!(owned.cell_count(), 3);
}

#[test]
fn looking_down_the_doors_draws_every_room_looking_away_only_ours() {
    let c: Cells<8> = Cells::from_nodes(HOUSE);
    let through = cam(Vec3::new(1.0, 1.0, 2.0), Vec3::new(12.0, 1.0, 2.0));
    assert_eq!(c.visible(&through, SCREEN) & 0b111, 0b111);
    assert_eq!(drawn(&c, &through), ["prop0", "prop1", "prop2", "sky"]);

    let away = cam(Vec3::new(3.0, 1.0, 2.0), Vec3::new(-5.0, 1.0, 2.0));
    assert_eq!(c.visible(&away, SCREEN) & 0b111, 0b001);
    // Only room 0's prop reaches the renderer; the sky (no cell) always does.
    assert_eq!(drawn(&c, &away), ["prop0", "sky"]);

    // From room 1 looking back toward room 0.
    let back = cam(Vec3::new(7.0, 1.0, 2.0), Vec3::new(0.0, 1.0, 2.0));
    assert_eq!(c.visible(&back, SCREEN) & 0b111, 0b011);
    // Outside the house, everything is visible.
    assert_eq!(
        c.visible(&cam(Vec3::new(-10.0, 1.0, 2.0), Vec3::ZERO), SCREEN),
        u64::MAX
    );
}

#[test]
fn baked_cells_match_exported_ones() {
    let env = TscnEnv {
        clear: [0; 3],
        camera: Camera3::default(),
        light: DEFAULT_LIGHT,
    };
    let nodes: Vec<TscnNode> = HOUSE[..5]
        .iter()
        .map(|n| {
            let mut t = TscnNode::new("v").at(n.position());
            t.vis = n.vis.map(|v| match v {
                VisVolume::Cell { half } => (1, 0, 0, [half.x, half.y, half.z]),
                VisVolume::Portal { half, a, b } => (2, a, b, [half.x, half.y, half.z]),
            });
            t
        })
        .collect();
    let baked = BakedScene::parse(tscn(&env, &nodes)).unwrap();
    let from_blob: Cells<8> = Cells::from_nodes(baked.nodes());
    let exported: Cells<8> = Cells::from_nodes(HOUSE);
    assert_eq!(from_blob.cell_count(), exported.cell_count());
    for camera in [
        cam(Vec3::new(1.0, 1.0, 2.0), Vec3::new(12.0, 1.0, 2.0)),
        cam(Vec3::new(3.0, 1.0, 2.0), Vec3::new(-5.0, 1.0, 2.0)),
        cam(Vec3::new(3.0, 1.0, 0.3), Vec3::new(4.0, 1.0, 2.0)),
    ] {
        assert_eq!(
            from_blob.visible(&camera, SCREEN),
            exported.visible(&camera, SCREEN)
        );
    }
}

#[test]
fn capacity_limits_and_conservative_fallbacks() {
    let mut c: Cells<2> = Cells::default();
    for i in 0..MAX_CELLS {
        assert_eq!(c.add_cell(Aabb::EMPTY), Some(i as u8));
    }
    assert_eq!(c.add_cell(Aabb::EMPTY), None, "64 cells max (u64 mask)");
    let door = Portal {
        bounds: Aabb::EMPTY,
        a: 0,
        b: 1,
    };
    assert!(c.add_portal(door) && c.add_portal(door));
    assert!(!c.add_portal(door), "P portals max");
    assert_eq!(c.portal_count(), 2);

    // A portal naming a cell that does not exist is ignored.
    let mut dangling: Cells<4> = Cells::new();
    dangling.add_cell(Aabb::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0)));
    dangling.add_portal(Portal {
        bounds: Aabb::new(Vec3::new(1.5, 0.0, 3.9), Vec3::new(2.5, 2.0, 4.1)),
        a: 0,
        b: 5,
    });
    let look = cam(Vec3::new(2.0, 1.0, 1.0), Vec3::new(2.0, 1.0, 10.0));
    assert_eq!(dangling.visible(&look, SCREEN), 0b1);

    // A hall with 40 doors in view overflows the fixed search stack: the
    // answer is "everything visible", never a missing room.
    let mut hall: Cells<48> = Cells::new();
    hall.add_cell(Aabb::new(
        Vec3::new(-20.0, 0.0, 0.0),
        Vec3::new(20.0, 3.0, 5.0),
    ));
    for i in 0..40 {
        // 15 cm wide alcoves across the 6 m in front of the camera.
        let x = -3.0 + i as f32 * 0.15;
        hall.add_cell(Aabb::new(
            Vec3::new(x, 0.0, 5.0),
            Vec3::new(x + 0.15, 3.0, 9.0),
        ));
        hall.add_portal(Portal {
            bounds: Aabb::new(Vec3::new(x + 0.02, 0.5, 4.9), Vec3::new(x + 0.13, 2.5, 5.1)),
            a: 0,
            b: (i + 1) as u8,
        });
    }
    let view = cam(Vec3::new(0.0, 1.5, 0.5), Vec3::new(0.0, 1.5, 10.0));
    assert_eq!(hall.visible(&view, SCREEN), u64::MAX);

    // A portal box crossing the near plane covers the whole screen.
    let mut close: Cells<2> = Cells::new();
    close.add_cell(Aabb::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0)));
    close.add_cell(Aabb::new(
        Vec3::new(0.0, 0.0, 4.0),
        Vec3::new(4.0, 3.0, 8.0),
    ));
    close.add_portal(Portal {
        bounds: Aabb::new(Vec3::new(1.0, 0.0, 3.0), Vec3::new(3.0, 3.0, 5.0)),
        a: 0,
        b: 1,
    });
    let standing_in_door = cam(Vec3::new(2.0, 1.5, 3.5), Vec3::new(2.0, 1.5, 0.0));
    assert_eq!(close.visible(&standing_in_door, SCREEN) & 0b11, 0b11);

    assert!(
        in_view(NO_CELL, 0) && in_view(200, 0),
        "cells past 63 are always drawn"
    );
    assert!(in_view(5, 1 << 5) && !in_view(5, !(1 << 5)));
}

#[test]
fn hidden_rooms_never_reach_the_renderer() {
    let c: Cells<8> = Cells::from_nodes(HOUSE);
    // In room 0 near its side wall, looking diagonally at the first door:
    // room 1 shows through it, room 2's door is outside that slice.
    let diagonal = cam(Vec3::new(3.0, 1.0, 0.3), Vec3::new(4.0, 1.0, 2.0));
    let mask = c.visible(&diagonal, SCREEN);
    assert_eq!(mask & 0b111, 0b011, "{mask:b}");
    assert_eq!(drawn(&c, &diagonal), ["prop0", "prop1", "sky"]);
    let mut all = Recorder::new();
    all.add_mesh(PROP, bounded_cube());
    all.set_camera(&diagonal);
    all.begin_frame(Color::BLACK);
    scene::draw_nodes(&mut all, HOUSE, &Mat34::IDENTITY, 0.0, diagonal.eye);
    all.end_frame();
    assert_eq!(
        all.stats().models,
        4,
        "without the mask room 2 pays its T&L"
    );
}
