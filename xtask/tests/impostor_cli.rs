//! `cargo xtask impostor new|bake|check|list` through the real binary, on a
//! throwaway external project (`--project`) and on the engine's own assets:
//! what an agent runs to make the same impostors the editor's model
//! inspector makes.

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

/// A project with one fake-object model.
fn project(dir: &Path) {
    std::fs::create_dir_all(dir.join("assets/shared/models")).unwrap();
    std::fs::write(
        dir.join("assets/shared/models/block.prim.ron"),
        "(shape: Box(size: (1.0, 2.0, 1.0)), color: (90, 160, 220, 255), pivot: Bottom)",
    )
    .unwrap();
    std::fs::write(
        dir.join("assets/manifest.toml"),
        "version = 1\n\n[models.block]\nfile = \"models/block.prim.ron\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("trino.toml"),
        "[project]\nname = \"blocks\"\ntitle = \"Blocks\"\n\n[n64]\npackage = \"blocks\"\n\n\
         [3ds]\npackage = \"blocks\"\nbin = \"blocks\"\n",
    )
    .unwrap();
}

#[test]
fn new_bake_check_and_list_an_impostor_in_a_project() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let p = tmp.path().to_str().unwrap();
    let o = xtask(&[
        "impostor",
        "new",
        "block",
        "--model",
        "models/block",
        "--views",
        "4",
        "--tile",
        "32",
        "--project",
        p,
    ]);
    assert!(o.status.success(), "{}{}", text(&o.stdout), text(&o.stderr));
    let out = text(&o.stdout);
    assert!(
        out.contains("baked impostor `block`: 4 views, 64x64 atlas (32 px tiles), N64 64x64 CI4"),
        "{out}"
    );
    assert!(out.contains("impostor: Some(\"impostors/block\")"), "{out}");
    let assets = tmp.path().join("assets");
    for f in [
        "impostors/block.impostor.ron",
        "shared/impostors/block.png",
        "shared/impostors/block.timp",
        "n64/impostors/block.png",
        "n64/impostors/block.timp",
    ] {
        assert!(assets.join(f).exists(), "{f}");
    }
    let def = std::fs::read_to_string(assets.join("impostors/block.impostor.ron")).unwrap();
    assert!(
        def.contains("model: \"models/block\"") && def.contains("views: 4"),
        "{def}"
    );

    let o = xtask(&["impostor", "list", "--json", "--project", p]);
    assert!(o.status.success());
    let json = text(&o.stdout);
    assert!(
        json.contains("\"name\":\"block\"") && json.contains("\"state\":\"fresh\""),
        "{json}"
    );
    assert!(json.contains("\"n64_tile\":32"), "{json}");
    let o = xtask(&["impostor", "check", "--project", p]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(text(&o.stdout).contains("ok  block"));

    // A model edit makes it stale; `bake --all` fixes it.
    std::fs::write(
        assets.join("shared/models/block.prim.ron"),
        "(shape: Box(size: (2.0, 2.0, 1.0)), color: (90, 160, 220, 255), pivot: Bottom)",
    )
    .unwrap();
    let o = xtask(&["impostor", "check", "--project", p]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("stale"), "{}", text(&o.stderr));
    let o = xtask(&["impostor", "list", "--project", p]);
    assert!(text(&o.stdout).contains("· stale"), "{}", text(&o.stdout));
    let o = xtask(&["impostor", "bake", "--all", "--project", p]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let o = xtask(&["impostor", "check", "block", "--project", p]);
    assert!(o.status.success(), "{}", text(&o.stderr));
}

#[test]
fn bad_impostor_invocations_fail_with_the_usage_or_a_reason() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let p = tmp.path().to_str().unwrap();
    for args in [
        &["impostor"][..],
        &["impostor", "frob"],
        &["impostor", "bake"],
    ] {
        let o = xtask(args);
        assert!(!o.status.success(), "{args:?}");
        assert!(
            text(&o.stderr).contains("cargo xtask impostor new"),
            "{args:?}"
        );
    }
    for (args, needle) in [
        (
            &["impostor", "new", "x", "--project", p][..],
            "needs --model",
        ),
        (
            &[
                "impostor",
                "new",
                "x",
                "--model",
                "models/block",
                "--views",
                "99",
                "--project",
                p,
            ],
            "views 99",
        ),
        (
            &[
                "impostor",
                "new",
                "x",
                "--model",
                "models/block",
                "--tile",
                "48",
                "--project",
                p,
            ],
            "power of two",
        ),
        (
            &["impostor", "new", "x", "--bogus", "1", "--project", p],
            "unknown flag",
        ),
        (
            &["impostor", "bake", "nope", "--project", p],
            "nope.impostor.ron",
        ),
        (
            &[
                "impostor",
                "new",
                "y",
                "--model",
                "models/ghost",
                "--project",
                p,
            ],
            "not in the manifest",
        ),
    ] {
        let o = xtask(args);
        assert!(!o.status.success(), "{args:?}");
        assert!(
            text(&o.stderr).contains(needle),
            "{args:?}: {}",
            text(&o.stderr)
        );
    }
}

/// The engine's own impostors (castle64's self-test) are baked and fresh:
/// a model edit that forgets `cargo xtask impostor bake` fails here.
#[test]
fn the_engines_impostors_are_fresh() {
    let o = xtask(&["impostor", "check"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(text(&o.stdout).contains("ok  selftest-boar"));
}
