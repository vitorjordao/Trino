//! `cargo xtask assets group|check|report` through the real binary, on a
//! throwaway project (issue #56): what an agent runs to author groups and
//! hold scenes to them without the editor.

use std::path::{Path, PathBuf};
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
        .unwrap_or_else(|e| panic!("{e}: {}{}", text(&o.stdout), text(&o.stderr)))
}

/// Two models, a 3 MB blob in its own group (past the N64's 1.8 MB for
/// assets, inside the Expansion Pak's 5.8 MB) and two scenes: a level and
/// its cellar.
fn project(dir: &Path) {
    std::fs::create_dir_all(dir.join("assets/shared/models")).unwrap();
    std::fs::create_dir_all(dir.join("assets/shared/data")).unwrap();
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    for name in ["crate", "barrel"] {
        std::fs::write(
            dir.join(format!("assets/shared/models/{name}.prim.ron")),
            "(shape: Box(size: (1.0, 1.0, 1.0)), color: (160, 120, 80, 255))",
        )
        .unwrap();
    }
    std::fs::write(dir.join("assets/shared/data/huge.bin"), vec![7u8; 3 << 20]).unwrap();
    std::fs::write(
        dir.join("assets/manifest.toml"),
        "version = 1\n\n[models.crate]\nfile = \"models/crate.prim.ron\"\n\n\
         [models.barrel]\nfile = \"models/barrel.prim.ron\"\n\n\
         [blobs.huge]\nfile = \"data/huge.bin\"\ngroups = [\"huge\"]\n\n[groups.huge]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("trino.toml"),
        "[project]\nname = \"crates\"\ntitle = \"Crates\"\n\n[n64]\npackage = \"crates\"\n\n\
         [3ds]\npackage = \"crates\"\nbin = \"crates\"\n",
    )
    .unwrap();
    // The level needs `level`; the cellar is a cell streaming `cellar`.
    std::fs::write(
        dir.join("scenes/level.scene.ron"),
        r#"(version: 6, name: "level", requires: ["boot", "level"], entities: [
            (name: "crate", model: Some((path: "models/crate"))),
            (name: "cellar", vis: Some(Cell(size: (8.0, 4.0, 8.0), groups: ["cellar"])),
             children: [(name: "barrel", model: Some((path: "models/barrel")))]),
        ])"#,
    )
    .unwrap();
}

