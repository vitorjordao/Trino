//! `cargo xtask anim list|check|simulate|new` and `model inspect --anim
//! --compressed` through the real binary (issue #65): on the engine's own
//! graphs (`assets/anim/`) and on a throwaway copy of the engine's content
//! where a graph is broken on purpose.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn xtask_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(args)
        .current_dir(dir)
        .env_remove("TRINO_PROJECT")
        .output()
        .expect("spawn xtask")
}

fn xtask(args: &[&str]) -> Output {
    xtask_in(&root(), args)
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn list_and_check_the_engines_graphs() {
    let o = xtask(&["anim", "list", "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    let names: Vec<&str> = v["graphs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"strider") && names.contains(&"robot"),
        "{names:?}"
    );
    let o = xtask(&["anim", "check", "strider", "--json"]);
    assert!(o.status.success(), "{}{}", text(&o.stdout), text(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], true);
    let rows = v["results"].as_array().unwrap();
    // Checked against each platform's bake of the model.
    let platforms: Vec<&str> = rows
        .iter()
        .map(|r| r["platform"].as_str().unwrap())
        .collect();
    assert_eq!(platforms, ["pc", "n64", "3ds", "web"]);
    assert!(rows.iter().all(|r| r["bytes"].as_u64().unwrap() > 32));
    let o = xtask(&["anim", "check", "nope"]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("no graph `nope`"));
}

#[test]
fn simulate_writes_the_state_timeline_an_agent_reads() {
    let dir = tempfile::tempdir_in(root().join("target")).unwrap();
    let script = dir.path().join("hop.ron");
    std::fs::write(
        &script,
        r#"Script(duration: 3.0, steps: [
            (at: 0.2, set: {"speed": 1.4}),
            (at: 1.0, trigger: ["hop"]),
        ])"#,
    )
    .unwrap();
    let out = dir.path().join("trace.json");
    let o = xtask(&[
        "anim",
        "simulate",
        "strider",
        "--script",
        script.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--json",
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    // --json is JSON only: the summary; the trace is in the file.
    let summary: Value = serde_json::from_slice(&o.stdout).unwrap();
    let trace: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let path: Vec<&str> = trace["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["to"].as_str().unwrap())
        .collect();
    assert_eq!(path, ["hop", "locomotion"]);
    assert_eq!(summary["digest"], trace["digest"]);
    // Events by name: the clip's (the manifest's) and the state's.
    let events: Vec<&str> = trace["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    for want in ["foot_l", "foot_r", "crouch", "sfx:jump", "land"] {
        assert!(events.contains(&want), "{want} in {events:?}");
    }
    // The same script twice: the same digest (what the self-test pins).
    let again = xtask(&[
        "anim",
        "simulate",
        "strider",
        "--script",
        script.to_str().unwrap(),
        "--json",
    ]);
    let full: Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(full["digest"], trace["digest"]);
}

/// A copy of the engine's content to break things in.
fn content_copy() -> tempfile::TempDir {
    let base = root().join("target/test-projects");
    std::fs::create_dir_all(&base).unwrap();
    let tmp = tempfile::tempdir_in(&base).unwrap();
    let copy = |from: &Path, to: &Path| {
        let mut stack = vec![(from.to_path_buf(), to.to_path_buf())];
        while let Some((a, b)) = stack.pop() {
            std::fs::create_dir_all(&b).unwrap();
            for e in std::fs::read_dir(&a).unwrap().flatten() {
                let p = e.path();
                let q = b.join(e.file_name());
                if p.is_dir() {
                    stack.push((p, q));
                } else {
                    std::fs::copy(&p, &q).unwrap();
                }
            }
        }
    };
    copy(&root().join("assets"), &tmp.path().join("assets"));
    std::fs::write(
        tmp.path().join("trino.toml"),
        "[project]\nname = \"p\"\ntitle = \"P\"\n\n[n64]\npackage = \"p\"\n\n[3ds]\npackage = \"p\"\nbin = \"p\"\n",
    )
    .unwrap();
    tmp
}

#[test]
fn check_names_a_broken_graph_and_new_writes_a_starter() {
    let tmp = content_copy();
    let dir = tmp.path();
    std::fs::write(
        dir.join("assets/anim/broken.anim.ron"),
        r#"AnimGraph(
            model: "models/strider",
            params: { "speed": Float(0.0) },
            states: { "a": Clip(clip: "swim"), "b": Clip(clip: "hop") },
            transitions: [(from: "a", to: "c", when: Trigger("speed"))],
        )"#,
    )
    .unwrap();
    let o = xtask_in(dir, &["anim", "check", "broken", "--platform", "n64"]);
    assert!(!o.status.success());
    let out = text(&o.stdout);
    assert!(
        out.contains("clip `swim` is not in models/strider"),
        "{out}"
    );
    assert!(out.contains("to `c`: no such state"), "{out}");
    assert!(
        out.contains("Trigger(`speed`) needs a Trigger parameter"),
        "{out}"
    );
    // `new`: one state per clip, which checks clean.
    let o = xtask_in(
        dir,
        &[
            "anim",
            "new",
            "walker",
            "--model",
            "models/strider",
            "--json",
        ],
    );
    assert!(o.status.success(), "{}", text(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["states"].as_array().unwrap().len(), 4);
    let o = xtask_in(dir, &["anim", "check", "walker", "--platform", "pc"]);
    assert!(o.status.success(), "{}", text(&o.stdout));
    let o = xtask_in(dir, &["anim", "new", "walker", "--model", "models/strider"]);
    assert!(!o.status.success(), "no overwrite");
    let o = xtask_in(dir, &["anim", "frobnicate"]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("anim simulate"));
}

#[test]
fn model_inspect_reports_root_motion_events_and_compression() {
    let o = xtask(&[
        "model",
        "inspect",
        "models/strider",
        "--anim",
        "--compressed",
        "--json",
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["anmc"], true, "the manifest compresses its clips");
    let walk = v["anim"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "walk")
        .unwrap()
        .clone();
    assert_eq!(walk["root_motion"], true);
    assert!(walk["root_distance"].as_f64().unwrap() > 0.79);
    assert!(walk["root_foot_slide"].as_f64().unwrap() < walk["foot_slide"].as_f64().unwrap());
    assert_eq!(walk["events"].as_array().unwrap().len(), 2);
    let c = v["compression"].as_array().unwrap();
    assert_eq!(c.len(), 4);
    for r in c {
        assert!(r["bytes"].as_u64() < r["raw_bytes"].as_u64(), "{r}");
        assert!(r["max_rotation_deg"].as_f64().unwrap() <= 0.5, "{r}");
    }
    // The robot isn't compressed in the manifest: inspect says what it would save.
    let o = xtask(&["model", "inspect", "models/robot", "--compressed"]);
    assert!(o.status.success());
    assert!(text(&o.stdout).contains("not baked so"), "{}", text(&o.stdout));
}
