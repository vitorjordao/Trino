//! Every manifest asset kind end to end: masters in a temp project →
//! `bake_all` → `index.toml` → `load_dir`, plus the per-platform format
//! decisions, the error paths a broken project hits, and the text-level
//! manifest edits the editor uses.

mod common;

use std::path::{Path, PathBuf};

use common::*;
use trino_asset_pipeline::manifest::edit;
use trino_asset_pipeline::{Manifest, Platform, bake_all, load_dir};
use trino_core::text::BitmapFont;
use trino_core::{BakedScene, Mesh, Shape, SpriteId, asset_id};

const MANIFEST: &str = r#"
version = 1

[sprites.hero]
file = "sprites/hero.png"
formats = { n64 = "CI8", "3ds" = "RGB565" }

[sprites.rgb]
file = "sprites/rgb.png"
formats = { n64 = "RGBA5551" }

[sounds.beep]
file = "sounds/beep.wav"

[sounds.hum]
file = "sounds/hum.wav"
loop = true

[music.theme]
file = "music/theme.wav"

[music.sting]
file = "music/theme.wav"
loop = false

[models.tri]
file = "models/tri.gltf"

[models.block]
file = "models/block.prim.ron"
texture = "sprites/hero"

[fonts.pixel]
file = "fonts/pixel.png"
cell = [4, 6]
chars = "AB?"
proportional = false
advance = 5

[blobs.data]
file = "data/level.bin"

[scenes.room]
file = "scenes/room.scene.ron"
"#;

const ROOM: &str = r#"(
    version: 2,
    name: "room",
    environment: (clear: (1, 2, 3)),
    entities: [
        (
            name: "crate",
            transform: (pos: (4.0, 0.0, 0.0), rot: (0.0, 0.0, 0.0), scale: (1.0, 1.0, 1.0)),
            tags: ["solid", "pushable"],
            model: Some((path: "models/block", material: Some("materials/stone"))),
            collider: Some((shape: Auto)),
            sound: Some((path: "sounds/hum", radius: 6.0, volume: 0.5)),
        ),
        (name: "spawn", tags: ["spawn"]),
        (name: "hud-icon", sprite: Some((path: "sprites/hero"))),
    ],
)"#;

/// A complete project in `dir`: `assets/` (manifest + masters + a
/// material) next to `scenes/`. Returns the assets root.
fn project(dir: &Path) -> PathBuf {
    let assets = dir.join("assets");
    let shared = assets.join("shared");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(assets.join("manifest.toml"), MANIFEST).unwrap();
    write_png_with(&shared.join("sprites/hero.png"), 4, 2, |x, _| {
        [x as u8 * 60, 0, 0, 255]
    });
    // RGB (no alpha channel): the bake adds opaque alpha.
    write_png_typed(
        &shared.join("sprites/rgb.png"),
        2,
        1,
        png::ColorType::Rgb,
        &[10, 20, 30, 40, 50, 60],
    );
    write_wav_i16(&shared.join("sounds/beep.wav"), 22_050, 1, &[16_384; 8]);
    // Stereo float: averaged to mono.
    write_wav_f32(
        &shared.join("sounds/hum.wav"),
        11_025,
        2,
        &[0.5, 0.25, -1.0, 0.0],
    );
    write_wav_i16(&shared.join("music/theme.wav"), 32_000, 1, &[-8192, 8192]);
    write_triangle_gltf(&shared.join("models/tri.gltf"));
    std::fs::create_dir_all(shared.join("models")).unwrap();
    std::fs::write(
        shared.join("models/block.prim.ron"),
        "(shape: Box(size: (2.0, 1.0, 1.0)), color: (90, 90, 90, 255))",
    )
    .unwrap();
    // 3 cells of 4x6: 'A' full, 'B' left half, '?' one pixel.
    write_png_with(&shared.join("fonts/pixel.png"), 12, 6, |x, y| match x / 4 {
        0 => [255, 0, 0, 255],
        1 if x % 4 < 2 => [0, 255, 0, 255],
        2 if x % 4 == 0 && y == 0 => [9, 9, 9, 255],
        _ => [0, 0, 0, 0],
    });
    std::fs::create_dir_all(shared.join("data")).unwrap();
    std::fs::write(shared.join("data/level.bin"), [0u8, 1, 2, 254, 255]).unwrap();
    std::fs::create_dir_all(assets.join("materials")).unwrap();
    std::fs::write(
        assets.join("materials/stone.material.ron"),
        "(version: 1, tint: (200, 100, 50, 255), shading: Flat)",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    std::fs::write(dir.join("scenes/room.scene.ron"), ROOM).unwrap();
    assets
}

fn leak(bytes: Vec<u8>) -> &'static [u8] {
    Box::leak(bytes.into_boxed_slice())
}

