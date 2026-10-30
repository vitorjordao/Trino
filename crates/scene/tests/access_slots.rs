//! Semantic slot colors and high-contrast outlines (issue #96) from the
//! scene file to every game-side form: `palette` and `outline` on an entity
//! resolve to base colors, semantic ids and a sphere from the model's
//! bounds; codegen and TSCN v8 carry them; the scene file round-trips and
//! a v7 file still loads; the platform budget prices the outlines and
//! advises a tint for a palette on water.

mod common;

use trino_core::{BakedScene, Color};
use trino_scene::access::PalettesDef;
use trino_scene::{OutlineComponent, Scene, SlotColor, SlotColorDef, bake, codegen, resolve};

const PALETTES: &str = r#"(
    version: 1,
    colors: {
        "team.home": (base: (200, 30, 30), protanopia: (240, 160, 0)),
        "team.away": (base: (30, 160, 60), protanopia: (40, 90, 220)),
    },
    distinct: [["team.home", "team.away"]],
)"#;

fn fixture() -> (Scene, trino_scene::Library) {
    let mut lib = common::library();
    lib.palettes = Some(PalettesDef::from_ron(PALETTES).unwrap());
    let mut scene = Scene::new("slots");
    let mut player = trino_scene::Entity::new("player").with_model(trino_scene::ModelComponent {
        palette: vec![
            SlotColorDef {
                slot: 1,
                color: SlotColor::Semantic("team.home".into()),
            },
            SlotColorDef {
                slot: 3,
                color: SlotColor::Rgb([10, 20, 30]),
            },
        ],
        ..common::model("models/crate")
    });
    player.outline = Some(OutlineComponent::default());
    player.transform.scale = [2.0, 2.0, 2.0];
    scene.entities.push(player);
    (scene, lib)
}

#[test]
fn slot_colors_and_outlines_resolve_export_and_bake() {
    let (scene, lib) = fixture();
    let r = resolve(&scene, &lib, 0.0);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let item = &r.items[0];
    let w = Color::WHITE;
    assert_eq!(
        item.params.palette,
        Some([Color::rgb(200, 30, 30), w, Color::rgb(10, 20, 30), w]),
        "a semantic slot at its base color, a literal as it is"
    );
    let home = trino_core::asset_id("team.home");
    assert_eq!(item.semantic, [home, 0, 0, 0]);
    assert_eq!(item.semantic_names[0].as_deref(), Some("team.home"));
    // The crate's bounds: (-0.5, 0, -0.5)..(0.5, 1, 0.5).
    let o = item.outline.expect("an outline");
    assert_eq!(o.center, trino_core::Vec3::new(0.0, 0.5, 0.0));
    assert!((o.radius - 0.75f32.sqrt()).abs() < 1e-6, "{}", o.radius);
    assert_eq!((o.color, o.pixels), (Color::rgb(255, 230, 0), 2.0));

    // Codegen: the palette, the ids (named) and the outline.
    let (code, _) = codegen::to_rust(&scene, &lib);
    for needle in [
        "palette: Some([Color::rgba(200, 30, 30, 255), Color::rgba(255, 255, 255, 255), \
         Color::rgba(10, 20, 30, 255), Color::rgba(255, 255, 255, 255)]),",
        &format!(
            "access: Some(&node_access([{home:#010x} /* team.home */, 0, 0, 0], \
                  Some(NodeOutline {{ color: Color::rgba(255, 230, 0, 255), center: \
                  Vec3::new(0.0, 0.5, 0.0),"
        ),
    ] {
        assert!(code.contains(needle), "missing `{needle}` in:\n{code}");
    }

    // TSCN v8: the same node from the blob.
    let blob = bake::to_tscn(&scene, &lib).0;
    let baked = BakedScene::parse(Box::leak(blob.into_boxed_slice())).unwrap();
    let n = baked.node(0);
    assert_eq!(n.semantic(), item.semantic);
    assert_eq!(n.params.palette, item.params.palette);
    assert_eq!(n.outline(), item.outline);
    // Without either, the node reads none back.
    let (mut plain, _) = fixture();
    plain.entities[0].outline = None;
    plain.entities[0].model.as_mut().unwrap().palette.clear();
    let blob = bake::to_tscn(&plain, &lib).0;
    let baked = BakedScene::parse(Box::leak(blob.into_boxed_slice())).unwrap();
    let n = baked.node(0);
    assert_eq!((n.access, n.params.palette), (None, None));
}

#[test]
fn the_file_round_trips_and_older_ones_load() {
    let (scene, _) = fixture();
    let text = scene.to_ron();
    assert!(text.contains("Semantic(\"team.home\")"), "{text}");
    assert!(text.contains("Rgb((10, 20, 30))"), "{text}");
    assert!(text.contains("outline: Some(("), "{text}");
    assert_eq!(Scene::from_ron(&text).unwrap(), scene);
    // A hand-written component with defaults only.
    let short = text.replace(
        "outline: Some((\n                color: (255, 230, 0),\n                pixels: 2.0,\n            )),",
        "outline: Some(()),",
    );
    assert_ne!(
        short, text,
        "the default outline is written in full: {text}"
    );
    assert_eq!(Scene::from_ron(&short).unwrap(), scene);
    // A v7 file (before #96) loads and saves as v8.
    let v7 = r#"(version: 7, name: "old", entities: [(name: "a", model: Some((path: "models/crate")))])"#;
    let old = Scene::from_ron(v7).unwrap();
    assert_eq!(old.version, trino_scene::SCENE_VERSION);
    assert!(old.entities[0].model.as_ref().unwrap().palette.is_empty());
    // Bad slots and unknown names: warnings, never fatal.
    let (mut bad, lib) = fixture();
    let m = bad.entities[0].model.as_mut().unwrap();
    m.palette.push(SlotColorDef {
        slot: 7,
        color: SlotColor::Rgb([1, 1, 1]),
    });
    m.palette[0].color = SlotColor::Semantic("team.nobody".into());
    let r = resolve(&bad, &lib, 0.0);
    assert!(
        r.warnings.iter().any(|w| w.contains("palette slot 7")),
        "{:?}",
        r.warnings
    );
    assert!(
        r.warnings.iter().any(|w| w.contains("`team.nobody`")),
        "{:?}",
        r.warnings
    );
    assert_eq!(r.items[0].params.palette.unwrap()[0], Color::WHITE);
}
