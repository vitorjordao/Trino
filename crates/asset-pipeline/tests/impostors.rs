//! The impostor bake end to end, in a temp project: a fake-object model and
//! its `impostors/<name>.impostor.ron` → `impostor::bake` → atlases (PC/3DS
//! RGBA, N64 indexed CI4 that fits TMEM), `TIMP` tables and manifest
//! entries → `bake_all` → a scene LOD level drawing it on each platform.

use std::path::Path;

use trino_asset_pipeline::impostor::{self, ImpostorDef, N64_CI4_TEXELS};
use trino_asset_pipeline::{Platform, bake_all, bake_scene_for, load_dir};
use trino_core::{BakedScene, Impostor, LodPick, SpriteId};

/// A project with one model (a box) and the impostor settings for it.
fn project(dir: &Path) {
    let assets = dir.join("assets");
    std::fs::create_dir_all(assets.join("shared/models")).unwrap();
    std::fs::write(
        assets.join("shared/models/block.prim.ron"),
        "(shape: Box(size: (1.0, 2.0, 1.0)), color: (200, 180, 160, 255), pivot: Bottom)",
    )
    .unwrap();
    std::fs::write(
        assets.join("manifest.toml"),
        "version = 1\n\n[models.block]\nfile = \"models/block.prim.ron\"\n",
    )
    .unwrap();
    let def = ImpostorDef {
        views: 8,
        tile: 32,
        pitch: 10.0,
        ..ImpostorDef::new("models/block")
    };
    impostor::save_def(&assets, "block", &def).unwrap();
}

fn luminance(atlas: &impostor::Atlas, view: u8) -> f32 {
    let r = atlas.impostor.region(view);
    let (mut sum, mut n) = (0u32, 0u32);
    for y in r.pos.y as u32..(r.pos.y + r.size.y) as u32 {
        for x in r.pos.x as u32..(r.pos.x + r.size.x) as u32 {
            let p = atlas.texel(x, y);
            if p[3] != 0 {
                sum += p[0] as u32 + p[1] as u32 + p[2] as u32;
                n += 1;
            }
        }
    }
    sum as f32 / n.max(1) as f32
}

#[test]
fn bake_writes_atlases_tables_and_manifest_entries() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let assets = tmp.path().join("assets");
    let report = impostor::bake(&assets, "block").unwrap();
    assert!(report.manifest_changed);
    assert_eq!(report.written.len(), 4, "{:?}", report.written);

    // PC/3DS atlas: 8 views of 32 px in a 4x2 grid, each with the model
    // inside a clear 1-texel margin.
    let pc = &report.pc;
    assert_eq!((pc.width, pc.height), (128, 64));
    assert_eq!(
        pc.impostor.sprite,
        SpriteId::from_path("sprites/impostor-block")
    );
    for v in 0..8 {
        let r = pc.impostor.region(v);
        assert!(
            pc.opaque_in(v) > 100,
            "view {v} is drawn: {}",
            pc.opaque_in(v)
        );
        for i in 0..32 {
            let (x0, y0) = (r.pos.x as u32, r.pos.y as u32);
            for (x, y) in [
                (x0 + i, y0),
                (x0, y0 + i),
                (x0 + i, y0 + 31),
                (x0 + 31, y0 + i),
            ] {
                assert_eq!(pc.texel(x, y)[3], 0, "view {v}: margin at {x},{y}");
            }
        }
    }
    // The default light comes from -X/-Z: the views from there (yaw 270
    // and 180) show lit faces, the ones from +X/+Z the shaded ones — the
    // atlas is the right way round.
    assert!(
        luminance(pc, 6) > luminance(pc, 2) + 20.0,
        "-X lit, +X shaded"
    );
    assert!(
        luminance(pc, 4) > luminance(pc, 0) + 20.0,
        "-Z lit, +Z shaded"
    );
    // The quad covers the model's bounding sphere.
    let sphere = (0.5f32 * 0.5 + 1.0 + 0.5 * 0.5).sqrt();
    assert!(pc.impostor.size > sphere * 2.0 && pc.impostor.size < sphere * 2.3);
    assert!(
        (pc.impostor.center.y - 1.0).abs() < 1e-4,
        "{:?}",
        pc.impostor.center
    );

    // N64: smaller tiles, so the whole atlas fits TMEM as CI4.
    let n64 = &report.n64;
    assert_eq!(n64.impostor.tile, 16);
    assert!(n64.width * n64.height <= N64_CI4_TEXELS);
    let png = std::fs::read(assets.join("n64/impostors/block.png")).unwrap();
    let dec = png::Decoder::new(std::io::Cursor::new(png));
    let reader = dec.read_info().unwrap();
    let info = reader.info();
    assert_eq!(info.color_type, png::ColorType::Indexed);
    assert_eq!(info.bit_depth, png::BitDepth::Four);
    assert!(info.palette.as_ref().unwrap().len() <= 16 * 3);
    assert_eq!(info.trns.as_ref().unwrap()[0], 0, "index 0 is clear");

    // The tables parse to the atlases' descriptors and name the model.
    for (file, atlas) in [("shared", pc), ("n64", n64)] {
        let bytes = std::fs::read(assets.join(format!("{file}/impostors/block.timp"))).unwrap();
        assert_eq!(Impostor::parse(&bytes).unwrap(), atlas.impostor);
        assert_ne!(Impostor::model_hash(&bytes), 0);
    }
    let manifest = std::fs::read_to_string(assets.join("manifest.toml")).unwrap();
    assert!(manifest.contains("[sprites.impostor-block]"));
    assert!(manifest.contains("formats = { n64 = \"CI4\" }"));
    assert!(manifest.contains("[blobs.impostor-block]"));

    // Baking again changes nothing (deterministic bytes, entries once).
    let again = impostor::bake(&assets, "block").unwrap();
    assert!(again.written.is_empty() && !again.manifest_changed);
    assert_eq!(again.pc.rgba, report.pc.rgba);
    assert_eq!(
        std::fs::read_to_string(assets.join("manifest.toml")).unwrap(),
        manifest
    );
    assert!(impostor::check(&assets, "block").is_ok());

    // A model edit makes it stale until it is baked again.
    std::fs::write(
        assets.join("shared/models/block.prim.ron"),
        "(shape: Box(size: (1.0, 3.0, 1.0)), color: (200, 180, 160, 255), pivot: Bottom)",
    )
    .unwrap();
    let err = impostor::check(&assets, "block").unwrap_err();
    assert!(
        err.contains("stale") && err.contains("impostor bake block"),
        "{err}"
    );
    impostor::bake(&assets, "block").unwrap();
    assert!(impostor::check(&assets, "block").is_ok());
}

