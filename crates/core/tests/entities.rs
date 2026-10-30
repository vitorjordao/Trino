//! Runtime entities: an `EntityStore` filled from exported nodes and from a
//! baked scene, then played like a game frame — generational handles,
//! queries, colliders feeding `move_aabb`, trigger overlaps, positional
//! sound sources feeding `Ambience`, and drawing with LOD and cell masks.

mod common;

use common::*;
use trino_core::physics;
use trino_core::render3d::{Camera3, DEFAULT_LIGHT};
use trino_core::{
    Aabb, Ambience, BakedScene, ClipId, Collider, Color, Entity, EntityId, EntityStore, Listener,
    LodLevel, Lods, Mat34, ModelId, ModelParams, NO_CELL, Renderer, SceneNode, SoundId,
    SoundSource, Tags, Vec3,
};

const CUBE: ModelId = ModelId(11);
const FAR: ModelId = ModelId(12);

const fn at(name: &'static str, tags: &'static [&'static str], x: f32) -> SceneNode {
    SceneNode {
        name,
        tags: Tags::List(tags),
        matrix: Mat34 {
            m: [
                [1.0, 0.0, 0.0, x],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
        },
        model: Some(CUBE),
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

const LODS: &[LodLevel] = &[LodLevel::model(20.0, Some(FAR))];

const NODES: &[SceneNode] = &[
    SceneNode {
        collider: Some(Collider::solid_box(Vec3::new(5.0, 0.5, 5.0))),
        ..at("level/floor", &["solid"], 0.0)
    },
    SceneNode {
        collider: Some(Collider {
            shape: trino_core::Shape::Box {
                half: Vec3::new(0.5, 0.5, 0.5),
            },
            offset: Vec3::ZERO,
            trigger: true,
            layer: trino_core::physics::LAYER_DEFAULT,
            one_way: false,
        }),
        model: None,
        ..at("level/coin", &["coin"], 3.0)
    },
    SceneNode {
        sound: Some(SoundSource {
            sound: SoundId(5),
            radius: 8.0,
            volume: 1.0,
        }),
        lod: Lods::List(LODS),
        anim: Some((ClipId(9), 2.0)),
        cell: 1,
        ..at("level/torch", &["light"], -3.0)
    },
];

#[test]
fn handles_are_generational_and_capacity_is_fixed() {
    let mut w: EntityStore<3> = EntityStore::new();
    assert_eq!((w.capacity(), w.len(), w.is_empty()), (3, 0, true));
    assert_eq!(w.spawn_nodes(NODES), 3);
    assert_eq!(w.len(), 3);
    assert_eq!(w.spawn(Entity::EMPTY), None, "full");
    assert_eq!(w.spawn_nodes(NODES), 0);

    let coin = w.find("coin").unwrap();
    assert_eq!(coin.index(), 1);
    assert!(w.despawn(coin));
    assert!(!w.despawn(coin), "stale handle");
    assert!(w.get(coin).is_none() && w.get_mut(coin).is_none());
    // The slot is reused, but the old handle does not answer to it.
    let new = w
        .spawn(Entity {
            name: "level/key",
            ..Entity::EMPTY
        })
        .unwrap();
    assert_eq!(new.index(), coin.index());
    assert_ne!(new, coin);
    assert!(w.get(coin).is_none());
    assert_eq!(w.get(new).unwrap().name, "level/key");

    let ids: Vec<EntityId> = w.iter().map(|(id, _)| id).collect();
    w.clear();
    assert!(w.is_empty());
    assert!(
        ids.iter().all(|id| w.get(*id).is_none()),
        "clear invalidates every handle"
    );
    // Spawning from any iterator (e.g. a baked scene).
    assert_eq!(w.spawn_iter(NODES.iter().copied()), 3);
    assert_eq!(EntityStore::<2>::default().capacity(), 2);
}

#[test]
fn entities_move_query_and_collide() {
    let mut w: EntityStore<8> = EntityStore::new();
    w.spawn_nodes(NODES);
    let torch = w.find("level/torch").unwrap();
    assert!(w.get(torch).unwrap().is_named("torch"));
    assert!(!w.get(torch).unwrap().is_named("orch"));
    assert_eq!(w.with_tag("light").collect::<Vec<_>>(), [torch]);
    assert_eq!(w.with_tag("nothing").count(), 0);
    {
        let e = w.get_mut(torch).unwrap();
        e.translate(Vec3::new(0.0, 2.0, 0.0));
        assert_eq!(e.position(), Vec3::new(-3.0, 2.0, 0.0));
        e.set_position(Vec3::new(-4.0, 1.0, 0.0));
        e.user = 42;
    }
    assert_eq!(w.get(torch).unwrap().position(), Vec3::new(-4.0, 1.0, 0.0));
    assert_eq!(w.get(torch).unwrap().user, 42);
    for (_, e) in w.iter_mut() {
        e.translate(Vec3::new(0.0, 0.0, 1.0));
    }
    assert_eq!(w.get(torch).unwrap().position().z, 1.0);

    // Solids feed the mover: a player falls onto the floor entity.
    let mut solids = [Aabb::EMPTY; 8];
    // `EntityId` has no public constructor: prefill with any live handle.
    let mut owners = [torch; 8];
    let n = w.solids(&mut solids, Some(&mut owners));
    assert_eq!(n, 1, "the coin is a trigger; the torch has no collider");
    assert_eq!(owners[0], w.find("floor").unwrap());
    let floor_top = solids[0].max.y;
    let mut pos = Vec3::new(0.0, 3.0, 1.0);
    let half = Vec3::new(0.3, 0.5, 0.3);
    let mut vel = Vec3::ZERO;
    let mut on = false;
    for _ in 0..90 {
        vel.y -= 0.5;
        let out = physics::move_aabb(pos, half, vel, 1.0 / 30.0, &solids[..n]);
        pos = out.pos;
        vel = out.vel;
        on = out.on_ground;
    }
    assert!(on);
    assert_eq!(pos.y, floor_top + half.y);

    // Walking through the coin's trigger.
    let player = Aabb::from_center_half(Vec3::new(3.0, 0.5, 1.0), half);
    let coin = w.find("coin").unwrap();
    assert_eq!(w.triggers_overlapping(&player).collect::<Vec<_>>(), [coin]);
    w.get_mut(coin).unwrap().visible = false;
    assert_eq!(
        w.triggers_overlapping(&player).count(),
        0,
        "hidden triggers are inert"
    );
    // Hidden solids drop out of collision too.
    w.get_mut(owners[0]).unwrap().visible = false;
    assert_eq!(w.solids(&mut solids, None), 0);
    assert_eq!(w.solids(&mut [], None), 0);
}

#[test]
fn sound_sources_drive_ambience() {
    let mut w: EntityStore<8> = EntityStore::new();
    w.spawn_nodes(NODES);
    let torch = w.find("torch").unwrap();
    let sources: Vec<_> = w.sound_sources().collect();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].0, torch.index() as u32);
    assert_eq!(sources[0].1, Vec3::new(-3.0, 0.0, 0.0));

    let mut audio = MockAudio::new(4);
    let mut amb: Ambience<2> = Ambience::new();
    let near = Listener::from_camera(&Camera3 {
        eye: Vec3::new(-1.0, 0.0, 0.0),
        target: Vec3::new(-1.0, 0.0, 5.0),
        fov_y: 1.0,
    });
    amb.update(&mut audio, &near, w.sound_sources());
    assert_eq!(audio.voices.len(), 1);
    assert_eq!(audio.voices[0].sound, SoundId(5));
    assert!(audio.voices[0].params.pan < 0.0, "torch on the left");
    // Hiding the entity silences it.
    w.get_mut(torch).unwrap().visible = false;
    assert_eq!(w.sound_sources().count(), 0);
    amb.update(&mut audio, &near, w.sound_sources());
    assert!(audio.voices.is_empty());
}

#[test]
fn draw_applies_lod_animation_visibility_and_cells() {
    let mut w: EntityStore<8> = EntityStore::new();
    w.spawn_nodes(NODES);
    let mut r = Recorder::new();
    r.add_mesh(CUBE, bounded_cube());
    r.add_mesh(FAR, bounded_cube());
    r.set_camera(&Camera3 {
        eye: Vec3::new(0.0, 2.0, -8.0),
        target: Vec3::ZERO,
        fov_y: 1.0,
    });
    r.set_light(&DEFAULT_LIGHT);
    r.begin_frame(Color::BLACK);
    w.draw(&mut r, &Mat34::IDENTITY, 1.5, Vec3::new(0.0, 2.0, -8.0));
    r.end_frame();
    let m = r.models();
    assert_eq!(m.iter().map(|d| d.0).collect::<Vec<_>>(), [CUBE, CUBE]);
    let anim = m[1].2.anim.unwrap();
    assert_eq!((anim.clip, anim.time), (ClipId(9), 3.0), "time x speed");
    assert!(m.iter().all(|d| d.3 > 0), "both on screen");

    // Far away the torch uses its LOD; a root matrix offsets everything.
    let root =
        Mat34::from_rotation_scale_translation(Vec3::ZERO, Vec3::ONE, Vec3::new(0.0, 0.0, 0.0));
    r.begin_frame(Color::BLACK);
    w.draw(&mut r, &root, 0.0, Vec3::new(-3.0, 0.0, -25.0));
    assert_eq!(r.models()[1].0, FAR);

    // The torch is in cell 1: hidden by a mask without it.
    r.begin_frame(Color::BLACK);
    w.draw_in(&mut r, &Mat34::IDENTITY, 0.0, Vec3::ZERO, 0b01);
    assert_eq!(r.models().len(), 1);
    // Invisible entities never draw; entity-set params.anim is kept when
    // the entity has no scene clip.
    let floor = w.find("floor").unwrap();
    w.get_mut(floor).unwrap().visible = false;
    let torch = w.find("torch").unwrap();
    let e = w.get_mut(torch).unwrap();
    e.anim = None;
    e.params.anim = Some(trino_core::AnimRef::new(ClipId(3), 0.25));
    r.begin_frame(Color::BLACK);
    w.draw(&mut r, &Mat34::IDENTITY, 9.0, Vec3::ZERO);
    let m = r.models();
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].2.anim.unwrap().clip, ClipId(3));
    assert_eq!(m[0].2.anim.unwrap().time, 0.25);
}