#[test]
fn every_asset_kind_bakes_indexes_and_loads_on_pc() {
    let dir = tempfile::tempdir().unwrap();
    let assets = project(dir.path());
    let out = dir.path().join("baked");
    let report = bake_all(&assets, Platform::Pc, &out).unwrap();

    // 2 sprites, 2 sounds, 2 music, 2 models, font atlas + table, blob, scene.
    let kinds: Vec<(&str, &str)> = report
        .entries
        .iter()
        .map(|e| (e.logical.as_str(), e.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("sprites/hero", "sprite"),
            ("sprites/rgb", "sprite"),
            ("sounds/beep", "sound"),
            ("sounds/hum", "sound"),
            ("music/sting", "music"),
            ("music/theme", "music"),
            ("models/block", "model"),
            ("models/tri", "model"),
            ("fonts/pixel", "sprite"),
            ("fonts/pixel", "font"),
            ("blobs/data", "blob"),
            ("scenes/room", "blob"),
        ]
    );
    assert!(
        report.entries.iter().all(|e| e.changed),
        "first bake writes"
    );
    let formats: Vec<&str> = report.entries.iter().map(|e| e.format.as_str()).collect();
    assert_eq!(
        formats,
        [
            "RGBA8", "RGBA8", "F32_MONO", "F32_MONO", "F32_MONO", "F32_MONO", "TMDL", "TMDL",
            "RGBA8", "TFNT", "RAW", "TSCN"
        ]
    );
    let index = std::fs::read_to_string(out.join("index.toml")).unwrap();
    for e in &report.entries {
        assert!(index.contains(&format!("file = \"{}\"", e.file)), "{index}");
    }

    let loaded = load_dir(&out, None).unwrap();

    // Sprites: the RGB master gained alpha 255; the font atlas is a sprite
    // under the font's own handle.
    assert_eq!(loaded.sprites.len(), 3);
    let rgb = loaded
        .sprites
        .iter()
        .find(|s| s.logical == "sprites/rgb")
        .unwrap();
    assert_eq!(rgb.rgba, [10, 20, 30, 255, 40, 50, 60, 255]);
    let hero = loaded
        .sprites
        .iter()
        .find(|s| s.id == asset_id("sprites/hero"))
        .unwrap();
    assert_eq!((hero.width, hero.height), (4, 2));
    assert_eq!(&hero.rgba[4..8], &[60, 0, 0, 255]);

    // Sounds vs music land in separate registries; stereo is averaged.
    assert_eq!(loaded.sounds.len(), 2);
    assert_eq!(loaded.music.len(), 2);
    let beep = loaded
        .sounds
        .iter()
        .find(|s| s.logical == "sounds/beep")
        .unwrap();
    assert_eq!((beep.sample_rate, beep.samples.len()), (22_050, 8));
    assert!((beep.samples[0] - 0.5).abs() < 1e-4);
    let hum = loaded
        .sounds
        .iter()
        .find(|s| s.logical == "sounds/hum")
        .unwrap();
    assert_eq!(hum.sample_rate, 11_025);
    assert_eq!(hum.samples, [0.375, -0.5]);
    let theme = loaded
        .music
        .iter()
        .find(|m| m.id == asset_id("music/theme"))
        .unwrap();
    assert_eq!(theme.sample_rate, 32_000);
    assert!((theme.samples[0] + 0.25).abs() < 1e-4);

    // Models: valid TMDL; the textured fake object keeps UVs and its
    // default texture (TXUV + TXID).
    assert_eq!(loaded.models.len(), 2);
    let block = loaded
        .models
        .iter()
        .find(|m| m.logical == "models/block")
        .unwrap();
    let mesh = Mesh::from_tmdl(&block.tmdl).unwrap();
    assert!(mesh.has_uvs());
    assert_eq!(
        mesh.default_texture(),
        Some(SpriteId::from_path("sprites/hero"))
    );
    let (min, max) = mesh.aabb().unwrap();
    assert!((max.x - min.x - 2.0).abs() < 1e-5 && (max.y - min.y - 1.0).abs() < 1e-5);
    let tri = loaded
        .models
        .iter()
        .find(|m| m.logical == "models/tri")
        .unwrap();
    let tri = Mesh::from_tmdl(&tri.tmdl).unwrap();
    assert_eq!((tri.vertex_count, tri.index_count), (3, 3));
    assert!(!tri.has_uvs(), "untextured models drop their UVs");

    // Fonts: the table parses and holds the listed chars, monospaced at the
    // declared advance.
    assert_eq!(loaded.fonts.len(), 1);
    let font = BitmapFont::parse(&loaded.fonts[0].table).unwrap();
    assert_eq!((font.glyph_count, font.line_height), (3, 6));
    let a = font.glyph('A').unwrap();
    let b = font.glyph('B').unwrap();
    assert_eq!((a.x, a.w, a.advance), (0, 4, 5));
    assert_eq!((b.x, b.w), (4, 4));
    assert!(font.glyph('C').is_none());
    assert_eq!(font.glyph_or_fallback('C'), font.glyph('?'));
    let atlas = loaded
        .sprites
        .iter()
        .find(|s| s.id == loaded.fonts[0].id)
        .unwrap();
    assert_eq!((atlas.width, atlas.height), (12, 6));
    assert_eq!(&atlas.rgba[0..4], &[255, 255, 255, 255], "ink is white");
    assert_eq!(&atlas.rgba[24..28], &[0, 0, 0, 0], "transparent is black");

    // Blobs: byte for byte.
    assert_eq!(loaded.blobs.len(), 2);
    let data = loaded
        .blobs
        .iter()
        .find(|b| b.logical == "blobs/data")
        .unwrap();
    assert_eq!(data.bytes, [0, 1, 2, 254, 255]);

    // Scenes: a TSCN blob the runtime parses, with the material, the Auto
    // collider (the model's baked box), the sound and the tags resolved;
    // the 2D-only sprite entity is not exported.
    let room = loaded
        .blobs
        .iter()
        .find(|b| b.id == asset_id("scenes/room"))
        .unwrap();
    let scene = BakedScene::parse(leak(room.bytes.clone())).unwrap();
    assert_eq!(scene.node_count(), 2);
    assert_eq!(scene.clear(), trino_core::Color::rgb(1, 2, 3));
    let node = scene.find("crate").unwrap();
    assert_eq!(
        node.model,
        Some(trino_core::ModelId::from_path("models/block"))
    );
    assert_eq!(node.params.tint, trino_core::Color::rgba(200, 100, 50, 255));
    assert_eq!(node.params.shading, trino_core::Shading::Flat);
    assert!(node.has_tag("pushable") && node.has_tag("solid") && !node.has_tag("spawn"));
    let collider = node.collider.unwrap();
    assert!(!collider.trigger);
    match collider.shape {
        Shape::Box { half } => {
            assert!((half.x - 1.0).abs() < 1e-5 && (half.y - 0.5).abs() < 1e-5);
        }
        other => panic!("{other:?}"),
    }
    let world = node.world_aabb().unwrap();
    assert!((world.center().x - 4.0).abs() < 1e-5);
    let sound = node.sound.unwrap();
    assert_eq!(sound.sound, trino_core::SoundId::from_path("sounds/hum"));
    assert_eq!((sound.radius, sound.volume), (6.0, 0.5));
    let spawn = scene.find("spawn").unwrap();
    assert!(spawn.model.is_none() && spawn.has_tag("spawn"));

    // Loop flags (they matter to the console bakers).
    let manifest = Manifest::load(&assets.join("manifest.toml")).unwrap();
    assert!(manifest.sounds["hum"].sound_loops());
    assert!(!manifest.sounds["beep"].sound_loops());
    assert!(manifest.music["theme"].music_loops());
    assert!(!manifest.music["sting"].music_loops());

    // A filtered load only returns the requested handles.
    let only = load_dir(&out, Some(&[asset_id("blobs/data")])).unwrap();
    assert_eq!(only.blobs.len(), 1);
    assert!(only.sprites.is_empty() && only.models.is_empty() && only.fonts.is_empty());

    // Rebaking unchanged masters touches nothing; an edited blob and model
    // are the only changed handles.
    assert!(
        bake_all(&assets, Platform::Pc, &out)
            .unwrap()
            .changed_ids()
            .is_empty()
    );
    std::fs::write(assets.join("shared/data/level.bin"), [7u8]).unwrap();
    std::fs::write(
        assets.join("shared/models/block.prim.ron"),
        "(shape: Box(size: (3.0, 1.0, 1.0)))",
    )
    .unwrap();
    let mut changed = bake_all(&assets, Platform::Pc, &out).unwrap().changed_ids();
    changed.sort();
    // The scene's Auto collider follows the model's new bounds.
    let mut expected = vec![
        asset_id("blobs/data"),
        asset_id("models/block"),
        asset_id("scenes/room"),
    ];
    expected.sort();
    assert_eq!(changed, expected);
}

