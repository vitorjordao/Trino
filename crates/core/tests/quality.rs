//! What the quality presets change in the engine's content code (issue
//! #55): the LOD bias moves every LOD start (`SceneNode`, `draw_nodes`,
//! `EntityStore::draw` read `Renderer::lod_bias`), the particle scale caps
//! an effect (`ParticleSystem::scale_limit`). At 1 — every console — both
//! change nothing.

mod common;

use common::*;
use trino_core::render3d::{Camera3, DEFAULT_LIGHT};
use trino_core::scene;
use trino_core::{
    Color, Effect, Emitter, EntityStore, LodLevel, LodPick, Lods, Mat34, ModelId, ModelParams,
    NO_CELL, ParticleSystem, QualitySettings, Renderer, SceneNode, SpriteId, Tags, Vec3,
};

const CUBE: ModelId = ModelId(0x51);
const FAR: ModelId = ModelId(0x52);

const LEVELS: &[LodLevel] = &[
    LodLevel::model(10.0, Some(FAR)),
    LodLevel::model(30.0, None),
];

const LAMP: SceneNode = SceneNode {
    name: "lamp",
    tags: Tags::List(&[]),
    matrix: Mat34::IDENTITY,
    model: Some(CUBE),
    params: ModelParams::DEFAULT,
    anim: None,
    collider: None,
    lod: Lods::List(LEVELS),
    sound: None,
    cell: NO_CELL,
    vis: None,
};

fn at(z: f32) -> Vec3 {
    Vec3::new(0.0, 0.0, -z)
}

#[test]
fn the_lod_bias_moves_every_lod_start() {
    // Unbiased: the far model from 10, nothing from 30.
    assert_eq!(LAMP.lod_from(at(12.0)), LodPick::Model(FAR));
    assert_eq!(LAMP.lod_from_biased(at(12.0), 1.0), LAMP.lod_from(at(12.0)));
    // Ultra (1.5): the full model up to 15, the far one up to 45.
    assert_eq!(LAMP.lod_from_biased(at(12.0), 1.5), LodPick::Model(CUBE));
    assert_eq!(LAMP.lod_from_biased(at(40.0), 1.5), LodPick::Model(FAR));
    assert_eq!(LAMP.lod_from_biased(at(46.0), 1.5), LodPick::None);
    // Low (0.6): the far model from 6, nothing from 18.
    assert_eq!(LAMP.lod_from_biased(at(7.0), 0.6), LodPick::Model(FAR));
    assert_eq!(LAMP.lod_from_biased(at(19.0), 0.6), LodPick::None);
    // The levels themselves.
    let lods = Lods::List(LEVELS);
    assert_eq!(
        lods.pick_biased(Some(CUBE), 144.0, 1.5),
        LodPick::Model(CUBE)
    );
    assert_eq!(lods.pick(Some(CUBE), 144.0), LodPick::Model(FAR));
}

fn recorder(bias: f32) -> Recorder {
    let mut r = Recorder::new();
    r.add_mesh(CUBE, bounded_cube());
    r.add_mesh(FAR, bounded_cube());
    r.set_camera(&Camera3 {
        eye: at(12.0),
        target: Vec3::ZERO,
        fov_y: 1.0,
    });
    r.set_light(&DEFAULT_LIGHT);
    r.lod_bias = bias;
    r
}

#[test]
fn scene_and_entity_draws_follow_the_renderers_bias() {
    for (bias, want) in [(1.0, FAR), (1.5, CUBE), (0.6, FAR)] {
        let mut r = recorder(bias);
        r.begin_frame(Color::BLACK);
        scene::draw_nodes(&mut r, &[LAMP], &Mat34::IDENTITY, 0.0, at(12.0));
        r.end_frame();
        assert_eq!(r.models()[0].0, want, "draw_nodes at bias {bias}");

        let mut w: EntityStore<2> = EntityStore::new();
        w.spawn_nodes(&[LAMP]);
        let mut r = recorder(bias);
        r.begin_frame(Color::BLACK);
        w.draw(&mut r, &Mat34::IDENTITY, 0.0, at(12.0));
        r.end_frame();
        assert_eq!(r.models()[0].0, want, "EntityStore::draw at bias {bias}");
    }
    // Low's 0.6 culls what 1.0 still draws far away.
    let mut r = recorder(0.6);
    r.begin_frame(Color::BLACK);
    scene::draw_nodes(&mut r, &[LAMP], &Mat34::IDENTITY, 0.0, at(20.0));
    assert!(r.models().is_empty());
}

fn effect(max: u16) -> Effect {
    Effect {
        emitter: Emitter {
            sprite: SpriteId(4),
            rate: 100.0,
            life: (2.0, 2.0),
            ..Emitter::DEFAULT
        },
        max,
    }
}

fn burst(max: u16, scale: f32) -> usize {
    let mut s: ParticleSystem<32> = ParticleSystem::from_effect(&effect(max), 9);
    s.scale_limit(scale);
    s.burst(1000);
    s.len()
}

#[test]
fn the_particle_scale_caps_an_effect() {
    assert_eq!(burst(12, 1.0), 12, "1: the baked cap");
    assert_eq!(burst(12, QualitySettings::LOW.particle_scale), 6);
    assert_eq!(burst(12, QualitySettings::MEDIUM.particle_scale), 9);
    assert_eq!(burst(12, 0.01), 1, "an effect that is on keeps one");
    assert_eq!(burst(0, 4.0), 0, "an effect off stays off");
    assert_eq!(burst(12, 4.0), 32, "never past the system's storage");
    assert_eq!(QualitySettings::LOW.particle_cap(12), 6);
    assert_eq!(
        QualitySettings::fixed(trino_core::QualityPreset::Low).particle_cap(12),
        12
    );
}
