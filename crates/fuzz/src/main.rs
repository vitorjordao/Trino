//! `trino-fuzz`: what `cargo xtask fuzz` runs on the stable toolchain.
//!
//! - `list [--json]`: every target, its formats and its corpus size;
//! - `smoke [<target>..] [--iters N] [--seed S] [--json]`: the deterministic
//!   mutation run of the tests, failures saved to `target/fuzz-smoke/`;
//! - `repro <target> <file>` / `repro <file>`: run one input (a libFuzzer
//!   crash file, a regression) and report the panic, if any;
//! - `corpus [--out <dir>] [--commit-seeds]`: bake castle64's, the
//!   showcase's and Solid Eleven's assets for the PC and the Web, sort every
//!   blob (and the consoles' staged files, when built) into one folder per
//!   target, add the writers' seeds and the repo's text files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use trino_fuzz::{Seeds, TARGETS, Target, mutate};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json = args.iter().any(|a| a == "--json");
    let args: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--json")
        .collect();
    match args.first().copied() {
        Some("list") | None => list(json),
        Some("smoke") => smoke(&args[1..], json),
        Some("repro") => repro(&args[1..], json),
        Some("corpus") => corpus(&args[1..], json),
        Some(other) => {
            eprintln!("trino-fuzz: unknown command `{other}` (list, smoke, repro, corpus)");
            ExitCode::FAILURE
        }
    }
}

fn corpus_dir() -> PathBuf {
    trino_fuzz::repo_root().join("fuzz/corpus")
}

fn list(json: bool) -> ExitCode {
    if json {
        let rows: Vec<String> = TARGETS
            .iter()
            .map(|t| {
                format!(
                    "{{\"name\":\"{}\",\"formats\":[{}],\"max_len\":{},\"seeds\":{},\"regressions\":{},\"corpus\":{}}}",
                    t.name,
                    t.formats.iter().map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(","),
                    t.max_len,
                    trino_fuzz::seeds(t.name).len(),
                    trino_fuzz::regressions(t.name).len(),
                    trino_fuzz::files_in(&corpus_dir().join(t.name)).len()
                )
            })
            .collect();
        println!("{{\"targets\":[{}]}}", rows.join(","));
    } else {
        println!(
            "{:<18} {:>6} {:>6} {:>7}  formats",
            "target", "seeds", "regr", "corpus"
        );
        for t in TARGETS {
            println!(
                "{:<18} {:>6} {:>6} {:>7}  {}",
                t.name,
                trino_fuzz::seeds(t.name).len(),
                trino_fuzz::regressions(t.name).len(),
                trino_fuzz::files_in(&corpus_dir().join(t.name)).len(),
                t.formats.join(", ")
            );
        }
    }
    ExitCode::SUCCESS
}

/// Silence the default panic message (the run reports each failure once).
fn quiet_panics() {
    std::panic::set_hook(Box::new(|_| {}));
}

/// The input running now, for the watchdog.
static CURRENT: std::sync::Mutex<(Vec<u8>, Option<std::time::Instant>, &str)> =
    std::sync::Mutex::new((Vec::new(), None, ""));

/// Longest an input may run (libFuzzer's `-timeout` in the fuzz builds).
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// A thread that saves the running input and exits when it overruns
/// [`TIMEOUT`]: a hang is a failure like a panic (a console would freeze).
fn watchdog() {
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let c = CURRENT.lock().unwrap();
            if let Some(start) = c.1
                && start.elapsed() > TIMEOUT
            {
                let dir = trino_fuzz::repo_root().join("target/fuzz-smoke").join(c.2);
                let _ = std::fs::create_dir_all(&dir);
                let p = dir.join(format!("timeout-{:08x}", trino_core::save::checksum(&c.0)));
                let _ = std::fs::write(&p, &c.0);
                eprintln!(
                    "{}: an input ran over {} s: {}",
                    c.2,
                    TIMEOUT.as_secs(),
                    p.display()
                );
                std::process::exit(2);
            }
        }
    });
}

