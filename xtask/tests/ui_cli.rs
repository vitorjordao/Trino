//! `cargo xtask ui check|shot|export|list` through the real binary (issue
//! #58): the loop an agent or a script runs after editing a
//! `ui/*.ui.ron` by hand, against the fixtures in `tests/ui_fixtures/`
//! (one per kind of problem) and the screens the repo ships.

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

fn fixture(name: &str) -> String {
    format!("xtask/tests/ui_fixtures/{name}.ui.ron")
}

/// `(platform, level, message)` of every issue `ui check --json` reports.
fn issues(report: &serde_json::Value) -> Vec<(String, String, String)> {
    report["screens"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["issues"].as_array().unwrap().iter())
        .map(|i| {
            (
                i["platform"].as_str().unwrap_or("").to_string(),
                i["level"].as_str().unwrap().to_string(),
                i["message"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// A shot needs a GPU: skip without one, unless `TRINO_REQUIRE_GPU` is set.
fn gpu_missing(o: &Output) -> bool {
    let missing = !o.status.success() && text(&o.stderr).contains("no GPU");
    assert!(
        !missing || std::env::var("TRINO_REQUIRE_GPU").is_err(),
        "{}",
        text(&o.stderr)
    );
    if missing {
        eprintln!("skipping: {}", text(&o.stderr));
    }
    missing
}

#[test]
fn ui_check_accepts_the_shipped_screens() {
    // The engine's (the self-test screen), on every target.
    let o = xtask(&["ui", "check", "--json"]);
    assert!(o.status.success(), "{}{}", text(&o.stdout), text(&o.stderr));
    let report = json(&o);
    assert_eq!(report["errors"], 0);
    assert_eq!(report["warnings"], 0);
    let screen = &report["screens"][0];
    assert_eq!(screen["name"], "selftest");
    let platforms: Vec<&str> = screen["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["platform"].as_str().unwrap())
        .collect();
    for p in ["n64", "3ds", "pc", "web"] {
        assert!(platforms.contains(&p), "{p} in {platforms:?}");
    }
    let n64 = screen["targets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["platform"] == "n64")
        .unwrap();
    assert_eq!(n64["max_nodes"], 128);
    assert!(n64["nodes"].as_u64().unwrap() <= 128);
    // Every new project's starter screens, in its own language and fonts
    // (the template folder holds `ui/` and `assets/manifest.toml`), and
    // Solid Eleven's pause menu against Solid Eleven's assets and its two
    // languages (issue #59).
    let o = xtask(&[
        "ui",
        "check",
        "templates/standalone/ui/title.ui.ron",
        "templates/standalone/ui/pause.ui.ron",
        "templates/standalone/ui/options.ui.ron",
        "templates/standalone/ui/controls.ui.ron",
        "examples/solid-eleven/ui/pause.ui.ron",
    ]);
    let out = text(&o.stdout);
    assert!(o.status.success(), "{out}{}", text(&o.stderr));
    assert!(
        out.contains("5 screen(s), 4 target(s), 2 language(s): 0 error(s), 0 warning(s)"),
        "{out}"
    );
}

#[test]
fn ui_check_reports_each_fixture_with_its_platform_and_level() {
    // (fixture, platform, level, message) — the platform is empty for
    // structural issues (every target has them).
    let cases = [
        (
            "unreachable",
            "n64",
            "error",
            "the pad cannot reach this control",
        ),
        (
            "unreachable",
            "3ds",
            "error",
            "the pad cannot reach this control",
        ),
        (
            "unreachable",
            "pc",
            "warning",
            "the pad cannot reach this control",
        ),
        (
            "budget",
            "n64",
            "error",
            "over the target's 128 (Caps::max_ui_nodes)",
        ),
        ("overflow", "n64", "warning", "text overflows its 12×10 box"),
        ("broken", "", "error", "duplicate id \"ok\""),
        (
            "broken",
            "",
            "error",
            "nav.down: no node has id \"nowhere\"",
        ),
        ("broken", "", "error", "a Slider needs max > min (5..5)"),
        ("unparsable", "", "error", "Unexpected field named `colour`"),
        ("overlap", "pc", "warning", "overlaps Button \"a\""),
        ("overlap", "3ds", "warning", "overlaps Button \"a\""),
        (
            "missing",
            "",
            "error",
            "sprites/no-such-sprite is not in the project's manifest",
        ),
        (
            "missing",
            "",
            "error",
            "models/no-such-model is not in the project's manifest",
        ),
    ];
    for name in [
        "unreachable",
        "budget",
        "overflow",
        "broken",
        "unparsable",
        "overlap",
        "missing",
    ] {
        let o = xtask(&["ui", "check", &fixture(name), "--json"]);
        assert!(!o.status.success(), "{name} must fail the check");
        let report = json(&o);
        let found = issues(&report);
        for (_, platform, level, message) in cases.iter().filter(|c| c.0 == name) {
            assert!(
                found
                    .iter()
                    .any(|(p, l, m)| p == platform && l == level && m.contains(message)),
                "{name}: no {level} on '{platform}' saying {message:?} in {found:#?}"
            );
        }
        let errors = found.iter().filter(|i| i.1 == "error").count();
        assert_eq!(report["errors"], errors, "{name}");
        assert_eq!(report["warnings"], found.len() - errors, "{name}");
    }
    // The budget fits the PC: one target, no issue.
    let o = xtask(&["ui", "check", &fixture("budget"), "--platform", "pc"]);
    assert!(o.status.success(), "{}", text(&o.stdout));
    assert!(
        text(&o.stdout).contains("202/512 nodes"),
        "{}",
        text(&o.stdout)
    );
    // Warnings fail too (an overflow is a bug a player sees): once with the
    // default texts and once per language of the engine's tables (#59).
    let o = xtask(&["ui", "check", &fixture("overflow"), "--platform", "pc"]);
    assert!(!o.status.success());
    assert!(
        text(&o.stdout).contains("0 error(s), 4 warning(s)"),
        "{}",
        text(&o.stdout)
    );
}

#[test]
fn ui_check_and_shot_refuse_bad_arguments() {
    let o = xtask(&["ui", "check", "--platform", "gameboy"]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("gameboy"), "{}", text(&o.stderr));
    let o = xtask(&["ui", "frobnicate"]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("cargo xtask ui check"),
        "{}",
        text(&o.stderr)
    );
    // Under --json the error is JSON too.
    let o = xtask(&["ui", "shot", "no-such-screen", "--json"]);
    assert!(!o.status.success());
    let e = json(&o);
    assert!(
        e["error"].as_str().unwrap().contains("no-such-screen"),
        "{e}"
    );
    let o = xtask(&["ui", "check", "--lang", "xx-XX"]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("xx-XX"), "{}", text(&o.stderr));
}

#[test]
fn ui_shot_renders_each_target_at_its_size() {
    let tmp = tempfile::tempdir().unwrap();
    let out = |name: &str| tmp.path().join(name).to_str().unwrap().to_string();
    // (platform, extra args, picture size): the 3DS stacks its two screens.
    let cases: [(&str, &[&str], (u64, u64)); 4] = [
        ("n64", &[], (320, 240)),
        ("3ds", &[], (400, 480)),
        ("pc", &["--size", "2560x1080"], (2560, 1080)),
        ("web", &["--size", "800x600"], (800, 600)),
    ];
    for (platform, extra, (w, h)) in cases {
        let png = out(&format!("selftest-{platform}.png"));
        let mut args = vec![
            "ui",
            "shot",
            "selftest",
            "--platform",
            platform,
            "--out",
            &png,
        ];
        args.extend_from_slice(extra);
        args.push("--json");
        let o = xtask(&args);
        if gpu_missing(&o) {
            return;
        }
        assert!(o.status.success(), "{platform}: {}", text(&o.stderr));
        let r = json(&o);
        assert_eq!(
            (r["width"].as_u64(), r["height"].as_u64()),
            (Some(w), Some(h)),
            "{r}"
        );
        assert_eq!(r["platform"], platform);
        assert_eq!(r["overflow"], 0);
        assert!(r["nodes"].as_u64().unwrap() >= 6, "{r}");
        assert!(r["draws"].as_u64().unwrap() > 0, "{r}");
        let decoder =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&png).unwrap()));
        let info = decoder.read_info().unwrap().info().clone();
        assert_eq!((info.width as u64, info.height as u64), (w, h));
    }
    // A focus that names no control is an error, not a silent default.
    let o = xtask(&[
        "ui",
        "shot",
        "selftest",
        "--focus",
        "nope",
        "--out",
        &out("x.png"),
    ]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("--focus"), "{}", text(&o.stderr));
}

/// `ui export` writes what the screen's static code is, and Solid Eleven's
/// committed `pause_gen.rs` is what its `ui/pause.ui.ron` exports to today
/// (edit the RON, run the export, commit both).
#[test]
fn ui_export_writes_static_code_and_the_committed_exports_are_fresh() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("pause_gen.rs");
    let o = xtask(&[
        "ui",
        "export",
        "examples/solid-eleven/ui/pause.ui.ron",
        out.to_str().unwrap(),
        "--json",
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let r = json(&o);
    assert_eq!(r["screen"], "se_pause");
    let fresh = std::fs::read_to_string(&out).unwrap();
    assert_eq!(r["bytes"], fresh.len());
    assert!(fresh.contains("pub static SCREEN"), "{fresh}");
    // Every Solid Eleven screen (issue #95) ships as `src/ui/<name>_gen.rs`:
    // none may be stale.
    let mut screens = 0;
    for entry in std::fs::read_dir(root().join("examples/solid-eleven/ui")).unwrap() {
        let path = entry.unwrap().path();
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".ui.ron"))
        else {
            continue;
        };
        let out = tmp.path().join(format!("{name}_gen.rs"));
        let o = xtask(&[
            "ui",
            "export",
            path.to_str().unwrap(),
            out.to_str().unwrap(),
        ]);
        assert!(o.status.success(), "{name}: {}", text(&o.stderr));
        let fresh = std::fs::read_to_string(&out).unwrap();
        let gen_path = format!("examples/solid-eleven/src/ui/{name}_gen.rs");
        let committed = std::fs::read_to_string(root().join(&gen_path))
            .unwrap_or_else(|_| panic!("{gen_path} is missing"));
        assert!(
            committed == fresh,
            "{gen_path} is stale: run `cargo xtask ui export \
             examples/solid-eleven/ui/{name}.ui.ron {gen_path}`"
        );
        screens += 1;
    }
    assert!(screens >= 3, "{screens} Solid Eleven screens");
    // A broken screen exports nothing.
    let bad = tmp.path().join("bad.rs");
    let o = xtask(&["ui", "export", &fixture("broken"), bad.to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(!bad.exists());
    assert!(
        text(&o.stderr).contains("duplicate id"),
        "{}",
        text(&o.stderr)
    );
}

#[test]
fn ui_list_names_the_screens() {
    let o = xtask(&["ui", "list", "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let r = json(&o);
    let names: Vec<&str> = r["screens"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    assert!(names.contains(&"selftest"), "{r}");
    assert!(r["languages"].is_array(), "{r}");
    let o = xtask(&["ui", "list"]);
    assert!(text(&o.stdout).contains("selftest"), "{}", text(&o.stdout));
}
