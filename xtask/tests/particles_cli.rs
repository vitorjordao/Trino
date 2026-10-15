//! `cargo xtask particles new|check|inspect` through the real binary: on the
//! engine's own self-test effect (24 / 8 / 16 particles on PC / N64 / 3DS)
//! and on a throwaway project.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

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

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn project(dir: &Path) {
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("assets/manifest.toml"), "version = 1\n").unwrap();
    std::fs::write(
        dir.join("trino.toml"),
        "[project]\nname = \"p\"\ntitle = \"P\"\n\n[n64]\npackage = \"p\"\n\n[3ds]\npackage = \"p\"\nbin = \"p\"\n",
    )
    .unwrap();
}

#[test]
fn inspect_simulates_each_platform_cap_deterministically() {
    let o = xtask(&[
        "particles",
        "inspect",
        "selftest-sparks",
        "--seconds",
        "2",
        "--seed",
        "3",
        "--json",
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    let rows = v["platforms"].as_array().unwrap();
    let cap = |k: &str| rows.iter().find(|r| r["platform"] == k).unwrap().clone();
    for (k, max) in [("pc", 24), ("n64", 8), ("3ds", 16)] {
        let r = cap(k);
        assert_eq!(r["cap"], max, "{r}");
        assert!(r["peak"].as_u64().unwrap() <= max, "{r}");
        assert_eq!(
            r["peak_tris"].as_u64().unwrap(),
            2 * r["peak"].as_u64().unwrap()
        );
    }
    // Same seed, same hash; another seed, another.
    let again: Value = serde_json::from_slice(
        &xtask(&[
            "particles",
            "inspect",
            "selftest-sparks",
            "--seconds",
            "2",
            "--seed",
            "3",
            "--json",
        ])
        .stdout,
    )
    .unwrap();
    assert_eq!(again, v);
    let other: Value = serde_json::from_slice(
        &xtask(&[
            "particles",
            "inspect",
            "selftest-sparks",
            "--seconds",
            "2",
            "--seed",
            "4",
            "--json",
        ])
        .stdout,
    )
    .unwrap();
    assert_ne!(other["platforms"][0]["hash"], v["platforms"][0]["hash"]);
    let o = xtask(&["particles", "check"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(
        text(&o.stdout).contains(
            "ok  selftest-sparks: sprites/selftest_quadrants · max PC 24 / N64 8 / 3DS 16"
        )
    );
}

#[test]
fn new_declares_an_effect_and_bad_invocations_say_why() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let p = tmp.path().to_str().unwrap();
    let o = xtask(&[
        "particles",
        "new",
        "dust",
        "--sprite",
        "sprites/dust",
        "--project",
        p,
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(text(&o.stdout).contains("declared [particles.dust]"));
    let file = tmp
        .path()
        .join("assets/shared/particles/dust.particles.ron");
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("sprite: \"sprites/dust\"")
    );
    let o = xtask(&["particles", "inspect", "dust", "--project", p]);
    assert!(o.status.success());
    assert!(
        text(&o.stdout).contains("pc   cap  32"),
        "{}",
        text(&o.stdout)
    );
    // A broken file fails `check` with its name.
    std::fs::write(&file, "(version: 1, sprite: \"s\", spread: 400.0)").unwrap();
    let o = xtask(&["particles", "check", "--project", p]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("dust") && text(&o.stderr).contains("spread"));
    for (args, needle) in [
        (&["particles"][..], "cargo xtask particles new"),
        (
            &["particles", "inspect", "nope", "--project", p],
            "no [particles.nope]",
        ),
        (
            &[
                "particles",
                "inspect",
                "dust",
                "--seconds",
                "x",
                "--project",
                p,
            ],
            "bad value",
        ),
        (
            &["particles", "inspect", "dust", "--frob", "--project", p],
            "unknown flag",
        ),
        (
            &[
                "particles",
                "new",
                "Bad Name",
                "--sprite",
                "s",
                "--project",
                p,
            ],
            "effect name",
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