#[test]
fn console_bakes_record_and_validate_each_platforms_format() {
    let dir = tempfile::tempdir().unwrap();
    let assets = project(dir.path());
    let format_of = |platform: Platform, logical: &str, kind: &str| {
        let out = dir.path().join(platform.key());
        let report = bake_all(&assets, platform, &out).unwrap();
        report
            .entries
            .iter()
            .find(|e| e.logical == logical && e.kind == kind)
            .unwrap()
            .format
            .clone()
    };
    assert_eq!(format_of(Platform::N64, "sprites/hero", "sprite"), "CI8");
    assert_eq!(
        format_of(Platform::N64, "sprites/rgb", "sprite"),
        "RGBA5551"
    );
    assert_eq!(
        format_of(Platform::N64, "fonts/pixel", "sprite"),
        "CI4",
        "fonts default to CI4 on the N64"
    );
    assert_eq!(
        format_of(Platform::N3ds, "sprites/hero", "sprite"),
        "RGB565"
    );
    assert_eq!(format_of(Platform::N3ds, "sprites/rgb", "sprite"), "RGBA8");
    assert_eq!(format_of(Platform::N3ds, "fonts/pixel", "sprite"), "RGBA8");

    // Platform names round-trip; overrides live under the key.
    for p in [Platform::Pc, Platform::N64, Platform::N3ds] {
        assert_eq!(Platform::parse(p.key()), Some(p));
        assert!(
            p.supported_texture_formats()
                .contains(&p.default_texture_format().unwrap_or("CI4"))
        );
    }
    assert_eq!(Platform::parse("n3ds"), Some(Platform::N3ds));
    assert_eq!(Platform::parse("gba"), None);

    // A 3DS-only override beats the master on the 3DS only.
    write_png(&assets.join("3ds/sprites/rgb.png"), 1, 1, [1, 2, 3, 4]);
    let out = dir.path().join("3ds-over");
    bake_all(&assets, Platform::N3ds, &out).unwrap();
    let rgb = load_dir(&out, Some(&[asset_id("sprites/rgb")])).unwrap();
    assert_eq!(rgb.sprites[0].rgba, [1, 2, 3, 4]);
    let pc = dir.path().join("pc-over");
    bake_all(&assets, Platform::Pc, &pc).unwrap();
    let rgb = load_dir(&pc, Some(&[asset_id("sprites/rgb")])).unwrap();
    assert_eq!(rgb.sprites[0].width, 2);

    // A declared format the platform cannot represent fails the bake.
    let text = std::fs::read_to_string(assets.join("manifest.toml")).unwrap();
    std::fs::write(
        assets.join("manifest.toml"),
        text.replace("\"3ds\" = \"RGB565\"", "\"3ds\" = \"CI4\""),
    )
    .unwrap();
    let err = bake_all(&assets, Platform::N3ds, &dir.path().join("bad")).unwrap_err();
    assert!(err.to_string().contains("not supported on 3ds"), "{err}");
}

