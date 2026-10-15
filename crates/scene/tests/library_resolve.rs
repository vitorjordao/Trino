//! `Library` + `resolve`: a project's materials and prefabs loaded and
//! hot-reloaded from disk, and a scene flattened into the draw list the
//! editor renders (transforms, materials, prefab instances and overrides,
//! colliders, LOD, sound, cells and portals) — including every warning a
//! broken scene produces instead of failing.

mod common;

use std::path::Path;

use trino_core::{Camera3, ClipId, Color, ModelId, Shape, Vec2, Vec3};
use trino_scene::{
    ColliderComponent, DrawItem, Entity, Library, MaterialDef, Override, Prefab, PrefabInstance,
    Resolved, Scene, ShadingDef, SpriteComponent, VisComponent, resolve,
};

fn item<'a>(r: &'a Resolved, name: &str) -> &'a DrawItem {
    r.items
        .iter()
        .find(|i| i.name == name)
        .unwrap_or_else(|| panic!("no item `{name}`"))
}

fn near(a: Vec3, b: Vec3) -> bool {
    let d = a - b;
    d.dot(d) < 1e-8
}

#[test]
fn resolve_flattens_every_component() {
    let scene = common::scene();
    let lib = common::library();
    let r = resolve(&scene, &lib, 2.0);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert!(
        r.items.iter().all(|i| i.name != "hidden"),
        "invisible: skipped"
    );

    // Materials: the entity tint multiplies the material's.
    let ground = item(&r, "ground");
    assert_eq!(ground.material.as_deref(), Some("materials/stone"));
    assert_eq!(ground.params.tint, Color::rgba(100, 100, 50, 255));
    assert_eq!(ground.owner, vec![0]);
    assert!(!ground.from_prefab);

    // Animation clips play at time × speed.
    let hero = item(&r, "hero");
    let anim = hero.params.anim.unwrap();
    assert_eq!(anim.clip, ClipId::from_name("walk"));
    assert_eq!(anim.time, 3.0);
    assert!(matches!(
        hero.collider.unwrap().shape,
        Shape::Capsule {
            radius: 0.4,
            height: 1.8
        }
    ));

    // Children compose with their parent; LOD levels are sorted and pick
    // the model by camera distance.
    let hat = item(&r, "hero/hat");
    assert_eq!(hat.owner, vec![1, 0]);
    let at = hat.matrix.transform_point(Vec3::ZERO);
    assert!(near(at, Vec3::new(1.0, 2.0, 2.0)), "{at:?}");
    let distances: Vec<f32> = hat.lod.iter().map(|l| l.distance).collect();
    assert_eq!(distances, [10.0, 30.0]);
    let from = |z: f32| hat.model_from(at + Vec3::new(0.0, 0.0, z));
    assert_eq!(from(1.0), Some(ModelId::from_path("models/hat")));
    assert_eq!(from(15.0), Some(ModelId::from_path("models/hat-far")));
    assert_eq!(from(45.0), None, "culled");
    assert_eq!(
        item(&r, "coin").model_from(Vec3::new(1e4, 0.0, 0.0)),
        Some(ModelId::from_path("models/coin")),
        "no LOD: always the base model"
    );

    // Auto colliders take the model's bounds (box center = offset).
    let c = item(&r, "crate").collider.unwrap();
    assert_eq!(
        c.shape,
        Shape::Box {
            half: Vec3::new(0.5, 0.5, 0.5)
        }
    );
    assert!(near(c.offset, Vec3::new(0.0, 0.5, 0.0)));
    let coin = item(&r, "coin");
    assert!(coin.collider.unwrap().trigger);
    let sound = coin.sound.unwrap();
    assert_eq!(sound.sound, trino_core::SoundId::from_path("sounds/chime"));
    assert_eq!((sound.radius, sound.volume), (4.0, 0.8));

    // Prefab parts: owned by the instance, flagged, overrides applied.
    let tree_at = scene
        .entities
        .iter()
        .position(|e| e.name == "tree-1")
        .unwrap();
    let leaves = item(&r, "tree-1/tree/trunk/leaves");
    assert_eq!(leaves.owner, vec![tree_at]);
    assert!(leaves.from_prefab);
    assert_eq!(leaves.params.tint, Color::rgba(10, 200, 10, 255));
    assert!(near(
        leaves.matrix.transform_point(Vec3::ZERO),
        Vec3::new(-5.0, 2.0, 0.0)
    ));

    // 2D-only sprites stay editor-side.
    let label = item(&r, "label");
    assert!(label.sprite.is_some() && !label.exported());
    assert!(item(&r, "spawn").exported(), "markers are exported");

    // Cells and portals: from the house, looking at the doorway, the yard
    // is visible; looking away, only the house.
    let cells = r.cells();
    assert_eq!((cells.cell_count(), cells.portal_count()), (2, 1));
    let screen = Vec2::new(320.0, 240.0);
    let eye = Vec3::new(20.0, 2.0, 0.0);
    let toward = Camera3 {
        eye,
        target: Vec3::new(30.0, 2.0, 0.0),
        fov_y: 1.0,
    };
    let away = Camera3 {
        target: Vec3::new(10.0, 2.0, 0.0),
        ..toward
    };
    assert_eq!(r.visible_cells(&toward, screen), 0b11);
    assert_eq!(r.visible_cells(&away, screen), 0b01);
    // A scene without cells sees everything.
    let open = resolve(&Scene::new("open"), &lib, 0.0);
    assert_eq!(open.visible_cells(&toward, screen), u64::MAX);
}

