//! `cargo xtask fuzz` (issue #68): the fuzz targets of `crates/fuzz`.
//!
//! - `fuzz list [--json]`: targets, formats, seeds, regressions, corpus;
//! - `fuzz corpus [--commit-seeds] [--json]`: bake castle64's, the
//!   showcase's and Solid Eleven's assets and sort them into
//!   `fuzz/corpus/<target>/` (`--commit-seeds` refreshes `fuzz/seeds/`);
//! - `fuzz smoke [<target>..] [--iters N] [--json]`: the stable mutation run
//!   (no nightly needed);
//! - `fuzz run <target>|all [--time S] [--jobs N]`: libFuzzer through
//!   cargo-fuzz (nightly, `cargo install cargo-fuzz`), with ASan, the
//!   corpus and the committed seeds; crashes land in `fuzz/artifacts/<t>/`;
//! - `fuzz seeds [<target>..]`: write the writers' seeds (`seedgen.rs`)
//!   into `fuzz/seeds/` without baking (a new `Seeds::Committed` target);
//! - `fuzz repro [<target>] <file> [--json]`: replay one input;
//! - `fuzz keep <file>`: copy a crash into `fuzz/regressions/<target>/`, which
//!   `cargo test -p trino-fuzz` replays on every PR;
//! - `fuzz coverage [<target>..|all] [--json]` (issue #121): `cargo fuzz
//!   coverage` over each target's corpus and seeds, then `llvm-cov export`
//!   to a summary per target (lines, functions and regions of the engine
//!   files the target reaches) in `fuzz/coverage/summary.{json,md}` and a
//!   per-file table in `fuzz/coverage/<target>/report.txt` (nightly +
//!   `rustup component add llvm-tools-preview --toolchain nightly`).
//!
//! The stable commands run the `trino-fuzz` binary (it links the PC crates,
//! which xtask itself does not, so the console jobs need no ALSA).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

/// `cargo run -p trino-fuzz -- <args>` (the workspace's dev profile is
/// optimized, and overflow checks stay on).
fn trino_fuzz(root: &Path, args: &[&str]) -> ExitCode {
    let status = Command::new(cargo())
        .current_dir(root)
        .args(["run", "--quiet", "-p", "trino-fuzz", "--"])
        .args(args)
        .status();
    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("xtask fuzz: cannot run cargo: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Every target and its `max_len`, from `trino-fuzz list --json`.
fn targets(root: &Path) -> Result<Vec<(String, u64)>, String> {
    let out = Command::new(cargo())
        .current_dir(root)
        .args(["run", "--quiet", "-p", "trino-fuzz", "--", "list", "--json"])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    Ok(v["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            (
                t["name"].as_str().unwrap_or("").to_string(),
                t["max_len"].as_u64().unwrap_or(4096),
            )
        })
        .collect())
}

pub fn run(root: &Path, rest: &[&str]) -> ExitCode {
    let value = |flag: &str| {
        rest.iter()
            .position(|a| *a == flag)
            .and_then(|i| rest.get(i + 1))
            .copied()
    };
    match rest.first().copied() {
        Some("list") | None => trino_fuzz(root, &[&["list"], &rest[1.min(rest.len())..]].concat()),
        Some(cmd @ ("smoke" | "repro" | "corpus" | "seeds")) => {
            let mut args = vec![cmd];
            args.extend_from_slice(&rest[1..]);
            trino_fuzz(root, &args)
        }
        Some("keep") => match rest.get(1) {
            Some(file) => keep(root, Path::new(file)),
            None => {
                eprintln!("usage: cargo xtask fuzz keep <crash file>");
                ExitCode::FAILURE
            }
        },
        Some("run") => {
            let Some(which) = rest.get(1).copied() else {
                eprintln!("usage: cargo xtask fuzz run <target>|all [--time S] [--jobs N]");
                return ExitCode::FAILURE;
            };
            let time: u64 = value("--time").and_then(|v| v.parse().ok()).unwrap_or(60);
            let jobs: u32 = value("--jobs").and_then(|v| v.parse().ok()).unwrap_or(1);
            let all = match targets(root) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("xtask fuzz: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let chosen: Vec<&(String, u64)> = if which == "all" {
                all.iter().collect()
            } else {
                match all.iter().find(|t| t.0 == which) {
                    Some(t) => vec![t],
                    None => {
                        eprintln!("xtask fuzz: no target `{which}` (cargo xtask fuzz list)");
                        return ExitCode::FAILURE;
                    }
                }
            };
            let mut failed = Vec::new();
            for (name, max_len) in chosen {
                if !libfuzzer(root, name, *max_len, time, jobs) {
                    failed.push(name.clone());
                }
            }
            if failed.is_empty() {
                ExitCode::SUCCESS
            } else {
                eprintln!(
                    "xtask fuzz: crashes in {} — see fuzz/artifacts/<target>/; `cargo xtask fuzz repro <file>`, then `fuzz keep <file>` once fixed",
                    failed.join(", ")
                );
                ExitCode::FAILURE
            }
        }
        Some("coverage") => coverage(root, &rest[1..]),
        Some(other) => {
            eprintln!(
                "xtask fuzz: unknown command `{other}` (list, corpus, seeds, smoke, run, repro, keep, coverage)"
            );
            ExitCode::FAILURE
        }
    }
}

