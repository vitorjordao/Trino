//! Scene data on the runtime side: exported `SceneNode` slices and baked
//! `TSCN` blobs (built to the ADR 0005 §3 layout and served through
//! `Platform::blob`) → queries (`find`, `with_tag`, `solids`), distance LOD
//! and `draw_nodes` / `draw_nodes_in` on a renderer that runs the T&L.

mod common;

use common::*;
use trino_core::render3d::{Camera3, DEFAULT_LIGHT, Fog, Light};
use trino_core::scene::{self, SceneError, TSCN_ENV_BYTES, TSCN_HEADER, TSCN_NODE_BYTES};
use trino_core::{
    Aabb, BakedScene, BlobId, ClipId, Collider, Color, LodLevel, Lods, Mat34, ModelId, ModelParams,
    NO_CELL, Platform, Renderer, SceneNode, Shading, Shape, SoundId, SpriteId, Tags, Vec3,
    VisVolume,
};

const CUBE: ModelId = ModelId(0xC0BE);
const CUBE_FAR: ModelId = ModelId(0xFA4);

const fn node(name: &'static str, tags: &'static [&'static str], x: f32, z: f32) -> SceneNode {
    SceneNode {
        name,
        tags: Tags::List(tags),
        matrix: Mat34 {
            m: [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, z]],
        },
        model: Some(CUBE),
        params: ModelParams::DEFAULT,
        anim: None,
        collider: None,
        lod: Lods::NONE,
        sound: None,
        cell: NO_CELL,
        vis: None,
    }
}

const LEVELS: &[LodLevel] = &[
    LodLevel::model(10.0, Some(CUBE_FAR)),
    LodLevel::model(30.0, None),
];

/// An exported level: a crate, a lamp with LOD, a spawn marker, a trigger.
const LEVEL: &[SceneNode] = &[
    SceneNode {
        collider: Some(Collider::solid_box(Vec3::new(0.5, 0.5, 0.5))),
        ..node("yard/crate", &["prop", "solid"], 0.0, 0.0)
    },
    SceneNode {
        lod: Lods::List(LEVELS),
        ..node("yard/lamp", &["prop"], 0.0, 0.0)
    },
    SceneNode {
        model: None,
        ..node("spawn", &["spawn"], 3.0, -2.0)
    },
    SceneNode {
        model: None,
        collider: Some(Collider {
            shape: Shape::Sphere { radius: 1.0 },
            offset: Vec3::new(0.0, 1.0, 0.0),
            trigger: true,
        }),
        ..node("goal", &["goal"], 8.0, 0.0)
    },
];

fn recorder() -> Recorder {
    let mut r = Recorder::new();
    r.add_mesh(CUBE, bounded_cube());
    r.add_mesh(CUBE_FAR, faces_tmdl(&CUBE_FACES[..1], 1.0));
    r
}

#[test]
fn exported_nodes_query_by_name_tag_and_collider() {
    assert_eq!(scene::find(LEVEL, "crate").unwrap().name, "yard/crate");
    assert_eq!(scene::find(LEVEL, "yard/lamp").unwrap().name, "yard/lamp");
    assert!(
        scene::find(LEVEL, "rate").is_none(),
        "leaf names align on `/`"
    );
    assert_eq!(
        scene::find(LEVEL, "spawn").unwrap().position(),
        Vec3::new(3.0, 0.0, -2.0)
    );
    let props: Vec<_> = scene::with_tag(LEVEL, "prop").map(|n| n.name).collect();
    assert_eq!(props, ["yard/crate", "yard/lamp"]);
    assert!(LEVEL[0].has_tag("solid") && !LEVEL[1].has_tag("solid"));

    // Only solid colliders make level collision; the trigger is skipped.
    let mut out = [Aabb::EMPTY; 8];
    assert_eq!(scene::solids(LEVEL, &mut out), 1);
    assert_eq!(
        out[0],
        Aabb::new(Vec3::new(-0.5, -0.5, -0.5), Vec3::new(0.5, 0.5, 0.5))
    );
    let mut none: [Aabb; 0] = [];
    assert_eq!(
        scene::solids(LEVEL, &mut none),
        0,
        "stops at the buffer size"
    );
    let goal = LEVEL[3].world_aabb().unwrap();
    assert_eq!(goal.center(), Vec3::new(8.0, 1.0, 0.0));
    assert_eq!(goal.half(), Vec3::ONE);
    assert_eq!(LEVEL[2].world_aabb(), None);

    // Tags: static lists and baked `\n`-packed strings behave alike.
    let packed = Tags::Packed("prop\nsolid");
    assert!(packed.contains("solid") && !packed.contains("soli"));
    assert_eq!(packed.iter().collect::<Vec<_>>(), ["prop", "solid"]);
    assert!(Tags::NONE.is_empty() && !packed.is_empty());
}

