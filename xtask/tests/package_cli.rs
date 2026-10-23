//! `cargo xtask package` and `deck check` through the real binary (issue
//! #61). The quick checks run everywhere; the end-to-end package (a
//! scaffolded project built with fat LTO, its AppImage or zip run outside
//! the repository, a deliberate crash's report) is `#[ignore]`d here and
//! run by CI's `package` job on each OS:
//! `cargo test -p xtask --test package_cli -- --ignored`.

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

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn package_refuses_the_engine_and_bad_arguments() {
    let o = xtask(&["package", "pc"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("packages a project"), "{}", text(&o));
    let o = xtask(&["package", "pc", "--target", "amiga"]);
    assert!(text(&o).contains("linux, windows or macos"), "{}", text(&o));
    let o = xtask(&["package", "3ds"]);
    assert!(text(&o).contains("--cia"), "{}", text(&o));
}

#[test]
fn deck_check_passes_on_castle64() {
    let o = xtask(&["deck", "check", "--json"]);
    let r: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(o.status.success(), "{r:#}");
    assert_eq!(r["fail"], 0);
    let checks = r["checks"].as_array().unwrap();
    let legibility = checks.iter().find(|c| c["check"] == "legibility").unwrap();
    assert_eq!(legibility["status"], "pass", "{legibility}");
    for c in [
        "controller",
        "resolution",
        "default settings",
        "launcher",
        "suspend",
    ] {
        assert!(checks.iter().any(|x| x["check"] == c), "{c}");
    }
}

/// A scaffolded project packaged for this OS: the artifacts, the stamp,
/// the marker, the itch layout, and `--smoke` (a run outside the
/// repository, its first frame, a deliberate crash's report).
#[test]
#[ignore = "builds a game with fat LTO (minutes); CI's package job runs it"]
fn a_scaffolded_project_packages_and_runs_outside_the_repo() {
    let base = root().join("target/test-projects");
    std::fs::create_dir_all(&base).unwrap();
    let dir = base.join("pkg-blank");
    let _ = std::fs::remove_dir_all(&dir);
    let o = xtask(&["new", "pkg-blank", "--standalone", dir.to_str().unwrap()]);
    assert!(o.status.success(), "{}", text(&o));
    let out = dir.join("dist");
    let o = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["package", "pc", "--itch", "--smoke", "--json", "--project"])
        .arg(&dir)
        .current_dir(root())
        .env_remove("TRINO_PROJECT")
        .env("CARGO_TARGET_DIR", root().join("target/package-test"))
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", text(&o));
    let r: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(
        r["smoke"].as_str().unwrap().starts_with("TRINO_RUN_OK"),
        "{r:#}"
    );
    assert_ne!(r["commit"], "dev");
    let artifacts: Vec<PathBuf> = r["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| PathBuf::from(a.as_str().unwrap()))
        .collect();
    assert!(artifacts.iter().all(|a| a.is_file()), "{artifacts:?}");
    let shot = std::fs::read_dir(&out)
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().ends_with("-smoke.png"));
    assert!(shot, "the smoke run's frame");
    let folder = Path::new(r["folder"].as_str().unwrap());
    let marker = std::fs::read_to_string(find(folder, "trino-package.txt").unwrap()).unwrap();
    assert!(
        marker.contains("name=pkg-blank") && marker.contains("assets=assets/pc"),
        "{marker}"
    );
    let itch = Path::new(r["extra"]["itch"]["dir"].as_str().unwrap());
    assert!(itch.join(".itch.toml").is_file());
}

fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.file_name().is_some_and(|n| n == name) {
            return Some(p);
        }
        if p.is_dir()
            && let Some(f) = find(&p, name)
        {
            return Some(f);
        }
    }
    None
}
