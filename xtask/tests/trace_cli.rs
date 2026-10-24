//! `cargo xtask trace collect|report|diff` (issue #62) end to end: a log
//! recorded in ares (`tests/fixtures/trace/n64-hub.log`, the `n64-hub`
//! session of `cargo xtask test n64 --trace`) collects into the committed
//! Perfetto JSON, the report names the N64 renderer's stages, and the diff
//! subtracts what it should. `TRINO_BLESS=1` rewrites the golden JSON.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Scratch space under `target/` (never the system temp dir).
fn scratch(name: &str) -> PathBuf {
    let d = root().join("target/test-projects/trace").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn xtask(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(args)
        .current_dir(root())
        .output()
        .expect("xtask runs")
}

fn ok(out: &Output) -> String {
    assert!(
        out.status.success(),
        "xtask failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_str(&ok(out)).expect("--json prints JSON only")
}

#[test]
fn collect_turns_an_ares_log_into_the_golden_perfetto_json() {
    let dir = scratch("collect");
    let out = dir.join("n64-hub.json");
    let log = root().join("tests/fixtures/trace/n64-hub.log");
    let v = json(&xtask(&[
        "trace",
        "collect",
        log.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(v["sessions"][0]["session"], "n64-hub");
    assert_eq!(v["sessions"][0]["platform"], "n64");
    assert_eq!(v["sessions"][0]["dropped"], 0);
    let got: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let golden = root().join("tests/fixtures/trace/n64-hub.json");
    if std::env::var_os("TRINO_BLESS").is_some() {
        std::fs::write(&golden, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
    }
    let want: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&golden).unwrap()).unwrap();
    assert_eq!(got, want, "collect drifted from {}", golden.display());
    // Chrome Trace Event shape: complete events with µs times per track.
    let events = got["traceEvents"].as_array().unwrap();
    assert!(events.iter().any(|e| e["ph"] == "X" && e["name"] == "draw"));
    assert!(
        events
            .iter()
            .any(|e| e["ph"] == "C" && e["name"] == "rsp_tris")
    );
    assert!(
        events
            .iter()
            .any(|e| e["ph"] == "i" && e["name"] == "frame")
    );
    assert!(
        events
            .iter()
            .any(|e| e["ph"] == "M" && e["args"]["name"] == "n64-hub")
    );
}

#[test]
fn report_names_the_n64_stages_per_frame() {
    let log = root().join("tests/fixtures/trace/n64-hub.log");
    let v = json(&xtask(&[
        "trace",
        "report",
        log.to_str().unwrap(),
        "--json",
    ]));
    let s = &v["sessions"][0];
    assert_eq!(s["session"], "n64-hub");
    let zones: Vec<&str> = s["zones"]
        .as_array()
        .unwrap()
        .iter()
        .map(|z| z["name"].as_str().unwrap())
        .collect();
    for want in [
        "render",
        "update",
        "draw",
        "begin",
        "end",
        "sort",
        "submit",
        "rsp queue",
    ] {
        assert!(zones.contains(&want), "{want} missing from {zones:?}");
    }
    // Four measured frames: four marks, three whole frames.
    assert_eq!(s["frames"]["count"], 3);
    let render = s["zones"]
        .as_array()
        .unwrap()
        .iter()
        .find(|z| z["name"] == "render")
        .unwrap();
    assert_eq!(render["count"], 4);
    let draws = s["counters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "draws")
        .unwrap();
    assert_eq!(draws["samples"], 4);
    // The text form too.
    let text = ok(&xtask(&["trace", "report", log.to_str().unwrap()]));
    assert!(
        text.contains("n64-hub (n64)") && text.contains("rsp queue"),
        "{text}"
    );
}

/// A one-zone trace of `n` frames with the zone at `dur` µs each.
fn synthetic(dir: &Path, name: &str, session: &str, dur: u64, n: u64) -> PathBuf {
    let mut events = vec![
        serde_json::json!({"ph": "M", "name": "process_name", "pid": 1, "tid": 0,
            "args": {"name": session}}),
    ];
    for i in 0..n {
        let t = i * 1000;
        events.push(
            serde_json::json!({"ph": "i", "name": "frame", "s": "p", "pid": 1,
            "tid": 0, "ts": t, "args": {"frame": i}}),
        );
        events.push(
            serde_json::json!({"ph": "X", "name": "tnl", "pid": 1, "tid": 0,
            "ts": t, "dur": dur}),
        );
    }
    let p = dir.join(name);
    std::fs::write(&p, serde_json::json!({ "traceEvents": events }).to_string()).unwrap();
    p
}

#[test]
fn diff_reports_b_minus_a_per_zone() {
    let dir = scratch("diff");
    let a = synthetic(&dir, "a.json", "pc-hub", 400, 5);
    let b = synthetic(&dir, "b.json", "n64-hub", 1000, 5);
    let v = json(&xtask(&[
        "trace",
        "diff",
        a.to_str().unwrap(),
        b.to_str().unwrap(),
        "--json",
    ]));
    // Paired by the name past the platform prefix.
    let d = &v["diffs"][0];
    assert_eq!(d["a"]["session"], "pc-hub");
    assert_eq!(d["b"]["session"], "n64-hub");
    let z = &d["zones"][0];
    assert_eq!(z["name"], "tnl");
    assert_eq!(z["mean_delta_us"], 600.0);
    assert_eq!(z["mean_pct"], 150.0);
    assert_eq!(z["p95_delta_us"], 600);
    assert_eq!(z["per_frame_delta_us"], 600.0);
    // Unpairable names need --session.
    let c = synthetic(&dir, "c.json", "other", 1, 2);
    let two = dir.join("two.json");
    let mut t: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&a).unwrap()).unwrap();
    let mut extra = t["traceEvents"].as_array().unwrap().clone();
    for e in &mut extra {
        e["pid"] = 2.into();
        if e["ph"] == "M" {
            e["args"]["name"] = "second".into();
        }
    }
    t["traceEvents"].as_array_mut().unwrap().extend(extra);
    std::fs::write(&two, t.to_string()).unwrap();
    let out = xtask(&["trace", "diff", c.to_str().unwrap(), two.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--session"));
    let v = json(&xtask(&[
        "trace",
        "diff",
        c.to_str().unwrap(),
        two.to_str().unwrap(),
        "--session",
        "other=second",
        "--json",
    ]));
    assert_eq!(v["diffs"][0]["b"]["session"], "second");
}

#[test]
fn collect_refuses_a_log_with_a_missing_line() {
    let dir = scratch("gap");
    let log = std::fs::read_to_string(root().join("tests/fixtures/trace/n64-hub.log")).unwrap();
    let cut: String = log
        .lines()
        .enumerate()
        .filter(|(i, _)| *i != 3)
        .map(|(_, l)| format!("{l}\n"))
        .collect();
    let p = dir.join("cut.log");
    std::fs::write(&p, cut).unwrap();
    let out = xtask(&["trace", "collect", p.to_str().unwrap()]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("n64-hub") && err.contains("missing before 3"),
        "{err}"
    );
}

#[test]
fn trace_flag_goes_with_platform_commands_only() {
    let out = xtask(&["scene", "check", "--trace"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--trace goes with"));
}
