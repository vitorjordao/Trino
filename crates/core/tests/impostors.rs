//! Impostors through the public API: which pre-rendered view a camera
//! sees (by its yaw around the object, the object's own yaw included),
//! that an impostor draws as one billboard of exactly 2 triangles, that a
//! LOD chain mesh → light mesh → impostor → nothing switches at the same
//! distances every time, and that baked `TIMP` tables read back.

mod common;

use common::*;
use trino_core::impostor::{ImpostorError, TIMP_HEADER};
use trino_core::render3d::Camera3;
use trino_core::scene::{self, LOD_RECORD_BYTES};
use trino_core::{
    Color, Impostor, LodLevel, LodPick, Lods, Mat34, ModelId, ModelParams, NO_CELL, Rect, Renderer,
    SceneNode, SpriteId, Tags, Vec3,
};

const TREE: Impostor = Impostor {
    sprite: SpriteId(0x1A),
    views: 8,
    cols: 4,
    tile: 16,
    size: 4.0,
    center: Vec3::new(0.0, 2.0, 0.0),
};

const FULL: ModelId = ModelId(0xF011);
const LIGHT: ModelId = ModelId(0x1167);

fn at_yaw(deg: f32) -> Mat34 {
    Mat34::from_rotation_scale_translation(
        Vec3::new(0.0, deg.to_radians(), 0.0),
        Vec3::ONE,
        Vec3::ZERO,
    )
}

/// An eye `dist` away at world yaw `deg` (0 = +Z, 90 = +X), level with
/// the impostor's center.
fn eye(deg: f32, dist: f32) -> Vec3 {
    let a = deg.to_radians();
    Vec3::new(a.sin() * dist, 2.0, a.cos() * dist)
}

#[test]
fn the_view_follows_the_camera_yaw_around_the_object() {
    let m = Mat34::IDENTITY;
    // Every 45° around an unrotated object is the next view.
    for i in 0..8u8 {
        assert_eq!(TREE.view_for(&m, eye(i as f32 * 45.0, 30.0)), i, "view {i}");
    }
    // Views switch half way between two rendered yaws.
    assert_eq!(TREE.view_for(&m, eye(22.0, 30.0)), 0);
    assert_eq!(TREE.view_for(&m, eye(23.0, 30.0)), 1);
    assert_eq!(TREE.view_for(&m, eye(-22.0, 30.0)), 0);
    assert_eq!(TREE.view_for(&m, eye(-23.0, 30.0)), 7);
    // Height and distance don't matter, only the yaw.
    let high = Vec3::new(0.0, 50.0, 3.0);
    assert_eq!(TREE.view_for(&m, high), 0);
    // A turned object turns its views: rotated 90°, the camera at +X sees
    // what an unrotated one shows a camera at +Z.
    assert_eq!(TREE.view_for(&at_yaw(90.0), eye(90.0, 30.0)), 0);
    assert_eq!(TREE.view_for(&at_yaw(90.0), eye(0.0, 30.0)), 6);
    assert_eq!(TREE.view_for(&at_yaw(-45.0), eye(0.0, 30.0)), 1);
    // One view always answers 0.
    let one = Impostor { views: 1, ..TREE };
    assert_eq!(one.view_for(&m, eye(200.0, 9.0)), 0);
}

#[test]
fn an_impostor_costs_one_billboard_of_two_triangles() {
    let mut r = Recorder::new();
    r.set_camera(&Camera3 {
        eye: eye(90.0, 40.0),
        target: Vec3::new(0.0, 2.0, 0.0),
        fov_y: 0.8,
    });
    r.begin_frame(Color::BLACK);
    let m =
        Mat34::from_rotation_scale_translation(Vec3::ZERO, Vec3::new(2.0, 2.0, 2.0), Vec3::ZERO);
    TREE.draw(&mut r, &m, eye(90.0, 40.0), Color::rgba(200, 255, 255, 255));
    r.end_frame();
    assert_eq!(r.stats().tris, 2, "two triangles");
    let b = r.billboards();
    assert_eq!(b.len(), 1);
    let (sprite, pos, params, visible) = b[0];
    assert!(visible);
    assert_eq!(sprite, TREE.sprite);
    assert_eq!(
        pos,
        Vec3::new(0.0, 4.0, 0.0),
        "the center scales with the object"
    );
    assert!((params.size.x - 8.0).abs() < 1e-4, "{:?}", params.size);
    assert_eq!(
        params.region,
        Some(Rect::new(32.0, 0.0, 16.0, 16.0)),
        "view 2"
    );
    assert_eq!(params.tint, Color::rgba(200, 255, 255, 255));
}

