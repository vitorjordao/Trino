//! `cargo xtask perf` (issue #79, ADR 0038): console perf reports and A/B
//! comparisons over code layouts.
//!
//! - `perf report <file.json..> [--json]` — print what `cargo xtask test
//!   n64` wrote to `target/perf/<target>-<scene>.json` (stages, work,
//!   backend counters, zones, the RSP fit, the CPU samples), or an A/B
//!   report.
//! - `perf ab --base <rev> [--head <rev>] [--paddings N | --pads a,b,..]
//!   [--control] [--scenes a,b] [--out dir] [--reuse] [--json]` — builds
//!   the baseline (and the head, when it is a revision) in its own git
//!   worktree and `target/`, runs `cargo xtask test n64` on each arm with
//!   the same `TRINO_N64_TEXT_PAD` paddings, and prints mean (min–max) per
//!   scene and stage. `--control` adds a third arm: the baseline plus a
//!   no-op patch (linked code that never runs), so the spread the layout
//!   alone causes stands beside the change's (#69: castle64 moved +0.7 ms
//!   with no change to its draws). Each arm's T&L work is compared too:
//!   identical work + a time shift = layout.
//!
//! The run logs stay in `<out>/<arm>/pad_<p>.log`; `--reuse` reads them
//! again instead of running (a report from a finished sweep).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use trino_trace::perf::{AbReport, PerfReport, parse_fields};

const USAGE: &str = "\
usage:
  cargo xtask perf report <file.json..> [--json]
  cargo xtask perf ab --base <rev> [--head <rev>] [--paddings N | --pads a,b,..] [--control]
                      [--scenes a,b] [--out <dir>] [--reuse] [--json]";

/// The paddings an A/B samples, in order (the `n64-performance` skill's
/// sweep): `--paddings N` takes the first N.
pub const PADDINGS: [u32; 8] = [0, 352, 1024, 2048, 3072, 4096, 6144, 8192];

/// The stages the A/B table prints.
const AB_STAGES: [&str; 5] = ["frame", "tnl", "sort", "submit", "end"];

pub fn run(root: &Path, args: &[&str]) -> ExitCode {
    let result = match args {
        ["report", rest @ ..] => report_cmd(rest),
        ["ab", rest @ ..] => ab_cmd(root, rest),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask perf: {e}");
            ExitCode::FAILURE
        }
    }
}

fn report_cmd(args: &[&str]) -> Result<(), String> {
    let json = args.contains(&"--json");
    let files: Vec<&str> = args.iter().copied().filter(|a| *a != "--json").collect();
    if files.is_empty() {
        return Err(USAGE.into());
    }
    let mut out = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?;
        if let Ok(ab) = AbReport::from_json(&text) {
            if json {
                out.push(serde_json::to_value(&ab).map_err(|e| e.to_string())?);
            } else {
                println!("{}", ab.table(&AB_STAGES));
            }
            continue;
        }
        let r = PerfReport::from_json(&text).map_err(|e| format!("{f}: {e}"))?;
        if json {
            out.push(serde_json::to_value(&r).map_err(|e| e.to_string())?);
        } else {
            println!("{}", r.table());
        }
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
        );
    }
    Ok(())
}

/// What one `test n64` run wrote, as a build's name: `plain`, `trace`,
/// `prof-rsp`, `prof-sample`.
pub fn build_name(p: &crate::project::Project) -> &'static str {
    use crate::project::ProfMode;
    match (p.trace, p.prof) {
        (_, Some(ProfMode::Rsp)) => "prof-rsp",
        (_, Some(ProfMode::Sample)) => "prof-sample",
        (true, _) => "trace",
        (false, _) => "plain",
    }
}

/// The run's `TRINO_N64_TEXT_PAD`.
pub fn text_pad() -> u32 {
    std::env::var("TRINO_N64_TEXT_PAD")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0)
}

