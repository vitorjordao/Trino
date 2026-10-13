//! `cargo xtask new <name> --standalone <dir>` end to end: scaffold a
//! project outside the repository, then build and test it with plain
//! `cargo` from inside it — what a user does. With `TRINO_REQUIRE_GPU=1`
//! and a display (`DISPLAY` / `WAYLAND_DISPLAY`; CI runs this test under
//! `xvfb-run`) the PC host also runs a 30-frame smoke and must print
//! `TRINO_RUN_OK`.
//!
//! Builds share this workspace's `target/` (same `Cargo.lock`, same
//! profiles), so only the generated crates compile.

use std::path::Path;
use std::process::Command;

#[test]
fn standalone_project_builds_tests_and_runs() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("orbit-cat");

    let out = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["new", "orbit-cat", "--standalone"])
        .arg(&dir)
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "scaffold failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let target = repo.join("target");
    let run = |args: &[&str], envs: &[(&str, &str)]| {
        let out = Command::new(&cargo)
            .args(args)
            .envs(envs.iter().copied())
            .env("CARGO_TARGET_DIR", &target)
            .current_dir(&dir)
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "cargo {args:?} failed:\n{text}");
        text
    };
    let tests = run(&["test", "--offline"], &[]);
    assert!(tests.contains("moves_with_dpad ... ok"), "{tests}");

    let display = ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|v| std::env::var_os(v).is_some_and(|s| !s.is_empty()));
    if std::env::var("TRINO_REQUIRE_GPU").as_deref() == Ok("1") && display {
        let smoke = run(
            &["run", "--offline", "-p", "orbit-cat-pc"],
            &[("TRINO_SMOKE_FRAMES", "30")],
        );
        assert!(smoke.contains("TRINO_RUN_OK frames=30"), "{smoke}");
        assert!(dir.join("target/assets/pc/index.toml").exists());
    }
}
