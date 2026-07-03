//! Enforces the engine's inviolable dependency direction:
//!
//! - `trino-core` has ZERO dependencies (dev-dependencies allowed).
//! - `trino-game-api` depends only on `trino-core`.
//! - `trino-platform-*` may use external crates (wgpu, libdragon FFI, ...)
//!   but among workspace crates may depend only on `trino-core`.
//! - Game crates (`trino-game*`, anything under `examples/`) depend only on
//!   `trino-core` and `trino-game-api` — no external crates, so games stay
//!   portable to all three consoles by construction.
//!
//! Runs as part of `cargo test --workspace`; CI fails if anyone bends the
//! architecture.

use std::collections::BTreeSet;
use std::process::Command;

fn workspace_metadata() -> serde_json::Value {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .expect("failed to run cargo metadata");
    assert!(
        out.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("cargo metadata produced invalid JSON")
}

/// Names of normal (non-dev, non-build) dependencies of a package.
fn normal_deps(pkg: &serde_json::Value) -> BTreeSet<String> {
    pkg["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["kind"].is_null()) // null = normal dependency
        .map(|d| d["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn dependency_direction_is_enforced() {
    let meta = workspace_metadata();
    let packages = meta["packages"].as_array().unwrap();
    let mut violations = Vec::new();

    for pkg in packages {
        let name = pkg["name"].as_str().unwrap();
        let manifest = pkg["manifest_path"].as_str().unwrap().replace('\\', "/");
        let deps = normal_deps(pkg);

        let check = |allowed: &[&str], rule: &str| -> Option<String> {
            let bad: Vec<&String> = deps
                .iter()
                .filter(|d| !allowed.contains(&d.as_str()))
                .collect();
            (!bad.is_empty()).then(|| format!("{name}: {rule}; forbidden deps: {bad:?}"))
        };

        let violation = if name == "trino-core" {
            check(&[], "trino-core must have zero dependencies")
        } else if name == "trino-game-api" {
            check(
                &["trino-core"],
                "trino-game-api may depend only on trino-core",
            )
        } else if name.starts_with("trino-platform-") {
            // External crates allowed; workspace crates other than core are not.
            let workspace_names: BTreeSet<&str> = packages
                .iter()
                .map(|p| p["name"].as_str().unwrap())
                .collect();
            let bad: Vec<&String> = deps
                .iter()
                .filter(|d| workspace_names.contains(d.as_str()) && d.as_str() != "trino-core")
                .collect();
            (!bad.is_empty()).then(|| {
                format!("{name}: platform crates may depend only on trino-core within the workspace; found: {bad:?}")
            })
        } else if name.starts_with("trino-game") || manifest.contains("/examples/") {
            check(
                &["trino-core", "trino-game-api"],
                "game crates may depend only on trino-core and trino-game-api",
            )
        } else {
            None // apps/*, xtask, editor, asset-pipeline: unconstrained gluers
        };

        if let Some(v) = violation {
            violations.push(v);
        }
    }

    assert!(
        violations.is_empty(),
        "dependency direction violated:\n  {}",
        violations.join("\n  ")
    );
}