fn node(lod: &'static [LodLevel]) -> SceneNode {
    SceneNode {
        name: "tree",
        tags: Tags::NONE,
        matrix: Mat34::IDENTITY,
        model: Some(FULL),
        params: ModelParams::DEFAULT,
        anim: None,
        collider: None,
        lod: Lods::List(lod),
        sound: None,
        cell: NO_CELL,
        vis: None,
        glow: None,
    }
}

const CHAIN: &[LodLevel] = &[
    LodLevel::model(12.0, Some(LIGHT)),
    LodLevel::impostor(30.0, TREE),
    LodLevel::model(80.0, None),
];

#[test]
fn a_lod_chain_switches_at_the_same_distances_every_time() {
    let n = node(CHAIN);
    let picks = || {
        [0.0, 11.99, 12.0, 29.99, 30.0, 79.99, 80.0, 500.0]
            .map(|d| n.lod_from(Vec3::new(0.0, 0.0, d)))
    };
    let want = [
        LodPick::Model(FULL),
        LodPick::Model(FULL),
        LodPick::Model(LIGHT),
        LodPick::Model(LIGHT),
        LodPick::Impostor(TREE),
        LodPick::Impostor(TREE),
        LodPick::None,
        LodPick::None,
    ];
    assert_eq!(picks(), want);
    assert_eq!(picks(), picks(), "deterministic");
    // `model_from` keeps answering "which mesh": none for an impostor.
    assert_eq!(n.model_from(Vec3::new(0.0, 0.0, 40.0)), None);

    // draw_nodes draws each pick: a mesh up close, one billboard far.
    let mut r = Recorder::new();
    r.add_mesh(FULL, bounded_cube());
    r.add_mesh(LIGHT, bounded_cube());
    for (d, models, billboards) in [(5.0, 1, 0), (20.0, 1, 0), (40.0, 0, 1), (90.0, 0, 0)] {
        let e = Vec3::new(0.0, 2.0, d);
        r.calls.clear();
        r.set_camera(&Camera3 {
            eye: e,
            target: Vec3::new(0.0, 2.0, 0.0),
            fov_y: 0.8,
        });
        r.begin_frame(Color::BLACK);
        scene::draw_nodes(&mut r, &[n], &Mat34::IDENTITY, 0.0, e);
        r.end_frame();
        assert_eq!(r.models().len(), models, "at {d}");
        assert_eq!(r.billboards().len(), billboards, "at {d}");
        if billboards == 1 {
            assert_eq!(r.stats().tris, 2, "the impostor at {d}");
        }
    }
}

#[test]
fn packed_lod_records_carry_the_impostor() {
    let mut packed = Vec::new();
    for l in CHAIN {
        packed.extend_from_slice(&l.distance.to_le_bytes());
        packed.extend_from_slice(&l.model.map_or(0, |m| m.0).to_le_bytes());
        match l.impostor {
            Some(i) => {
                packed.extend_from_slice(&i.sprite.0.to_le_bytes());
                packed.extend_from_slice(&[i.views, i.cols]);
                packed.extend_from_slice(&i.tile.to_le_bytes());
                f32s(&mut packed, &[i.size, i.center.x, i.center.y, i.center.z]);
            }
            None => packed.extend_from_slice(&[0; LOD_RECORD_BYTES - 8]),
        }
    }
    assert_eq!(packed.len(), CHAIN.len() * LOD_RECORD_BYTES);
    let packed: &'static [u8] = Box::leak(packed.into_boxed_slice());
    let lods = Lods::Packed(packed);
    assert!(lods.iter().eq(CHAIN.iter().copied()));
    assert_eq!(lods.pick(Some(FULL), 35.0 * 35.0), LodPick::Impostor(TREE));
}

#[test]
fn timp_tables_read_back_and_bad_ones_are_refused() {
    let mut buf = vec![0u8; TIMP_HEADER + 8 * TREE.views as usize];
    TREE.write(0xDEAD_BEEF, 0.35, &mut buf);
    assert_eq!(Impostor::parse(&buf), Ok(TREE));
    assert_eq!(Impostor::model_hash(&buf), 0xDEAD_BEEF);
    assert_eq!(TREE.atlas_size(), (64, 32));
    for i in 0..TREE.views {
        let r = TREE.region(i);
        assert_eq!((r.size.x, r.size.y), (16.0, 16.0));
        assert!(r.pos.x + r.size.x <= 64.0 && r.pos.y + r.size.y <= 32.0);
    }
    assert_eq!(Impostor::parse(&buf[..10]), Err(ImpostorError::Truncated));
    let mut wide = buf.clone();
    wide[7] = 0; // no columns
    assert_eq!(Impostor::parse(&wide), Err(ImpostorError::BadLayout));
}
