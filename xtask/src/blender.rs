//! `cargo xtask blender castle64 [--only a,b] [--no-render]` — build
//! castle64's Blender-authored models (`tools/blender/castle64_models.py`):
//! writes the `.glb`s under `assets/shared/models/castle64/`, gates each with
//! the engine's bake against its triangle budget, QA renders in
//! `target/blender_castle64/`. Fails when Blender is absent.
//!
//! `cargo xtask blender smoke [--no-render]` — run `tools/blender/smoke_test.py`
//! headless: builds a static prop and a rigid-part character with the
//! toolkit, exports them and gates them with the engine's own bake (see the
//! `blender-asset` skill). Skips cleanly (exit 0) when Blender is absent, so
//! it is safe in any checklist.
//!
//! Blender is found via `$TRINO_BLENDER`, then `blender` on PATH, then the
//! portable installs under `~/.local/opt/blender*/`.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn find_blender() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("TRINO_BLENDER") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let exe = if cfg!(windows) {
        "blender.exe"
    } else {
        "blender"
    };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join(exe);
            if p.exists() {
                return Some(p);
            }
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let opt = home.join(".local/opt");
    let mut found: Vec<PathBuf> = std::fs::read_dir(&opt)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("blender"))
        .flat_map(|e| {
            let base = e.path();
            let mut v = vec![base.join(exe)];
            if let Ok(sub) = std::fs::read_dir(&base) {
                v.extend(sub.flatten().map(|s| s.path().join(exe)));
            }
            v
        })
        .filter(|p| p.exists())
        .collect();
    found.sort();
    found.pop()
}

pub fn run(root: &Path, args: &[&str]) -> ExitCode {
    match args {
        ["smoke", rest @ ..] => smoke(root, rest),
        ["castle64", rest @ ..] => castle64(root, rest),
        _ => {
            eprintln!(
                "xtask: usage: cargo xtask blender smoke [--no-render] | castle64 [--only a,b] [--no-render]"
            );
            ExitCode::FAILURE
        }
    }
}

fn smoke(root: &Path, rest: &[&str]) -> ExitCode {
    let Some(blender) = find_blender() else {
        println!(
            "[blender] Blender not found ($TRINO_BLENDER, PATH, ~/.local/opt/blender*) — skipped"
        );
        return ExitCode::SUCCESS;
    };
    let out = root.join("target/blender_smoke");
    println!("[blender] {} -> {}", blender.display(), out.display());
    let output = Command::new(&blender)
        .args(["--background", "--factory-startup", "--python"])
        .arg(root.join("tools/blender/smoke_test.py"))
        .arg("--")
        .arg(&out)
        .args(rest)
        .current_dir(root)
        .output();
    let output = match output {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[blender] failed to run {}: {e}", blender.display());
            return ExitCode::FAILURE;
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(line) = stdout
        .lines()
        .find_map(|l| l.strip_prefix("TRINO_BLENDER_SMOKE "))
    else {
        eprintln!(
            "[blender] no report — Blender output:\n{}\n{}",
            stdout,
            String::from_utf8_lossy(&output.stderr)
        );
        return ExitCode::FAILURE;
    };
    let report: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[blender] bad report ({e}): {line}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(gates) = report["gates"].as_object() {
        for (name, g) in gates {
            let mark = if g["ok"].as_bool() == Some(true) {
                "ok  "
            } else {
                "FAIL"
            };
            println!("[blender] {mark} {name}: {}", g["detail"]);
        }
    }
    if report["ok"].as_bool() == Some(true) {
        println!(
            "[blender] PASS (Blender {}) — renders in {}",
            report["blender"].as_str().unwrap_or("?"),
            out.display()
        );
        ExitCode::SUCCESS
    } else {
        eprintln!("[blender] FAIL: {}", report["failed"]);
        if let Some(e) = report["error"].as_str() {
            eprintln!("{e}");
        }
        ExitCode::FAILURE
    }
}

fn castle64(root: &Path, rest: &[&str]) -> ExitCode {
    let Some(blender) = find_blender() else {
        eprintln!("[blender] Blender not found ($TRINO_BLENDER, PATH, ~/.local/opt/blender*)");
        return ExitCode::FAILURE;
    };
    println!("[blender] {} building castle64 models", blender.display());
    let output = Command::new(&blender)
        .args(["--background", "--factory-startup", "--python"])
        .arg(root.join("tools/blender/castle64_models.py"))
        .arg("--")
        .args(rest)
        .current_dir(root)
        .output();
    let output = match output {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[blender] failed to run {}: {e}", blender.display());
            return ExitCode::FAILURE;
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(line) = stdout
        .lines()
        .find_map(|l| l.strip_prefix("TRINO_BLENDER_MODELS "))
    else {
        eprintln!(
            "[blender] no report — Blender output:\n{}\n{}",
            stdout,
            String::from_utf8_lossy(&output.stderr)
        );
        return ExitCode::FAILURE;
    };
    let report: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[blender] bad report ({e}): {line}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(models) = report["models"].as_object() {
        for (name, m) in models {
            let mark = if m["ok"].as_bool() == Some(true) {
                "ok  "
            } else {
                "FAIL"
            };
            match m["error"].as_str() {
                Some(e) => println!("[blender] {mark} {name}: {e}"),
                None => println!(
                    "[blender] {mark} {name}: {} tris (budget {})",
                    m["triangles"], m["budget"]
                ),
            }
        }
    }
    if report["ok"].as_bool() == Some(true) {
        println!(
            "[blender] castle64 models written (Blender {}); QA renders in {}",
            report["blender"].as_str().unwrap_or("?"),
            root.join("target/blender_castle64").display()
        );
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
