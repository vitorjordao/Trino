//! `cargo xtask scene <check|export>` — text-first tooling for scenes,
//! prefabs and materials (the loop an AI agent or a script uses after
//! editing the RON files by hand).
//!
//! - `scene check [scene.ron ...]` parses every material/prefab and the
//!   given scenes (default: all of `scenes/`), resolves them and prints
//!   warnings (missing references, prefab cycles, stale overrides).
//!   Non-zero exit on parse errors or warnings.
//!   It also resolves every scene for each platform and flags a variant
//!   (or base model) the manifest does not bake for that platform.
//! - `scene export [--platform pc|n64|3ds] <scene.ron> <out.rs>` writes the
//!   scene as `no_std` Rust data (`trino_core::SceneNode`) for console
//!   games, with that platform's variants applied (default `pc`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use trino_scene::{Library, Scene, Target, codegen, resolve_for};

const USAGE: &str = "xtask: usage:\n  cargo xtask scene check [scene.ron ...]\n  \
                     cargo xtask scene export [--platform pc|n64|3ds] <scene.ron> <out.rs>";

pub fn run(root: &Path, args: &[&str]) -> ExitCode {
    match args {
        ["check", files @ ..] => check(root, files),
        ["export", rest @ ..] => {
            let mut target = Target::Pc;
            let mut files = Vec::new();
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                if *a == "--platform" {
                    match it.next().and_then(|v| Target::parse(v)) {
                        Some(t) => target = t,
                        None => {
                            eprintln!("xtask: --platform needs pc, n64 or 3ds\n{USAGE}");
                            return ExitCode::FAILURE;
                        }
                    }
                } else {
                    files.push(*a);
                }
            }
            match files.as_slice() {
                [input, output] => export(root, Path::new(input), Path::new(output), target),
                _ => {
                    eprintln!("{USAGE}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

/// Models a platform draws that its manifest does not bake for it (the
/// draw would silently show nothing there).
fn platform_problems(root: &Path, scene: &Scene, lib: &Library) -> Vec<String> {
    let Ok(manifest) = trino_asset_pipeline::Manifest::load(&root.join("assets/manifest.toml"))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for target in Target::ALL {
        let platform = trino_asset_pipeline::Platform::parse(target.key()).unwrap();
        let r = resolve_for(scene, lib, 0.0, target);
        for item in &r.items {
            let paths = item
                .model
                .iter()
                .map(|(_, p)| p)
                .chain(item.lod.iter().filter_map(|l| match &l.draw {
                    trino_scene::LodDraw::Model(_, p) => Some(p),
                    _ => None,
                }));
            for path in paths {
                let name = path.strip_prefix("models/").unwrap_or(path);
                if let Some(decl) = manifest.models.get(name)
                    && !decl.bakes_for(platform)
                {
                    let msg = format!(
                        "{}: draws `{path}` on {}, but the manifest bakes it only for {} \
                         (give the entity a {} variant)",
                        item.name,
                        target.label(),
                        decl.platforms.as_deref().unwrap_or_default().join(", "),
                        target.key()
                    );
                    if !out.contains(&msg) {
                        out.push(msg);
                    }
                }
            }
        }
    }
    out
}

/// Materials + prefabs from disk, plus baked model bounds (`Auto`
/// colliders resolve against them).
pub fn load_library(root: &Path) -> Library {
    let mut lib = Library::load(root);
    lib.model_bounds = trino_asset_pipeline::manifest_model_bounds(&root.join("assets"));
    lib.impostors = trino_asset_pipeline::impostor::baked(&root.join("assets"));
    lib
}

fn check(root: &Path, files: &[&str]) -> ExitCode {
    let lib = load_library(root);
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
                let r = resolve_for(&scene, &lib, 0.0, Target::Pc);
                for t in [Target::N64, Target::N3ds] {
                    // Only what the console's variants add to the PC's list.
                    for w in resolve_for(&scene, &lib, 0.0, t).warnings {
                        if !r.warnings.contains(&w) {
                            problems.push(format!("{}: [{}] {w}", path.display(), t.key()));
                        }
                    }
                }
                problems.extend(
                    r.warnings
                        .iter()
                        .map(|w| format!("{}: {w}", path.display())),
                );
                problems.extend(
                    platform_problems(root, &scene, &lib)
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

fn export(root: &Path, input: &Path, output: &Path, target: Target) -> ExitCode {
    let scene = match Scene::load(&root.join(input)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("xtask: {e}");
            return ExitCode::FAILURE;
        }
    };
    let lib = load_library(root);
    let (code, warnings) = codegen::to_rust_for(&scene, &lib, target);
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    match std::fs::write(root.join(output), code) {
        Ok(()) => {
            println!("wrote {} ({})", output.display(), target.key());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("xtask: cannot write {}: {e}", output.display());
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_flags_models_a_platform_does_not_bake() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("assets")).unwrap();
        std::fs::write(
            tmp.path().join("assets/manifest.toml"),
            "version = 1\n[models.big]\nfile = \"big.glb\"\nplatforms = [\"pc\"]\n\
             [models.small]\nfile = \"small.glb\"\n",
        )
        .unwrap();
        let mut scene = Scene::new("s");
        let mut e = trino_scene::Entity::new("statue")
            .with_model(trino_scene::ModelComponent::new("models/big"));
        let lib = Library::default();
        scene.entities.push(e.clone());
        let problems = platform_problems(tmp.path(), &scene, &lib);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains("on N64") && problems[1].contains("on 3DS"));
        assert!(problems[0].contains("give the entity a n64 variant"));

        // Variants for both consoles fix it.
        let small = trino_scene::Variant {
            model: Some("models/small".into()),
            ..Default::default()
        };
        e.platforms.n64 = Some(small.clone());
        e.platforms.n3ds = Some(small);
        scene.entities[0] = e;
        assert!(platform_problems(tmp.path(), &scene, &lib).is_empty());
    }
}
