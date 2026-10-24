//! `cargo xtask scene check|export` through the real binary: the
//! text-first loop an agent or a script runs after editing scene, prefab
//! and material files by hand.

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

/// A scene whose references dangle: an unknown material, an unknown
/// prefab and a portal naming a cell that does not exist.
const BROKEN: &str = r#"(
    version: 2,
    name: "broken",
    entities: [
        (
            name: "crate",
            model: Some((path: "models/crate", material: Some("materials/no-such-material"))),
        ),
        (
            name: "ghost",
            prefab: Some((path: "prefabs/no-such-prefab")),
        ),
        (
            name: "door",
            vis: Some(Portal(size: (1.0, 2.0, 0.2), a: "hall", b: "nowhere")),
        ),
    ],
)"#;

fn write(dir: &Path, name: &str, body: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    p.to_str().unwrap().to_string()
}

#[test]
fn scene_check_accepts_every_repo_scene() {
    let o = xtask(&["scene", "check"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let out = text(&o.stdout);
    let scenes: Vec<_> = std::fs::read_dir(root().join("scenes"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".scene.ron"))
        .collect();
    assert!(!scenes.is_empty());
    for e in &scenes {
        let name = e.file_name().to_string_lossy().to_string();
        let line = out
            .lines()
            .find(|l| l.starts_with("ok  ") && l.contains(&name))
            .unwrap_or_else(|| panic!("no ok line for {name}:\n{out}"));
        assert!(line.ends_with("draw items)"), "{line}");
    }
    let materials = std::fs::read_dir(root().join("assets/materials"))
        .unwrap()
        .count();
    let prefabs = std::fs::read_dir(root().join("prefabs")).unwrap().count();
    assert!(
        out.contains(&format!(
            "{materials} material(s), {prefabs} prefab(s), {} scene(s)",
            scenes.len()
        )),
        "{out}"
    );

    // One named file is checked alone.
    let one = xtask(&["scene", "check", "scenes/showcase.scene.ron"]);
    assert!(one.status.success(), "{}", text(&one.stderr));
    assert!(text(&one.stdout).contains(", 1 scene(s)"));
}

#[test]
fn scene_check_reports_dangling_references_and_parse_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let broken = write(tmp.path(), "broken.scene.ron", BROKEN);
    let o = xtask(&["scene", "check", &broken]);
    assert!(!o.status.success(), "dangling references fail the check");
    let err = text(&o.stderr);
    assert!(
        err.contains("crate: unknown material `materials/no-such-material`"),
        "{err}"
    );
    assert!(err.contains("no-such-prefab"), "{err}");
    assert!(
        err.contains("portal names unknown cell `hall`"),
        "portal warning: {err}"
    );
    assert!(
        err.contains("broken.scene.ron"),
        "warnings name the file: {err}"
    );
    // The scene still resolved (and was listed) before the verdict.
    assert!(text(&o.stdout).contains("ok  "), "{}", text(&o.stdout));

    let garbage = write(
        tmp.path(),
        "garbage.scene.ron",
        "( version: 2, entities: [ (name: ",
    );
    let o = xtask(&["scene", "check", &garbage]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("error: "), "{}", text(&o.stderr));

    let o = xtask(&["scene", "check", "/no/such.scene.ron"]);
    assert!(!o.status.success(), "a missing scene is an error");
}

#[test]
fn scene_without_a_subcommand_prints_the_usage() {
    for args in [
        &["scene"][..],
        &["scene", "frob"],
        &["scene", "export", "one-arg"],
        &["scene", "export", "--platform", "wii", "a.ron", "b.rs"],
    ] {
        let o = xtask(args);
        assert!(!o.status.success(), "{args:?}");
        assert!(
            text(&o.stderr)
                .contains("cargo xtask scene export [--platform pc|n64|3ds|web|n64_expanded|n3ds_new] [--bake-grading] <scene.ron> <out.rs>"),
            "{args:?}: {}",
            text(&o.stderr)
        );
    }
}

/// A scene with an N64 variant: `--platform n64` exports the console's
/// model, the default (PC) the base one.
#[test]
fn scene_export_applies_the_platform_variants() {
    let tmp = tempfile::tempdir().unwrap();
    let scene = write(
        tmp.path(),
        "variants.scene.ron",
        r#"(version: 3, name: "variants", entities: [(
            name: "rock",
            model: Some((path: "models/rock")),
            platforms: (n64: Some((model: Some("models/crate"))), n3ds: Some((disabled: true))),
        )])"#,
    );
    let hex = |p: &str| format!("ModelId({:#010x})", trino_core::asset_id(p));
    let export = |platform: Option<&str>| {
        let out = tmp
            .path()
            .join(format!("{}.rs", platform.unwrap_or("default")));
        let mut args = vec!["scene", "export"];
        if let Some(p) = platform {
            args.extend(["--platform", p]);
        }
        args.extend([scene.as_str(), out.to_str().unwrap()]);
        let o = xtask(&args);
        assert!(o.status.success(), "{}", text(&o.stderr));
        std::fs::read_to_string(&out).unwrap()
    };
    let pc = export(None);
    assert!(pc.contains(&hex("models/rock")) && !pc.contains(&hex("models/crate")));
    let n64 = export(Some("n64"));
    assert!(n64.contains(&hex("models/crate")) && !n64.contains(&hex("models/rock")));
    assert!(n64.contains("scene export --platform n64"));
    let n3ds = export(Some("3ds"));
    assert!(!n3ds.contains("\"rock\""), "disabled on the 3DS: {n3ds}");
}