#[test]
fn baked_impostors_reach_the_bake_and_scene_lod_levels() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let assets = tmp.path().join("assets");
    impostor::bake(&assets, "block").unwrap();

    // The PC bake loads the atlas as a sprite and the table as a blob.
    let out = tmp.path().join("baked");
    bake_all(&assets, Platform::Pc, &out).unwrap();
    let loaded = load_dir(&out, None).unwrap();
    let sprite = SpriteId::from_path("sprites/impostor-block").0;
    let atlas = loaded.sprites.iter().find(|s| s.id == sprite).unwrap();
    assert_eq!((atlas.width, atlas.height), (128, 64));
    let blob = trino_core::BlobId::from_path("blobs/impostor-block").0;
    let table = loaded.blobs.iter().find(|b| b.id == blob).unwrap();
    assert_eq!(Impostor::parse(&table.bytes).unwrap().tile, 32);

    // Descriptors per platform for scenes; the 3DS shares the PC atlas.
    let per = &impostor::baked(&assets)["impostors/block"];
    assert_eq!(per[0].unwrap().tile, 32);
    assert_eq!(per[1].unwrap().tile, 16);
    assert_eq!(per[2], per[0]);

    // A scene LOD chain: the mesh up close, the impostor from 20 m — not
    // on the N64, where its level is `Never`.
    std::fs::create_dir_all(tmp.path().join("scenes")).unwrap();
    std::fs::write(
        tmp.path().join("scenes/yard.scene.ron"),
        r#"(version: 3, name: "yard", entities: [(
            name: "block",
            model: Some((path: "models/block", lod: [
                (distance: 20.0, impostor: Some("impostors/block"), platforms: (n64: Some(Never))),
                (distance: 60.0, platforms: (n64: Some(At(40.0)))),
            ])),
        )])"#,
    )
    .unwrap();
    let leak = |b: Vec<u8>| -> &'static [u8] { Box::leak(b.into_boxed_slice()) };
    let far = trino_core::Vec3::new(0.0, 1.0, 30.0);
    for (platform, want_far, cull_at) in [
        (Platform::Pc, Some(32u16), 60.0),
        (Platform::N3ds, Some(32), 60.0),
        (Platform::N64, None, 40.0),
    ] {
        let blob = bake_scene_for(&assets, "scenes/yard.scene.ron", platform).unwrap();
        let scene = BakedScene::parse(leak(blob)).unwrap();
        let node = scene.find("block").unwrap();
        match (node.lod_from(far), want_far) {
            (LodPick::Impostor(i), Some(tile)) => assert_eq!(i.tile, tile, "{platform:?}"),
            (LodPick::Model(_), None) => {}
            (other, _) => panic!("{platform:?}: {other:?}"),
        }
        let gone = trino_core::Vec3::new(0.0, 1.0, cull_at + 0.5);
        assert_eq!(node.lod_from(gone), LodPick::None, "{platform:?}");
    }
}

#[test]
fn bad_settings_and_missing_models_fail_with_a_reason() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let assets = tmp.path().join("assets");
    let err = impostor::bake(&assets, "nope").unwrap_err();
    assert!(err.contains("nope.impostor.ron"), "{err}");
    impostor::save_def(&assets, "ghost", &ImpostorDef::new("models/ghost")).unwrap();
    let err = impostor::bake(&assets, "ghost").unwrap_err();
    assert!(err.contains("not in the manifest"), "{err}");
    assert!(impostor::bake(&assets, "Bad Name").is_err());
    let err = impostor::check(&assets, "block").unwrap_err();
    assert!(err.contains("not baked"), "{err}");
    let def = ImpostorDef {
        n64_tile: Some(64),
        ..impostor::load_def(&assets, "block").unwrap()
    };
    impostor::save_def(&assets, "block", &def).unwrap();
    let err = impostor::bake(&assets, "block").unwrap_err();
    assert!(err.contains("TMEM"), "{err}");
    assert_eq!(impostor::names(&assets), ["block", "ghost"]);
}