#[test]
fn a_broken_project_reports_every_error_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path().join("assets");
    let shared = assets.join("shared");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(
        assets.join("manifest.toml"),
        r#"
version = 1
[sprites.gray]
file = "gray.png"
[sounds.surround]
file = "surround.wav"
[music.gone]
file = "gone.wav"
[models.empty]
file = "empty.gltf"
[models.bad-prim]
file = "bad.prim.ron"
[fonts.nocell]
file = "font.png"
[fonts.nosize]
file = "font.ttf"
[fonts.bitmap]
file = "font.bmp"
[fonts.ragged]
file = "ragged.png"
cell = [4, 4]
[fonts.toomany]
file = "font.png"
cell = [2, 2]
chars = "abcdefgh"
[blobs.missing]
file = "nope.bin"
[scenes.missing]
file = "scenes/nope.scene.ron"
"#,
    )
    .unwrap();
    write_png_typed(
        &shared.join("gray.png"),
        1,
        1,
        png::ColorType::Grayscale,
        &[7],
    );
    write_wav_i16(&shared.join("surround.wav"), 8000, 3, &[0, 0, 0]);
    std::fs::write(
        shared.join("empty.gltf"),
        r#"{"asset": {"version": "2.0"}}"#,
    )
    .unwrap();
    std::fs::write(shared.join("bad.prim.ron"), "(shape: Sphere(radius: -1.0))").unwrap();
    write_png(&shared.join("font.png"), 4, 2, [255; 4]);
    write_png(&shared.join("ragged.png"), 6, 4, [255; 4]);
    std::fs::write(shared.join("font.ttf"), b"not a font").unwrap();
    std::fs::write(shared.join("font.bmp"), b"BM").unwrap();

    let err = bake_all(&assets, Platform::Pc, &dir.path().join("out")).unwrap_err();
    let text = err.to_string();
    assert!(text.starts_with(&format!("asset bake failed with {} error(s)", err.0.len())));
    for needle in [
        "unsupported PNG color type",
        "3 channels not supported",
        "not found",
        "glTF contains no mesh",
        "`radius` must be > 0",
        "need `cell = [w, h]`",
        "need `size = <pixels>`",
        "unsupported font master `.bmp`",
        "not a whole number of 4x4 cells",
        "lists 8 characters but the grid has 2 cells",
        "nope.bin",
        "scenes/nope.scene.ron",
    ] {
        assert!(text.contains(needle), "missing `{needle}` in:\n{text}");
    }
    assert_eq!(err.0.len(), 12, "{text}");
    assert!(
        !dir.path().join("out/index.toml").exists(),
        "a failed bake writes no index"
    );
}