/// A hardware tier (issue #54) applies its own variant over its console's,
/// and falls back to the console's where it has none.
#[test]
fn scene_export_applies_the_tier_variants_over_the_consoles() {
    let tmp = tempfile::tempdir().unwrap();
    let scene = write(
        tmp.path(),
        "tiers.scene.ron",
        r#"(version: 5, name: "tiers", entities: [(
            name: "rock",
            model: Some((path: "models/rock")),
            platforms: (
                n64: Some((model: Some("models/crate"))),
                n64_expanded: Some((model: Some("models/statue"))),
                n3ds: Some((disabled: true)),
            ),
        )])"#,
    );
    let hex = |p: &str| format!("ModelId({:#010x})", trino_core::asset_id(p));
    let export = |platform: &str| {
        let out = tmp.path().join(format!("{platform}.rs"));
        let o = xtask(&[
            "scene",
            "export",
            "--platform",
            platform,
            scene.as_str(),
            out.to_str().unwrap(),
        ]);
        assert!(o.status.success(), "{}", text(&o.stderr));
        std::fs::read_to_string(&out).unwrap()
    };
    let n64 = export("n64");
    assert!(n64.contains(&hex("models/crate")) && !n64.contains(&hex("models/statue")));
    let pak = export("n64_expanded");
    assert!(pak.contains(&hex("models/statue")) && !pak.contains(&hex("models/crate")));
    assert!(pak.contains("scene export --platform n64_expanded"));
    // No `n3ds_new` variant: the New 3DS takes the 3DS's (disabled).
    let new = export("n3ds_new");
    assert!(!new.contains("\"rock\""), "{new}");
}

/// The exported code is exactly what `examples/showcase` commits (the
/// `scene_codegen` guard checks the library side; this is the command).
#[test]
fn scene_export_writes_the_committed_showcase_code() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("showcase_gen.rs");
    let o = xtask(&[
        "scene",
        "export",
        "scenes/showcase.scene.ron",
        out.to_str().unwrap(),
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(text(&o.stdout).starts_with("wrote "));
    assert!(
        text(&o.stderr).is_empty(),
        "no warnings: {}",
        text(&o.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        std::fs::read_to_string(root().join("examples/showcase/src/showcase_gen.rs")).unwrap()
    );
}

#[test]
fn scene_export_warns_on_dangling_references_and_fails_on_io_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let broken = write(tmp.path(), "broken.scene.ron", BROKEN);
    let out = tmp.path().join("broken_gen.rs");
    let o = xtask(&["scene", "export", &broken, out.to_str().unwrap()]);
    assert!(o.status.success(), "warnings do not block the export");
    assert!(text(&o.stderr).contains("warning: "), "{}", text(&o.stderr));
    let code = std::fs::read_to_string(&out).unwrap();
    assert!(code.contains("SceneNode"), "{code}");
    assert!(
        code.contains("\"crate\""),
        "the resolvable entity is exported"
    );

    let o = xtask(&[
        "scene",
        "export",
        "/no/such.scene.ron",
        out.to_str().unwrap(),
    ]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).starts_with("xtask: "),
        "{}",
        text(&o.stderr)
    );

    let nowhere = tmp.path().join("no-dir/out.rs");
    let o = xtask(&["scene", "export", &broken, nowhere.to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("cannot write"),
        "{}",
        text(&o.stderr)
    );
}
