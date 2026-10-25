//! `cargo xtask physics check|query|layers` through the real binary (issue
//! #66): what an agent runs after writing colliders by hand — against the
//! physics playground the repo ships and a fixture with one of each
//! problem.

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

const PLAYGROUND: &str = "scenes/physics_playground.scene.ron";

#[test]
fn check_counts_the_playground_and_its_budget() {
    let o = xtask(&["physics", "check", PLAYGROUND, "--json"]);
    assert!(o.status.success(), "{}{}", text(&o.stdout), text(&o.stderr));
    let r = json(&o);
    assert_eq!(r["ok"], true);
    let s = &r["scenes"][0];
    assert_eq!(s["one_way"], 1);
    assert_eq!(s["ramps"], 2);
    assert_eq!(s["meshes"][0]["model"], "models/rock");
    assert_eq!(s["meshes"][0]["triangles"], 28);
    assert_eq!(s["triggers"][0], "goal-zone");
    let step = s["characters"][0]["step_height"].as_f64().unwrap();
    assert!((step - 0.3).abs() < 1e-6, "{step}");
    let n64 = s["budget"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["platform"] == "n64")
        .unwrap();
    assert_eq!(
        (n64["collision_tris"].as_u64(), n64["fits"].as_bool()),
        (Some(28), Some(true))
    );
    let issues = s["issues"].as_array().unwrap();
    assert!(
        issues
            .iter()
            .any(|i| i["level"] == "info" && i["message"].as_str().unwrap().contains("ramp-50")),
        "{issues:?}"
    );
    // The text form says the same in a line.
    let o = xtask(&["physics", "check", PLAYGROUND]);
    assert!(text(&o.stdout).contains("2 ramp(s), 1 mesh collider(s) (28 triangles)"));
}

#[test]
fn check_reports_each_problem_and_fails() {
    let o = xtask(&[
        "physics",
        "check",
        "xtask/tests/physics_fixtures/problems.scene.ron",
        "--json",
    ]);
    assert!(!o.status.success());
    let r = json(&o);
    assert_eq!(r["ok"], false);
    let messages: Vec<(String, String)> = r["scenes"][0]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["level"].as_str().unwrap().to_string(),
                i["message"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let has =
        |level: &str, part: &str| messages.iter().any(|(l, m)| l == level && m.contains(part));
    assert!(
        has("warning", "`floor` and `sunk-crate` overlap"),
        "{messages:?}"
    );
    assert!(
        has("error", "trigger `Entity 3` has no name"),
        "{messages:?}"
    );
    assert!(has("error", "no collision layer `ghosts`"), "{messages:?}");
    assert!(
        has(
            "error",
            "mesh collider `models/crate` has no baked collision"
        ),
        "{messages:?}"
    );
}

#[test]
fn queries_answer_like_the_engine() {
    let q = |args: &[&str]| {
        let mut all = vec!["physics", "query", PLAYGROUND];
        all.extend_from_slice(args);
        all.push("--json");
        let o = xtask(&all);
        assert!(o.status.success(), "{}{}", text(&o.stdout), text(&o.stderr));
        json(&o)
    };
    // Down onto the 30° ramp: its slope's normal.
    let r = q(&["--ray", "-7,10,-4", "0,-1,0"]);
    assert_eq!(r["hit"]["entity"], "ramp-30");
    assert_eq!(r["hit"]["kind"], "ramp");
    let ny = r["hit"]["normal"][1].as_f64().unwrap();
    assert!((ny - 30f64.to_radians().cos()).abs() < 1e-3, "{r}");
    // Onto the dome's triangles.
    let r = q(&["--ray", "-6,10,6.5", "0,-1,0"]);
    assert_eq!(
        (r["hit"]["entity"].as_str(), r["hit"]["kind"].as_str()),
        (Some("dome"), Some("mesh"))
    );
    // A character dropped on the 50° ramp slides off its low end (+Z) to
    // the ground.
    let r = q(&["--drop", "-1,6,-5"]);
    assert_eq!(r["ground"]["entity"], "ground");
    assert!(r["feet"][2].as_f64().unwrap() > -3.6, "{r}");
    // One dropped on the stairs stands on a step; on the one-way, on top.
    let r = q(&["--drop", "5,3,-2.4", "--character", "0.3,1.7,45,0.3"]);
    assert!(
        r["ground"]["entity"]
            .as_str()
            .unwrap()
            .starts_with("stairs/step-"),
        "{r}"
    );
    let r = q(&["--drop", "9.5,4,-1.5"]);
    assert_eq!(r["ground"]["kind"], "one_way");
    // The goal zone is a trigger, not a solid.
    let r = q(&["--overlap", "1.5,1,9", "0.3,0.3,0.3"]);
    assert_eq!(r["triggers"][0], "goal-zone");
    assert!(r["solids"].as_array().unwrap().is_empty());
    // A box cast toward the 30° ramp stops at its box (shape casts test
    // ramps as their box).
    let r = q(&["--cast", "box:0.3,0.3,0.3", "-7,0.6,4", "-7,0.6,-10"]);
    assert_eq!(r["hit"]["entity"], "ramp-30");
    // Bad arguments say what is wrong.
    let o = xtask(&["physics", "query", PLAYGROUND, "--ray", "1,2", "0,-1,0"]);
    assert!(!o.status.success() && text(&o.stderr).contains("expected x,y,z"));
}

#[test]
fn layers_print_and_edit_the_matrix() {
    let dir = root().join("target/test-projects/physics-layers");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(
        dir.join("trino.toml"),
        "[project]\nname = \"layers\"\ntitle = \"Layers\"\n\n[n64]\npackage = \"layers\"\n\n\
         [3ds]\npackage = \"layers\"\nbin = \"layers\"\n",
    )
    .unwrap();
    let p = dir.to_str().unwrap();
    let run = |args: &[&str]| {
        let mut all = vec!["physics", "layers"];
        all.extend_from_slice(args);
        all.extend(["--project", p]);
        xtask(&all)
    };
    // No file: one `world` layer.
    let r = json(&run(&["--json"]));
    assert_eq!(r["layers"][0]["name"], "world");
    assert!(run(&["add", "player"]).status.success());
    assert!(run(&["add", "pickup"]).status.success());
    assert!(run(&["set", "player", "pickup", "off"]).status.success());
    let r = json(&run(&["--json"]));
    assert_eq!(r["layers"][1]["mask"], 0b011);
    assert_eq!(r["layers"][2]["mask"], 0b101);
    let file = std::fs::read_to_string(dir.join("assets/physics.ron")).unwrap();
    assert!(file.contains("(\"pickup\", \"player\")"), "{file}");
    assert!(!run(&["set", "player", "ghosts", "on"]).status.success());
    assert!(run(&["remove", "pickup"]).status.success());
    let r = json(&run(&["--json"]));
    assert_eq!(r["layers"].as_array().unwrap().len(), 2);
    assert!(r["ignore"].as_array().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