#[test]
fn lod_levels_switch_models_with_distance() {
    let lamp = &LEVEL[1];
    assert_eq!(lamp.model_from(Vec3::new(0.0, 0.0, -5.0)), Some(CUBE));
    assert_eq!(
        lamp.model_from(Vec3::new(0.0, 0.0, -10.0)),
        Some(CUBE_FAR),
        "at 10 exactly"
    );
    assert_eq!(lamp.model_from(Vec3::new(0.0, 0.0, -29.0)), Some(CUBE_FAR));
    assert_eq!(
        lamp.model_from(Vec3::new(0.0, 0.0, -31.0)),
        None,
        "culled far away"
    );
    assert_eq!(
        LEVEL[0].model_from(Vec3::new(0.0, 0.0, -1e4)),
        Some(CUBE),
        "no LOD"
    );
    let lods = Lods::List(LEVELS);
    assert_eq!((lods.len(), lods.is_empty()), (2, false));
    assert_eq!(lods.get(1), Some(LEVELS[1]));
    assert_eq!(lods.get(2), None);
    assert!(Lods::NONE.is_empty());
}

#[test]
fn draw_nodes_draws_model_nodes_with_lod_and_animation() {
    let mut r = recorder();
    let eye = Vec3::new(0.0, 0.0, -5.0);
    r.begin_frame(Color::BLACK);
    scene::draw_nodes(&mut r, LEVEL, &Mat34::IDENTITY, 0.0, eye);
    r.end_frame();
    let m = r.models();
    assert_eq!(m.len(), 2, "markers and triggers draw nothing");
    assert_eq!(m[0].0, CUBE);
    assert_eq!(m[1].0, CUBE);
    assert_eq!(m[0].3, 2, "face-on cube: two triangles");
    assert_eq!(r.stats().tris, 4);

    // From afar the lamp swaps to its cheap level; the root moves all.
    let mut far = recorder();
    far.set_camera(&Camera3 {
        eye: Vec3::new(0.0, 0.0, -20.0),
        ..Camera3::default()
    });
    far.begin_frame(Color::BLACK);
    let root =
        Mat34::from_rotation_scale_translation(Vec3::ZERO, Vec3::ONE, Vec3::new(1.0, 0.0, 0.0));
    scene::draw_nodes(&mut far, LEVEL, &root, 0.0, Vec3::new(0.0, 0.0, -20.0));
    let m = far.models();
    assert_eq!(m.iter().map(|d| d.0).collect::<Vec<_>>(), [CUBE, CUBE_FAR]);
    assert_eq!(m[0].1.m[0][3], 1.0, "root applied to the node matrix");

    // Animated nodes get their clip at time * speed.
    let walker = [SceneNode {
        anim: Some((ClipId::from_name("walk"), 2.0)),
        ..node("walker", &[], 0.0, 0.0)
    }];
    let mut r = recorder();
    r.begin_frame(Color::BLACK);
    scene::draw_nodes(&mut r, &walker, &Mat34::IDENTITY, 1.5, Vec3::ZERO);
    let anim = r.models()[0].2.anim.unwrap();
    assert_eq!((anim.clip, anim.time), (ClipId::from_name("walk"), 3.0));
}

