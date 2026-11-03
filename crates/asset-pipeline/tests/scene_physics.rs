//! Physics 2.1 (issue #115): the scene bake writes every cell's physics and
//! grid into the TSCN (v9), and a v8 blob (no section) loads the same
//! parts by gathering its nodes at load — the migration of ADR 0005 §3.
//! Then the interior's rooms stream their physics with their cells.

use std::path::PathBuf;

use trino_asset_pipeline::bake_scene;
use trino_core::physics::{
    self, CellPart, CellWorld, CharacterController, ControllerConfig, Solid, V3,
};
use trino_core::scene::{TSCN_ENV_PHYSICS_BYTES, TSCN_ENV_V8_BYTES, TSCN_HEADER};
use trino_core::vis::NO_CELL;
use trino_core::{Aabb, BakedScene, Cells};

fn assets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

fn leak(b: Vec<u8>) -> &'static [u8] {
    Box::leak(b.into_boxed_slice())
}

/// The same blob as v8: no physics pointer in the environment, the
/// section left as unreferenced data.
fn as_v8(v9: &[u8]) -> Vec<u8> {
    let mut b = v9.to_vec();
    b[4..6].copy_from_slice(&8u16.to_le_bytes());
    let at = TSCN_HEADER + TSCN_ENV_V8_BYTES;
    b.drain(at..at + TSCN_ENV_PHYSICS_BYTES);
    b
}

const BLANK: CellPart<'static> = CellPart {
    cell: 0,
    base: 0,
    grid: None,
    trigger_base: 0,
    triggers: &[],
};

struct Bufs {
    solids: Vec<Solid>,
    sc: Vec<u8>,
    triggers: Vec<Solid>,
    tc: Vec<u8>,
    meshes: Vec<(u8, u32)>,
    u16s: Vec<u16>,
}

fn bufs() -> Bufs {
    let blank = Solid::new(Aabb::EMPTY, 0);
    Bufs {
        solids: vec![blank; 64],
        sc: vec![0; 64],
        triggers: vec![blank; 16],
        tc: vec![0; 16],
        meshes: vec![(0, 0); 16],
        u16s: vec![0; 4096],
    }
}

#[test]
fn baked_and_gathered_physics_are_the_same_parts() {
    let blob = bake_scene(&assets(), "scenes/interior.scene.ron").unwrap();
    let v9 = BakedScene::parse(leak(blob.clone())).unwrap();
    assert_eq!(v9.version(), 9);
    let section = v9.physics().expect("the bake wrote a physics section");
    let (parts, solids, triggers, meshes, _) = section.counts();
    assert_eq!((parts, solids, triggers, meshes), (4, 7, 1, 0));
    let v8 = BakedScene::parse(leak(as_v8(&blob))).unwrap();
    assert_eq!(v8.version(), 8);
    assert!(v8.physics().is_none());
    assert_eq!(v8.node_count(), v9.node_count());
    for i in 0..v8.node_count() {
        assert_eq!(v8.node(i).name, v9.node(i).name);
        assert_eq!(v8.node(i).matrix, v9.node(i).matrix);
    }
    let (mut a, mut b) = (bufs(), bufs());
    let (mut pa, mut pb) = (vec![BLANK; 8], vec![BLANK; 8]);
    let (baked, _) = physics::load_scene(
        &v9.view(),
        &mut a.solids,
        &mut a.sc,
        &mut a.triggers,
        &mut a.tc,
        &mut a.meshes,
        &mut a.u16s,
        &mut pa,
    )
    .unwrap();
    let (gathered, _) = physics::load_scene(
        &v8.view(),
        &mut b.solids,
        &mut b.sc,
        &mut b.triggers,
        &mut b.tc,
        &mut b.meshes,
        &mut b.u16s,
        &mut pb,
    )
    .unwrap();
    assert_eq!(baked.len(), gathered.len());
    for (x, y) in baked.iter().zip(gathered) {
        assert_eq!(
            (x.cell, x.base, x.trigger_base),
            (y.cell, y.base, y.trigger_base)
        );
        assert_eq!(x.solids(), y.solids());
        assert_eq!(x.triggers, y.triggers);
        assert_eq!(x.grid.map(|g| g.raw()), y.grid.map(|g| g.raw()));
    }
    // Cells 0..2 are the rooms (a floor each, the pad in room B), then the
    // outer walls in no cell.
    let cells: Vec<u8> = baked.iter().map(|p| p.cell).collect();
    assert_eq!(cells, [0, 1, 2, NO_CELL]);
    assert_eq!(baked[1].triggers.len(), 1);
    assert_eq!(baked[3].solids().len(), 4);
}

#[test]
fn the_interior_streams_its_physics_with_its_rooms() {
    let blob = bake_scene(&assets(), "scenes/interior.scene.ron").unwrap();
    let scene = BakedScene::parse(leak(blob)).unwrap();
    let cells = Cells::<8>::from_nodes(scene.nodes());
    let mut b = bufs();
    let mut parts = vec![BLANK; 8];
    let (parts, _) = physics::load_scene(
        &scene.view(),
        &mut b.solids,
        &mut b.sc,
        &mut b.triggers,
        &mut b.tc,
        &mut b.meshes,
        &mut b.u16s,
        &mut parts,
    )
    .unwrap();
    let mut world = CellWorld::new(parts, &[], &[]).unwrap();
    let cfg = ControllerConfig::new(V3::new(0.3, 0.55, 0.3), 25.0, 20.0);
    // Walk east from room A (cell 0) into room B (cell 1), B unloaded: the
    // body falls through B's floor.
    let start = V3::new(4.0, 0.6, 4.0);
    let a = cells.cell_of(start.into()).unwrap();
    assert_eq!(a, 0);
    let walk = |world: &CellWorld, loaded: u64| {
        let mut w = *world;
        w.set_loaded(loaded);
        let mut c = CharacterController::new(cfg, start);
        for _ in 0..150 {
            c.vel.x = 4.0;
            c.step_in(1.0 / 60.0, &w, &[]);
        }
        c
    };
    let fell = walk(&world, 0b001);
    assert!(fell.pos.y < -1.0, "{:?}", fell.pos);
    let walked = walk(&world, 0b011);
    assert!(walked.on_ground && walked.pos.x > 12.0, "{:?}", walked.pos);
    assert_eq!(cells.cell_of(walked.pos.into()), Some(1));
    world.set_loaded(0b111);
    assert!(
        world
            .raycast(
                trino_core::Vec3::new(20.0, 3.0, 4.0),
                trino_core::Vec3::new(0.0, -1.0, 0.0),
                10.0,
                physics::LAYER_ALL
            )
            .is_some()
    );
}
