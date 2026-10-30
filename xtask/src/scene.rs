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
//!   games, with that platform's variants applied (default `pc`; a hardware
//!   tier, `n64_expanded` / `n3ds_new`, applies its own over its console's).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use trino_scene::{Library, Scene, Target, codegen, resolve_for};

const USAGE: &str = "xtask: usage:\n  cargo xtask scene check [scene.ron ...]\n  \
                     cargo xtask scene export [--platform pc|n64|3ds|web|n64_expanded|n3ds_new] \
                     [--bake-grading] <scene.ron> <out.rs>\n  \
                     cargo xtask scene shot <scene.ron> [--platform pc|n64|3ds|web] \
                     [--post game|on|off|bloom] [--size WxH] [--out <file.png>] [--json]\n  \
                     cargo xtask scene budget <scene.ron> \
                     [--platform pc|n64|3ds|web|n64_expanded|n3ds_new] [--stereo] [--json]";

/// `--platform`'s values, for its error.
const PLATFORMS: &str = "pc, n64, 3ds, web, n64_expanded or n3ds_new";

pub fn run(root: &Path, args: &[&str]) -> ExitCode {
    match args {
        ["check", files @ ..] => check(root, files),
        ["budget", rest @ ..] => budget(root, rest),
        ["shot", rest @ ..] => crate::post::scene_shot_cmd(root, rest),
        ["export", rest @ ..] => {
            let mut target = Target::Pc;
            let mut files = Vec::new();
            let mut grade = false;
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                if *a == "--bake-grading" {
                    grade = true;
                } else if *a == "--platform" {
                    match it.next().and_then(|v| Target::parse(v)) {
                        Some(t) => target = t,
                        None => {
                            eprintln!("xtask: --platform needs {PLATFORMS}\n{USAGE}");
                            return ExitCode::FAILURE;
                        }
                    }
                } else {
                    files.push(*a);
                }
            }
            match files.as_slice() {
                [input, output] => export(root, Path::new(input), Path::new(output), target, grade),
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
    // (target, entity, model) already reported: a tier drawing what its
    // console draws is that console's problem, reported once.
    let mut missing: Vec<(Target, String, String)> = Vec::new();
    // The hardware tiers too: an `n64_expanded` variant may draw a model
    // only that tier has (issue #54).
    for target in Target::EVERY {
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
                    && !decl.bakes_for(target)
                {
                    let key = (item.name.to_string(), path.to_string());
                    if target.base() != target
                        && missing.contains(&(target.base(), key.0.clone(), key.1.clone()))
                    {
                        continue;
                    }
                    missing.push((target, key.0, key.1));
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
                for t in [
                    Target::N64,
                    Target::N3ds,
                    Target::N64Expanded,
                    Target::N3dsNew,
                ] {
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
                problems.extend(
                    trino_scene::anim::scene_problems(root, &r.items)
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

fn export(root: &Path, input: &Path, output: &Path, target: Target, grade: bool) -> ExitCode {
    let scene = match Scene::load(&root.join(input)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("xtask: {e}");
            return ExitCode::FAILURE;
        }
    };
    let lib = load_library(root);
    // `--bake-grading` (issue #64): a console's export grades the scene's
    // colors through its `post.grading`, as its TSCN bake does.
    let lut = match grade.then(|| export_grading(root, &scene, target)) {
        Some(Err(e)) => {
            eprintln!("xtask: {e}");
            return ExitCode::FAILURE;
        }
        Some(Ok(l)) => l,
        None => None,
    };
    let parsed = match lut.as_deref().map(trino_core::Lut::parse) {
        Some(Err(e)) => {
            eprintln!("xtask: grading LUT: {e:?}");
            return ExitCode::FAILURE;
        }
        Some(Ok(l)) => Some(l),
        None => None,
    };
    let (code, warnings) = codegen::to_rust_graded(&scene, &lib, target, parsed.as_ref());
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

/// The LUT a console export grades `scene` through (`None` on the PC and
/// the Web, which grade at run time).
fn export_grading(root: &Path, scene: &Scene, target: Target) -> Result<Option<Vec<u8>>, String> {
    let platform = match target {
        Target::N64 | Target::N64Expanded => trino_asset_pipeline::Platform::N64,
        Target::N3ds | Target::N3dsNew => trino_asset_pipeline::Platform::N3ds,
        Target::Pc | Target::Web => return Ok(None),
    };
    let grading = scene
        .environment
        .post
        .as_ref()
        .and_then(|p| p.grading.clone())
        .ok_or("--bake-grading needs `environment.post.grading` in the scene")?;
    let manifest = trino_asset_pipeline::Manifest::load(&root.join("assets/manifest.toml"))?;
    manifest
        .lut_blob(&root.join("assets"), platform, &grading)
        .map(Some)
}

/// `scene budget <scene.ron> [--platform p] [--json]`: what the scene costs
/// each platform from its camera (`trino_scene::budget`, the editor's
/// Game-view HUD): draws, T&L work, the N64's RSP/CPU split and why, the
/// impostors, the LOD picks and the draws sent for nothing. `--platform
/// n64_expanded|n3ds_new` estimates a console's expanded tier (its variants,
/// its RAM and cores; issue #54).
fn budget(root: &Path, args: &[&str]) -> ExitCode {
    let mut targets = Target::ALL.to_vec();
    let mut json = false;
    let mut stereo = false;
    let mut file = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match *a {
            "--json" => json = true,
            "--stereo" => stereo = true,
            "--platform" => match it.next().and_then(|v| Target::parse(v)) {
                Some(t) => targets = vec![t],
                None => {
                    eprintln!("xtask: --platform needs {PLATFORMS}\n{USAGE}");
                    return ExitCode::FAILURE;
                }
            },
            other if file.is_none() && !other.starts_with("--") => file = Some(other),
            other => {
                eprintln!("xtask: unexpected `{other}`\n{USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }
    let Some(file) = file else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let scene = match Scene::load(&root.join(file)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("xtask: {e}");
            return ExitCode::FAILURE;
        }
    };
    let lib = load_library(root);
    let assets = root.join("assets");
    let manifest = match trino_asset_pipeline::Manifest::load(&assets.join("manifest.toml")) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("xtask: {e}");
            return ExitCode::FAILURE;
        }
    };
    let camera = scene.environment.core_camera();
    let light = scene.environment.core_light();
    let tmem = n64_tmem(&assets, &manifest);
    let tmem = |id: trino_core::SpriteId| tmem.get(&id.0).copied().flatten();
    let mut rows = Vec::new();
    for target in targets {
        let platform = trino_asset_pipeline::Platform::of(target);
        // Each platform's own models (console overrides included; a tier
        // has its console's and its own).
        let meshes: std::collections::HashMap<u32, Vec<u8>> = manifest
            .models
            .iter()
            .filter(|(_, d)| d.bakes_for(target))
            .filter_map(|(name, d)| {
                let src = trino_asset_pipeline::resolve_source(&assets, platform, &d.file).ok()?;
                let blob = trino_asset_pipeline::bake_model(&src, &d.options()).ok()?;
                Some((trino_core::asset_id(&format!("models/{name}")), blob))
            })
            .collect();
        let resolved = resolve_for(&scene, &lib, 0.0, target);
        let screen = trino_scene::budget::screen_of(target, trino_core::Vec2::new(640.0, 480.0));
        let mut b = trino_scene::budget::estimate(
            &resolved,
            &|id| meshes.get(&id.0).map(Vec::as_slice),
            &camera,
            &light,
            screen,
            target,
        );
        // `--stereo` asks for the stereo frame even where the scene sets
        // none (the default separation and convergence).
        if stereo && b.stereo.is_none() {
            let def = trino_scene::StereoDef::default().core();
            b.stereo = trino_scene::budget::stereo_of(&b, Some(&def));
        }
        rows.push(b);
    }
    if json {
        let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        let items: Vec<String> = rows
            .iter()
            .map(|b| {
                let reasons: Vec<String> = b
                    .off_rsp
                    .iter()
                    .map(|(r, n)| format!("{{\"reason\":{},\"draws\":{n}}}", q(r.label())))
                    .collect();
                let gpu_reasons: Vec<String> = b
                    .off_gpu
                    .iter()
                    .map(|(r, n)| format!("{{\"reason\":{},\"draws\":{n}}}", q(r.label())))
                    .collect();
                let pc_reasons: Vec<String> = b
                    .off_pc_gpu
                    .iter()
                    .map(|(r, n)| format!("{{\"reason\":{},\"draws\":{n}}}", q(r.label())))
                    .collect();
                let wasted: Vec<String> = b.wasted.iter().map(|w| q(w)).collect();
                let paths: Vec<String> = b
                    .paths
                    .iter()
                    .map(|p| {
                        let reason = match p.path {
                            trino_scene::budget::RenderPath::Cpu(r) => q(r),
                            _ => "null".into(),
                        };
                        format!(
                            "{{\"name\":{},\"path\":\"{}\",\"reason\":{reason}}}",
                            q(&p.name),
                            p.path.key(b.target)
                        )
                    })
                    .collect();
                let water: Vec<String> = b
                    .water
                    .iter()
                    .map(|w| {
                        let bytes: Option<u32> = [w.texture, w.detail]
                            .into_iter()
                            .flatten()
                            .map(tmem)
                            .sum();
                        format!(
                            "{{\"name\":{},\"tris\":{},\"cycles\":{},\"tmem_bytes\":{}}}",
                            q(&w.name),
                            w.tris,
                            w.cycles,
                            bytes.map_or("null".into(), |v| v.to_string())
                        )
                    })
                    .collect();
                let caps = trino_scene::budget::caps_of(b.target);
                let ram = if caps.ram_bytes == u32::MAX {
                    "null".to_string()
                } else {
                    caps.ram_bytes.to_string()
                };
                let stereo = match b.stereo {
                    Some(s) => format!(
                        "{{\"eyes\":{},\"tris\":{},\"gpu_vertex_passes\":{},\"cpu_draws\":{},\"cpu_verts\":{},\"separation\":{},\"convergence\":{}}}",
                        s.eyes,
                        s.tris,
                        s.gpu_vertex_passes,
                        s.cpu_draws,
                        s.cpu_verts,
                        s.separation,
                        s.convergence
                    ),
                    None => "null".into(),
                };
                // High-contrast outlines (issue #96): the draws they add.
                let o = &b.outlines;
                let outlines = format!(
                    "{{\"draws\":{},\"rsp\":{},\"tris\":{}}}",
                    o.draws, o.rsp, o.tris
                );
                let s = &b.shadows;
                let shadows = format!(
                    "{{\"tier\":\"{}\",\"casters\":{},\"blobs\":{},\"planar\":{},\"mapped\":{},\"over_budget\":{},\"receivers\":{}}}",
                    s.tier.key(),
                    s.casters,
                    s.blobs,
                    s.planar,
                    s.mapped,
                    s.over_budget,
                    s.receivers
                );
                format!(
                    "{{\"platform\":\"{}\",\"tier\":\"{}\",\"ram_bytes\":{ram},\"model_bytes\":{},\"cpu_cores\":{},\"cpu_mhz\":{},\"crowd_scale\":{},\"particle_scale\":{},\"draws\":{},\"tris\":{},\"verts\":{},\"culled\":{},\"billboards\":{},\"lod\":{:?},\"rsp_draws\":{},\"rsp_tris\":{},\"cpu_draws\":{},\"off_rsp\":[{}],\"gpu_draws\":{},\"gpu_tris\":{},\"off_gpu\":[{}],\"pc_gpu_draws\":{},\"pc_gpu_tris\":{},\"off_pc_gpu\":[{}],\"wasted\":[{}],\"water\":[{}],\"paths\":[{}],\"web_calls\":{},\"web_pipelines\":{},\"web_switches\":{},\"web_warnings\":[{}],\"outlines\":{outlines},\"shadows\":{shadows},\"stereo\":{stereo},\"n64_cpu_us\":{},\"n64_rsp_us\":{},\"n64_frame_us\":{}}}",
                    b.target.key(),
                    b.target.hw_tier().key(),
                    b.model_bytes,
                    caps.cpu_cores,
                    caps.cpu_mhz,
                    caps.crowd_scale(),
                    caps.particle_scale(),
                    b.draws,
                    b.tris(),
                    b.tnl.verts,
                    b.tnl.culled,
                    b.billboards,
                    b.lod,
                    b.rsp_draws,
                    b.rsp_tris,
                    b.cpu_draws,
                    reasons.join(","),
                    b.gpu_draws,
                    b.gpu_tris,
                    gpu_reasons.join(","),
                    b.pc_gpu_draws,
                    b.pc_gpu_tris,
                    pc_reasons.join(","),
                    wasted.join(","),
                    water.join(","),
                    paths.join(","),
                    b.web.calls,
                    b.web.pipelines,
                    b.web.switches,
                    trino_scene::budget::web_warnings(b)
                        .iter()
                        .map(|w| q(w))
                        .collect::<Vec<_>>()
                        .join(","),
                    b.n64_cpu_us(),
                    b.n64_rsp_us(),
                    b.n64_frame_us()
                )
            })
            .collect();
        println!("[{}]", items.join(","));
        return ExitCode::SUCCESS;
    }
    println!("{file}: from the scene camera");
    // "3 fog (pond, river, ...)": a reason with the entities it holds back.
    let reason = |b: &trino_scene::budget::FrameBudget, label: &str, n: u32| {
        let names = b.off_path(label);
        let shown: Vec<&str> = names.iter().copied().take(4).collect();
        let more = if names.len() > 4 { ", ..." } else { "" };
        format!("{n} {label} ({}{more})", shown.join(", "))
    };
    for b in &rows {
        println!(
            "  {:4} {} draws, {} tris, {} billboards, LOD full/light/imp/cull {:?}",
            b.target.key(),
            b.draws,
            b.tris(),
            b.billboards,
            b.lod
        );
        // The tier's hardware and the models' share of its RAM (#54).
        if let Some(line) = trino_scene::budget::hardware_line(b.target) {
            println!("       {line}");
        }
        if let Some(line) = trino_scene::budget::ram_line(b) {
            println!("       {line}");
        }
        if let Some(line) = trino_scene::budget::shadow_line(b) {
            println!("       {line}");
        }
        if let Some(line) = trino_scene::budget::stereo_line(b) {
            println!("       {line}");
        }
        if let Some(line) = trino_scene::budget::outline_line(b) {
            println!("       {line}");
        }
        if b.target.base() == Target::N64 {
            for line in trino_scene::budget::water_palette_lines(b) {
                println!("       {line}");
            }
        }
        if b.target.base() == Target::N64 {
            println!(
                "       N64 ≈{:.1} ms ({:.0}% of 30 fps): CPU {:.1} ms on {} draws, RSP {:.1} ms on {} draws",
                b.n64_frame_us() as f32 / 1000.0,
                b.n64_frame_percent(),
                b.n64_cpu_us() as f32 / 1000.0,
                b.cpu_draws,
                b.n64_rsp_us() as f32 / 1000.0,
                b.rsp_draws
            );
            if !b.off_rsp.is_empty() {
                let r: Vec<String> = b
                    .off_rsp
                    .iter()
                    .map(|(r, n)| reason(b, r.label(), *n))
                    .collect();
                println!("       off the RSP fast path: {}", r.join(", "));
            }
            for line in trino_scene::budget::water_lines(b, &tmem) {
                println!("       {line}");
            }
        }
        if b.target == Target::Pc {
            println!("       {}", trino_scene::budget::pc_gpu_line(b));
            if !b.off_pc_gpu.is_empty() {
                let r: Vec<String> = b
                    .off_pc_gpu
                    .iter()
                    .map(|(r, n)| reason(b, r.label(), *n))
                    .collect();
                println!("       off the PC GPU path: {}", r.join(", "));
            }
        }
        if b.target == Target::Web {
            println!(
                "       {}",
                trino_scene::budget::pc_gpu_line(b).replacen("PC", "Web", 1)
            );
            println!("       {}", trino_scene::budget::web_line(b));
            for w in trino_scene::budget::web_warnings(b) {
                println!("       warning: {w}");
            }
        }
        if b.target.base() == Target::N3ds {
            println!(
                "       3DS: GPU T&L on {} draws ({} tris), CPU on {} draws",
                b.gpu_draws, b.gpu_tris, b.cpu_draws
            );
            if !b.off_gpu.is_empty() {
                let r: Vec<String> = b
                    .off_gpu
                    .iter()
                    .map(|(r, n)| reason(b, r.label(), *n))
                    .collect();
                println!("       off the GPU path: {}", r.join(", "));
            }
        }
        if !b.wasted.is_empty() {
            println!(
                "       sent but unseen ({}): {}",
                b.wasted.len(),
                b.wasted.join(", ")
            );
        }
    }
    ExitCode::SUCCESS
}

/// Each sprite's N64 TMEM bytes (by handle), from its master's size and
/// its manifest `n64` format (`None`: a palette or an unknown format).
fn n64_tmem(
    assets: &Path,
    manifest: &trino_asset_pipeline::Manifest,
) -> std::collections::HashMap<u32, Option<u32>> {
    manifest
        .sprites
        .iter()
        .filter_map(|(name, d)| {
            let src = trino_asset_pipeline::resolve_source(
                assets,
                trino_asset_pipeline::Platform::N64,
                &d.file,
            )
            .ok()?;
            let file = std::fs::File::open(src).ok()?;
            let info = png::Decoder::new(std::io::BufReader::new(file))
                .read_info()
                .ok()?;
            let (w, h) = (info.info().width, info.info().height);
            let format = d.formats.get("n64").map_or("RGBA5551", String::as_str);
            Some((
                trino_core::asset_id(&format!("sprites/{name}")),
                trino_scene::budget::tmem_bytes(w, h, format),
            ))
        })
        .collect()
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
        scene.entities[0] = e.clone();
        assert!(platform_problems(tmp.path(), &scene, &lib).is_empty());

        // A tier's own variant is checked on that tier.
        e.platforms.n64_expanded = Some(trino_scene::Variant {
            model: Some("models/big".into()),
            ..Default::default()
        });
        scene.entities[0] = e;
        let problems = platform_problems(tmp.path(), &scene, &lib);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("on N64+Pak"));
        assert!(problems[0].contains("give the entity a n64_expanded variant"));
    }
}
