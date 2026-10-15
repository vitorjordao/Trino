//! The authored text formats on disk — scenes (v2, v1 migration),
//! prefabs and materials — and the entity-tree edits the editor and agents
//! make: what goes into a file is what comes back out, and every version
//! or parse problem is a readable error.

mod common;

use trino_core::{Color, Mat34, Shading, Vec3};
use trino_scene::prefab::{apply_overrides, find_by_name_path};
use trino_scene::{
    ColliderComponent, Entity, MaterialDef, ModelComponent, Override, Prefab, SCENE_VERSION, Scene,
    ShadingDef, ShapeDef, SoundComponent, SpriteComponent,
};

#[test]
fn scenes_round_trip_through_files_with_every_component() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/dir/fixture.scene.ron");
    let scene = common::scene();
    scene.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("version: 2"));
    let back = Scene::load(&path).unwrap();
    assert_eq!(back, scene);
    assert_eq!(back.to_ron(), text, "stable text: clean diffs");

    // Defaults keep hand-written files short.
    let short = Scene::from_ron(
        r#"(version: 2, name: "short", entities: [(
            name: "torch",
            sound: Some((path: "sounds/fire")),
            collider: Some(()),
            model: Some((path: "models/torch", anim: Some((clip: "flicker")))),
        )])"#,
    )
    .unwrap();
    let torch = &short.entities[0];
    assert_eq!(torch.sound, Some(SoundComponent::new("sounds/fire")));
    assert_eq!(torch.sound.as_ref().unwrap().radius, 10.0);
    assert_eq!(torch.collider, Some(ColliderComponent::default()));
    assert_eq!(torch.collider.as_ref().unwrap().shape, ShapeDef::Auto);
    assert_eq!(
        torch.model.as_ref().unwrap().anim.as_ref().unwrap().speed,
        1.0
    );
    assert!(short.environment.fog.is_none());
    assert_eq!(short.environment.clear, [24, 26, 40]);
}

