//! A scene with every component → the two game-side forms: `bake::to_tscn`
//! (read back by `trino_core::BakedScene`) and `codegen::to_rust`. Both must
//! carry exactly what `resolve` computed, so a level shipped as data and the
//! same level shipped as code behave the same.

mod common;

use trino_core::scene::SceneError;
use trino_core::{BakedScene, Color, Shading, VisVolume};
use trino_scene::{DrawItem, bake, codegen, resolve};

fn leak(bytes: Vec<u8>) -> &'static [u8] {
    Box::leak(bytes.into_boxed_slice())
}

fn exported(items: &[DrawItem]) -> Vec<&DrawItem> {
    items.iter().filter(|i| i.exported()).collect()
}

#[test]
fn tscn_blobs_carry_everything_resolve_computed() {
    let (scene, lib) = (common::scene(), common::library());
    let resolved = resolve(&scene, &lib, 0.0);
    let items = exported(&resolved.items);
    let (blob, warnings) = bake::to_tscn(&scene, &lib);
    assert_eq!(warnings, resolved.warnings);
    assert!(warnings.is_empty(), "{warnings:?}");
    let baked = BakedScene::parse(leak(blob)).unwrap();

    // Environment.
    assert_eq!(baked.clear(), Color::rgb(10, 20, 30));
    assert_eq!(baked.camera(), scene.environment.core_camera());
    assert_eq!(baked.light(), scene.environment.core_light());
    assert_eq!(baked.light().fog.near, 5.0);
    assert!(baked.light().fog.enabled());

    // The sprite-only label and the hidden entity are not exported.
    assert_eq!(baked.node_count(), items.len());
    assert!(baked.find("label").is_none() && baked.find("hidden").is_none());
    for (item, node) in items.iter().zip(baked.nodes()) {
        assert_eq!(node.name, item.name);
        let tags: Vec<&str> = node.tags.iter().collect();
        assert_eq!(tags, item.tags);
        assert_eq!(node.matrix, item.matrix, "{}", item.name);
        assert_eq!(node.model, item.model.as_ref().map(|(id, _)| *id));
        assert_eq!(node.params.tint, item.params.tint, "{}", item.name);
        assert_eq!(node.params.emissive, item.params.emissive);
        assert_eq!(node.params.shading, item.params.shading);
        assert_eq!(node.params.double_sided, item.params.double_sided);
        assert_eq!(node.params.texture, item.params.texture);
        assert_eq!(node.anim.map(|a| a.0), item.params.anim.map(|a| a.clip));
        assert_eq!(node.collider, item.collider, "{}", item.name);
        let lod: Vec<_> = node.lod.iter().map(|l| (l.distance, l.model)).collect();
        let want: Vec<_> = item
            .lod
            .iter()
            .map(|(d, m)| (*d, m.as_ref().map(|(id, _)| *id)))
            .collect();
        assert_eq!(lod, want, "{}", item.name);
        assert_eq!(node.sound, item.sound);
        assert_eq!(node.cell, item.cell, "{}", item.name);
        assert_eq!(node.vis, item.vis, "{}", item.name);
    }

    // Spot checks of the authored values themselves.
    let ground = baked.find("ground").unwrap();
    assert!(ground.has_tag("floor") && ground.has_tag("static"));
    // Material tint (200, 100, 50) × entity tint (128, 255, 255).
    assert_eq!(ground.params.tint, Color::rgba(100, 100, 50, 255));
    assert_eq!(ground.params.shading, Shading::Flat);
    assert!(ground.params.double_sided);
    assert_eq!(
        ground.params.texture,
        Some(trino_core::SpriteId::from_path("sprites/rock"))
    );
    let hero = baked.find("hero").unwrap();
    assert_eq!(
        hero.anim,
        Some((trino_core::ClipId::from_name("walk"), 1.5)),
        "the clip speed survives the bake"
    );
    // The hat inherits the hero's transform: 2x scale, 90° yaw.
    let hat = baked.find("hero/hat").unwrap();
    let p = hat.position();
    assert!((p.x - 1.0).abs() < 1e-4 && (p.y - 2.0).abs() < 1e-4 && (p.z - 2.0).abs() < 1e-4);
    assert_eq!(hat.params.shading, Shading::Unlit);
    assert_eq!(hat.lod.len(), 2);
    assert_eq!(hat.lod.get(0).unwrap().distance, 10.0, "sorted");
    let coin = baked.find("coin").unwrap();
    assert!(coin.collider.unwrap().trigger);
    assert_eq!(coin.sound.unwrap().radius, 4.0);
    let door = baked.find("door").unwrap();
    assert!(matches!(
        door.vis,
        Some(VisVolume::Portal { a: 0, b: 1, .. })
    ));
    assert_eq!(baked.find("house/lamp").unwrap().cell, 0, "cell members");
    assert_eq!(baked.find("spawn").unwrap().cell, trino_core::NO_CELL);
    let leaves = baked.find("tree-1/tree/trunk/leaves").unwrap();
    assert_eq!(
        leaves.params.tint,
        Color::rgba(10, 200, 10, 255),
        "override"
    );
    let weird = baked.find("say \"hi\" \\o/").unwrap();
    assert!(weird.model.is_none());

    // The runtime's cells agree with the editor's.
    let cells: trino_core::Cells<8> = trino_core::Cells::from_nodes(baked.nodes());
    assert_eq!(cells.cell_count(), resolved.cells().cell_count());
    assert_eq!(cells.portal_count(), 1);
}

