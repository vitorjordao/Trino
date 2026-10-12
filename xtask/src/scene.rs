//! `cargo xtask scene <check|export>` — text-first tooling for scenes,
//! prefabs and materials (the loop an AI agent or a script uses after
//! editing the RON files by hand).
//!
//! - `scene check [scene.ron ...]` parses every material/prefab and the
//!   given scenes (default: all of `scenes/`), resolves them and prints
//!   warnings (missing references, prefab cycles, stale overrides).
//!   Non-zero exit on parse errors or warnings.
//! - `scene export <scene.ron> <out.rs>` writes the scene as `no_std` Rust
//!   data (`trino_core::SceneNode`) for console games.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use trino_scene::{Library, Scene, codegen, resolve};

pub fn run(root: &Path, args: &[&str]) -> ExitCode {
    match args {
        ["check", files @ ..] => check(root, files),
        ["export", input, output] => export(root, Path::new(input), Path::new(output)),
        _ => {
            eprintln!(
                "xtask: usage:\n  cargo xtask scene check [scene.ron ...]\n  \
                 cargo xtask scene export <scene.ron> <out.rs>"
            );
            ExitCode::FAILURE
        }
    }
}

fn check(root: &Path, files: &[&str]) -> ExitCode {
    let lib = Library::load(root);
    let mut problems = lib.errors.clone();
    let scenes: Vec<PathBuf> = if files.is_empty() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(root.join("scenes"))
            .map(|d| {
                d.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.to_string_lossy().ends_with(".scene.ron"))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    } else {
        files.iter().map(|f| root.join(f)).collect()
    };
    for path in &scenes {
        match Scene::load(path) {
            Ok(scene) => {
                let r = resolve(&scene, &lib, 0.0);
                problems.extend(
                    r.warnings
                        .into_iter()
                        .map(|w| format!("{}: {w}", path.display())),
                );
                println!("ok  {} ({} draw items)", path.display(), r.items.len());
            }
            Err(e) => problems.push(e),
        }
    }
    println!(
        "{} material(s), {} prefab(s), {} scene(s)",
        lib.materials.len(),
        lib.prefabs.len(),
        scenes.len()
    );
    if problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        for p in &problems {
            eprintln!("error: {p}");
        }
        ExitCode::FAILURE
    }
}

fn export(root: &Path, input: &Path, output: &Path) -> ExitCode {
    let scene = match Scene::load(&root.join(input)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("xtask: {e}");
            return ExitCode::FAILURE;
        }
    };
    let lib = Library::load(root);
    let (code, warnings) = codegen::to_rust(&scene, &lib);
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    match std::fs::write(root.join(output), code) {
        Ok(()) => {
            println!("wrote {}", output.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("xtask: cannot write {}: {e}", output.display());
            ExitCode::FAILURE
        }
    }
}