/// Finish a run's reports and write them to `target/perf/<target>-<scene>.json`
/// (the folder's earlier reports of this target are replaced). `trace`:
/// the run's decoded trace (zones per `<target>-<scene>` session); `syms`:
/// the ELF's `nm -S -C` output, to symbolize samples. Returns the tables
/// to print for an instrumented build, else a one-line summary.
pub fn write_reports(
    root: &Path,
    target: &str,
    collector: trino_trace::perf::Collector,
    trace: Option<&trino_trace::Trace>,
    syms: Option<&str>,
    instrumented: bool,
) -> Result<String, String> {
    let syms = syms.map(trino_trace::perf::Symbols::parse);
    let reports = collector.finish(syms.as_ref(), |scene| {
        let s = trace?.session(&format!("{target}-{scene}"))?;
        Some(trino_trace::SessionStats::of(s).to_json())
    });
    if reports.is_empty() {
        return Ok("no perf scenes".into());
    }
    let dir = root.join("target/perf");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if let Ok(d) = std::fs::read_dir(&dir) {
        for e in d.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with(&format!("{target}-")) && name.ends_with(".json") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let mut tables = String::new();
    for r in &reports {
        let f = dir.join(format!("{target}-{}.json", r.scene));
        std::fs::write(&f, r.to_json()).map_err(|e| format!("{}: {e}", f.display()))?;
        if instrumented {
            tables.push_str(&r.table());
            tables.push('\n');
        }
    }
    Ok(format!(
        "{} reports -> {}{}{}",
        reports.len(),
        dir.display(),
        if tables.is_empty() { "" } else { "\n" },
        tables
    ))
}

#[derive(Debug)]
struct AbOpts {
    base: String,
    head: Option<String>,
    pads: Vec<u32>,
    control: bool,
    scenes: Vec<String>,
    out: Option<PathBuf>,
    reuse: bool,
    json: bool,
}

fn ab_opts(args: &[&str]) -> Result<AbOpts, String> {
    let mut o = AbOpts {
        base: String::new(),
        head: None,
        pads: PADDINGS.to_vec(),
        control: false,
        scenes: Vec::new(),
        out: None,
        reuse: false,
        json: false,
    };
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        let mut val = || it.next().copied().ok_or(format!("{a} needs a value"));
        match a {
            "--base" => o.base = val()?.to_string(),
            "--head" => o.head = Some(val()?.to_string()),
            "--paddings" => {
                let n: usize = val()?.parse().map_err(|_| "--paddings takes a number")?;
                if n == 0 || n > PADDINGS.len() {
                    return Err(format!("--paddings 1..={}", PADDINGS.len()));
                }
                o.pads = PADDINGS[..n].to_vec();
            }
            "--pads" => {
                o.pads = val()?
                    .split(',')
                    .map(|p| p.trim().parse::<u32>().map_err(|_| format!("--pads: {p}")))
                    .collect::<Result<_, _>>()?;
            }
            "--control" => o.control = true,
            "--scenes" => o.scenes = val()?.split(',').map(str::to_string).collect(),
            "--out" => o.out = Some(PathBuf::from(val()?)),
            "--reuse" => o.reuse = true,
            "--json" => o.json = true,
            other => return Err(format!("unknown option {other}\n{USAGE}")),
        }
    }
    if o.base.is_empty() {
        return Err(format!("--base <rev> is required\n{USAGE}"));
    }
    Ok(o)
}

/// The no-op patch of the control arm: a function that is linked (a
/// `#[used]` pointer keeps it) but never called, appended to the N64
/// backend so every function after it moves.
pub const CONTROL_PATCH: &str = r#"
/// `cargo xtask perf ab`'s control arm (issue #79): linked, never called,
/// so only the code layout changes.
#[cfg(target_os = "none")]
#[used]
static TRINO_AB_CONTROL: fn(u32) -> u32 = trino_ab_control;

#[cfg(target_os = "none")]
#[inline(never)]
fn trino_ab_control(mut x: u32) -> u32 {
    for i in 0..37u32 {
        x = x.rotate_left(i % 7).wrapping_mul(0x9E37_79B9) ^ i;
    }
    x
}
"#;