#[test]
fn draw_nodes_in_honors_the_cell_mask() {
    let mut nodes = [
        node("a", &[], 0.0, 0.0),
        node("b", &[], 1.0, 0.0),
        node("sky", &[], 2.0, 0.0),
    ];
    nodes[0].cell = 0;
    nodes[1].cell = 1;
    let mut r = recorder();
    r.begin_frame(Color::BLACK);
    scene::draw_nodes_in(&mut r, &nodes, &Mat34::IDENTITY, 0.0, Vec3::ZERO, 0b01);
    let drawn: Vec<f32> = r.models().iter().map(|m| m.1.m[0][3]).collect();
    assert_eq!(drawn, [0.0, 2.0], "cell 1 hidden; NO_CELL always drawn");
}

fn level_blob() -> &'static [u8] {
    let env = TscnEnv {
        clear: [10, 20, 30],
        camera: Camera3 {
            eye: Vec3::new(0.0, 2.0, -6.0),
            target: Vec3::new(0.0, 0.5, 0.0),
            fov_y: 1.0,
        },
        light: Light {
            dir: Vec3::new(0.0, -1.0, 0.0),
            ambient: 0.5,
            color: Color::rgb(255, 240, 220),
            ambient_color: Color::rgb(90, 90, 120),
            fog: Fog {
                color: Color::rgb(1, 2, 3),
                near: 5.0,
                far: 40.0,
            },
        },
    };
    let mut crate_ = TscnNode::new("yard/crate").at(Vec3::new(0.0, 0.5, 0.0));
    crate_.tags = vec!["prop".into(), "solid".into()];
    crate_.model = Some(CUBE.0);
    crate_.collider = Some((1, [0.5, 0.5, 0.5], [0.0; 3], false));
    crate_.shading = 1;
    crate_.tint = [255, 200, 200, 255];
    crate_.double_sided = true;
    crate_.texture = Some(77);
    crate_.shadow = Some((3, 0.4));
    crate_.receiver = Some((Vec3::new(0.0, 1.0, 0.0), 0.5, 0.5));
    let mut lamp = TscnNode::new("yard/lamp").at(Vec3::new(2.0, 0.0, 0.0));
    lamp.model = Some(CUBE.0);
    lamp.lods = vec![(10.0, CUBE_FAR.0), (30.0, 0)];
    lamp.anim = Some((ClipId::from_name("sway").0, 0.5));
    lamp.shading = 2;
    lamp.emissive = [40, 40, 0, 255];
    lamp.sound = Some((SoundId::from_path("sounds/hum").0, 6.0, 0.7));
    lamp.cell = 0;
    let mut goal = TscnNode::new("goal").at(Vec3::new(8.0, 0.0, 0.0));
    goal.tags = vec!["goal".into()];
    goal.collider = Some((3, [0.4, 1.8, 0.0], [0.0, 0.9, 0.0], true));
    let mut room = TscnNode::new("room").at(Vec3::new(0.0, 1.5, 0.0));
    room.vis = Some((1, 0, 0, [4.0, 1.5, 4.0]));
    let mut door = TscnNode::new("door").at(Vec3::new(4.0, 1.0, 0.0));
    door.vis = Some((2, 0, 1, [0.1, 1.0, 0.5]));
    let mut ball = TscnNode::new("ball").at(Vec3::new(-3.0, 1.0, 0.0));
    ball.collider = Some((2, [0.25, 0.0, 0.0], [0.0; 3], false));
    tscn(&env, &[crate_, lamp, goal, room, door, ball])
}

