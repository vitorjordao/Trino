//! Per-platform variants (`platforms` on an entity) end to end: the RON
//! text both ways, the v2 → v3 scene and v1 → v2 prefab migrations, what
//! `resolve_for` draws on each target, and the two exports (`TSCN` and
//! codegen) choosing the right variant per platform.

mod common;

use trino_core::physics::Shape;
use trino_core::{BakedScene, ModelId, Vec3};
use trino_scene::prefab::PREFAB_VERSION;
use trino_scene::{
    ColliderComponent, Entity, Library, ModelComponent, PlatformVariants, Prefab, PrefabInstance,
    SCENE_VERSION, Scene, ShapeDef, Target, Variant, bake, codegen, resolve, resolve_for,
};

fn leak(bytes: Vec<u8>) -> &'static [u8] {
    Box::leak(bytes.into_boxed_slice())
}

fn id(path: &str) -> ModelId {
    ModelId(trino_core::asset_id(path))
}

/// A tree with a cheaper N64 model and material, a 3DS scale, a crate
/// that is not on the N64, and a decoration hidden on the 3DS.
fn scene() -> Scene {
    let mut s = Scene::new("variants");
    let mut tree = Entity::new("tree").with_model(ModelComponent::new("models/tree"));
    tree.collider = Some(ColliderComponent {
        shape: ShapeDef::Box {
            size: [1.0, 4.0, 1.0],
        },
        ..ColliderComponent::default()
    });
    tree.platforms = PlatformVariants {
        n64: Some(Variant {
            model: Some("models/tree-low".into()),
            material: Some("materials/glow".into()),
            ..Variant::default()
        }),
        n3ds: Some(Variant {
            scale: Some([0.5, 0.5, 0.5]),
            ..Variant::default()
        }),
        ..PlatformVariants::default()
    };
    tree.children.push(
        Entity::new("bird")
            .at([0.0, 4.0, 0.0])
            .with_model(ModelComponent::new("models/bird")),
    );
    let mut crate_ = Entity::new("crate").with_model(ModelComponent::new("models/crate"));
    crate_.collider = Some(ColliderComponent::default());
    crate_.platforms.n64 = Some(Variant {
        disabled: true,
        ..Variant::default()
    });
    crate_
        .children
        .push(Entity::new("lid").with_model(ModelComponent::new("models/lid")));
    let mut flag = Entity::new("flag").with_model(ModelComponent::new("models/flag"));
    flag.platforms.n3ds = Some(Variant {
        visible: Some(false),
        ..Variant::default()
    });
    s.entities.extend([tree, crate_, flag]);
    s
}

fn names(scene: &Scene, lib: &Library, t: Target) -> Vec<String> {
    resolve_for(scene, lib, 0.0, t)
        .items
        .into_iter()
        .map(|i| i.name)
        .collect()
}

#[test]
fn variants_round_trip_through_ron_both_ways() {
    // Struct → text → struct, and the text is stable.
    let s = scene();
    let text = s.to_ron();
    assert!(text.contains("version: 6"), "{text}");
    assert!(text.contains("n64: Some(") && text.contains("n3ds: Some("));
    assert!(
        !text.contains("n64_expanded"),
        "no tier variant, no tier key"
    );
    assert!(text.contains("disabled: true"));
    let back = Scene::from_ron(&text).unwrap();
    assert_eq!(back, s);
    assert_eq!(back.to_ron(), text);

    // Hand-written text (what an agent writes) → struct → text → struct.
    let hand = r#"(
        version: 3,
        name: "hand",
        entities: [(
            name: "rock",
            model: Some((path: "models/rock")),
            platforms: (
                n64: Some((model: Some("models/rock-low"), scale: Some((2.0, 2.0, 2.0)))),
                pc: Some((visible: Some(false))),
            ),
        )],
    )"#;
    let parsed = Scene::from_ron(hand).unwrap();
    let rock = &parsed.entities[0];
    let n64 = rock.platforms.get(Target::N64).unwrap();
    assert_eq!(n64.model.as_deref(), Some("models/rock-low"));
    assert_eq!(n64.scale, Some([2.0, 2.0, 2.0]));
    assert_eq!(rock.platforms.pc.as_ref().unwrap().visible, Some(false));
    assert!(rock.platforms.n3ds.is_none(), "3DS: same as PC");
    let again = Scene::from_ron(&parsed.to_ron()).unwrap();
    assert_eq!(again, parsed);

    // An entity without variants writes no `platforms` block at all.
    let plain = Scene::new("plain");
    let mut plain = plain;
    plain.entities.push(Entity::new("e"));
    assert!(!plain.to_ron().contains("platforms"));
}