/// The file the control patch goes into (relative to the engine root).
pub const CONTROL_FILE: &str = "crates/platform-n64/src/lib.rs";

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed in {}",
            args.join(" "),
            dir.display()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A detached worktree of `rev` at `dir` (reused when it is already there
/// at that commit).
fn worktree(root: &Path, dir: &Path, rev: &str) -> Result<(), String> {
    let commit = git(root, &["rev-parse", &format!("{rev}^{{commit}}")])?;
    if dir.join(".git").exists() {
        if git(dir, &["rev-parse", "HEAD"])? == commit {
            return Ok(());
        }
        git(dir, &["checkout", "--detach", "--force", &commit])?;
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    git(
        root,
        &[
            "worktree",
            "add",
            "--detach",
            &dir.display().to_string(),
            &commit,
        ],
    )?;
    Ok(())
}

/// Append the control patch once.
fn apply_control(dir: &Path) -> Result<(), String> {
    let f = dir.join(CONTROL_FILE);
    let text = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
    if !text.contains("TRINO_AB_CONTROL") {
        std::fs::write(&f, format!("{text}{CONTROL_PATCH}"))
            .map_err(|e| format!("{}: {e}", f.display()))?;
    }
    Ok(())
}

/// Scene → fields of one run's log.
pub fn scenes_of(log: &str) -> trino_trace::perf::RunScenes {
    let mut out = BTreeMap::new();
    for line in log.lines() {
        if let Some((scene, f)) = parse_fields(line, "TRINO_PERF_SCENE:") {
            let f: BTreeMap<String, u64> = f
                .into_iter()
                .filter_map(|(k, v)| Some((k, v.parse().ok()?)))
                .collect();
            out.insert(scene, f);
        }
    }
    out
}

/// Run `cargo xtask test n64 --expansion-pak on` in `dir` with padding
/// `pad`, its output into `log`.
fn run_arm(dir: &Path, pad: u32, log: &Path) -> Result<(), String> {
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let file = std::fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = file.try_clone().map_err(|e| e.to_string())?;
    println!("[perf ab] {} pad {pad} -> {}", dir.display(), log.display());
    let status = Command::new("cargo")
        .args(["xtask", "test", "n64", "--expansion-pak", "on"])
        .current_dir(dir)
        // Each arm builds into its own tree's target/ (AGENTS pitfall).
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("TRINO_PROF")
        .env("TRINO_N64_TEXT_PAD", pad.to_string())
        .stdout(file)
        .stderr(err)
        .status()
        .map_err(|e| format!("cargo xtask test n64: {e}"))?;
    // A failing gate (a golden, a ceiling) still printed its scenes: the
    // lines are what counts here.
    if !status.success() {
        println!("[perf ab]   (the run failed its gates; its perf lines still count)");
    }
    Ok(())
}

fn ab_cmd(root: &Path, args: &[&str]) -> Result<(), String> {
    let o = ab_opts(args)?;
    let out = o.out.clone().unwrap_or_else(|| root.join("target/perf-ab"));
    let base_dir = out.join("trees/base");
    let control_dir = out.join("trees/control");
    let head_dir = match &o.head {
        Some(_) => out.join("trees/head"),
        None => root.to_path_buf(),
    };
    let mut arms: Vec<(&str, PathBuf)> = vec![("base", base_dir.clone()), ("head", head_dir)];
    if o.control {
        arms.push(("control", control_dir.clone()));
    }
    if !o.reuse {
        worktree(root, &base_dir, &o.base)?;
        if let Some(h) = &o.head {
            worktree(root, &out.join("trees/head"), h)?;
        }
        if o.control {
            worktree(root, &control_dir, &o.base)?;
            apply_control(&control_dir)?;
        }
        // Padding-major: every arm at one padding before the next.
        for &pad in &o.pads {
            for (arm, dir) in &arms {
                run_arm(dir, pad, &out.join(arm).join(format!("pad_{pad}.log")))?;
            }
        }
    }
    let mut runs = Vec::new();
    for (arm, _) in &arms {
        let mut pads = Vec::new();
        for &pad in &o.pads {
            let f = out.join(arm).join(format!("pad_{pad}.log"));
            let text = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
            let mut s = scenes_of(&text);
            if !o.scenes.is_empty() {
                s.retain(|k, _| o.scenes.contains(k));
            }
            if s.is_empty() {
                return Err(format!("{}: no TRINO_PERF_SCENE lines", f.display()));
            }
            pads.push(s);
        }
        runs.push((arm.to_string(), pads));
    }
    let head = o.head.clone().unwrap_or_else(|| "working tree".into());
    let report = AbReport::build("n64", &o.base, &head, &o.pads, &runs);
    let file = out.join("ab.json");
    std::fs::write(&file, report.to_json()).map_err(|e| format!("{}: {e}", file.display()))?;
    if o.json {
        println!("{}", report.to_json());
    } else {
        println!("{}", report.table(&AB_STAGES));
        println!("[perf ab] report: {}", file.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ab_options_parse_and_validate() {
        let o = ab_opts(&["--base", "main", "--paddings", "3", "--control", "--json"]).unwrap();
        assert_eq!(
            (o.base.as_str(), o.pads.as_slice()),
            ("main", &[0, 352, 1024][..])
        );
        assert!(o.control && o.json && !o.reuse && o.head.is_none());
        let o = ab_opts(&["--base", "x", "--pads", "0,64", "--scenes", "hub,throne"]).unwrap();
        assert_eq!(o.pads, vec![0, 64]);
        assert_eq!(o.scenes, vec!["hub", "throne"]);
        assert!(
            ab_opts(&["--paddings", "2"])
                .unwrap_err()
                .contains("--base")
        );
        assert!(ab_opts(&["--base", "x", "--paddings", "9"]).is_err());
        assert!(ab_opts(&["--base", "x", "--frobnicate"]).is_err());
    }

    #[test]
    fn a_log_gives_its_scenes_and_the_control_patch_is_valid_rust() {
        let log = "[ares] TRINO_PERF_SCENE:hub frames=4 update=1 render=10 verts=3\n\
                   [ares] TRINO_FRAME:x avg_us=1\n\
                   [ares] TRINO_PERF_SCENE:micro-tri frames=4 render=2\n";
        let s = scenes_of(log);
        assert_eq!(s.len(), 2);
        assert_eq!(s["hub"]["verts"], 3);
        // The patch is only for the N64 build and keeps its function
        // alive (balanced, `#[used]`, `cfg`-gated: the host build of the
        // backend stays an empty lib).
        let p = CONTROL_PATCH;
        assert_eq!(p.matches('{').count(), p.matches('}').count());
        assert!(p.contains("#[used]") && p.matches("#[cfg(target_os = \"none\")]").count() == 2);
    }

    #[test]
    fn the_control_patch_applies_once() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/test-projects/perf-ab-control");
        let f = dir.join(CONTROL_FILE);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, "#![no_std]\n").unwrap();
        apply_control(&dir).unwrap();
        apply_control(&dir).unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert_eq!(text.matches("static TRINO_AB_CONTROL").count(), 1);
    }
}
