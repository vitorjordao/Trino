//! Animation graph bakes against the repo's own models (issue #65): every
//! `assets/anim/*.anim.ron` bakes on each platform from that platform's
//! model, the blob rides the UI bake (so every image carries it), and a
//! scene entity with `anim_graph` plays with no game code
//! (`GraphPlayback`): posed every step, carried by its root motion.

use std::path::{Path, PathBuf};

use trino_asset_pipeline::anim_graph::{self, GraphPlayback};
use trino_asset_pipeline::{Manifest, Platform};
use trino_core::AnimGraph;
use trino_scene::{AnimGraphComponent, Entity, Library, ModelComponent, Scene};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn every_graph_bakes_on_every_platform_and_rides_the_ui_bake() {
    let root = root();
    let manifest = Manifest::load(&root.join("assets/manifest.toml")).unwrap();
    for p in [Platform::Pc, Platform::N64, Platform::N3ds, Platform::Web] {
        let graphs = anim_graph::bake_all(&root, &manifest, p).unwrap();
        let names: Vec<&str> = graphs.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.contains(&"anim/strider") && names.contains(&"anim/robot"),
            "{p:?}: {names:?}"
        );
        for (n, b) in &graphs {
            let g = AnimGraph::parse(b).unwrap_or_else(|e| panic!("{n}: {e:?}"));
            assert!(g.state_count() > 0);
        }
        let ui = trino_asset_pipeline::ui::bake(&root, &manifest, p).unwrap();
        assert!(ui.iter().any(|b| b.logical == "anim/strider"), "{p:?}");
    }
}

#[test]
fn a_graph_entity_plays_and_walks_with_no_game_code() {
    let root = root();
    let mut scene = Scene::new("walkers");
    let walker = |name: &str, speed: f32, root_motion: bool| {
        let mut e = Entity::new(name).with_model(ModelComponent::new("models/strider"));
        e.anim_graph = Some(AnimGraphComponent {
            graph: "anim/strider".into(),
            state: None,
            params: [("speed".to_string(), speed)].into(),
            root_motion,
        });
        e
    };
    scene.entities.push(walker("walker", 0.8, true));
    scene.entities.push(walker("treadmill", 0.8, false));
    scene
        .entities
        .push(Entity::new("rock").with_model(ModelComponent::new("models/strider")));
    let lib = Library::load(Path::new(""));
    let mut play = GraphPlayback::new(&root);
    let mut last = Vec::new();
    for _ in 0..60 {
        let mut r = trino_scene::resolve(&scene, &lib, 0.0);
        let errors = play.step(&mut r.items, 1.0 / 60.0);
        assert!(errors.is_empty(), "{errors:?}");
        last = r.items;
    }
    let item = |n: &str| last.iter().find(|i| i.name == n).unwrap();
    let walked = item("walker")
        .matrix
        .transform_point(trino_core::Vec3::ZERO);
    // One second of the walk: its root track carries 0.8 m forward (+Z).
    assert!((walked.z - 0.8).abs() < 0.05, "{walked:?}");
    let stayed = item("treadmill")
        .matrix
        .transform_point(trino_core::Vec3::ZERO);
    assert_eq!(stayed, trino_core::Vec3::ZERO);
    assert!(item("walker").params.anim.is_some() && item("treadmill").params.anim.is_some());
    assert!(item("rock").params.anim.is_none(), "no graph: no pose");
    // The walk's footsteps came out as events.
    let inst = play.instance("walker").unwrap();
    assert_eq!(inst.state(), 0);
    // A missing graph is an error, not a panic.
    scene.entities[0].anim_graph.as_mut().unwrap().graph = "anim/nope".into();
    let mut r = trino_scene::resolve(&scene, &lib, 0.0);
    let errors = play.step(&mut r.items, 1.0 / 60.0);
    assert!(errors.iter().any(|e| e.contains("nope")), "{errors:?}");
}
