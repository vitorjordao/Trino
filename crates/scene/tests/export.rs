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
        assert_eq!(node.params.water, item.params.water, "{}", item.name);
        assert_eq!(node.anim.map(|a| a.0), item.params.anim.map(|a| a.clip));
        assert_eq!(node.collider, item.collider, "{}", item.name);
        let lod: Vec<_> = node.lod.iter().map(|l| (l.distance, l.model)).collect();
        let want: Vec<_> = item
            .lod
            .iter()
            .map(|l| (l.distance, l.draw.pick().model()))
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
    // A water material survives the bake (TSCN v4).
    let pond = baked.find("pond").unwrap().params.water.unwrap();
    assert_eq!(pond.flow_speed, trino_core::Vec2::new(0.0, -0.3));
    assert_eq!(pond.wave_amplitude, 0.05);
    assert!(baked.find("ground").unwrap().params.water.is_none());
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
        "LodLevel { distance: 30.0, model: None, impostor: None }",
        "/* models/hat-far */",
        "Some(SoundSource { sound: SoundId(",
        "water: Some(Water { flow_speed: Vec2::new(0.0, -0.3),",
        "water: None,",
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

#[test]
fn codegen_exports_the_scene_ambient_occlusion() {
    let (mut scene, lib) = (common::scene(), common::library());
    // None without an `ao` block, and with `enabled: false`.
    let (code, _) = codegen::to_rust(&scene, &lib);
    assert!(code.contains("pub const AO: Option<AmbientOcclusion> = None;"));
    scene.environment.ao = Some(trino_scene::AmbientOcclusionDef {
        enabled: false,
        ..Default::default()
    });
    let (code, _) = codegen::to_rust(&scene, &lib);
    assert!(code.contains("pub const AO: Option<AmbientOcclusion> = None;"));
    scene.environment.ao = Some(trino_scene::AmbientOcclusionDef {
        radius: 1.25,
        intensity: 1.5,
        bias: 0.05,
        quality: trino_scene::AoQualityDef::High,
        ..Default::default()
    });
    let (code, _) = codegen::to_rust(&scene, &lib);
    assert!(
        code.contains(
            "pub const AO: Option<AmbientOcclusion> = Some(AmbientOcclusion {\n    radius: 1.25,\n    \
             intensity: 1.5,\n    bias: 0.05,\n    quality: AoQuality::High,\n});"
        ),
        "{code}"
    );
    // TSCN carries no AO (consoles bake theirs): the blob is unchanged.
    let with = bake::to_tscn(&scene, &lib).0;
    scene.environment.ao = None;
    assert_eq!(with, bake::to_tscn(&scene, &lib).0);
}