#[test]
fn v2_scenes_and_v1_prefabs_migrate() {
    let v2 = r#"(version: 2, name: "old", entities: [(
        name: "house",
        model: Some((path: "models/house")),
        children: [(name: "door")],
    )])"#;
    let scene = Scene::from_ron(v2).unwrap();
    assert_eq!(scene.version, SCENE_VERSION);
    assert_eq!(scene.version, 6);
    assert!(scene.entities[0].platforms.is_empty());
    assert_eq!(scene.entities[0].children[0].name, "door");
    // Saved, it is a v6 file that reads back identically.
    let text = scene.to_ron();
    assert!(text.contains("version: 6"));
    assert_eq!(Scene::from_ron(&text).unwrap(), scene);

    let v1 = r#"(version: 1, root: (name: "lamp", children: [(name: "bulb")]))"#;
    let prefab = Prefab::from_ron(v1).unwrap();
    assert_eq!(prefab.version, PREFAB_VERSION);
    assert!(prefab.to_ron().contains("version: 4"));
    let future = r#"(version: 9, root: (name: "x"))"#;
    assert!(Prefab::from_ron(future).unwrap_err().contains("newer"));
    // The v2 path still reports bad files as parse errors.
    let bad = r#"(version: 2, name: "x", entities: 3)"#;
    assert!(
        Scene::from_ron(bad)
            .unwrap_err()
            .contains("scene parse error")
    );
}

/// The hardware tiers (issue #54, scene v5 / prefab v4): `n64_expanded`
/// and `n3ds_new` round-trip, a v4 file migrates with none, and each tier
/// resolves its own variant over its console's, falling back to it.
#[test]
fn tier_variants_round_trip_migrate_and_fall_back_to_their_console() {
    let hand = r#"(
        version: 5,
        name: "tiers",
        entities: [(
            name: "crowd",
            model: Some((path: "models/crowd")),
            platforms: (
                n64: Some((model: Some("models/crowd-low"))),
                n64_expanded: Some((model: Some("models/crowd-mid"))),
                n3ds_new: Some((scale: Some((2.0, 1.0, 1.0)))),
            ),
        )],
    )"#;
    let s = Scene::from_ron(hand).unwrap();
    let text = s.to_ron();
    assert!(text.contains("n64_expanded: Some(") && text.contains("n3ds_new: Some("));
    assert_eq!(Scene::from_ron(&text).unwrap(), s);
    let p = &s.entities[0].platforms;
    assert_eq!(
        p.get(Target::N64Expanded).unwrap().model.as_deref(),
        Some("models/crowd-mid")
    );
    assert!(p.own(Target::N3ds).is_none());
    assert_eq!(p.get(Target::N3dsNew).unwrap().scale, Some([2.0, 1.0, 1.0]));

    let lib = common::library();
    let model = |t: Target| {
        resolve_for(&s, &lib, 0.0, t).items[0]
            .model
            .as_ref()
            .map(|(_, p)| p.clone())
    };
    assert_eq!(model(Target::Pc).as_deref(), Some("models/crowd"));
    assert_eq!(model(Target::N64).as_deref(), Some("models/crowd-low"));
    assert_eq!(
        model(Target::N64Expanded).as_deref(),
        Some("models/crowd-mid")
    );
    assert_eq!(model(Target::N3ds).as_deref(), Some("models/crowd"));
    assert_eq!(model(Target::N3dsNew).as_deref(), Some("models/crowd"));
    // Without its own variant a tier is its console.
    let only_n64 = hand.replace(
        r#"n64_expanded: Some((model: Some("models/crowd-mid"))),"#,
        "",
    );
    let s = Scene::from_ron(&only_n64).unwrap();
    assert_eq!(
        resolve_for(&s, &lib, 0.0, Target::N64Expanded).items[0]
            .model
            .as_ref()
            .map(|(_, p)| p.as_str()),
        Some("models/crowd-low")
    );

    // v4 (before the tiers) migrates to v5 unchanged.
    let v4 = r#"(version: 4, name: "old", entities: [(name: "e",
        platforms: (n64: Some((visible: Some(false)))))])"#;
    let old = Scene::from_ron(v4).unwrap();
    assert_eq!(old.version, SCENE_VERSION);
    assert!(old.entities[0].platforms.own(Target::N64Expanded).is_none());
    assert_eq!(
        old.entities[0]
            .platforms
            .get(Target::N64Expanded)
            .unwrap()
            .visible,
        Some(false)
    );
}