#[test]
fn sprites_accumulate_their_parents_2d_transform() {
    let mut scene = Scene::new("hud");
    let mut panel = Entity::new("panel").at([10.0, 10.0, 0.0]);
    panel.transform.scale = [2.0, 3.0, 1.0];
    panel.transform.rot = [0.0, 0.0, 90.0];
    let mut icon = Entity::new("icon").at([5.0, 1.0, 0.0]);
    icon.sprite = Some(SpriteComponent {
        flip_x: true,
        tint: [1, 2, 3, 4],
        ..SpriteComponent::new("sprites/icon")
    });
    panel.children.push(icon);
    scene.entities.push(panel);
    let r = resolve(&scene, &Library::default(), 0.0);
    let s = item(&r, "panel/icon").sprite.clone().unwrap();
    assert_eq!(s.pos, [20.0, 13.0]);
    assert_eq!(s.scale, [2.0, 3.0]);
    assert!((s.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    assert_eq!(s.id, trino_core::SpriteId::from_path("sprites/icon"));
    assert!(s.flip_x && !s.flip_y);
    assert_eq!(s.tint, [1, 2, 3, 4]);
}

#[test]
fn broken_scenes_resolve_with_warnings_instead_of_failing() {
    let mut lib = Library::default();
    // Two prefabs that instance each other, and one that instances itself.
    let mut a = Entity::new("a-root");
    a.prefab = Some(PrefabInstance::new("prefabs/b"));
    let mut b = Entity::new("b-root").with_model(common::model("models/b"));
    b.prefab = Some(PrefabInstance::new("prefabs/a"));
    let mut selfish = Entity::new("me");
    selfish.prefab = Some(PrefabInstance::new("prefabs/self"));
    lib.prefabs.insert("prefabs/a".into(), Prefab::new(a));
    lib.prefabs.insert("prefabs/b".into(), Prefab::new(b));
    lib.prefabs
        .insert("prefabs/self".into(), Prefab::new(selfish));

    let mut scene = Scene::new("broken");
    let mut loop_ = Entity::new("loop");
    loop_.prefab = Some(PrefabInstance::new("prefabs/a"));
    scene.entities.push(loop_);
    let mut me = Entity::new("mirror");
    me.prefab = Some(PrefabInstance::new("prefabs/self"));
    scene.entities.push(me);
    let mut ghost = Entity::new("ghost");
    ghost.prefab = Some(PrefabInstance::new("prefabs/missing"));
    scene.entities.push(ghost);
    let mut stale = Entity::new("stale");
    let mut inst = PrefabInstance::new("prefabs/b");
    inst.overrides.push(Override {
        target: "renamed".into(),
        visible: Some(false),
        ..Override::default()
    });
    stale.prefab = Some(inst);
    scene.entities.push(stale);
    scene.entities.push(
        Entity::new("rough").with_model(trino_scene::ModelComponent {
            material: Some("materials/nope".into()),
            ..common::model("models/rough")
        }),
    );
    let mut boxless = Entity::new("boxless").with_model(common::model("models/unbaked"));
    boxless.collider = Some(ColliderComponent::default());
    scene.entities.push(boxless);
    let mut door = Entity::new("door");
    door.vis = Some(VisComponent::Portal {
        size: [1.0; 3],
        a: "hall".into(),
        b: "nowhere".into(),
    });
    scene.entities.push(door);
    let mut hall = Entity::new("hall");
    hall.vis = Some(VisComponent::Cell { size: [4.0; 3] });
    scene.entities.push(hall);
    // More cells than the visibility mask holds (the hall is cell 0).
    for i in 0..64 {
        let mut c = Entity::new(format!("room{i}"));
        c.vis = Some(VisComponent::Cell { size: [1.0; 3] });
        scene.entities.push(c);
    }

    let r = resolve(&scene, &lib, 0.0);
    let has = |needle: &str| r.warnings.iter().any(|w| w.contains(needle));
    assert!(has("prefab cycle through `prefabs/a`"), "{:?}", r.warnings);
    assert!(has("prefab cycle through `prefabs/self`"));
    assert!(has("ghost: unknown prefab `prefabs/missing`"));
    assert!(has("override target `renamed` not found in `prefabs/b`"));
    assert!(has("rough: unknown material `materials/nope`"));
    assert!(has(
        "boxless: `Auto` collider needs a model with known bounds"
    ));
    assert!(has("door: portal names unknown cell `nowhere`"));
    assert!(has("room63: more than 64 cells"));

    // Degraded, not dropped: the cycle expands once, the unknown material
    // falls back to the default surface, the collider to a unit box, the
    // bad portal loses its volume.
    assert!(r.items.iter().any(|i| i.name == "loop/a-root/b-root"));
    assert_eq!(item(&r, "rough").params.tint, Color::WHITE);
    assert_eq!(
        item(&r, "boxless").collider.unwrap().shape,
        Shape::Box {
            half: Vec3::new(0.5, 0.5, 0.5)
        }
    );
    assert!(item(&r, "door").vis.is_none());
    assert!(item(&r, "room63").vis.is_none());
    assert!(item(&r, "room62").vis.is_some());
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn the_library_loads_and_hot_reloads_project_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let wood = Library::material_file(root, "materials/wood");
    assert!(wood.ends_with("assets/materials/wood.material.ron"));
    MaterialDef {
        tint: [150, 100, 50, 255],
        ..MaterialDef::default()
    }
    .save(&wood)
    .unwrap();
    let tree_file = Library::prefab_file(root, "prefabs/nature/tree");
    assert!(tree_file.ends_with("prefabs/nature/tree.prefab.ron"));
    Prefab::new(Entity::new("tree")).save(&tree_file).unwrap();
    write(&root.join("assets/materials/bad.material.ron"), "(oops");
    write(&root.join("prefabs/notes.txt"), "not a prefab");

    let mut lib = Library::load(root);
    assert_eq!(lib.materials.len(), 1);
    assert_eq!(
        lib.prefabs.keys().collect::<Vec<_>>(),
        ["prefabs/nature/tree"]
    );
    assert_eq!(lib.errors.len(), 1);
    assert!(
        lib.errors[0].contains("bad.material.ron"),
        "{:?}",
        lib.errors
    );

    // `./` prefixes don't matter to the watcher's paths.
    let dotted = root.join("./assets/materials/wood.material.ron");
    assert_eq!(
        Library::logical_for_file(root, &dotted).as_deref(),
        Some("materials/wood")
    );
    assert_eq!(
        Library::logical_for_file(root, &root.join("scenes/x.scene.ron")),
        None
    );

    // An edit is picked up; a broken save keeps the last good version.
    MaterialDef {
        shading: ShadingDef::Unlit,
        ..MaterialDef::default()
    }
    .save(&wood)
    .unwrap();
    assert_eq!(
        lib.reload_file(root, &wood).unwrap().as_deref(),
        Some("materials/wood")
    );
    assert_eq!(lib.materials["materials/wood"].shading, ShadingDef::Unlit);
    write(&wood, "(version: 1, tint: oops)");
    assert!(lib.reload_file(root, &wood).is_err());
    assert_eq!(lib.materials["materials/wood"].shading, ShadingDef::Unlit);

    // A new prefab appears, a deleted file disappears.
    let lamp = Library::prefab_file(root, "prefabs/lamp");
    Prefab::new(Entity::new("lamp").with_model(common::model("models/lamp")))
        .save(&lamp)
        .unwrap();
    assert_eq!(
        lib.reload_file(root, &lamp).unwrap().as_deref(),
        Some("prefabs/lamp")
    );
    assert_eq!(lib.prefabs["prefabs/lamp"].root.name, "lamp");
    std::fs::remove_file(&wood).unwrap();
    assert_eq!(
        lib.reload_file(root, &wood).unwrap().as_deref(),
        Some("materials/wood")
    );
    assert!(!lib.materials.contains_key("materials/wood"));
    // Files outside the library are ignored.
    assert_eq!(
        lib.reload_file(root, &root.join("prefabs/notes.txt"))
            .unwrap(),
        None
    );
}

#[test]
fn prefabs_can_be_made_applied_unpacked_and_nested() {
    let mut lib = common::library();
    let mut scene = Scene::new("s");
    let mut house = Entity::new("house").at([3.0, 0.0, 0.0]);
    house
        .children
        .push(Entity::new("roof").with_model(common::model("models/roof")));
    let mut garden = Entity::new("garden").at([0.0, 0.0, 4.0]);
    garden.prefab = Some(PrefabInstance::new("prefabs/tree"));
    house.children.push(garden);
    scene.entities.push(house);
    let before = resolve(&scene, &lib, 0.0);

    // Make a prefab of the house: the scene keeps an instance in place and
    // resolves to the same world matrices — including the nested tree.
    let prefab = lib.make_prefab(&mut scene, &[0], "prefabs/house").unwrap();
    assert_eq!(prefab.root.transform, trino_scene::Transform::default());
    assert_eq!(
        scene.entities[0].prefab.as_ref().unwrap().path,
        "prefabs/house"
    );
    let after = resolve(&scene, &lib, 0.0);
    assert!(after.warnings.is_empty(), "{:?}", after.warnings);
    let models = |r: &Resolved| -> Vec<(String, [f32; 3])> {
        r.items
            .iter()
            .filter(|i| i.model.is_some())
            .map(|i| {
                let p = i.matrix.transform_point(Vec3::ZERO);
                (i.model.as_ref().unwrap().1.clone(), [p.x, p.y, p.z])
            })
            .collect()
    };
    assert_eq!(models(&after), models(&before));
    assert!(after.items.iter().all(|i| i.owner == vec![0]));

    // Prefab files round-trip.
    let reread = Prefab::from_ron(&prefab.to_ron()).unwrap();
    assert_eq!(reread, prefab);

    // Apply an instance override into the prefab itself.
    scene.entities[0]
        .prefab
        .as_mut()
        .unwrap()
        .overrides
        .push(Override {
            target: "roof".into(),
            material: Some("materials/glow".into()),
            ..Override::default()
        });
    let (logical, applied, stale) = lib.apply_overrides_to_prefab(&mut scene, &[0]).unwrap();
    assert_eq!(logical, "prefabs/house");
    assert!(stale.is_empty());
    assert_eq!(
        applied.root.children[0]
            .model
            .as_ref()
            .unwrap()
            .material
            .as_deref(),
        Some("materials/glow")
    );

    // Unpack: the link becomes real children.
    lib.unpack(&mut scene, &[0]).unwrap();
    assert!(scene.entities[0].prefab.is_none());
    let unpacked = resolve(&scene, &lib, 0.0);
    assert!(
        unpacked
            .items
            .iter()
            .any(|i| i.name == "house/house/roof" && !i.from_prefab)
    );
    assert_eq!(models(&unpacked), models(&before));

    // Error paths leave the scene unchanged.
    assert!(
        lib.unpack(&mut scene, &[0])
            .unwrap_err()
            .contains("not a prefab instance")
    );
    assert!(lib.unpack(&mut scene, &[9]).is_err());
    assert!(lib.make_prefab(&mut scene, &[9], "prefabs/x").is_err());
    assert!(lib.apply_overrides_to_prefab(&mut scene, &[0]).is_err());
    let mut orphan = Entity::new("orphan");
    let mut inst = PrefabInstance::new("prefabs/gone");
    inst.overrides.push(Override::default());
    orphan.prefab = Some(inst);
    scene.entities.push(orphan);
    assert!(
        lib.apply_overrides_to_prefab(&mut scene, &[1])
            .unwrap_err()
            .contains("unknown prefab")
    );
    assert!(lib.unpack(&mut scene, &[1]).is_err());
    assert!(
        scene.entities[1].prefab.is_some(),
        "a failed unpack keeps the link"
    );
    assert!(lib.apply_overrides_to_prefab(&mut scene, &[7]).is_err());
}