#[test]
fn corrupt_tscn_blobs_are_rejected_up_front() {
    let (blob, _) = bake::to_tscn(&common::scene(), &common::library());
    let mut bad = blob.clone();
    bad[0] = b'X';
    assert_eq!(
        BakedScene::parse(leak(bad)).unwrap_err(),
        SceneError::BadMagic
    );
    let mut bad = blob.clone();
    bad[4] = 9;
    assert_eq!(
        BakedScene::parse(leak(bad)).unwrap_err(),
        SceneError::UnsupportedVersion(9)
    );
    assert_eq!(
        BakedScene::parse(leak(blob[..blob.len() / 2].to_vec())).unwrap_err(),
        SceneError::Truncated
    );
    assert_eq!(
        BakedScene::parse(leak(blob[..20].to_vec())).unwrap_err(),
        SceneError::Truncated
    );
    // The first node's name points past the string table.
    let mut bad = blob.clone();
    let name_len = trino_core::scene::TSCN_HEADER + trino_core::scene::TSCN_ENV_BYTES + 4;
    bad[name_len..name_len + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        BakedScene::parse(leak(bad)).unwrap_err(),
        SceneError::BadString
    );
}

#[test]
fn generated_code_lists_every_exported_node_and_component() {
    let (scene, lib) = (common::scene(), common::library());
    let (code, warnings) = codegen::to_rust(&scene, &lib);
    assert!(warnings.is_empty());
    assert_eq!(code, codegen::to_rust(&scene, &lib).0, "deterministic");
    assert!(code.starts_with("// @generated by trino-scene from scene `fixture`"));
    let resolved = resolve(&scene, &lib, 0.0);
    assert_eq!(
        code.matches("    SceneNode {").count(),
        exported(&resolved.items).len()
    );
    for needle in [
        "pub const CLEAR: Color = Color::rgb(10, 20, 30);",
        "pub const CAMERA: Camera3",
        "fog: Fog { color: Color::rgba(50, 60, 70, 255), near: 5.0, far: 40.0 }",
        "pub static NODES: &[SceneNode] = &[",
        "name: \"hero/hat\"",
        "tags: Tags::List(&[\"floor\", \"static\"])",
        "shading: Shading::Flat",
        "shading: Shading::Unlit",
        "double_sided: true",
        "Shape::Box { half: Vec3::new(5.0, 0.5, 5.0) }",
        "Shape::Sphere { radius: 0.5 }",
        "Shape::Capsule { radius: 0.4, height: 1.8 }",
        "trigger: true",
        "offset: Vec3::new(0.0, -0.5, 0.0)",
        "lod: Lods::List(&[LodLevel { distance: 10.0, model: Some(ModelId(",
        "LodLevel { distance: 30.0, model: None }",
        "/* models/hat-far */",
        "Some(SoundSource { sound: SoundId(",
        "radius: 4.0, volume: 0.8 })",
        "vis: Some(VisVolume::Cell { half: Vec3::new(4.0, 2.0, 4.0) })",
        "vis: Some(VisVolume::Portal {",
        "a: 0, b: 1 })",
        "cell: NO_CELL",
        "cell: 0,",
        "lod: Lods::NONE",
        "name: \"say \\\"hi\\\" \\\\o/\"",
        "/* models/ground */",
    ] {
        assert!(code.contains(needle), "missing `{needle}` in:\n{code}");
    }
    assert!(code.contains(&format!(
        "texture: Some(SpriteId({:#010x}))",
        trino_core::asset_id("sprites/rock")
    )));
    assert!(code.contains(&format!(
        "anim: Some((ClipId({:#010x}), 1.5))",
        trino_core::asset_id("walk")
    )));
    assert!(!code.contains("\"label\"") && !code.contains("\"hidden\""));
    assert!(!code.contains("-0.0"), "no negative zero churn");
}