/// Targets that draw on a GPU (`trino_fuzz::gpu_targets`).
const GPU_TARGETS: &[&str] = &["pc_renderer"];

/// One libFuzzer run: the corpus folder (written to) and the committed
/// seeds (read). Bounded time and memory per input, like the invariants
/// say.
fn libfuzzer(root: &Path, name: &str, max_len: u64, time: u64, jobs: u32) -> bool {
    let fuzz = root.join("fuzz");
    let corpus = fuzz.join("corpus").join(name);
    let _ = std::fs::create_dir_all(&corpus);
    let seeds = fuzz.join("seeds").join(name);
    // The rustup proxy on the PATH, which takes `+nightly` (`$CARGO`, under
    // `cargo xtask`, is the toolchain's own cargo and refuses it).
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&fuzz)
        // `-a`: overflow checks and debug assertions on. A size product that
        // wraps silently in a release build is the bug on the consoles'
        // 32-bit `usize` (issue #68 found five).
        .args(["+nightly", "fuzz", "run", "-O", "-a", name])
        .arg(&corpus)
        .arg(&seeds)
        .arg("--")
        .arg(format!("-max_total_time={time}"))
        .arg(format!("-max_len={max_len}"))
        .arg("-rss_limit_mb=2048")
        .arg("-malloc_limit_mb=1024")
        .arg("-timeout=10")
        .arg("-print_final_stats=1");
    if jobs > 1 {
        cmd.arg(format!("-fork={jobs}")).arg("-ignore_crashes=0");
    }
    // The GPU target runs the (uninstrumented) Vulkan driver, whose
    // allocations outlive the input and the process: LeakSanitizer blames
    // them per input (`-detect_leaks`) and at exit (`ASAN_OPTIONS`).
    let gpu = GPU_TARGETS.contains(&name);
    if gpu {
        cmd.arg("-detect_leaks=0");
    }
    // basis_universal's transcoder reaches the binaries through two crates;
    // ASan reports the duplicated globals as an ODR violation, not a bug.
    let mut asan =
        std::env::var("ASAN_OPTIONS").unwrap_or_else(|_| "detect_odr_violation=0".into());
    if gpu {
        asan.push_str(":detect_leaks=0");
    }
    cmd.env("ASAN_OPTIONS", asan);
    println!("fuzz {name}: {time} s");
    match cmd.status() {
        Ok(s) => s.success(),
        Err(e) => {
            eprintln!(
                "xtask fuzz: cargo fuzz: {e} (needs `cargo install cargo-fuzz` and a nightly toolchain)"
            );
            false
        }
    }
}

