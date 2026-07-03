//! Integration tests: bake fixtures generated on the fly into a temp dir,
//! then assert on the index (snapshot), hybrid resolution, error paths and
//! the in-place reload contract (same handle, new content).

use std::path::Path;

use trino_asset_pipeline::{Platform, bake_all, load_dir};
use trino_core::asset_id;

/// 2x2 PNG, RGBA, solid `color`.
fn write_png(path: &Path, color: [u8; 4]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 2, 2);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    let pixels: Vec<u8> = (0..4).flat_map(|_| color).collect();
    writer.write_image_data(&pixels).unwrap();
}

fn write_wav(path: &Path, value: i16, frames: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 22_050,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for _ in 0..frames {
        writer.write_sample(value).unwrap();
    }
    writer.finalize().unwrap();
}

fn fixture(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(
        root.join("manifest.toml"),
        r#"
version = 1

[sprites.player]
file = "sprites/player.png"
formats = { n64 = "CI4" }

[sounds.beep]
file = "sounds/beep.wav"
"#,
    )
    .unwrap();
    write_png(&root.join("shared/sprites/player.png"), [255, 0, 0, 255]);
    write_wav(&root.join("shared/sounds/beep.wav"), 8192, 32);
}

#[test]
fn bake_produces_deterministic_index_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let (root, out) = (dir.path().join("assets"), dir.path().join("out"));
    fixture(&root);

    let report = bake_all(&root, Platform::Pc, &out).unwrap();
    assert_eq!(report.entries.len(), 2);

    let index = std::fs::read_to_string(out.join("index.toml")).unwrap();
    let player_id = asset_id("sprites/player");
    let beep_id = asset_id("sounds/beep");
    // Sprites bake before sounds; within a kind, manifest (BTreeMap) order.
    let expected = format!(
        "version = 1\n\n\
         [[asset]]\nlogical = \"sprites/player\"\nid = {player_id}\nkind = \"sprite\"\nfile = \"{player_id:08x}.sprite\"\nformat = \"RGBA8\"\n\n\
         [[asset]]\nlogical = \"sounds/beep\"\nid = {beep_id}\nkind = \"sound\"\nfile = \"{beep_id:08x}.sound\"\nformat = \"F32_MONO\"\n"
    );
    // Snapshot: byte-exact. If this changes, the loader and every platform
    // baker must be reviewed together.
    assert_eq!(index, expected);

    // Second bake with no changes touches nothing.
    let report2 = bake_all(&root, Platform::Pc, &out).unwrap();
    assert!(report2.changed_ids().is_empty());
}

#[test]
fn loader_round_trips_baked_data() {
    let dir = tempfile::tempdir().unwrap();
    let (root, out) = (dir.path().join("assets"), dir.path().join("out"));
    fixture(&root);
    bake_all(&root, Platform::Pc, &out).unwrap();

    let assets = load_dir(&out, None).unwrap();
    assert_eq!(assets.sprites.len(), 1);
    assert_eq!(assets.sounds.len(), 1);

    let sprite = &assets.sprites[0];
    assert_eq!(sprite.id, asset_id("sprites/player"));
    assert_eq!((sprite.width, sprite.height), (2, 2));
    assert_eq!(&sprite.rgba[0..4], &[255, 0, 0, 255]);

    let sound = &assets.sounds[0];
    assert_eq!(sound.sample_rate, 22_050);
    assert_eq!(sound.samples.len(), 32);
    assert!((sound.samples[0] - 0.25).abs() < 1e-3); // 8192/32768
}

#[test]
fn platform_override_wins_and_reload_keeps_handle() {
    let dir = tempfile::tempdir().unwrap();
    let (root, out) = (dir.path().join("assets"), dir.path().join("out"));
    fixture(&root);
    bake_all(&root, Platform::Pc, &out).unwrap();
    let v1 = load_dir(&out, None).unwrap();
    assert_eq!(&v1.sprites[0].rgba[0..4], &[255, 0, 0, 255]);

    // "Edit" the master: green now. Rebake reports exactly this handle.
    write_png(&root.join("shared/sprites/player.png"), [0, 255, 0, 255]);
    let report = bake_all(&root, Platform::Pc, &out).unwrap();
    assert_eq!(report.changed_ids(), vec![asset_id("sprites/player")]);

    // Selective reload by handle: same id, new content.
    let v2 = load_dir(&out, Some(&report.changed_ids())).unwrap();
    assert_eq!(v2.sprites.len(), 1);
    assert_eq!(v2.sounds.len(), 0);
    assert_eq!(v2.sprites[0].id, v1.sprites[0].id);
    assert_eq!(&v2.sprites[0].rgba[0..4], &[0, 255, 0, 255]);

    // A PC override beats the shared master for PC only.
    write_png(&root.join("pc/sprites/player.png"), [0, 0, 255, 255]);
    bake_all(&root, Platform::Pc, &out).unwrap();
    let v3 = load_dir(&out, None).unwrap();
    assert_eq!(&v3.sprites[0].rgba[0..4], &[0, 0, 255, 255]);
}

#[test]
fn n64_bake_fails_without_explicit_format() {
    let dir = tempfile::tempdir().unwrap();
    let (root, out) = (dir.path().join("assets"), dir.path().join("out"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("manifest.toml"),
        "version = 1\n[sprites.hero]\nfile = \"sprites/hero.png\"\n",
    )
    .unwrap();
    write_png(&root.join("shared/sprites/hero.png"), [1, 2, 3, 255]);

    // PC is fine (RGBA8 default)...
    bake_all(&root, Platform::Pc, &out).unwrap();
    // ...N64 must fail loudly.
    let err = bake_all(&root, Platform::N64, &out).unwrap_err();
    assert!(err.to_string().contains("no format declared"), "{err}");
}

#[test]
fn missing_source_is_a_bake_error() {
    let dir = tempfile::tempdir().unwrap();
    let (root, out) = (dir.path().join("assets"), dir.path().join("out"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("manifest.toml"),
        "version = 1\n[sprites.ghost]\nfile = \"sprites/ghost.png\"\n",
    )
    .unwrap();
    let err = bake_all(&root, Platform::Pc, &out).unwrap_err();
    assert!(err.to_string().contains("not found"), "{err}");
}
