//! `cargo xtask access check|shot` through the real binary (issue #60): the
//! loop an agent or a script runs after editing `assets/palettes.ron`, a
//! caption, the high-contrast theme or a screen — on the projects the repo
//! ships, on a fixture with one problem of each kind
//! (`tests/access_fixtures/broken`), and the shots a reviewer looks at.

use std::path::PathBuf;
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn xtask(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(args)
        .current_dir(root())
        .env_remove("TRINO_PROJECT")
        .output()
        .expect("spawn xtask")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}\n{}", text(&o.stdout), text(&o.stderr)))
}

#[test]
fn access_check_passes_on_the_projects_the_repo_ships() {
    // The engine: castle64's doors, the self-test's teams, three captions,
    // the stripes icon, the self-test screen.
    let o = xtask(&["access", "check", "--json"]);
    let r = json(&o);
    assert!(o.status.success(), "{r:#}");
    assert_eq!(r["errors"], 0);
    assert_eq!(r["warnings"], 0);
    assert_eq!(r["colors"], 7);
    assert_eq!(r["captions"], 3);
    assert_eq!(r["important"], 2);
    assert_eq!(r["icons"], 1);
    let checklist = r["checklist"].as_array().unwrap();
    assert!(checklist.iter().all(|l| l["passed"] == true));
    assert!(checklist.iter().any(|l| {
        l["check"]
            .as_str()
            .unwrap()
            .contains("castle64.door.green and castle64.door.red")
    }));
    // Every new project's screens at 200 %, Solid Eleven's captions in
    // English and Portuguese and its pause menu.
    for dir in ["templates/standalone", "examples/solid-eleven"] {
        let o = xtask(&["access", "check", dir]);
        let out = text(&o.stdout);
        assert!(o.status.success(), "{dir}: {out}");
        assert!(out.contains(": 0 errors, 0 warnings"), "{out}");
    }
    let o = xtask(&["access", "check", "templates/standalone", "--json"]);
    let r = json(&o);
    assert_eq!(
        r["screens"], 8,
        "title, pause, options, controls, accessibility, about, crash, save_notice"
    );
}

#[test]
fn access_check_reports_one_problem_of_each_kind() {
    let o = xtask(&[
        "access",
        "check",
        "xtask/tests/access_fixtures/broken",
        "--json",
    ]);
    assert!(!o.status.success());
    let r = json(&o);
    let findings = r["findings"].as_array().unwrap();
    let has = |area: &str, mode: Option<&str>, msg: &str| {
        findings.iter().any(|f| {
            f["area"] == area
                && mode.is_none_or(|m| f["mode"] == m)
                && f["message"].as_str().unwrap().contains(msg)
        })
    };
    assert!(has(
        "palettes",
        Some("deuteranopia"),
        "give one of them a `deuteranopia` alternative"
    ));
    assert!(has("palettes", Some("off"), "contrast 1.56:1"));
    assert!(has("theme", None, "under 7.0:1"));
    assert!(has("captions", None, "an important sound has no caption"));
    assert!(has("captions", None, "pt-BR has no \"cap.whistle\""));
    assert!(has(
        "ui",
        None,
        "text overflows its box at 200 % [n64, en-US]"
    ));
    // A node in a distinct group's color with no icon (issue #96): the
    // check the editor's UI tab shows on that node.
    assert!(has(
        "icons",
        None,
        "shows team.home (a distinct group's color) with no color-vision icon"
    ));
    let warnings = findings.iter().filter(|f| f["level"] == "warning").count();
    assert_eq!(r["warnings"], warnings);
    assert_eq!(warnings, 2, "the fallback language, the icon");
    // One platform only.
    let o = xtask(&[
        "access",
        "check",
        "xtask/tests/access_fixtures/broken",
        "--platform",
        "n64",
        "--json",
    ]);
    let r = json(&o);
    assert_eq!(r["platforms"], serde_json::json!(["n64"]));
    assert!(
        !r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["message"].as_str().unwrap().contains("[pc"))
    );
}

#[test]
fn access_shot_renders_a_screen_and_a_scene_as_a_player_sees_them() {
    let out = root().join("target/test-projects/access-shots");
    let _ = std::fs::remove_dir_all(&out);
    let screen = out.join("selftest-deutan.png");
    let o = xtask(&[
        "access",
        "shot",
        "selftest",
        "--cvd",
        "deuteranopia",
        "--text",
        "200",
        "--contrast",
        "--platform",
        "n64",
        "--out",
        screen.to_str().unwrap(),
        "--json",
    ]);
    let err = text(&o.stderr) + &text(&o.stdout);
    if err.contains("no GPU") {
        assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{err}");
        return;
    }
    assert!(o.status.success(), "{err}");
    let r = json(&o);
    assert_eq!(r["platform"], "n64");
    assert_eq!(r["cvd"], "deuteranopia");
    assert_eq!(r["view"], "deuteranopia");
    assert_eq!(r["text_scale"], 200);
    assert_eq!(
        (r["width"].as_u64(), r["height"].as_u64()),
        (Some(320), Some(240))
    );
    assert!(screen.is_file());
    // `--view off` shows the player's palette as drawn.
    let o = xtask(&[
        "access",
        "shot",
        "selftest",
        "--cvd",
        "protanopia",
        "--view",
        "off",
        "--out",
        out.join("x.png").to_str().unwrap(),
        "--json",
    ]);
    let r = json(&o);
    assert_eq!(r["cvd"], "protanopia");
    assert_eq!(r["view"], serde_json::Value::Null);
    // A scene through the simulation (the console look, the 3DS's size).
    let scene = out.join("showcase-tritan.png");
    let o = xtask(&[
        "access",
        "shot",
        "scenes/showcase.scene.ron",
        "--cvd",
        "tritanopia",
        "--platform",
        "3ds",
        "--out",
        scene.to_str().unwrap(),
        "--json",
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let r = json(&o);
    assert_eq!(
        (r["width"].as_u64(), r["height"].as_u64()),
        (Some(400), Some(240))
    );
    assert!(scene.is_file());
}

#[test]
fn access_refuses_bad_arguments() {
    for args in [
        &["access", "check", "--platform", "gameboy"][..],
        &["access", "shot", "selftest", "--cvd", "martian"],
        &["access", "shot", "selftest", "--text", "300"],
        &["access", "shot"],
        &["access", "frobnicate"],
    ] {
        let o = xtask(args);
        assert!(!o.status.success(), "{args:?}");
    }
    let o = xtask(&["access", "shot", "selftest", "--text", "90", "--json"]);
    assert!(json(&o)["error"].as_str().unwrap().contains("100 to 200"));
}