#[test]
fn scene_bakes_fail_on_library_errors_and_scene_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let assets = project(dir.path());
    // A scene warning (unknown material) fails the bake: baked levels must
    // be clean.
    std::fs::write(
        dir.path().join("scenes/room.scene.ron"),
        ROOM.replace("materials/stone", "materials/marble"),
    )
    .unwrap();
    let err = bake_all(&assets, Platform::Pc, &dir.path().join("o1")).unwrap_err();
    assert!(
        err.to_string()
            .contains("unknown material `materials/marble`"),
        "{err}"
    );
    // A library that fails to load (broken prefab) also fails it.
    std::fs::write(dir.path().join("scenes/room.scene.ron"), ROOM).unwrap();
    std::fs::create_dir_all(dir.path().join("prefabs")).unwrap();
    std::fs::write(dir.path().join("prefabs/broken.prefab.ron"), "(oops").unwrap();
    let err = bake_all(&assets, Platform::Pc, &dir.path().join("o2")).unwrap_err();
    assert!(err.to_string().contains("library error"), "{err}");
}

/// Two names whose logical paths under `prefix` share a 32-bit handle.
/// FNV-1a has no structure to exploit; a birthday search over ~10^5
/// names finds a pair.
fn colliding_names(prefix: &str) -> (String, String) {
    let mut seen = std::collections::HashMap::new();
    for i in 0u32.. {
        let name = format!("n{i}");
        if let Some(prev) = seen.insert(asset_id(&format!("{prefix}/{name}")), name.clone()) {
            return (prev, name);
        }
    }
    unreachable!()
}