#[test]
fn old_new_and_broken_scene_files_get_clear_answers() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = dir.path().join("old.scene.ron");
    std::fs::write(
        &v1,
        r#"(version: 1, name: "old", entities: [
            (name: "a", transform: (pos: (3.0, 4.0), scale: (2.0, 2.0), rotation: 1.5707964)),
            (name: "b"),
        ])"#,
    )
    .unwrap();
    let migrated = Scene::load(&v1).unwrap();
    assert_eq!(migrated.version, SCENE_VERSION);
    let a = &migrated.entities[0];
    assert_eq!(a.transform.pos, [3.0, 4.0, 0.0]);
    assert!((a.transform.rot[2] - 90.0).abs() < 1e-4);
    assert_eq!(
        migrated.entities[1].transform,
        trino_scene::Transform::default()
    );

    for (text, needle) in [
        (r#"(version: 0, name: "x")"#, "no migration path"),
        (r#"(version: 7, name: "x")"#, "newer than this engine"),
        (r#"(version: 1, name: 5)"#, "scene (v1) parse error"),
        (
            r#"(version: 2, name: "x", entities: 3)"#,
            "scene parse error",
        ),
        ("not ron at all", "scene parse error"),
    ] {
        let err = Scene::from_ron(text).unwrap_err();
        assert!(err.contains(needle), "{text}: {err}");
    }
    let err = Scene::load(&dir.path().join("missing.scene.ron")).unwrap_err();
    assert!(err.contains("cannot read"), "{err}");
    std::fs::write(dir.path().join("bad.scene.ron"), "(").unwrap();
    let err = Scene::load(&dir.path().join("bad.scene.ron")).unwrap_err();
    assert!(
        err.contains("bad.scene.ron"),
        "the error names the file: {err}"
    );
    // Saving into a path that cannot be a directory fails cleanly.
    let blocker = dir.path().join("file");
    std::fs::write(&blocker, "").unwrap();
    assert!(Scene::new("x").save(&blocker.join("x.scene.ron")).is_err());
}

#[test]
fn entity_tree_edits_keep_paths_consistent() {
    let mut scene = Scene::new("tree");
    for name in ["a", "b", "c"] {
        scene.insert(None, Entity::new(name));
    }
    assert_eq!(
        scene.insert(Some(&[1]), Entity::new("b1")),
        Some(vec![1, 0])
    );
    assert_eq!(
        scene.insert(Some(&[1, 0]), Entity::new("b1x")),
        Some(vec![1, 0, 0])
    );
    assert_eq!(scene.insert(Some(&[9]), Entity::new("nope")), None);

    let mut order = Vec::new();
    scene.walk(|path, e| order.push((path.to_vec(), e.name.clone())));
    let names: Vec<&str> = order.iter().map(|(_, n)| n.as_str()).collect();
    assert_eq!(names, ["a", "b", "b1", "b1x", "c"], "depth first");
    assert_eq!(order[3].0, vec![1, 0, 0]);

    // Moving an earlier sibling under a later one fixes up the target path.
    assert_eq!(scene.reparent(&[0], Some(&[2])), Some(vec![1, 0]));
    assert_eq!(scene.get(&[1, 0]).unwrap().name, "a");
    assert_eq!(scene.get(&[0]).unwrap().name, "b");
    // Never into its own subtree.
    assert_eq!(scene.reparent(&[0], Some(&[0, 0])), None);
    assert_eq!(scene.reparent(&[5], None), None);

    scene.get_mut(&[0, 0]).unwrap().name = "renamed".into();
    assert_eq!(scene.get(&[0, 0]).unwrap().name, "renamed");
    assert!(scene.get(&[]).is_none() && scene.get(&[0, 7]).is_none());
    assert!(scene.get_mut(&[3]).is_none());
    assert_eq!(scene.siblings_mut(&[0, 0]).unwrap().len(), 1);
    assert_eq!(scene.siblings_mut(&[0]).unwrap().len(), 2);
    assert!(scene.siblings_mut(&[]).is_none());

    assert_eq!(scene.remove(&[0, 0]).unwrap().name, "renamed");
    assert!(scene.remove(&[0, 0]).is_none());
    assert!(scene.remove(&[]).is_none());

    scene.insert(None, Entity::new("c-2"));
    assert_eq!(scene.unique_name("c"), "c-3");
    assert_eq!(scene.unique_name("fresh"), "fresh");
}

#[test]
fn transforms_and_environment_map_onto_the_core_types() {
    let mut e = Entity::new("e").at([1.0, 2.0, 3.0]);
    e.transform.rot = [0.0, 90.0, 0.0];
    e.transform.scale = [2.0, 2.0, 2.0];
    let m: Mat34 = e.transform.matrix();
    let p = m.transform_point(Vec3::new(1.0, 0.0, 0.0));
    // 90° yaw (degrees in the file) turns +x into -z; then scale, then move.
    assert!(
        (p - Vec3::new(1.0, 2.0, 1.0)).dot(p - Vec3::new(1.0, 2.0, 1.0)) < 1e-8,
        "{p:?}"
    );

    let scene = common::scene();
    let cam = scene.environment.core_camera();
    assert_eq!(cam.eye, Vec3::new(0.0, 5.0, -10.0));
    assert!(
        (cam.fov_y - 50f32.to_radians()).abs() < 1e-6,
        "degrees → radians"
    );
    let light = scene.environment.core_light();
    assert_eq!(light.color, Color::rgb(250, 240, 230));
    assert_eq!(light.ambient_color, Color::rgb(100, 110, 120));
    assert_eq!(light.fog.color, Color::rgb(50, 60, 70));
    assert_eq!((light.fog.near, light.fog.far), (5.0, 40.0));
    let plain = Scene::new("plain").environment.core_light();
    assert!(!plain.fog.enabled());
    assert_eq!(plain.dir, trino_core::DEFAULT_LIGHT.dir);

    assert_eq!(
        SpriteComponent::new("sprites/a").sprite_id(),
        trino_core::SpriteId::from_path("sprites/a")
    );
    assert_eq!(
        ModelComponent::new("models/a").model_id(),
        trino_core::ModelId::from_path("models/a")
    );
}

#[test]
fn materials_parse_save_and_become_model_params() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("assets/materials/lava.material.ron");
    let lava = MaterialDef {
        tint: [255, 128, 0, 200],
        shading: ShadingDef::Unlit,
        emissive: [90, 30, 0],
        double_sided: true,
        texture: Some("sprites/lava".into()),
        ..MaterialDef::default()
    };
    lava.save(&path).unwrap();
    assert_eq!(MaterialDef::load(&path).unwrap(), lava);
    let p = lava.params(None);
    assert_eq!(p.tint, Color::rgba(255, 128, 0, 200));
    assert_eq!(p.emissive, Color::rgb(90, 30, 0));
    assert_eq!(p.shading, Shading::Unlit);
    assert!(p.double_sided && p.anim.is_none());
    assert_eq!(
        p.texture,
        Some(trino_core::SpriteId::from_path("sprites/lava"))
    );
    // The entity tint multiplies (rounded).
    let p = lava.params(Some([128, 255, 0, 255]));
    assert_eq!(p.tint, Color::rgba(128, 128, 0, 200));

    // Shading variants map 1:1 and have labels for the editor.
    let cores: Vec<Shading> = ShadingDef::ALL.iter().map(|s| s.core()).collect();
    assert_eq!(cores, [Shading::Gouraud, Shading::Flat, Shading::Unlit]);
    let labels: Vec<&str> = ShadingDef::ALL.iter().map(|s| s.label()).collect();
    assert!(labels[0].starts_with("Gouraud") && labels[2].starts_with("Unlit"));

    // Minimal files get defaults; future versions and junk are refused.
    let min = MaterialDef::from_ron("(version: 1)").unwrap();
    assert_eq!(min, MaterialDef::default());
    assert_eq!(min.params(None).tint, Color::WHITE);
    for (text, needle) in [
        ("(version: 2)", "newer than this engine"),
        ("(tint: (1, 2, 3, 4))", "material parse error"),
    ] {
        assert!(MaterialDef::from_ron(text).unwrap_err().contains(needle));
    }
    let err = MaterialDef::load(&dir.path().join("none.material.ron")).unwrap_err();
    assert!(err.contains("cannot read"));
}

#[test]
fn prefab_files_and_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prefabs/lamp.prefab.ron");
    let mut root = Entity::new("lamp");
    let mut post = Entity::new("post").with_model(ModelComponent::new("models/post"));
    post.children
        .push(Entity::new("bulb").with_model(ModelComponent::new("models/bulb")));
    root.children.push(post);
    root.children.push(Entity::new("marker"));
    let prefab = Prefab::new(root);
    prefab.save(&path).unwrap();
    assert_eq!(Prefab::load(&path).unwrap(), prefab);
    assert!(
        Prefab::from_ron("(version: 9, root: (name: \"x\"))")
            .unwrap_err()
            .contains("newer than this engine")
    );
    assert!(
        Prefab::from_ron("(")
            .unwrap_err()
            .contains("prefab parse error")
    );
    assert!(
        Prefab::load(&dir.path().join("none"))
            .unwrap_err()
            .contains("cannot read")
    );

    let mut tree = prefab.root.clone();
    assert_eq!(find_by_name_path(&mut tree, "").unwrap().name, "lamp");
    assert_eq!(
        find_by_name_path(&mut tree, "post/bulb").unwrap().name,
        "bulb"
    );
    assert!(find_by_name_path(&mut tree, "post/nope").is_none());
    let stale = apply_overrides(
        &mut tree,
        &[
            Override {
                target: "post/bulb".into(),
                material: Some("materials/glow".into()),
                tint: Some([1, 2, 3, 4]),
                visible: Some(false),
            },
            // No model: only visibility applies.
            Override {
                target: "marker".into(),
                tint: Some([9, 9, 9, 9]),
                visible: Some(false),
                ..Override::default()
            },
            Override {
                target: "gone".into(),
                ..Override::default()
            },
        ],
    );
    assert_eq!(stale, ["gone"]);
    let bulb = &tree.children[0].children[0];
    let m = bulb.model.as_ref().unwrap();
    assert_eq!(m.material.as_deref(), Some("materials/glow"));
    assert_eq!(m.tint, Some([1, 2, 3, 4]));
    assert!(!bulb.visible);
    let marker = &tree.children[1];
    assert!(!marker.visible && marker.model.is_none());
}