#[test]
fn group_check_and_report_hold_scenes_to_their_groups() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let p = tmp.path().to_str().unwrap();

    // The crate into `level` (declared on the way).
    let o = xtask(&[
        "assets",
        "group",
        "models/crate",
        "level",
        "--json",
        "--project",
        p,
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let doc = json(&o);
    assert_eq!(doc["groups"], serde_json::json!(["level"]));
    assert_eq!(
        doc["declared"],
        serde_json::json!(["boot", "huge", "level"])
    );
    let manifest = std::fs::read_to_string(tmp.path().join("assets/manifest.toml")).unwrap();
    assert!(
        manifest
            .contains("[models.crate]\nfile = \"models/crate.prim.ron\"\ngroups = [\"level\"]\n"),
        "{manifest}"
    );
    assert!(manifest.contains("[groups.level]"), "{manifest}");

    // The barrel into `cellar` too, but `cellar` is only ever a cell's: the
    // check passes once it is declared. Before: the cellar's group is
    // unknown.
    let o = xtask(&["assets", "check", "--json", "--project", p]);
    assert!(!o.status.success());
    let doc = json(&o);
    assert_eq!(doc["ok"], false);
    let problems: Vec<String> = doc["scenes"][0]["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0]
            .contains("cell `cellar` names group `cellar`, which the manifest does not declare"),
        "{problems:?}"
    );
    assert_eq!(
        doc["scenes"][0]["requires"],
        serde_json::json!(["boot", "level"])
    );
    assert_eq!(
        doc["scenes"][0]["cell_groups"],
        serde_json::json!([{ "cell": "cellar", "groups": ["cellar"] }])
    );

    let o = xtask(&["assets", "group", "models/barrel", "cellar", "--project", p]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert_eq!(text(&o.stdout).trim(), "models/barrel: groups [\"cellar\"]");
    let o = xtask(&["assets", "check", "--project", p]);
    assert!(o.status.success(), "{}{}", text(&o.stdout), text(&o.stderr));
    assert!(text(&o.stdout).contains("1 scene(s), 4 group(s), 0 problem(s)"));

    // The crate moves out of `level`'s reach: the level draws what it would
    // not load.
    let o = xtask(&["assets", "group", "models/crate", "huge", "--project", p]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let o = xtask(&["assets", "check", "--project", p]);
    assert!(!o.status.success());
    let err = text(&o.stderr);
    assert!(
        err.contains(
            "draws `models/crate` (groups [\"huge\"]), loaded by none of the scene's groups"
        ),
        "{err}"
    );
    // And a barrel outside the cellar would need `cellar` in `requires`;
    // inside it, the cell's group is enough (no line for it).
    assert!(!err.contains("models/barrel"), "{err}");

    // Requiring `huge` instead makes the level 3 MB: over the N64's budget
    // (the check names the groups and the blob), inside the Expansion Pak's.
    let level = tmp.path().join("scenes/level.scene.ron");
    let ron = std::fs::read_to_string(&level).unwrap();
    std::fs::write(
        &level,
        ron.replace(r#"["boot", "level"]"#, r#"["boot", "huge"]"#),
    )
    .unwrap();
    let o = xtask(&["assets", "check", "--project", p]);
    assert!(!o.status.success());
    let err = text(&o.stderr);
    assert!(err.contains("N64 for assets"), "{err}");
    assert!(err.contains("`blobs/huge` 3.0 MB (huge)"), "{err}");
    assert_eq!(
        err.matches("error: ").count(),
        1,
        "the N64 base tier only: {err}"
    );

    let o = xtask(&[
        "assets",
        "report",
        "--platform",
        "n64",
        "--json",
        "--project",
        p,
    ]);
    assert!(!o.status.success(), "over budget fails, like strict mode");
    let doc = json(&o);
    assert_eq!(doc["ok"], false);
    let tiers = doc["platforms"].as_array().unwrap();
    assert_eq!(tiers.len(), 2);
    let scenario = |tier: usize, name: &str| {
        tiers[tier]["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == name)
            .unwrap_or_else(|| panic!("{name}: {}", tiers[tier]))
            .clone()
    };
    assert_eq!(tiers[0]["tier"], "base");
    assert_eq!(scenario(0, "boot + huge")["fits"], false);
    assert_eq!(scenario(0, "scene level")["fits"], false);
    assert_eq!(scenario(0, "boot")["fits"], true);
    assert_eq!(tiers[1]["tier"], "expanded");
    assert_eq!(scenario(1, "scene level")["fits"], true);
    let huge = tiers[0]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["name"] == "huge")
        .unwrap();
    assert_eq!(huge["assets"], 2, "the blob and the crate");
    assert!(huge["bytes"].as_u64().unwrap() >= 3 << 20);
    assert!(huge["baked_stored"].is_null(), "never baked");

    // The text form names the overrun.
    let o = xtask(&[
        "assets",
        "report",
        "--platform",
        "n64",
        "--tier",
        "base",
        "--project",
        p,
    ]);
    let out = text(&o.stdout);
    assert!(
        out.contains("[N64 base] 4.0 MB RAM - 2.2 MB engine reserve"),
        "{out}"
    );
    assert!(out.contains("OVER by"), "{out}");
}

#[test]
fn bad_invocations_say_what_is_wrong_and_leave_the_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let p = tmp.path().to_str().unwrap();
    let before = std::fs::read_to_string(tmp.path().join("assets/manifest.toml")).unwrap();
    for (args, needle) in [
        (
            &["assets", "group", "models/ghost", "x"][..],
            "not found in the manifest",
        ),
        (
            &["assets", "group", "models/crate", "Bad Name"][..],
            "not a group name",
        ),
        (
            &["assets", "group", "prefabs/crate", "x"][..],
            "have no groups",
        ),
        (&["assets", "group", "models/crate"][..], "usage"),
        (
            &["assets", "report", "--platform", "wii"][..],
            "--platform needs",
        ),
        (
            &["assets", "report", "--tier", "deluxe"][..],
            "--tier needs",
        ),
        (
            &["assets", "report", "--frob"][..],
            "unknown argument `--frob`",
        ),
    ] {
        let mut full = args.to_vec();
        full.extend(["--project", p]);
        let o = xtask(&full);
        assert!(!o.status.success(), "{args:?}");
        assert!(
            text(&o.stderr).contains(needle),
            "{args:?}: {}",
            text(&o.stderr)
        );
    }
    let after = std::fs::read_to_string(tmp.path().join("assets/manifest.toml")).unwrap();
    assert_eq!(after, before);
    assert!(!tmp.path().join("assets/manifest.toml.edit").exists());
}

#[test]
fn the_engine_assets_check_clean() {
    let o = xtask(&["assets", "check", "--json"]);
    assert!(o.status.success(), "{}{}", text(&o.stdout), text(&o.stderr));
    let doc = json(&o);
    assert_eq!(doc["ok"], true);
    assert!(
        doc["groups"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g == "selftest_level")
    );
    let o = xtask(&["assets", "report", "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let doc = json(&o);
    assert_eq!(
        doc["platforms"].as_array().unwrap().len(),
        4,
        "N64 and 3DS, both tiers"
    );
}