#[test]
fn baked_scene_blob_reads_every_field() {
    let mut p = MemoryPlatform::new(64);
    let id = BlobId::from_path("scenes/yard");
    p.insert_blob(id, level_blob().to_vec());
    let s = BakedScene::parse(p.blob(id).unwrap()).unwrap();
    assert_eq!(s.node_count(), 6);
    assert_eq!(s.clear(), Color::rgb(10, 20, 30));
    let cam = s.camera();
    assert_eq!(
        (cam.eye, cam.target, cam.fov_y),
        (Vec3::new(0.0, 2.0, -6.0), Vec3::new(0.0, 0.5, 0.0), 1.0)
    );
    let l = s.light();
    assert_eq!(l.dir, Vec3::new(0.0, -1.0, 0.0));
    assert_eq!(
        (l.ambient, l.color, l.ambient_color),
        (0.5, Color::rgb(255, 240, 220), Color::rgb(90, 90, 120))
    );
    assert_eq!(
        l.fog,
        Fog {
            color: Color::rgb(1, 2, 3),
            near: 5.0,
            far: 40.0
        }
    );

    let c = s.find("crate").unwrap();
    assert_eq!(c.name, "yard/crate");
    assert!(c.has_tag("prop") && c.has_tag("solid") && !c.has_tag("goal"));
    assert_eq!(c.position(), Vec3::new(0.0, 0.5, 0.0));
    assert_eq!(c.model, Some(CUBE));
    assert_eq!(c.params.shading, Shading::Flat);
    assert_eq!(c.params.tint, Color::rgba(255, 200, 200, 255));
    assert!(c.params.double_sided);
    assert_eq!(c.params.texture, Some(SpriteId(77)));
    // Shadows (TSCN v5): its cast, and the receiver its top is; the scene's
    // shadows off.
    assert_eq!(
        c.params.shadow,
        trino_core::ShadowCast::Auto { blob_radius: 0.4 }
    );
    let tops: Vec<_> = s.receivers().collect();
    assert_eq!(
        tops,
        [trino_core::ShadowPlane::horizontal(
            Vec3::new(0.0, 1.0, 0.0),
            0.5,
            0.5
        )]
    );
    assert_eq!(s.shadows(), None);
    assert_eq!(
        s.find("lamp").unwrap().params.shadow,
        trino_core::ShadowCast::None
    );
    assert_eq!(
        c.collider.unwrap().shape,
        Shape::Box {
            half: Vec3::new(0.5, 0.5, 0.5)
        }
    );
    assert!(!c.collider.unwrap().trigger);
    assert_eq!(
        (c.anim, c.sound, c.cell, c.vis),
        (None, None, NO_CELL, None)
    );

    let lamp = s.find("yard/lamp").unwrap();
    assert_eq!(lamp.params.shading, Shading::Unlit);
    assert_eq!(lamp.params.emissive, Color::rgba(40, 40, 0, 255));
    assert_eq!(lamp.anim, Some((ClipId::from_name("sway"), 0.5)));
    let snd = lamp.sound.unwrap();
    assert_eq!(
        (snd.sound, snd.radius, snd.volume),
        (SoundId::from_path("sounds/hum"), 6.0, 0.7)
    );
    assert_eq!(lamp.cell, 0);
    assert_eq!(lamp.lod.len(), 2);
    assert_eq!(lamp.model_from(Vec3::new(2.0, 0.0, -5.0)), Some(CUBE));
    assert_eq!(lamp.model_from(Vec3::new(2.0, 0.0, -15.0)), Some(CUBE_FAR));
    assert_eq!(lamp.model_from(Vec3::new(2.0, 0.0, -35.0)), None);

    let goal = s.find("goal").unwrap();
    let gc = goal.collider.unwrap();
    assert!(gc.trigger);
    assert_eq!(
        gc.shape,
        Shape::Capsule {
            radius: 0.4,
            height: 1.8
        }
    );
    assert_eq!(gc.offset, Vec3::new(0.0, 0.9, 0.0));
    assert_eq!(goal.model, None);
    assert_eq!(
        s.find("ball").unwrap().collider.unwrap().shape,
        Shape::Sphere { radius: 0.25 }
    );
    assert_eq!(
        s.find("room").unwrap().vis,
        Some(VisVolume::Cell {
            half: Vec3::new(4.0, 1.5, 4.0)
        })
    );
    assert_eq!(
        s.find("door").unwrap().vis,
        Some(VisVolume::Portal {
            half: Vec3::new(0.1, 1.0, 0.5),
            a: 0,
            b: 1
        })
    );
    assert!(s.find("nothing").is_none());
    assert_eq!(s.nodes().count(), 6);
}