#[test]
fn a_baked_scene_spawns_into_the_store() {
    let mut crate_ = TscnNode::new("yard/crate").at(Vec3::new(1.0, 0.0, 0.0));
    crate_.model = Some(CUBE.0);
    crate_.collider = Some((1, [0.5, 0.5, 0.5], [0.0; 3], false));
    crate_.tags = vec!["prop".into()];
    let blob = tscn(
        &TscnEnv {
            clear: [0; 3],
            camera: Camera3::default(),
            light: DEFAULT_LIGHT,
        },
        &[crate_, TscnNode::new("spawn").at(Vec3::new(0.0, 1.0, -2.0))],
    );
    let scene = BakedScene::parse(blob).unwrap();
    let mut w: EntityStore<4> = EntityStore::new();
    assert_eq!(w.spawn_iter(scene.nodes()), 2);
    let c = w.find("crate").unwrap();
    assert!(w.get(c).unwrap().has_tag("prop"));
    assert_eq!(
        w.get(c).unwrap().world_aabb(),
        Some(Aabb::new(
            Vec3::new(0.5, -0.5, -0.5),
            Vec3::new(1.5, 0.5, 0.5)
        ))
    );
    let spawn = w.get(w.find("spawn").unwrap()).unwrap();
    assert_eq!(spawn.position(), Vec3::new(0.0, 1.0, -2.0));
    assert_eq!(spawn.model_from(Vec3::ZERO), None);
    assert_eq!(spawn.world_aabb(), None);
}
