//! `cargo xtask perf report|ab` (issue #79) end to end on the host: a
//! report built from ROM log lines prints its tables and JSON, and an A/B
//! over saved run logs (`--reuse`, what a finished sweep leaves in
//! `<out>/<arm>/pad_<p>.log`) reports each arm's spread, whether the work
//! matched, and the control's shift beside the change's. The sweep itself
//! (worktrees, `test n64` per padding) needs Docker and ares: CI's n64 job
//! runs `test n64`, and the PR quotes a real `perf ab --control`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use trino_trace::perf::{Collector, Symbols};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Scratch space under `target/` (never the system temp dir).
fn scratch(name: &str) -> PathBuf {
    let d = root().join("target/test-projects/perf").join(name);
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

/// What `cargo xtask test n64 --prof rsp` writes for a scene.
fn hub_report(dir: &Path) -> PathBuf {
    let mut c = Collector::default();
    for l in [
        "[ares] TRINO_PERF_SCENE:hub frames=4 update=300 render=13000 begin=1000 tnl=5200 \
         sort=400 submit=2100 verts=820 rsp_draws=41 memo_hit=37 memo_miss=4 rspq_words=1900",
        "[ares] TRINO_PERF_MODEL:hub model=0000abcd calls=2 us=150 verts=8 tris_in=4 emitted=4",
        "TRINO_PROF_RSP:hub empty_drain_us=45",
        "TRINO_PROF_RSP:hub ovl=color verts=30 tris=20 n=3 us=180",
        "TRINO_PROF_RSP:hub ovl=color verts=60 tris=20 n=1 us=90",
        "TRINO_PROF_RSP:hub ovl=color verts=60 tris=60 n=1 us=130",
    ] {
        c.feed("n64", "prof-rsp", 352, l);
    }
    let r = c.finish(Some(&Symbols::default()), |_| None).remove(0);
    let f = dir.join("n64-hub.json");
    std::fs::write(&f, r.to_json()).unwrap();
    f
}

#[test]
fn perf_report_prints_a_scenes_tables_and_json() {
    let dir = scratch("report");
    let f = hub_report(&dir);
    let text = ok(&xtask(&["perf", "report", f.to_str().unwrap()]));
    assert!(
        text.contains("n64 hub (prof-rsp build, pad 352): frame 12300 us"),
        "{text}"
    );
    assert!(
        text.contains("memo_hit=37") && text.contains("rspq_words=1900"),
        "{text}"
    );
    // 20 µs + 1 per vertex + 1 per triangle: the fit finds it.
    assert!(text.contains("color        5"), "{text}");
    assert!(text.contains("= 10.0 + 1.000·v + 1.000·t (1.00)"), "{text}");
    let v: serde_json::Value = serde_json::from_str(&ok(&xtask(&[
        "perf",
        "report",
        f.to_str().unwrap(),
        "--json",
    ])))
    .expect("--json prints JSON only");
    assert_eq!(v[0]["format"], "trino-perf-report");
    assert_eq!(v[0]["counters"]["memo_miss"], 4);
    assert_eq!(v[0]["rsp"]["fits"][0]["per_tri"], 1.0);
    // Not a report: a clear error.
    let bad = dir.join("bad.json");
    std::fs::write(&bad, "{}").unwrap();
    let out = xtask(&["perf", "report", bad.to_str().unwrap()]);
    assert!(!out.status.success());
}

/// One run log of an arm: its scenes' lines (the rest of a real log is
/// noise the parser skips).
fn log(frame: u64, tnl: u64, verts: u64) -> String {
    format!(
        "[n64] building...\n[ares] TRINO_STEP:trace\n\
         [ares] TRINO_PERF_SCENE:hub frames=4 update=0 render={frame} begin=0 tnl={tnl} \
         verts={verts} rsp_draws=40\n\
         [ares] TRINO_PERF_SCENE:throne frames=4 update=0 render=8000 begin=0 tnl=2000 \
         verts=300\n[n64] TEST PASS\n"
    )
}

#[test]
fn perf_ab_over_saved_runs_reports_spreads_work_and_the_control() {
    let out = scratch("ab");
    let runs = [
        ("base", [(13_000, 5_000), (13_600, 5_100)], 820),
        ("head", [(12_100, 4_100), (12_500, 4_300)], 820),
        ("control", [(13_500, 5_300), (12_900, 4_900)], 820),
    ];
    for (arm, pads, verts) in runs {
        for ((frame, tnl), p) in pads.iter().zip([0, 352]) {
            let d = out.join(arm);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(format!("pad_{p}.log")), log(*frame, *tnl, verts)).unwrap();
        }
    }
    let o = out.to_str().unwrap();
    let args = [
        "perf",
        "ab",
        "--base",
        "main",
        "--pads",
        "0,352",
        "--control",
        "--reuse",
        "--out",
        o,
    ];
    let text = ok(&xtask(&args));
    assert!(text.contains("control = base + a no-op patch"), "{text}");
    assert!(text.contains("[hub] work identical in every arm"), "{text}");
    assert!(
        text.contains("frame    base=13300 (13000–13600) head=12300 (12100–12500)"),
        "{text}"
    );
    assert!(text.contains("head−base -1000 (-7.5%)"), "{text}");
    assert!(text.contains("control−base -100 (-0.8%)"), "{text}");
    // The report file the editor's A/B view reads, and --json.
    assert!(out.join("ab.json").exists());
    let mut json_args = args.to_vec();
    json_args.push("--json");
    let v: serde_json::Value =
        serde_json::from_str(&ok(&xtask(&json_args))).expect("--json prints JSON only");
    assert_eq!(v["format"], "trino-perf-ab");
    assert_eq!(v["paddings"], serde_json::json!([0, 352]));
    let hub = &v["scenes"][0];
    assert_eq!(hub["scene"], "hub");
    assert_eq!(
        hub["arms"][0]["stages"]["frame"]["per_pad"],
        serde_json::json!([13000, 13600])
    );
    // One arm doing more work: no longer a layout question.
    std::fs::write(out.join("head/pad_352.log"), log(12_500, 4_300, 821)).unwrap();
    let text = ok(&xtask(&args));
    assert!(text.contains("[hub] work DIFFERS between arms"), "{text}");
    // A missing run is an error that names the file.
    std::fs::remove_file(out.join("control/pad_0.log")).unwrap();
    let e = xtask(&args);
    assert!(!e.status.success());
    assert!(String::from_utf8_lossy(&e.stderr).contains("pad_0.log"));
}

#[test]
fn perf_and_rsp_reject_bad_usage() {
    for args in [&["perf"][..], &["perf", "ab"], &["rsp"]] {
        let out = xtask(args);
        assert!(!out.status.success(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("usage"),
            "{args:?}"
        );
    }
    let out = xtask(&["perf", "ab", "--base", "x", "--paddings", "99"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("--paddings 1..=8"));
}
