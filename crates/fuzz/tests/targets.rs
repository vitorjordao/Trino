//! The fuzz targets on the stable toolchain, on every PR (issue #68):
//! each over its committed seeds, its crash reproducers and a deterministic
//! mutation run, plus the guards that keep the target list complete.
//!
//! A failure prints the input's file under `target/fuzz-smoke/<target>/`:
//! `cargo xtask fuzz repro <file>` replays it, and the fix commits it to
//! `fuzz/regressions/<target>/` so this test replays it from then on.

use std::collections::BTreeSet;

use trino_fuzz::{TARGETS, mutate};

/// Mutations per target (`TRINO_FUZZ_ITERS` raises it locally).
fn iters() -> usize {
    std::env::var("TRINO_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(400)
}

#[test]
fn every_target_survives_its_seeds_regressions_and_mutations() {
    std::panic::set_hook(Box::new(|_| {}));
    let out = trino_fuzz::repo_root().join("target/fuzz-smoke");
    let mut failures = Vec::new();
    for t in TARGETS {
        let mut inputs = trino_fuzz::seeds(t.name);
        assert!(
            !inputs.is_empty(),
            "{}: no committed seeds (cargo xtask fuzz corpus --commit-seeds)",
            t.name
        );
        for (path, bytes) in trino_fuzz::regressions(t.name) {
            if let Err(e) = trino_fuzz::run_caught(t, &bytes) {
                failures.push(format!(
                    "{}: regression {} panics again: {e}",
                    t.name,
                    path.display()
                ));
            }
            inputs.push(bytes);
        }
        inputs.push(Vec::new());
        let mut rng = mutate::Rng::new(trino_core::asset_id(t.name) as u64);
        let mut all = inputs.clone();
        for k in 0..trino_fuzz::iters_for(t.name, iters()) {
            let a = &inputs[k % inputs.len()];
            let b = &inputs[rng.below(inputs.len())];
            all.push(mutate::mutate(&mut rng, a, b, t.max_len));
        }
        for input in all {
            if let Err(e) = trino_fuzz::run_caught(t, &input) {
                let dir = out.join(t.name);
                let _ = std::fs::create_dir_all(&dir);
                let file = dir.join(format!("crash-{:08x}", trino_core::save::checksum(&input)));
                let _ = std::fs::write(&file, &input);
                failures.push(format!(
                    "{}: {} ({} bytes): {e}",
                    t.name,
                    file.display(),
                    input.len()
                ));
                break;
            }
        }
    }
    let _ = std::panic::take_hook();
    assert!(
        failures.is_empty(),
        "fuzz smoke failures:\n{}",
        failures.join("\n")
    );
}

/// Every four-letter magic the engine parses has a target that names it:
/// a new format without one fails here (the `engine-feature` checklist).
#[test]
fn every_binary_format_has_a_target() {
    let root = trino_fuzz::repo_root();
    let covered: BTreeSet<&str> = TARGETS
        .iter()
        .flat_map(|t| t.formats.iter().copied())
        .collect();
    let mut magics = BTreeSet::new();
    let mut files = Vec::new();
    for d in ["crates/core/src", "crates/platform-web/src"] {
        collect_rs(&root.join(d), &mut files);
    }
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        for line in text.lines() {
            let l = line.trim();
            // `pub const X_MAGIC: &[u8; 4] = b"ABCD";` / `MAGIC: [u8; 4] = *b"ABCD"`.
            if l.starts_with("pub const")
                && l.contains("MAGIC")
                && l.contains("[u8; 4]")
                && let Some(i) = l.find("b\"")
            {
                magics.insert(l[i + 2..i + 6].to_string());
            }
        }
    }
    assert!(magics.len() >= 18, "found only {magics:?}");
    let missing: Vec<&String> = magics
        .iter()
        .filter(|m| !covered.contains(m.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "formats without a fuzz target (crates/fuzz/src/lib.rs TARGETS): {missing:?}"
    );
}

fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_rs(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// `fuzz/` (cargo-fuzz) has one binary per target and nothing else.
#[test]
fn cargo_fuzz_has_a_binary_per_target() {
    let root = trino_fuzz::repo_root();
    let manifest = std::fs::read_to_string(root.join("fuzz/Cargo.toml")).unwrap();
    let bins: BTreeSet<String> = manifest
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("name = \"")
                .and_then(|r| r.strip_suffix('"'))
        })
        .filter(|n| *n != "trino-fuzz-targets")
        .map(String::from)
        .collect();
    let names: BTreeSet<String> = TARGETS.iter().map(|t| t.name.to_string()).collect();
    assert_eq!(
        bins, names,
        "fuzz/Cargo.toml [[bin]]s vs trino_fuzz::TARGETS"
    );
    for t in TARGETS {
        let src = std::fs::read_to_string(root.join(format!("fuzz/fuzz_targets/{}.rs", t.name)))
            .unwrap_or_else(|_| panic!("fuzz/fuzz_targets/{}.rs", t.name));
        assert!(
            src.contains(&format!("\"{}\"", t.name)),
            "{} runs another target",
            t.name
        );
    }
}
