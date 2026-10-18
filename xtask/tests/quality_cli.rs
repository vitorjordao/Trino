//! `cargo xtask quality show|check|set|reset` through the real binary
//! (issue #55), on a temp project: what an agent edits is the file the
//! bake, the hosts and the editor's Quality tab read.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use trino_asset_pipeline::quality::QualityFile;
use trino_core::QualityPreset;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn xtask(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(args)
        .arg("--project")
        .arg(dir)
        .current_dir(root())
        .env_remove("TRINO_PROJECT")
        .output()
        .expect("spawn xtask")
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn json(dir: &Path, args: &[&str]) -> serde_json::Value {
    let o = xtask(dir, args);
    assert!(o.status.success(), "{}", text(&o.stderr));
    serde_json::from_slice(&o.stdout).unwrap()
}

fn project(dir: &Path, quality: &str) {
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("assets/manifest.toml"), "version = 1\n").unwrap();
    std::fs::write(
        dir.join("trino.toml"),
        format!("[project]\nname = \"p\"\ntitle = \"P\"\nplatforms = [\"pc\"]\n{quality}"),
    )
    .unwrap();
}

#[test]
fn show_set_and_reset_edit_the_presets_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    project(dir, "");
    let doc = json(dir, &["quality", "show", "--json"]);
    assert_eq!(doc["default"], "auto");
    assert_eq!(doc["file_exists"], false);
    let low = &doc["presets"][0];
    assert_eq!(low["preset"], "low");
    assert_eq!(low["render_scale"], 0.75);
    assert_eq!(low["lod_bias"], 0.6, "rounded, not an f32's noise");
    assert_eq!(doc["presets"][3]["render_scale"], 1.25);

    // A change writes only what differs; --json prints the new table.
    let doc = json(
        dir,
        &[
            "quality",
            "set",
            "low",
            "render_scale=0.6",
            "frame_limit=30",
            "dynamic_resolution=off",
            "--json",
        ],
    );
    assert_eq!(doc["file_exists"], true);
    assert_eq!(doc["presets"][0]["render_scale"], 0.6);
    assert_eq!(doc["presets"][0]["frame_limit"], "30");
    assert!(doc["presets"][0]["dynamic_resolution"].is_null());
    let file = std::fs::read_to_string(dir.join("assets/quality.ron")).unwrap();
    let table = QualityFile::parse(&file).and_then(|f| f.table()).unwrap();
    assert_eq!(table.get(QualityPreset::Low).render_scale, 0.6);
    assert!(
        !file.contains("medium:"),
        "an unchanged preset isn't written: {file}"
    );
    let o = xtask(dir, &["quality", "show"]);
    let shown = text(&o.stdout);
    assert!(shown.contains("low      0.60 off"), "{shown}");
    assert!(xtask(dir, &["quality", "check"]).status.success());

    // Out of the file's ranges, an unknown key or preset: refused, the
    // file untouched.
    for bad in [
        &["quality", "set", "low", "msaa=2"][..],
        &["quality", "set", "high", "render_scale=3"],
        &["quality", "set", "high", "bloom=on"],
        &["quality", "set", "epic", "msaa=4"],
        &["quality", "set", "low"],
    ] {
        let o = xtask(dir, bad);
        assert!(!o.status.success(), "{bad:?} passed");
        assert!(!text(&o.stderr).is_empty(), "{bad:?} said nothing");
    }
    assert_eq!(
        std::fs::read_to_string(dir.join("assets/quality.ron")).unwrap(),
        file
    );

    // Reset: the engine's values again.
    assert!(xtask(dir, &["quality", "reset", "low"]).status.success());
    let doc = json(dir, &["quality", "show", "--json"]);
    assert_eq!(doc["presets"][0]["render_scale"], 0.75);
}

#[test]
fn check_validates_trino_toml_and_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    project(
        dir,
        "\n[pc.quality]\ndefault = \"medium\"\npresets = \"assets/q.ron\"\n",
    );
    let o = xtask(dir, &["quality", "check"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(
        text(&o.stdout).contains("default medium"),
        "{}",
        text(&o.stdout)
    );
    std::fs::write(
        dir.join("assets/q.ron"),
        "(version: 1, ultra: (anisotropy: 3))\n",
    )
    .unwrap();
    let o = xtask(dir, &["quality", "check"]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("anisotropy"),
        "{}",
        text(&o.stderr)
    );
    project(dir, "\n[pc.quality]\ndefault = \"epic\"\n");
    let o = xtask(dir, &["quality", "check"]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("epic"), "{}", text(&o.stderr));
}

#[test]
fn run_pc_quality_takes_a_preset_or_auto_and_only_on_the_pc() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    project(dir, "");
    let o = xtask(dir, &["run", "pc", "--quality", "epic"]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("--quality needs low, medium, high, ultra or auto"));
    let o = xtask(dir, &["build", "pc", "--quality", "low"]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("--quality goes with `run pc`"));
    let o = xtask(dir, &["quality", "bogus"]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("cargo xtask quality bench [--json]"));
}
