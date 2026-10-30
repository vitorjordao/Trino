//! Asset groups in scenes (issue #56, scene v6): `requires` (the groups a
//! level needs, `["boot"]` for every older file), a cell's `groups` (what an
//! interior streams by room), and the asset list `cargo xtask assets check`
//! holds against them.

use trino_scene::{
    Entity, ModelComponent, SCENE_VERSION, Scene, SoundComponent, SpriteComponent, VisComponent,
    resolve,
};

#[test]
fn requires_defaults_to_boot_and_round_trips() {
    const { assert!(SCENE_VERSION >= 6, "`requires` arrived in v6") };
    assert_eq!(Scene::new("x").requires, ["boot"]);
    // Every older file needs only boot, as it always did.
    for v in 1..=5 {
        let text = if v == 1 {
            r#"(version: 1, name: "old", entities: [(name: "a")])"#.to_string()
        } else {
            format!(r#"(version: {v}, name: "old", entities: [(name: "a")])"#)
        };
        let s = Scene::from_ron(&text).unwrap();
        assert_eq!(s.version, SCENE_VERSION, "v{v}");
        assert_eq!(s.requires, ["boot"], "v{v}");
    }

    let mut s = Scene::new("castle");
    s.requires = vec!["boot".into(), "castle".into()];
    let text = s.to_ron();
    // ron's pretty printer ends lines in `\r\n` on Windows.
    let lf = text.replace("\r\n", "\n");
    assert!(lf.contains("version: 8"), "{text}");
    assert!(
        lf.contains("requires: [\n        \"boot\",\n        \"castle\",\n    ],"),
        "{text}"
    );
    let back = Scene::from_ron(&text).unwrap();
    assert_eq!(back, s);
    assert_eq!(back.to_ron(), text, "stable text");
}

fn interior() -> Scene {
    let mut s = Scene::new("interior");
    s.requires = vec!["boot".into(), "house".into()];
    let mut hall = Entity::new("hall").at([2.0, 1.5, 2.0]);
    hall.vis = Some(VisComponent::Cell {
        size: [4.0, 3.0, 4.0],
        groups: vec!["hall".into()],
    });
    hall.children
        .push(Entity::new("lamp").with_model(ModelComponent::new("models/lamp")));
    s.entities.push(hall);
    let mut kitchen = Entity::new("kitchen").at([6.0, 1.5, 2.0]);
    kitchen.vis = Some(VisComponent::Cell {
        size: [4.0, 3.0, 4.0],
        groups: vec!["kitchen".into(), "hall".into()],
    });
    let mut pot = Entity::new("pot").with_model(ModelComponent::new("models/pot"));
    pot.sound = Some(SoundComponent::new("sounds/boil"));
    kitchen.children.push(pot);
    s.entities.push(kitchen);
    // A cell without groups writes no `groups` key.
    let mut yard = Entity::new("yard").at([10.0, 1.5, 2.0]);
    yard.vis = Some(VisComponent::Cell {
        size: [4.0, 3.0, 4.0],
        groups: Vec::new(),
    });
    let mut sign = Entity::new("sign");
    sign.sprite = Some(SpriteComponent::new("sprites/sign"));
    yard.children.push(sign);
    s.entities.push(yard);
    s
}

#[test]
fn cell_groups_round_trip_and_resolve_in_cell_order() {
    let s = interior();
    let text = s.to_ron();
    assert!(
        text.replace("\r\n", "\n").contains("\"kitchen\",\n"),
        "{text}"
    );
    assert_eq!(text.matches("groups:").count(), 2, "{text}");
    assert_eq!(Scene::from_ron(&text).unwrap(), s);
    // Hand-written: a cell names its groups beside its size.
    let hand = Scene::from_ron(
        r#"(version: 6, name: "h", requires: ["boot"], entities: [(
            name: "cave",
            vis: Some(Cell(size: (8.0, 4.0, 8.0), groups: ["cave"])),
        )])"#,
    )
    .unwrap();
    assert_eq!(
        hand.cell_groups(),
        [("cave".to_string(), vec!["cave".to_string()])]
    );

    assert_eq!(
        s.cell_groups(),
        [
            ("hall".to_string(), vec!["hall".to_string()]),
            (
                "kitchen".to_string(),
                vec!["kitchen".to_string(), "hall".to_string()]
            ),
        ]
    );
    let lib = trino_scene::Library::default();
    let r = resolve(&s, &lib, 0.0);
    assert_eq!(
        r.cell_groups,
        [
            vec!["hall".to_string()],
            vec!["kitchen".to_string(), "hall".to_string()],
            vec![],
        ]
    );
    let (streamer, dropped) = r.cell_streamer::<4>(30);
    assert!(dropped.is_empty());
    let g = trino_core::GroupId::from_name;
    assert_eq!(streamer.groups(), &[g("hall"), g("kitchen")]);
    assert_eq!(streamer.wanted(0b010), 0b11, "the kitchen names both");
    assert_eq!(streamer.wanted(0b100), 0, "the yard none");
    // Past capacity, groups are reported, not silently lost.
    let (_, dropped) = r.cell_streamer::<1>(30);
    assert_eq!(dropped.len(), 1, "{dropped:?}");
    assert!(dropped[0].contains("kitchen"), "{dropped:?}");
}

#[test]
fn asset_refs_list_every_asset_once() {
    let mut s = interior();
    // A second pot and a far LOD level and an N64 variant.
    let mut pot2 = Entity::new("pot2").with_model(ModelComponent::new("models/pot"));
    let mut m = pot2.model.take().unwrap();
    m.lod.push(trino_scene::LodDef {
        distance: 20.0,
        path: Some("models/pot-far".into()),
        ..Default::default()
    });
    pot2.model = Some(m);
    pot2.platforms.n64 = Some(trino_scene::Variant {
        model: Some("models/pot-n64".into()),
        ..Default::default()
    });
    s.entities.push(pot2);
    assert_eq!(
        s.asset_refs(),
        [
            "models/lamp",
            "models/pot",
            "models/pot-far",
            "models/pot-n64",
            "sounds/boil",
            "sprites/sign",
        ]
    );
    assert!(Scene::new("empty").asset_refs().is_empty());
}

#[test]
fn codegen_names_the_groups_only_when_there_are_some() {
    let lib = trino_scene::Library::default();
    let (code, _) = trino_scene::codegen::to_rust(&interior(), &lib);
    assert!(
        code.contains(
            "pub const REQUIRES: &[trino_core::GroupId] = &[trino_core::GroupId::from_name(\"boot\"), trino_core::GroupId::from_name(\"house\")];"
        ),
        "{code}"
    );
    assert!(code.contains("    (0, trino_core::GroupId::from_name(\"hall\")),\n    (1, trino_core::GroupId::from_name(\"kitchen\")),\n    (1, trino_core::GroupId::from_name(\"hall\")),\n];"), "{code}");
    let (plain, _) = trino_scene::codegen::to_rust(&Scene::new("plain"), &lib);
    assert!(!plain.contains("REQUIRES") && !plain.contains("CELL_GROUPS"));
}