fn timed(t: &Target, input: &[u8]) -> Result<(), String> {
    *CURRENT.lock().unwrap() = (input.to_vec(), Some(std::time::Instant::now()), t.name);
    let r = trino_fuzz::run_caught(t, input);
    CURRENT.lock().unwrap().1 = None;
    r
}

/// The smoke run of one target: `(inputs run, failures as (input, message))`.
pub fn smoke_target(t: &Target, iters: usize, seed: u64) -> (usize, Vec<(Vec<u8>, String)>) {
    let mut inputs: Vec<Vec<u8>> = trino_fuzz::seeds(t.name);
    inputs.extend(
        trino_fuzz::files_in(&corpus_dir().join(t.name))
            .iter()
            .take(64)
            .filter_map(|p| std::fs::read(p).ok()),
    );
    inputs.extend(trino_fuzz::regressions(t.name).into_iter().map(|(_, b)| b));
    inputs.push(Vec::new());
    let mut fails = Vec::new();
    let mut run = 0;
    for i in &inputs {
        run += 1;
        if let Err(e) = timed(t, i) {
            fails.push((i.clone(), e));
        }
    }
    let mut rng = mutate::Rng::new(seed ^ trino_core::asset_id(t.name) as u64);
    for k in 0..iters {
        let a = &inputs[k % inputs.len()];
        let b = &inputs[rng.below(inputs.len())];
        let m = mutate::mutate(&mut rng, a, b, t.max_len);
        run += 1;
        if let Err(e) = timed(t, &m) {
            fails.push((m, e));
            if fails.len() >= 8 {
                break;
            }
        }
    }
    (run, fails)
}