#[test]
fn colliding_handles_fail_the_bake_for_every_kind() {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path().join("assets");
    write_png(&assets.join("shared/p.png"), 1, 1, [255; 4]);
    std::fs::create_dir_all(dir.path().join("scenes")).unwrap();
    for (table, prefix) in [
        ("sprites", "sprites"),
        ("sounds", "sounds"),
        ("music", "music"),
        ("models", "models"),
        ("fonts", "fonts"),
        ("blobs", "blobs"),
        ("scenes", "scenes"),
    ] {
        let (a, b) = colliding_names(prefix);
        assert_ne!(a, b);
        // Only the first of the pair is baked: the second is refused before
        // its source is even looked up.
        std::fs::write(
            assets.join("manifest.toml"),
            format!(
                "version = 1\n[{table}.{a}]\nfile = \"p.png\"\n[{table}.{b}]\nfile = \"p.png\"\n"
            ),
        )
        .unwrap();
        let err = bake_all(&assets, Platform::Pc, &dir.path().join("out")).unwrap_err();
        let collisions: Vec<&String> = err
            .0
            .iter()
            .filter(|e| e.contains("hash collision"))
            .collect();
        assert_eq!(collisions.len(), 1, "{table}: {err}");
        assert!(collisions[0].contains(&format!("`{prefix}/{a}`")), "{err}");
    }
}

#[test]
fn an_unwritable_output_directory_is_a_bake_error() {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path().join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join("manifest.toml"), "version = 1\n").unwrap();
    let file = dir.path().join("a-file");
    std::fs::write(&file, "").unwrap();
    let err = bake_all(&assets, Platform::Pc, &file.join("out")).unwrap_err();
    assert!(err.to_string().contains("cannot create"), "{err}");
}

#[test]
fn manifests_reject_unknown_fields_versions_and_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path().join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    let out = dir.path().join("out");

    let err = bake_all(&assets, Platform::Pc, &out).unwrap_err();
    assert!(err.to_string().contains("cannot read"), "{err}");

    std::fs::write(assets.join("manifest.toml"), "version = 2\n").unwrap();
    let err = bake_all(&assets, Platform::Pc, &out).unwrap_err();
    assert!(
        err.to_string().contains("unsupported manifest version 2"),
        "{err}"
    );

    std::fs::write(
        assets.join("manifest.toml"),
        "version = 1\n[sprites.p]\nfile = \"p.png\"\nfromats = {}\n",
    )
    .unwrap();
    let err = bake_all(&assets, Platform::Pc, &out).unwrap_err();
    assert!(err.to_string().contains("fromats"), "{err}");

    // An empty manifest bakes to an empty index.
    std::fs::write(assets.join("manifest.toml"), "version = 1\n").unwrap();
    let report = bake_all(&assets, Platform::Pc, &out).unwrap();
    assert!(report.entries.is_empty());
    let loaded = load_dir(&out, None).unwrap();
    assert!(loaded.sprites.is_empty() && loaded.blobs.is_empty());
}

#[test]
fn the_loader_rejects_corrupt_baked_output() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path();
    let check = |kind: &str, bytes: &[u8], needle: &str| {
        std::fs::write(
            out.join("index.toml"),
            format!(
                "version = 1\n[[asset]]\nlogical = \"x/y\"\nid = 7\nkind = \"{kind}\"\nfile = \"a.bin\"\nformat = \"F\"\n"
            ),
        )
        .unwrap();
        std::fs::write(out.join("a.bin"), bytes).unwrap();
        let err = load_dir(out, None).err().unwrap();
        assert!(err.contains(needle), "{kind}: {err}");
    };
    let mut sprite = b"TSPR".to_vec();
    sprite.extend_from_slice(&2u32.to_le_bytes());
    sprite.extend_from_slice(&2u32.to_le_bytes());
    sprite.extend_from_slice(&[0; 8]); // 2x2 needs 16 bytes
    check("sprite", b"XXXX12345678", "bad sprite blob");
    // Regression: blobs shorter than the magic used to panic in `split_at`.
    check("sprite", b"TS", "bad sprite blob");
    check("sound", b"", "bad sound blob");
    check("sprite", &sprite, "truncated sprite data");
    let mut sound = b"TSND".to_vec();
    sound.extend_from_slice(&8000u32.to_le_bytes());
    sound.extend_from_slice(&3u32.to_le_bytes());
    sound.extend_from_slice(&[0; 4]);
    check("sound", b"TSPR00000000", "bad sound blob");
    check("music", &sound, "truncated sound data");
    check("model", b"GLTF", "bad model blob");
    check("font", b"TFNT\x02\x00", "bad font table");
    check("texture", b"", "unknown asset kind `texture`");

    std::fs::write(out.join("index.toml"), "version = 9\n").unwrap();
    assert!(
        load_dir(out, None)
            .err()
            .unwrap()
            .contains("unsupported index version 9")
    );
    std::fs::write(
        out.join("index.toml"),
        "version = 1\n[[asset]]\nlogical = \"x\"\nid = 1\nkind = \"blob\"\nfile = \"gone.bin\"\n",
    )
    .unwrap();
    assert!(load_dir(out, None).err().unwrap().contains("gone.bin"));
    std::fs::remove_file(out.join("index.toml")).unwrap();
    assert!(load_dir(out, None).err().unwrap().contains("cannot read"));
}

