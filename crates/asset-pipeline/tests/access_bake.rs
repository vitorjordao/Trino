//! Accessibility data as a generated blob (issue #60): `assets/palettes.ron`,
//! the manifest's captions and icons and `ui/themes/high_contrast.ron` bake
//! into `access/data` on every target with no manifest entry, read back with
//! the core's parser, count in the memory plan; a palette that a mode can't
//! tell apart fails the bake, and the manifest refuses broken captions and
//! icons.

use std::path::Path;

use trino_asset_pipeline::{Manifest, Platform, access, bake_all, memory};
use trino_core::access::data::LOGICAL;
use trino_core::{AccessData, Color, CvdMode, HwTier, SpriteId, asset_id};

const PALETTES: &str = r#"(
    version: 1,
    colors: {
        "team.home": (base: (200, 40, 40), deuteranopia: (240, 160, 0), protanopia: (240, 160, 0)),
        "team.away": (base: (40, 160, 60), deuteranopia: (40, 90, 220), protanopia: (40, 90, 220), grayscale: (20, 20, 20)),
    },
    distinct: [["team.home", "team.away"]],
)"#;

const MANIFEST: &str = r#"version = 1

[sprites.kit]
file = "sprites/kit.png"
formats = { n64 = "CI4" }
cvd_icon = "sprites/stripes"

[sprites.stripes]
file = "sprites/stripes.png"
formats = { n64 = "RGBA5551" }

[sounds.whistle]
file = "sounds/whistle.wav"
caption = "cap.whistle"
important = true
direction = true
caption_seconds = 1.5

[sounds.step]
file = "sounds/step.wav"
"#;

fn project(root: &Path, palettes: &str) {
    std::fs::create_dir_all(root.join("assets")).unwrap();
    std::fs::create_dir_all(root.join("ui/themes")).unwrap();
    std::fs::write(root.join("assets/manifest.toml"), "version = 1\n").unwrap();
    std::fs::write(root.join("assets/palettes.ron"), palettes).unwrap();
    std::fs::write(
        root.join("ui/themes/high_contrast.ron"),
        "(focus: Some((0, 200, 255, 255)))",
    )
    .unwrap();
}

#[test]
fn palettes_and_the_theme_bake_on_every_target_and_read_back() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path(), PALETTES);
    for platform in [Platform::Pc, Platform::Web, Platform::N64, Platform::N3ds] {
        let out = dir.path().join(format!("out-{platform:?}"));
        let report = bake_all(&dir.path().join("assets"), platform, &out).unwrap();
        let e = report
            .entries
            .iter()
            .find(|e| e.logical == LOGICAL)
            .unwrap_or_else(|| panic!("{platform:?}: no {LOGICAL}"));
        assert_eq!(
            (e.kind, e.id, e.format.as_str()),
            ("blob", asset_id(LOGICAL), "TACC")
        );
        let bytes = std::fs::read(out.join(&e.file)).unwrap();
        let d = AccessData::parse(&bytes).unwrap();
        assert_eq!(
            d.color("team.home", CvdMode::Off),
            Some(Color::rgb(200, 40, 40))
        );
        assert_eq!(
            d.color("team.home", CvdMode::Deuteranopia),
            Some(Color::rgb(240, 160, 0))
        );
        // A mode without an alternative keeps the base.
        assert_eq!(
            d.color("team.home", CvdMode::Tritanopia),
            Some(Color::rgb(200, 40, 40))
        );
        let t = d.contrast_theme().unwrap();
        assert_eq!(t.focus, Color::rgb(0, 200, 255));
        assert_eq!(t.panel, trino_core::ui::Theme::HIGH_CONTRAST.panel);
    }
    let plan = memory::plan(&dir.path().join("assets"), Platform::N64, HwTier::Base).unwrap();
    let a = plan
        .assets
        .iter()
        .find(|a| a.logical == LOGICAL)
        .expect("access/data in the plan");
    assert!(a.bytes > 60);
}

#[test]
fn captions_and_icons_come_from_the_manifest() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("assets/manifest.toml"), MANIFEST).unwrap();
    let m = Manifest::load(&dir.path().join("assets/manifest.toml")).unwrap();
    let bytes = access::bake(dir.path(), &m).unwrap().expect("a blob");
    let d = AccessData::parse(&bytes).unwrap();
    let w = d.caption(asset_id("sounds/whistle")).unwrap();
    assert_eq!(w.key, "cap.whistle");
    assert!(w.important && w.direction);
    assert_eq!(w.priority, 100, "important sounds default to 100");
    assert_eq!(w.duration_ms, 1500);
    assert_eq!(d.caption(asset_id("sounds/step")), None);
    assert_eq!(
        d.icon(SpriteId::from_path("sprites/kit")),
        Some(SpriteId::from_path("sprites/stripes"))
    );
    let decls = access::captions(&m);
    assert_eq!(decls.len(), 2);
    // Nothing to hold: no blob.
    let empty = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(empty.path().join("assets")).unwrap();
    std::fs::write(empty.path().join("assets/manifest.toml"), "version = 1\n").unwrap();
    let m = Manifest::load(&empty.path().join("assets/manifest.toml")).unwrap();
    assert_eq!(access::bake(empty.path(), &m).unwrap(), None);
}

#[test]
fn a_palette_a_mode_cannot_tell_apart_fails_the_bake() {
    let dir = tempfile::tempdir().unwrap();
    // Red and green with no alternatives: one olive to a deuteranope.
    project(
        dir.path(),
        r#"(version: 1, colors: {"a": (base: (200, 40, 40)), "b": (base: (40, 160, 60))}, distinct: [["a", "b"]])"#,
    );
    let e = bake_all(
        &dir.path().join("assets"),
        Platform::Pc,
        &dir.path().join("out"),
    )
    .unwrap_err()
    .to_string();
    assert!(
        e.contains("palettes.ron") && e.contains("deuteranopia"),
        "{e}"
    );
}

#[test]
fn the_manifest_refuses_broken_captions_and_icons() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("manifest.toml");
    for (text, want) in [
        (
            "version = 1\n[sounds.a]\nfile = \"a.wav\"\ncaption = \"\"\n",
            "caption is empty",
        ),
        (
            "version = 1\n[sounds.a]\nfile = \"a.wav\"\ncaption = \"k\"\ncaption_seconds = 0\n",
            "caption_seconds",
        ),
        (
            "version = 1\n[sprites.a]\nfile = \"a.png\"\nformats = { n64 = \"CI4\" }\ncvd_icon = \"sprites/nope\"\n",
            "cvd_icon",
        ),
        (
            "version = 1\n[sounds.a]\nfile = \"a.wav\"\ncaptions = \"k\"\n",
            "unknown field",
        ),
    ] {
        std::fs::write(&path, text).unwrap();
        let e = Manifest::load(&path).unwrap_err();
        assert!(e.contains(want), "{want}: {e}");
    }
}