#[test]
fn each_platform_resolves_its_own_variant() {
    let (s, lib) = (scene(), common::library());

    // `resolve` is the PC: every entity, base models.
    let pc = resolve(&s, &lib, 0.0);
    let model_of = |items: &[trino_scene::DrawItem], name: &str| {
        items
            .iter()
            .find(|i| i.name == name)
            .and_then(|i| i.model.as_ref().map(|(id, _)| *id))
    };
    assert_eq!(model_of(&pc.items, "tree"), Some(id("models/tree")));
    assert_eq!(
        names(&s, &lib, Target::Pc),
        ["tree", "tree/bird", "crate", "crate/lid", "flag"]
    );

    // N64: the cheap tree with its material; the crate and its lid gone.
    let n64 = resolve_for(&s, &lib, 0.0, Target::N64);
    assert!(n64.warnings.is_empty(), "{:?}", n64.warnings);
    let tree = n64.items.iter().find(|i| i.name == "tree").unwrap();
    assert_eq!(tree.model.as_ref().unwrap().0, id("models/tree-low"));
    assert_eq!(tree.material.as_deref(), Some("materials/glow"));
    assert_eq!(tree.params.shading, trino_core::Shading::Unlit);
    assert_eq!(names(&s, &lib, Target::N64), ["tree", "tree/bird", "flag"]);

    // 3DS: the flag hidden, the tree drawn at half scale — but its child
    // and its collider keep the PC transform (variants are a look).
    let n3ds = resolve_for(&s, &lib, 0.0, Target::N3ds);
    assert_eq!(
        names(&s, &lib, Target::N3ds),
        ["tree", "tree/bird", "crate", "crate/lid"]
    );
    let small = n3ds.items.iter().find(|i| i.name == "tree").unwrap();
    let big = pc.items.iter().find(|i| i.name == "tree").unwrap();
    assert!((small.matrix.m[1][1] - 0.5).abs() < 1e-6);
    assert_eq!(
        small.collider.unwrap().world_aabb(&small.matrix),
        big.collider.unwrap().world_aabb(&big.matrix),
        "same collision box on every platform"
    );
    let bird = |items: &[trino_scene::DrawItem]| {
        items
            .iter()
            .find(|i| i.name == "tree/bird")
            .unwrap()
            .matrix
            .transform_point(Vec3::ZERO)
    };
    assert_eq!(bird(&n3ds.items), bird(&pc.items));
    assert_eq!(
        small.collider.unwrap().shape,
        Shape::Box {
            half: Vec3::new(1.0, 4.0, 1.0)
        },
        "unscaled by the 0.5 variant"
    );
}