/// Copy a crash file into `fuzz/regressions/<target>/` (the target is its
/// folder's name, as libFuzzer and `fuzz smoke` write them).
fn keep(root: &Path, file: &Path) -> ExitCode {
    let known = targets(root).unwrap_or_default();
    let target = file
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .filter(|t| known.iter().any(|(n, _)| n == t));
    let Some(target) = target else {
        eprintln!(
            "xtask fuzz keep: {} is not in a target's folder (fuzz/artifacts/<target>/, \
             target/fuzz-smoke/<target>/)",
            file.display()
        );
        return ExitCode::FAILURE;
    };
    let dir: PathBuf = root.join("fuzz/regressions").join(target);
    let _ = std::fs::create_dir_all(&dir);
    let to = dir.join(file.file_name().unwrap());
    match std::fs::copy(file, &to) {
        Ok(_) => {
            println!(
                "kept {} (replayed by `cargo test -p trino-fuzz`)",
                to.display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("xtask fuzz keep: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Sources a coverage percentage is not about: dependencies, the standard
/// library and the fuzz harness itself (path fragments).
const COVERAGE_IGNORE: [&str; 7] = [
    ".cargo/registry",
    ".cargo/git",
    "/rustc/",
    "/rustlib/",
    "fuzz/fuzz_targets/",
    "crates/fuzz/",
    "/target/",
];

fn coverage_ignored(file: &str) -> bool {
    COVERAGE_IGNORE.iter().any(|p| file.contains(p))
}

/// What `llvm-cov export` says about one target.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct CoverageSummary {
    pub target: String,
    /// Code regions of the functions the corpus entered: of the code this
    /// target reaches, how much ran (the headline; whole-file numbers count
    /// every unrelated function a reached file holds).
    pub reached: (u64, u64),
    /// Engine source files with at least one line run.
    pub files: usize,
    pub lines: (u64, u64),
    pub functions: (u64, u64),
    pub regions: (u64, u64),
    /// `(file, covered lines, lines)`, most lines first.
    pub per_file: Vec<(String, u64, u64)>,
}

fn pct((covered, total): (u64, u64)) -> f64 {
    if total == 0 {
        0.0
    } else {
        covered as f64 * 100.0 / total as f64
    }
}

/// Fold `llvm-cov export` JSON into a summary over the functions and the
/// files the target reached (a file no input touched says nothing about
/// this target, and the engine has hundreds).
pub fn summarize_coverage(target: &str, json: &serde_json::Value, root: &str) -> CoverageSummary {
    let mut s = CoverageSummary {
        target: target.to_string(),
        ..Default::default()
    };
    let files = json["data"][0]["files"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let get = |f: &serde_json::Value, k: &str| {
        (
            f["summary"][k]["covered"].as_u64().unwrap_or(0),
            f["summary"][k]["count"].as_u64().unwrap_or(0),
        )
    };
    for f in &files {
        let lines = get(f, "lines");
        if lines.0 == 0 {
            continue;
        }
        let funcs = get(f, "functions");
        let regions = get(f, "regions");
        s.files += 1;
        s.lines = (s.lines.0 + lines.0, s.lines.1 + lines.1);
        s.functions = (s.functions.0 + funcs.0, s.functions.1 + funcs.1);
        s.regions = (s.regions.0 + regions.0, s.regions.1 + regions.1);
        let name = f["filename"].as_str().unwrap_or("?");
        let name = name
            .strip_prefix(root)
            .unwrap_or(name)
            .trim_start_matches('/');
        s.per_file.push((name.to_string(), lines.0, lines.1));
    }
    s.per_file.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    // Regions: [line start, col, line end, col, count, file, expanded
    // file, kind]; kind 0 is code.
    for f in json["data"][0]["functions"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let file = f["filenames"][0].as_str().unwrap_or("");
        if f["count"].as_u64().unwrap_or(0) == 0 || coverage_ignored(file) {
            continue;
        }
        for r in f["regions"].as_array().into_iter().flatten() {
            if r[7].as_u64() != Some(0) {
                continue;
            }
            s.reached.1 += 1;
            if r[4].as_u64().unwrap_or(0) > 0 {
                s.reached.0 += 1;
            }
        }
    }
    s
}

/// The nightly toolchain's `llvm-cov` (`llvm-tools-preview`).
fn llvm_cov() -> Result<PathBuf, String> {
    let rustc = |args: &[&str]| {
        Command::new("rustc")
            .arg("+nightly")
            .args(args)
            .output()
            .map_err(|e| format!("rustc +nightly: {e}"))
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let sysroot = rustc(&["--print", "sysroot"])?;
    let host = rustc(&["-vV"])?
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        .ok_or("rustc -vV: no host")?;
    let exe = if cfg!(windows) {
        "llvm-cov.exe"
    } else {
        "llvm-cov"
    };
    let p = Path::new(&sysroot)
        .join("lib/rustlib")
        .join(&host)
        .join("bin")
        .join(exe);
    if p.exists() {
        Ok(p)
    } else {
        Err(format!(
            "no {} (rustup component add llvm-tools-preview --toolchain nightly)",
            p.display()
        ))
    }
}

/// `fuzz coverage [<target>..|all] [--json]`.
fn coverage(root: &Path, args: &[&str]) -> ExitCode {
    let json = args.contains(&"--json");
    let names: Vec<&str> = args
        .iter()
        .copied()
        .filter(|a| !a.starts_with("--"))
        .collect();
    let all = match targets(root) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("xtask fuzz: {e}");
            return ExitCode::FAILURE;
        }
    };
    let chosen: Vec<String> = if names.is_empty() || names == ["all"] {
        all.iter().map(|t| t.0.clone()).collect()
    } else if let Some(bad) = names.iter().find(|n| !all.iter().any(|t| t.0 == **n)) {
        eprintln!("xtask fuzz coverage: no target `{bad}` (cargo xtask fuzz list)");
        return ExitCode::FAILURE;
    } else {
        names.iter().map(|n| n.to_string()).collect()
    };
    let cov = match llvm_cov() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("xtask fuzz coverage: {e}");
            return ExitCode::FAILURE;
        }
    };
    let fuzz = root.join("fuzz");
    let out = fuzz.join("coverage");
    let root_str = std::fs::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf())
        .display()
        .to_string();
    let mut rows = Vec::new();
    let mut failed = Vec::new();
    for name in &chosen {
        match coverage_one(&fuzz, &cov, name, &root_str) {
            Ok(s) => {
                let mut text = format!(
                    "{name}: regions of reached functions {:.1}% ({}/{}); {} files reached, lines {:.1}% ({}/{}), functions {:.1}%, regions {:.1}%\n\n",
                    pct(s.reached),
                    s.reached.0,
                    s.reached.1,
                    s.files,
                    pct(s.lines),
                    s.lines.0,
                    s.lines.1,
                    pct(s.functions),
                    pct(s.regions)
                );
                for (f, c, n) in &s.per_file {
                    text.push_str(&format!("{:>6.1}%  {c:>6}/{n:<6}  {f}\n", pct((*c, *n))));
                }
                let _ = std::fs::write(out.join(name).join("report.txt"), text);
                rows.push(s);
            }
            Err(e) => {
                eprintln!("xtask fuzz coverage: {name}: {e}");
                failed.push(name.clone());
            }
        }
    }
    let row_json = |s: &CoverageSummary| {
        serde_json::json!({
            "target": s.target,
            "reached_function_regions": {"covered": s.reached.0, "count": s.reached.1, "percent": pct(s.reached)},
            "files": s.files,
            "lines": {"covered": s.lines.0, "count": s.lines.1, "percent": pct(s.lines)},
            "functions": {"covered": s.functions.0, "count": s.functions.1, "percent": pct(s.functions)},
            "regions": {"covered": s.regions.0, "count": s.regions.1, "percent": pct(s.regions)},
        })
    };
    let summary = serde_json::json!({
        "targets": rows.iter().map(row_json).collect::<Vec<_>>(),
        "failed": failed,
    });
    let _ = std::fs::create_dir_all(&out);
    let _ = std::fs::write(
        out.join("summary.json"),
        serde_json::to_string_pretty(&summary).unwrap_or_default(),
    );
    let mut md = String::from(
        "| target | regions of reached functions | files reached | their lines | functions | regions |\n|---|---:|---:|---:|---:|---:|\n",
    );
    for s in &rows {
        md.push_str(&format!(
            "| `{}` | {:.1}% ({}/{}) | {} | {:.1}% ({}/{}) | {:.1}% | {:.1}% |\n",
            s.target,
            pct(s.reached),
            s.reached.0,
            s.reached.1,
            s.files,
            pct(s.lines),
            s.lines.0,
            s.lines.1,
            pct(s.functions),
            pct(s.regions)
        ));
    }
    let _ = std::fs::write(out.join("summary.md"), &md);
    if json {
        println!("{summary}");
    } else {
        print!("{md}");
        println!("coverage in {}", out.display());
    }
    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// One target: `cargo fuzz coverage` over its corpus and seeds, then the
/// summary from `llvm-cov export`.
fn coverage_one(
    fuzz: &Path,
    cov: &Path,
    name: &str,
    root_str: &str,
) -> Result<CoverageSummary, String> {
    let corpus = fuzz.join("corpus").join(name);
    let _ = std::fs::create_dir_all(&corpus);
    let seeds = fuzz.join("seeds").join(name);
    println!("coverage {name}");
    let mut cmd = Command::new("cargo");
    cmd.current_dir(fuzz)
        .args(["+nightly", "fuzz", "coverage", name])
        .arg(&corpus)
        .arg(&seeds)
        // The coverage run's own output goes to stderr: `--json` prints JSON only.
        .stdout(std::process::Stdio::from(std::io::stderr()));
    if std::env::var_os("ASAN_OPTIONS").is_none() {
        cmd.env("ASAN_OPTIONS", "detect_odr_violation=0");
    }
    let status = cmd
        .status()
        .map_err(|e| format!("cargo fuzz coverage: {e}"))?;
    if !status.success() {
        return Err("cargo fuzz coverage failed".into());
    }
    let profdata = fuzz.join("coverage").join(name).join("coverage.profdata");
    let host = Command::new("rustc")
        .args(["+nightly", "-vV"])
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        })
        .ok_or("rustc -vV: no host")?;
    let bin = fuzz
        .join("target")
        .join(&host)
        .join("coverage")
        .join(&host)
        .join("release")
        .join(name);
    let out = Command::new(cov)
        .args(["export", "-skip-expansions"])
        .arg(format!("-instr-profile={}", profdata.display()))
        .arg(format!(
            "-ignore-filename-regex=({})",
            COVERAGE_IGNORE.map(|p| p.replace('.', "\\.")).join("|")
        ))
        .arg(&bin)
        .output()
        .map_err(|e| format!("llvm-cov: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "llvm-cov export: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("llvm-cov JSON: {e}"))?;
    Ok(summarize_coverage(name, &v, root_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_counts_only_the_files_a_target_reached() {
        let v = serde_json::json!({"data": [{"files": [
            {"filename": "/repo/crates/core/src/a.rs", "summary": {
                "lines": {"covered": 30, "count": 40},
                "functions": {"covered": 3, "count": 4},
                "regions": {"covered": 50, "count": 100}}},
            {"filename": "/repo/crates/core/src/b.rs", "summary": {
                "lines": {"covered": 0, "count": 900},
                "functions": {"covered": 0, "count": 9},
                "regions": {"covered": 0, "count": 9}}},
            {"filename": "/repo/crates/core/src/c.rs", "summary": {
                "lines": {"covered": 10, "count": 60},
                "functions": {"covered": 1, "count": 1},
                "regions": {"covered": 0, "count": 0}}}
        ], "functions": [
            {"name": "dep", "count": 9, "filenames": ["/h/.cargo/registry/x/src/lib.rs"],
             "regions": [[1, 1, 9, 2, 0, 0, 0, 0]]},
            {"name": "ran", "count": 3, "filenames": ["/repo/crates/core/src/a.rs"], "regions": [
                [1, 1, 9, 2, 3, 0, 0, 0], [2, 5, 3, 6, 0, 0, 0, 0], [4, 1, 4, 9, 0, 0, 0, 2]]},
            {"name": "never", "count": 0, "filenames": ["/repo/crates/core/src/a.rs"],
             "regions": [[10, 1, 12, 2, 0, 0, 0, 0]]}
        ]}]});
        let s = summarize_coverage("t", &v, "/repo");
        assert_eq!(
            (s.files, s.lines, s.functions, s.regions),
            (2, (40, 100), (4, 5), (50, 100))
        );
        assert_eq!(s.per_file[0], ("crates/core/src/c.rs".to_string(), 10, 60));
        assert_eq!(pct(s.lines), 40.0);
        // Of the code regions in functions that ran (the skipped region is
        // kind 2; neither the function that never ran nor a dependency's
        // counts).
        assert_eq!(s.reached, (1, 2));
        assert_eq!(pct((0, 0)), 0.0);
    }
}