#[test]
fn baked_scene_plays_like_exported_nodes() {
    let s = BakedScene::parse(level_blob()).unwrap();
    let nodes: Vec<SceneNode> = s.nodes().collect();
    // Solids: crate (box) and ball (sphere); the capsule goal is a trigger.
    let mut out = [Aabb::EMPTY; 8];
    assert_eq!(scene::solids(&nodes, &mut out), 2);
    assert_eq!(out[1].center(), Vec3::new(-3.0, 1.0, 0.0));
    assert_eq!(scene::with_tag(&nodes, "goal").count(), 1);

    let mut r = recorder();
    r.set_camera(&s.camera());
    r.set_light(&s.light());
    r.begin_frame(s.clear());
    scene::draw_nodes(&mut r, &nodes, &Mat34::IDENTITY, 2.0, s.camera().eye);
    r.end_frame();
    let m = r.models();
    assert_eq!(m.iter().map(|d| d.0).collect::<Vec<_>>(), [CUBE, CUBE]);
    assert_eq!(m[1].2.anim.unwrap().time, 1.0, "time x speed");
    assert!(r.stats().tris > 0);
    assert_eq!(r.calls[0], Call::Begin(Color::rgb(10, 20, 30)));

    // Only the lamp is in cell 0; hiding cell 0 hides only the lamp.
    r.begin_frame(Color::BLACK);
    scene::draw_nodes_in(&mut r, &nodes, &Mat34::IDENTITY, 0.0, s.camera().eye, 0);
    assert_eq!(r.models().len(), 1);
}

#[test]
fn baked_scene_rejects_bad_blobs() {
    let good = level_blob();
    let leak = |v: Vec<u8>| -> &'static [u8] { Box::leak(v.into_boxed_slice()) };
    assert_eq!(
        BakedScene::parse(&good[..20]).unwrap_err(),
        SceneError::Truncated
    );
    let mut magic = good.to_vec();
    magic[1] = b'X';
    assert_eq!(
        BakedScene::parse(leak(magic)).unwrap_err(),
        SceneError::BadMagic
    );
    // v1 (8-byte LOD records), v2 (no surface), v3 (no water), v4 (no
    // shadows) and future versions need a rebake.
    for v in [1u8, 2, 3, 4, 6] {
        let mut other = good.to_vec();
        other[4] = v;
        assert_eq!(
            BakedScene::parse(leak(other)).unwrap_err(),
            SceneError::UnsupportedVersion(v as u16)
        );
    }
    // More nodes announced than present, and an absurd count (overflow).
    let mut many = good.to_vec();
    many[8..12].copy_from_slice(&7u32.to_le_bytes());
    let many_len = many.len();
    assert!(many_len < TSCN_HEADER + TSCN_ENV_BYTES + 7 * TSCN_NODE_BYTES);
    assert_eq!(
        BakedScene::parse(leak(many)).unwrap_err(),
        SceneError::Truncated
    );
    let mut huge = good.to_vec();
    huge[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        BakedScene::parse(leak(huge)).unwrap_err(),
        SceneError::Truncated
    );
    // A name that is not UTF-8.
    let strings = TSCN_HEADER + TSCN_ENV_BYTES + 6 * TSCN_NODE_BYTES;
    let mut utf = good.to_vec();
    utf[strings] = 0xFF;
    assert_eq!(
        BakedScene::parse(leak(utf)).unwrap_err(),
        SceneError::BadString
    );
    // A tag range and a LOD table past the end of the data.
    let node0 = TSCN_HEADER + TSCN_ENV_BYTES;
    let mut tags = good.to_vec();
    tags[node0 + 8..node0 + 12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        BakedScene::parse(leak(tags)).unwrap_err(),
        SceneError::BadString
    );
    let mut lod = good.to_vec();
    let lod_count = node0 + TSCN_NODE_BYTES + 64 + 56;
    lod[lod_count..lod_count + 4].copy_from_slice(&1000u32.to_le_bytes());
    assert_eq!(
        BakedScene::parse(leak(lod)).unwrap_err(),
        SceneError::BadString
    );
    // An empty scene is fine.
    let empty = tscn(
        &TscnEnv {
            clear: [0; 3],
            camera: Camera3::default(),
            light: DEFAULT_LIGHT,
        },
        &[],
    );
    let e = BakedScene::parse(empty).unwrap();
    assert_eq!((e.node_count(), e.nodes().count()), (0, 0));
    assert_eq!(e.light(), DEFAULT_LIGHT);
}