/// Shadows (ADR 0025, scene v7, TSCN v5): the scene's settings and each
/// entity's Shadow component reach both game-side forms: the receivers'
/// tops (a collider's box, else the model's bounds as drawn), each draw's
/// cast, a prefab part's own; a receiver with no box warns.
#[test]
fn shadows_reach_the_tscn_and_the_generated_code() {
    use trino_core::{ShadowCast, ShadowPlane, Vec3};
    use trino_scene::{Scene, ShadowCastDef, ShadowComponent, ShadowTierDef, ShadowsDef};
    let (mut scene, mut lib) = (common::scene(), common::library());
    // None: no settings, no receivers, in both forms.
    let resolved = resolve(&scene, &lib, 0.0);
    assert_eq!(resolved.shadows, None);
    assert!(resolved.receivers().is_empty());
    let baked = BakedScene::parse(leak(bake::to_tscn(&scene, &lib).0)).unwrap();
    assert_eq!(baked.shadows(), None);
    assert_eq!(baked.receivers().count(), 0);
    let (code, _) = codegen::to_rust(&scene, &lib);
    assert!(code.contains("pub const SHADOWS: Option<ShadowSettings> = None;"));
    assert!(code.contains("pub const RECEIVERS: &[ShadowPlane] = &[\n];"));

    let def = ShadowsDef {
        enabled: true,
        tier_max: ShadowTierDef::Planar,
        map_size: 1024,
        distance: 25.0,
        softness: 90,
        color: [10, 20, 30, 120],
    };
    scene.environment.shadows = Some(def);
    let set = |scene: &mut Scene, name: &str, cast, receive| {
        let e = scene.entities.iter_mut().find(|e| e.name == name).unwrap();
        e.shadow = Some(ShadowComponent {
            cast,
            radius: 0.4,
            receive,
        });
    };
    set(&mut scene, "ground", ShadowCastDef::None, true); // its collider
    set(&mut scene, "crate", ShadowCastDef::Blob, true); // its Auto collider
    set(&mut scene, "hero", ShadowCastDef::Mesh, false);
    set(&mut scene, "pond", ShadowCastDef::Auto, true); // no box: warns
    // A model's bounds, scaled, without a collider.
    let mut top = trino_scene::Entity::new("crate-top")
        .at([6.0, 0.0, 0.0])
        .with_model(trino_scene::ModelComponent::new("models/crate"));
    top.transform.scale = [2.0, 2.0, 2.0];
    top.shadow = Some(ShadowComponent {
        cast: ShadowCastDef::None,
        receive: true,
        ..Default::default()
    });
    scene.entities.push(top);
    // A prefab part's own component.
    let tree = lib.prefabs.get_mut("prefabs/tree").unwrap();
    tree.root.children[0].shadow = Some(ShadowComponent {
        cast: ShadowCastDef::Mesh,
        ..Default::default()
    });

    let resolved = resolve(&scene, &lib, 0.0);
    assert_eq!(resolved.shadows, Some(def.core()));
    let v = Vec3::new;
    assert_eq!(
        resolved.receivers(),
        [
            ShadowPlane::top_of(v(-5.0, -1.0, -5.0), v(5.0, 0.0, 5.0)),
            ShadowPlane::top_of(v(-2.5, 0.0, -0.5), v(-1.5, 1.0, 0.5)),
            ShadowPlane::top_of(v(5.0, 0.0, -1.0), v(7.0, 2.0, 1.0)),
        ]
    );
    assert!(
        resolved
            .warnings
            .iter()
            .any(|w| w.starts_with("pond: a shadow receiver needs a collider")),
        "{:?}",
        resolved.warnings
    );
    let cast = |name: &str| {
        resolved
            .items
            .iter()
            .find(|i| i.name == name)
            .unwrap()
            .params
            .shadow
    };
    assert_eq!(cast("hero"), ShadowCast::Mesh);
    assert_eq!(cast("crate"), ShadowCast::Blob { radius: 0.4 });
    assert_eq!(cast("pond"), ShadowCast::Auto { blob_radius: 0.4 });
    assert_eq!(cast("ground"), ShadowCast::None);
    assert_eq!(cast("coin"), ShadowCast::None, "no component: none");
    assert_eq!(cast("tree-1/tree/trunk"), ShadowCast::Mesh);

    // TSCN v5: the same, read back by the runtime.
    let (blob, warnings) = bake::to_tscn(&scene, &lib);
    assert_eq!(warnings, resolved.warnings);
    let baked = BakedScene::parse(leak(blob)).unwrap();
    assert_eq!(baked.shadows(), resolved.shadows);
    assert_eq!(baked.receivers().collect::<Vec<_>>(), resolved.receivers());
    for (item, node) in exported(&resolved.items).iter().zip(baked.nodes()) {
        assert_eq!(node.params.shadow, item.params.shadow, "{}", item.name);
    }

    // The generated code.
    let (code, _) = codegen::to_rust(&scene, &lib);
    for needle in [
        "pub const SHADOWS: Option<ShadowSettings> = Some(ShadowSettings {\n    \
         tier_max: ShadowTier::Planar,\n    map_size: 1024,\n    distance: 25.0,\n    \
         softness: 90,\n    color: Color::rgba(10, 20, 30, 120),\n});",
        "    ShadowPlane::horizontal(Vec3::new(0.0, 0.0, 0.0), 5.0, 5.0),\n",
        "    ShadowPlane::horizontal(Vec3::new(6.0, 2.0, 0.0), 1.0, 1.0),\n",
        "shadow: ShadowCast::Blob { radius: 0.4 },",
        "shadow: ShadowCast::Auto { blob_radius: 0.4 },",
        "shadow: ShadowCast::Mesh,",
    ] {
        assert!(code.contains(needle), "missing `{needle}` in:\n{code}");
    }
    // Off keeps the tuning in the file, and nothing in the game.
    scene.environment.shadows.as_mut().unwrap().enabled = false;
    let (code, _) = codegen::to_rust(&scene, &lib);
    assert!(code.contains("pub const SHADOWS: Option<ShadowSettings> = None;"));
    let baked = BakedScene::parse(leak(bake::to_tscn(&scene, &lib).0)).unwrap();
    assert_eq!(baked.shadows(), None);
}
