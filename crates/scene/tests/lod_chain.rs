//! A model's LOD chain in the scene format — full mesh → light mesh →
//! impostor → nothing — with distances per platform: the RON, what
//! `resolve_for` keeps on each platform, the impostor descriptor each one
//! gets from the library, and the TSCN and codegen exports.

use trino_core::{BakedScene, Impostor, LodPick, SpriteId, Vec3};
use trino_scene::{
    Entity, LodAt, LodDef, LodDraw, LodPlatforms, ModelComponent, Scene, Target, bake, codegen,
    resolve_for,
};

fn imp(tile: u16) -> Impostor {
    Impostor {
        sprite: SpriteId::from_path("sprites/impostor-tree"),
        views: 8,
        cols: 4,
        tile,
        size: 6.0,
        center: Vec3::new(0.0, 3.0, 0.0),
    }
}

fn library() -> trino_scene::Library {
    let mut lib = trino_scene::Library::default();
    // The N64's atlas has smaller tiles.
    lib.impostors.insert(
        "impostors/tree".into(),
        [Some(imp(64)), Some(imp(16)), Some(imp(64)), Some(imp(64))],
    );
    lib
}

fn scene() -> Scene {
    let mut s = Scene::new("forest");
    s.entities
        .push(Entity::new("tree").with_model(ModelComponent {
            lod: vec![
                LodDef {
                    distance: 60.0,
                    ..LodDef::default()
                },
                LodDef {
                    distance: 12.0,
                    path: Some("models/tree-light".into()),
                    ..LodDef::default()
                },
                LodDef {
                    distance: 30.0,
                    impostor: Some("impostors/tree".into()),
                    platforms: LodPlatforms {
                        n64: Some(LodAt::Never),
                        n3ds: Some(LodAt::At(20.0)),
                        ..LodPlatforms::default()
                    },
                    ..LodDef::default()
                },
            ],
            ..ModelComponent::new("models/tree")
        }));
    s
}

fn leak(b: Vec<u8>) -> &'static [u8] {
    Box::leak(b.into_boxed_slice())
}

#[test]
fn the_chain_round_trips_through_ron() {
    let s = scene();
    let text = s.to_ron();
    assert!(
        text.contains("impostor: Some(\"impostors/tree\")"),
        "{text}"
    );
    assert!(text.contains("n64: Some(Never)") && text.contains("n3ds: Some(At(20.0))"));
    assert_eq!(Scene::from_ron(&text).unwrap(), s);
    // A level written by hand with the short forms.
    let hand = r#"(version: 3, name: "x", entities: [(name: "t", model: Some((
        path: "models/t",
        lod: [(distance: 9.0, impostor: Some("impostors/t"), platforms: (pc: Some(At(15.0))))],
    )))])"#;
    let parsed = Scene::from_ron(hand).unwrap();
    let l = &parsed.entities[0].model.as_ref().unwrap().lod[0];
    assert_eq!(l.distance_on(Target::Pc), Some(15.0));
    assert_eq!(l.distance_on(Target::N64), Some(9.0));
}

#[test]
fn each_platform_gets_its_own_chain() {
    let (s, lib) = (scene(), library());
    let chain = |t: Target| -> Vec<(f32, LodDraw)> {
        let r = resolve_for(&s, &lib, 0.0, t);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        r.items[0]
            .lod
            .iter()
            .map(|l| (l.distance, l.draw.clone()))
            .collect()
    };
    let light = LodDraw::Model(
        trino_core::ModelId::from_path("models/tree-light"),
        "models/tree-light".into(),
    );
    assert_eq!(
        chain(Target::Pc),
        vec![
            (12.0, light.clone()),
            (30.0, LodDraw::Impostor("impostors/tree".into(), imp(64))),
            (60.0, LodDraw::Cull),
        ]
    );
    // No impostor on the N64: the light mesh draws until the cull.
    assert_eq!(
        chain(Target::N64),
        vec![(12.0, light.clone()), (60.0, LodDraw::Cull)]
    );
    // The 3DS switches to it earlier, sorted before nothing changes.
    assert_eq!(
        chain(Target::N3ds),
        vec![
            (12.0, light),
            (20.0, LodDraw::Impostor("impostors/tree".into(), imp(64))),
            (60.0, LodDraw::Cull),
        ]
    );
    // The editor's preview distance and the game's eye agree.
    let r = resolve_for(&s, &lib, 0.0, Target::Pc);
    let item = &r.items[0];
    assert_eq!(item.lod_at(40.0), LodPick::Impostor(imp(64)));
    assert_eq!(item.lod_from(Vec3::new(0.0, 0.0, 40.0)), item.lod_at(40.0));
    assert_eq!(item.level_at(5.0), None);
    assert_eq!(item.level_at(31.0), Some(1));
}

#[test]
fn exports_carry_the_impostor_and_a_missing_one_warns() {
    let (s, lib) = (scene(), library());
    for (t, tile) in [
        (Target::Pc, Some(64)),
        (Target::N64, None),
        (Target::N3ds, Some(64)),
    ] {
        let (blob, warnings) = bake::to_tscn_for(&s, &lib, t);
        assert!(warnings.is_empty());
        let node = BakedScene::parse(leak(blob)).unwrap().node(0);
        match (node.lod_from(Vec3::new(0.0, 0.0, 35.0)), tile) {
            (LodPick::Impostor(i), Some(tile)) => assert_eq!(i.tile, tile),
            (LodPick::Model(_), None) => {}
            (other, _) => panic!("{t:?}: {other:?}"),
        }
    }
    let (code, _) = codegen::to_rust_for(&s, &lib, Target::Pc);
    assert!(
        code.contains("impostor: Some(Impostor { sprite: SpriteId(") && code.contains("tile: 64"),
        "{code}"
    );
    assert!(code.contains("/* impostors/tree */"));
    let (n64, _) = codegen::to_rust_for(&s, &lib, Target::N64);
    assert!(!n64.contains("Some(Impostor"), "skipped on the N64");

    // Not baked: a warning names the fix, and that level draws nothing.
    let r = resolve_for(&s, &trino_scene::Library::default(), 0.0, Target::Pc);
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("impostors/tree") && w.contains("impostor bake")),
        "{:?}",
        r.warnings
    );
    assert_eq!(r.items[0].lod[1].draw, LodDraw::Cull);
}

/// A LOD level's start on a hardware tier (issue #54): `n64_expanded: At(d)`
/// pushes it out on the Expansion Pak; a tier without its own start takes
/// its console's; the text round-trips.
#[test]
fn a_tier_starts_a_level_at_its_own_distance_or_its_consoles() {
    let at = LodPlatforms {
        n64: Some(LodAt::At(12.0)),
        n64_expanded: Some(LodAt::At(20.0)),
        n3ds: Some(LodAt::Never),
        ..LodPlatforms::default()
    };
    assert_eq!(at.get(Target::N64Expanded), Some(&LodAt::At(20.0)));
    assert_eq!(at.get(Target::N64), Some(&LodAt::At(12.0)));
    assert_eq!(at.get(Target::N3dsNew), Some(&LodAt::Never), "the 3DS's");
    assert_eq!(at.own(Target::N3dsNew), None);
    assert_eq!(at.get(Target::Pc), None);
    let text = ron::to_string(&at).unwrap();
    assert!(text.contains("n64_expanded:Some(At(20.0))"), "{text}");
    assert_eq!(ron::from_str::<LodPlatforms>(&text).unwrap(), at);
}