#[test]
fn prefab_parts_and_instances_take_variants() {
    let mut lib = common::library();
    // A prefab whose leaves are not on the N64.
    let mut tree = Entity::new("tree");
    let mut leaves = Entity::new("leaves").with_model(ModelComponent::new("models/leaves"));
    leaves.platforms.n64 = Some(Variant {
        disabled: true,
        ..Variant::default()
    });
    tree.children.push(leaves);
    tree.children
        .push(Entity::new("trunk").with_model(ModelComponent::new("models/trunk")));
    lib.prefabs
        .insert("prefabs/vtree".into(), Prefab::new(tree));
    let mut s = Scene::new("p");
    let mut inst = Entity::new("t1");
    inst.prefab = Some(PrefabInstance::new("prefabs/vtree"));
    // The instance itself is twice as big on the 3DS.
    inst.platforms.n3ds = Some(Variant {
        scale: Some([2.0, 2.0, 2.0]),
        ..Variant::default()
    });
    s.entities.push(inst);
    assert_eq!(
        names(&s, &lib, Target::N64),
        ["t1", "t1/tree", "t1/tree/trunk"]
    );
    let n3ds = resolve_for(&s, &lib, 0.0, Target::N3ds);
    let trunk = n3ds
        .items
        .iter()
        .find(|i| i.name == "t1/tree/trunk")
        .unwrap();
    assert!(
        (trunk.matrix.m[0][0] - 2.0).abs() < 1e-6,
        "the instance's parts scale"
    );
}

#[test]
fn exports_pick_the_variant_of_their_platform() {
    let (s, lib) = (scene(), common::library());
    for target in Target::ALL {
        let resolved = resolve_for(&s, &lib, 0.0, target);
        let (blob, warnings) = bake::to_tscn_for(&s, &lib, target);
        assert!(warnings.is_empty(), "{warnings:?}");
        let baked = BakedScene::parse(leak(blob)).unwrap();
        let items: Vec<_> = resolved.items.iter().filter(|i| i.exported()).collect();
        assert_eq!(baked.node_count(), items.len(), "{target:?}");
        for (item, node) in items.iter().zip(baked.nodes()) {
            assert_eq!(node.name, item.name);
            assert_eq!(node.model, item.model.as_ref().map(|(id, _)| *id));
            assert_eq!(node.matrix, item.matrix);
        }
    }
    let n64 = BakedScene::parse(leak(bake::to_tscn_for(&s, &lib, Target::N64).0)).unwrap();
    assert_eq!(n64.find("tree").unwrap().model, Some(id("models/tree-low")));
    assert!(n64.find("crate").is_none());
    // The PC wrapper is the PC variant.
    assert_eq!(
        bake::to_tscn(&s, &lib).0,
        bake::to_tscn_for(&s, &lib, Target::Pc).0
    );

    // Codegen: the N64 module names the low model and says how to redo it.
    let (code, _) = codegen::to_rust_for(&s, &lib, Target::N64);
    assert!(code.contains("scene export --platform n64"), "{code}");
    let low = format!("ModelId(0x{:08x})", id("models/tree-low").0);
    let full = format!("ModelId(0x{:08x})", id("models/tree").0);
    assert!(code.contains(&low) && !code.contains(&full), "{code}");
    assert!(!code.contains("\"crate\""));
    let (pc_code, _) = codegen::to_rust(&s, &lib);
    assert!(pc_code.contains(&full) && !pc_code.contains("--platform"));
}

#[test]
fn the_web_resolves_like_the_pc_unless_it_has_its_own_variant() {
    let lib = common::library();
    let mut s = scene();
    // No PC or Web variant: the Web draws what the PC draws.
    assert_eq!(names(&s, &lib, Target::Web), names(&s, &lib, Target::Pc));
    // A PC-only change (the flag hidden on PC) reaches the Web too...
    let flag = s.entities.iter_mut().find(|e| e.name == "flag").unwrap();
    flag.platforms.pc = Some(Variant {
        visible: Some(false),
        ..Variant::default()
    });
    assert!(!names(&s, &lib, Target::Web).contains(&"flag".to_string()));
    // ...until the Web says otherwise ("as written" there).
    let flag = s.entities.iter_mut().find(|e| e.name == "flag").unwrap();
    flag.platforms.web = Some(Variant::default());
    assert!(names(&s, &lib, Target::Web).contains(&"flag".to_string()));
    assert!(!names(&s, &lib, Target::Pc).contains(&"flag".to_string()));
    // The Web's block round-trips as `web` in a v4 file.
    let text = s.to_ron();
    assert!(text.contains("web: Some("), "{text}");
    assert_eq!(Scene::from_ron(&text).unwrap(), s);
    // A v3 file loads as v4.
    let v3 = text.replace("version: 4", "version: 3");
    let old = Scene::from_ron(&v3).unwrap();
    assert_eq!(old.version, SCENE_VERSION);
}