#[test]
fn manifest_edits_keep_comments_and_retarget_a_model_under_one_handle() {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path().join("assets");
    let shared = assets.join("shared");
    std::fs::create_dir_all(shared.join("models")).unwrap();
    std::fs::write(
        shared.join("models/rock.prim.ron"),
        "(shape: Sphere(radius: 0.5), segments: 6)",
    )
    .unwrap();
    write_triangle_gltf(&shared.join("models/rock.gltf"));
    write_png(&shared.join("sprites/moss.png"), 2, 2, [0, 120, 0, 255]);

    let name = edit::name_from_stem("Big Rock (v2)");
    assert_eq!(name, "big-rock-v2");
    assert!(edit::valid_name(&name));
    assert!(!edit::valid_name("Big Rock") && !edit::valid_name(""));

    let base = "# my project\nversion = 1\n";
    let text = edit::add_model(base, &name, "models/rock.prim.ron").unwrap();
    let text = edit::add_sprite(&text, "moss", "sprites/moss.png", "CI4").unwrap();
    assert!(text.starts_with("# my project"));
    assert!(edit::has_table(&text, "models", &name));
    assert!(edit::has_table(&text, "sprites", "moss"));
    assert!(!edit::has_table(&text, "models", "moss"));
    assert!(edit::add_model(&text, &name, "x").is_err());
    assert!(edit::add_sprite(&text, "moss", "x", "CI4").is_err());
    std::fs::write(assets.join("manifest.toml"), &text).unwrap();

    // The edited manifest bakes for the N64 too (the sprite got a format).
    bake_all(&assets, Platform::N64, &dir.path().join("n64")).unwrap();
    let out = dir.path().join("pc");
    bake_all(&assets, Platform::Pc, &out).unwrap();
    let id = asset_id(&format!("models/{name}"));
    let before = load_dir(&out, Some(&[id])).unwrap().models.remove(0);

    // Placeholder → real art: same handle, new mesh.
    let text = edit::set_model_file(&text, &name, "models/rock.gltf").unwrap();
    assert!(text.starts_with("# my project"));
    std::fs::write(assets.join("manifest.toml"), &text).unwrap();
    let report = bake_all(&assets, Platform::Pc, &out).unwrap();
    assert_eq!(report.changed_ids(), vec![id]);
    let after = load_dir(&out, Some(&[id])).unwrap().models.remove(0);
    assert_eq!(after.id, before.id);
    assert_eq!(Mesh::from_tmdl(&after.tmdl).unwrap().vertex_count, 3);
    assert!(Mesh::from_tmdl(&before.tmdl).unwrap().vertex_count > 3);

    assert!(
        edit::set_model_file(&text, "ghost", "x")
            .unwrap_err()
            .contains("not found")
    );
    let no_file = "version = 1\n[models.odd]\nanim_fps = 10\n[sprites.s]\nfile = \"s.png\"\n";
    assert!(
        edit::set_model_file(no_file, "odd", "x")
            .unwrap_err()
            .contains("no `file` line")
    );
}
