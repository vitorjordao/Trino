//! UI screens and string tables as generated blobs (issue #58): a
//! project's `ui/*.ui.ron` and `assets/i18n/*.ron` bake into `ui/<name>`
//! and `i18n/<lang>` on every target with no manifest entry, read back
//! with the core's parsers, count in the memory plan, and a broken screen
//! fails the bake with its file and its problem.

use std::path::Path;

use trino_asset_pipeline::{Platform, bake_all, memory};
use trino_core::ui::{StringTable, UiScreen, UiStrings};
use trino_core::{HwTier, asset_id};

const MENU: &str = r#"(
    version: 1,
    on_back: "back",
    root: (kind: Stack(), anchor: Center, children: [
        (kind: Button, id: "play", key: "menu.play", text: "Play", on_click: "play"),
        (kind: Button, id: "quit", key: "menu.quit", text: "Quit", on_click: "quit"),
    ]),
)"#;

fn project(root: &Path) {
    std::fs::create_dir_all(root.join("assets/i18n")).unwrap();
    std::fs::create_dir_all(root.join("ui")).unwrap();
    std::fs::write(root.join("assets/manifest.toml"), "version = 1\n").unwrap();
    std::fs::write(root.join("ui/menu.ui.ron"), MENU).unwrap();
    std::fs::write(
        root.join("assets/i18n/en-US.ron"),
        r#"Strings(lang: "en-US", entries: {"menu.play": "PLAY", "menu.quit": "QUIT"})"#,
    )
    .unwrap();
    std::fs::write(
        root.join("assets/i18n/pt-BR.ron"),
        r#"Strings(lang: "pt-BR", fallback: "en-US", entries: {"menu.play": "JOGAR"})"#,
    )
    .unwrap();
}

#[test]
fn screens_and_tables_bake_on_every_target_and_read_back() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path());
    for platform in [Platform::Pc, Platform::Web, Platform::N64, Platform::N3ds] {
        let out = dir.path().join(format!("out-{platform:?}"));
        let report = bake_all(&dir.path().join("assets"), platform, &out).unwrap();
        let blob = |logical: &str| {
            let e = report
                .entries
                .iter()
                .find(|e| e.logical == logical)
                .unwrap_or_else(|| panic!("{platform:?}: no {logical}"));
            assert_eq!((e.kind, e.id), ("blob", asset_id(logical)));
            std::fs::read(out.join(&e.file)).unwrap()
        };
        let bytes = blob("ui/menu");
        let screen = UiScreen::parse(&bytes).unwrap();
        assert_eq!(screen.name, "menu");
        assert_eq!(screen.on_back, "back");
        assert_eq!(screen.len(), 3);
        assert_eq!(screen.find("quit"), Some(2));
        assert_eq!(screen.node(2).key, "menu.quit");
        let en = blob("i18n/en-US");
        let en = StringTable::parse(&en).unwrap();
        assert_eq!(en.lookup("menu.quit"), Some("QUIT"));
        let pt = blob("i18n/pt-BR");
        let pt = StringTable::parse(&pt).unwrap();
        assert_eq!((pt.lang(), pt.fallback()), ("pt-BR", "en-US"));
        assert_eq!(pt.lookup("menu.play"), Some("JOGAR"));
        assert_eq!(
            pt.lookup("menu.quit"),
            None,
            "the fallback is the game's chain"
        );
        assert_eq!(en.get("menu.play"), Some("PLAY"));
    }
    // Resident like the mixer on the consoles: the memory plan counts them.
    let plan = memory::plan(&dir.path().join("assets"), Platform::N64, HwTier::Base).unwrap();
    for logical in ["ui/menu", "i18n/en-US", "i18n/pt-BR"] {
        let a = plan
            .assets
            .iter()
            .find(|a| a.logical == logical)
            .unwrap_or_else(|| panic!("{logical} in the plan"));
        assert_eq!(a.kind, "blob");
        assert!(a.bytes > 0);
    }
}

#[test]
fn a_broken_screen_fails_the_bake_with_its_file_and_problem() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path());
    std::fs::write(
        dir.path().join("ui/broken.ui.ron"),
        r#"(version: 1, root: (children: [(kind: Button, id: "a", text: "A"), (kind: Button, id: "a", text: "B")]))"#,
    )
    .unwrap();
    let err = bake_all(
        &dir.path().join("assets"),
        Platform::N64,
        &dir.path().join("out"),
    )
    .err()
    .expect("a duplicate id fails the bake")
    .to_string();
    assert!(
        err.contains("broken.ui.ron") && err.contains("duplicate id"),
        "{err}"
    );
    // A newer format than the engine reads is refused, not misread.
    std::fs::write(
        dir.path().join("ui/broken.ui.ron"),
        "(version: 99, root: ())",
    )
    .unwrap();
    let err = bake_all(
        &dir.path().join("assets"),
        Platform::Pc,
        &dir.path().join("out"),
    )
    .err()
    .unwrap()
    .to_string();
    assert!(err.contains("newer than this engine"), "{err}");
}