fn smoke(args: &[&str], json: bool) -> ExitCode {
    let mut iters = 2000usize;
    let mut seed = 1u64;
    let mut names = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--iters" => {
                iters = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(iters);
                i += 1;
            }
            "--seed" => {
                seed = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(seed);
                i += 1;
            }
            n => names.push(n),
        }
        i += 1;
    }
    let targets: Vec<&Target> = if names.is_empty() {
        TARGETS.iter().collect()
    } else {
        match names
            .iter()
            .map(|n| trino_fuzz::find(n).ok_or(*n))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(t) => t,
            Err(n) => {
                eprintln!("trino-fuzz: no target `{n}` (trino-fuzz list)");
                return ExitCode::FAILURE;
            }
        }
    };
    quiet_panics();
    watchdog();
    let out = trino_fuzz::repo_root().join("target/fuzz-smoke");
    let mut rows = Vec::new();
    let mut failed = false;
    for t in targets {
        let start = std::time::Instant::now();
        let (run, fails) = smoke_target(t, iters, seed);
        let ms = start.elapsed().as_millis();
        let mut saved = Vec::new();
        for (input, msg) in &fails {
            let dir = out.join(t.name);
            let _ = std::fs::create_dir_all(&dir);
            let p = dir.join(format!("crash-{:08x}", trino_core::save::checksum(input)));
            let _ = std::fs::write(&p, input);
            saved.push((p, msg.clone()));
        }
        failed |= !fails.is_empty();
        if json {
            rows.push(format!(
                "{{\"target\":\"{}\",\"inputs\":{run},\"ms\":{ms},\"failures\":[{}]}}",
                t.name,
                saved
                    .iter()
                    .map(|(p, m)| format!(
                        "{{\"file\":{:?},\"panic\":{:?}}}",
                        p.display().to_string(),
                        m
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        } else {
            println!(
                "{:<18} {:>7} inputs {:>6} ms  {}",
                t.name,
                run,
                ms,
                if fails.is_empty() { "ok" } else { "FAILED" }
            );
            for (p, m) in &saved {
                println!("    {} — {m}", p.display());
            }
        }
    }
    if json {
        println!("{{\"ok\":{},\"targets\":[{}]}}", !failed, rows.join(","));
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// The target a crash file belongs to: `fuzz/artifacts/<t>/…`,
/// `fuzz/regressions/<t>/…`, `fuzz/corpus/<t>/…` or `target/fuzz-smoke/<t>/…`.
fn target_of(file: &Path) -> Option<&'static Target> {
    let parent = file.parent()?.file_name()?.to_str()?;
    trino_fuzz::find(parent)
}

fn repro(args: &[&str], json: bool) -> ExitCode {
    let (t, file) = match args {
        [file] => match target_of(Path::new(file)) {
            Some(t) => (t, *file),
            None => {
                eprintln!(
                    "trino-fuzz: name the target (`repro <target> <file>`): `{file}` is not in a target's folder"
                );
                return ExitCode::FAILURE;
            }
        },
        [name, file] => match trino_fuzz::find(name) {
            Some(t) => (t, *file),
            None => {
                eprintln!("trino-fuzz: no target `{name}`");
                return ExitCode::FAILURE;
            }
        },
        _ => {
            eprintln!("usage: trino-fuzz repro [<target>] <file>");
            return ExitCode::FAILURE;
        }
    };
    let Ok(data) = std::fs::read(file) else {
        eprintln!("trino-fuzz: cannot read {file}");
        return ExitCode::FAILURE;
    };
    let r = trino_fuzz::run_caught(t, &data);
    if json {
        match &r {
            Ok(()) => println!(
                "{{\"target\":\"{}\",\"file\":{file:?},\"bytes\":{},\"ok\":true}}",
                t.name,
                data.len()
            ),
            Err(m) => println!(
                "{{\"target\":\"{}\",\"file\":{file:?},\"bytes\":{},\"ok\":false,\"panic\":{m:?}}}",
                t.name,
                data.len()
            ),
        }
    } else {
        match &r {
            Ok(()) => println!("{}: {} bytes, no panic", t.name, data.len()),
            Err(m) => println!("{}: {} bytes, PANIC: {m}", t.name, data.len()),
        }
    }
    if r.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Files under `dir`, recursively (skipping `target/` and hidden folders).
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if name.starts_with('.')
                || name == "target"
                || name == "node_modules"
                || name == "corpus"
            {
                continue;
            }
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn corpus(args: &[&str], json: bool) -> ExitCode {
    let root = trino_fuzz::repo_root();
    let out = match args.iter().position(|a| *a == "--out") {
        Some(i) => PathBuf::from(args.get(i + 1).copied().unwrap_or("fuzz/corpus")),
        None => corpus_dir(),
    };
    let commit = args.contains(&"--commit-seeds");
    let bake = root.join("target/fuzz-bake");
    // 1. The engine's and Solid Eleven's assets, baked for the PC and the Web.
    let mut sources: Vec<PathBuf> = Vec::new();
    for (name, assets) in [
        ("engine", root.join("assets")),
        ("solid-eleven", root.join("examples/solid-eleven/assets")),
    ] {
        for platform in [
            trino_asset_pipeline::Platform::Pc,
            trino_asset_pipeline::Platform::Web,
        ] {
            let dir = bake.join(name).join(platform.key());
            match trino_asset_pipeline::bake_all_with(&assets, platform, &dir, 4) {
                Ok(r) => eprintln!(
                    "baked {} {} asset(s) for {}",
                    r.entries.len(),
                    name,
                    platform.key()
                ),
                Err(e) => eprintln!("warning: {name} {}: {e}", platform.key()),
            }
            sources.push(dir);
        }
    }
    // 2. Whatever the consoles' builds staged, and the Web packs.
    for d in [
        "target/n64/filesystem",
        "target/n64/stage",
        "target/3ds/romfs",
        "target/3ds/stage",
        "target/assets",
        "target/web",
    ] {
        sources.push(root.join(d));
    }
    let mut blobs = Vec::new();
    for s in &sources {
        walk(s, &mut blobs);
    }
    // 3. The authored text and recordings.
    let mut authored = Vec::new();
    for d in ["scenes", "prefabs", "ui", "assets", "examples", "templates"] {
        walk(&root.join(d), &mut authored);
    }
    let mut by_target: BTreeMap<&'static str, Vec<Vec<u8>>> = BTreeMap::new();
    for p in &blobs {
        let Ok(bytes) = std::fs::read(p) else {
            continue;
        };
        if bytes.len() < 4 {
            continue;
        }
        for t in TARGETS {
            if let Seeds::Magic(ms) = t.seeds
                && ms.iter().any(|m| bytes[..4] == **m)
                && bytes.len() <= t.max_len
            {
                by_target.entry(t.name).or_default().push(bytes.clone());
            }
            if let Seeds::Files(exts) = t.seeds {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if exts.iter().any(|e| name.ends_with(&format!(".{e}"))) && bytes.len() <= t.max_len
                {
                    by_target.entry(t.name).or_default().push(bytes.clone());
                }
            }
        }
    }
    // The PC loader's target: a kind byte ([`host_targets::pc_asset`]'s
    // order) and a baked file.
    for p in &blobs {
        let Ok(bytes) = std::fs::read(p) else {
            continue;
        };
        let kind = match bytes.get(..4) {
            Some(b"TSPR") => 0u8,
            Some(b"TSN2") => 1,
            Some(b"TMIX") => 3,
            Some(b"TMDL") => 4,
            Some(b"TFNT") => 5,
            _ => continue,
        };
        if bytes.len() < (1 << 16) {
            let mut v = vec![kind];
            v.extend_from_slice(&bytes);
            by_target.entry("pc_asset").or_default().push(v);
        }
    }
    for p in &authored {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let Ok(bytes) = std::fs::read(p) else {
            continue;
        };
        for t in TARGETS {
            if let Seeds::Files(exts) = t.seeds
                && exts.iter().any(|e| name.ends_with(&format!(".{e}")))
                && bytes.len() <= t.max_len
            {
                by_target.entry(t.name).or_default().push(bytes.clone());
            }
        }
    }
    for (t, name, bytes) in trino_fuzz::seedgen::generated()
        .into_iter()
        .chain(trino_fuzz::seedgen::generated_host())
    {
        let _ = name;
        by_target.entry(t).or_default().push(bytes);
    }
    let mut rows = Vec::new();
    for t in TARGETS {
        let mut v = by_target.remove(t.name).unwrap_or_default();
        v.sort();
        v.dedup();
        let dir = out.join(t.name);
        let _ = std::fs::create_dir_all(&dir);
        for b in &v {
            let _ = std::fs::write(
                dir.join(format!("{:08x}", trino_core::save::checksum(b))),
                b,
            );
        }
        if commit {
            // The smallest few, so a fresh clone's tests start from real data.
            let seeds = root.join("fuzz/seeds").join(t.name);
            let _ = std::fs::remove_dir_all(&seeds);
            let _ = std::fs::create_dir_all(&seeds);
            let mut small: Vec<&Vec<u8>> = v.iter().filter(|b| b.len() <= 24 * 1024).collect();
            small.sort_by_key(|b| (b.len(), trino_core::save::checksum(b)));
            let pick: Vec<&Vec<u8>> = spread(&small, 4);
            for b in pick {
                let _ = std::fs::write(
                    seeds.join(format!("{:08x}", trino_core::save::checksum(b))),
                    b,
                );
            }
        }
        rows.push((t.name, v.len()));
    }
    if json {
        let r: Vec<String> = rows
            .iter()
            .map(|(n, c)| format!("{{\"target\":\"{n}\",\"inputs\":{c}}}"))
            .collect();
        println!(
            "{{\"out\":{:?},\"targets\":[{}]}}",
            out.display().to_string(),
            r.join(",")
        );
    } else {
        for (n, c) in &rows {
            println!("{n:<18} {c:>5} input(s)");
        }
        println!("corpus in {}", out.display());
    }
    ExitCode::SUCCESS
}

/// Up to `n` items spread over a sorted list (smallest, largest, between).
fn spread<'a>(v: &[&'a Vec<u8>], n: usize) -> Vec<&'a Vec<u8>> {
    if v.len() <= n {
        return v.to_vec();
    }
    (0..n).map(|i| v[i * (v.len() - 1) / (n - 1)]).collect()
}
