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
//! - `fuzz repro [<target>] <file> [--json]`: replay one input;
//! - `fuzz keep <file>`: copy a crash into `fuzz/regressions/<target>/`, which
//!   `cargo test -p trino-fuzz` replays on every PR.
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
        Some(cmd @ ("smoke" | "repro" | "corpus")) => {
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
        Some(other) => {
            eprintln!(
                "xtask fuzz: unknown command `{other}` (list, corpus, smoke, run, repro, keep)"
            );
            ExitCode::FAILURE
        }
    }
}

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
    // basis_universal's transcoder reaches the binaries through two crates;
    // ASan reports the duplicated globals as an ODR violation, not a bug.
    if std::env::var_os("ASAN_OPTIONS").is_none() {
        cmd.env("ASAN_OPTIONS", "detect_odr_violation=0");
    }
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
