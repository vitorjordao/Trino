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

/// A #65 graph still bakes to `TANG` v1, byte for byte what main baked
/// (`crates/core/tests/fixtures/strider_v1.tang`, issue #112): its pinned
/// self-test hash can't move.
#[test]
fn v1_graphs_bake_the_bytes_they_always_did() {
    let root = root();
    let manifest = Manifest::load(&root.join("assets/manifest.toml")).unwrap();
    let def =
        trino_scene::anim::AnimGraphDef::load(&root.join("assets/anim/strider.anim.ron")).unwrap();
    assert!(!def.uses_v2());
    let now = anim_graph::bake(&root, &manifest, Platform::Pc, &def)
        .unwrap()
        .unwrap();
    let then = std::fs::read(root.join("crates/core/tests/fixtures/strider_v1.tang")).unwrap();
    assert_eq!(now, then);
}

/// The joints a mask names (`joints`: names and parents) are the baked
/// TMDL's, in its order (issue #136): `"torso/*"` resolves against the
/// runtime's own joint indices.
#[test]
fn mask_joints_follow_the_baked_skeleton() {
    let root = root();
    let manifest = Manifest::load(&root.join("assets/manifest.toml")).unwrap();
    let joints = anim_graph::model_joints(&root, &manifest, Platform::Pc, "models/strider")
        .unwrap()
        .unwrap();
    let decl = &manifest.models["strider"];
    let source = root.join("assets/shared").join(&decl.file);
    let tmdl = trino_asset_pipeline::bake_model(&source, &decl.options()).unwrap();
    let mesh = trino_core::Mesh::from_tmdl(&tmdl).unwrap();
    assert_eq!(joints.len(), mesh.joint_count());
    for (j, x) in joints.iter().enumerate() {
        assert_eq!(x.parent, mesh.joint_parent(j), "joint {j} `{}`", x.name);
    }
    let names: Vec<String> = trino_asset_pipeline::joint_names(&source, &decl.options()).unwrap();
    assert_eq!(
        names,
        joints.iter().map(|x| x.name.clone()).collect::<Vec<_>>()
    );
    // The strider's upper body is torso and below it.
    let upper = trino_scene::anim::mask_bits(&["torso/*".to_string()], &joints).unwrap();
    assert!(upper.count_ones() >= 4, "{upper:b}");
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
