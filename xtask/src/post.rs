//! The post stack's tools (issue #64, ADR 0033), text-first like the rest:
//!
//! - `lut neutral [--size N] [--out x.png] [--json]`: the identity LUT as a
//!   strip PNG (`n²×n`), the file an artist grades in any image tool
//!   (default 32, `assets/shared/luts/neutral.png`).
//! - `lut inspect <file|luts/name> [--json]`: its size, how far it moves
//!   colors (the largest change against the identity) and a few samples.
//! - `post check [dir] [--json]` (`dir`: another project folder): every `[luts.*]` master bakes; every LUT a
//!   model, sprite or scene names is declared; a scene with `bake_grading`
//!   names a grading; the models a graded scene draws are graded by the
//!   same LUT (else the consoles' mood differs from the PC's). Non-zero
//!   exit on errors (mismatches are warnings).
//! - `scene shot <scene.ron> [--platform pc|n64|3ds|web] [--post game|on|off|bloom]
//!   [--size WxH] [--out x.png] [--json]`: the scene rendered headless as
//!   that target shows it, its post stack as the flag says (`game`: the
//!   scene's), its glows drawn, the consoles' assets and environment
//!   graded as their bake grades them: before/after images.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::json;
use trino_asset_pipeline::{Manifest, Platform};
use trino_core::{Color, Family, Lut, LutId, Material, ModelId, Renderer};
use trino_render_wgpu::{PcRenderer, PostView};

const LUT_USAGE: &str = "xtask: usage:\n  \
    cargo xtask lut neutral [--size 2..64] [--out <file.png>] [--json]\n  \
    cargo xtask lut inspect <file.png|file.cube|luts/name> [--json]";
const POST_USAGE: &str = "xtask: usage:\n  cargo xtask post check [dir] [--json]";
pub const SHOT_USAGE: &str = "xtask: usage:\n  \
    cargo xtask scene shot <scene.ron> [--platform pc|n64|3ds|web] \
    [--post game|on|off|bloom] [--size WxH] [--out <file.png>] [--json]";

fn finish(result: Result<bool, String>, json: bool) -> ExitCode {
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            if json {
                println!("{}", json!({ "error": e }));
            } else {
                eprintln!("{e}");
            }
            ExitCode::FAILURE
        }
    }
}

pub fn run_lut(root: &Path, args: &[&str]) -> ExitCode {
    let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let json = crate::web::take_flag(&mut args, "--json");
    let result = (|| {
        let size = crate::web::take_value(&mut args, "--size")?;
        let out = crate::web::take_value(&mut args, "--out")?;
        let rest: Vec<&str> = args.iter().map(String::as_str).collect();
        match rest.as_slice() {
            ["neutral"] => neutral(root, size.as_deref(), out.as_deref(), json),
            ["inspect", what] => inspect(root, what, json),
            _ => Err(LUT_USAGE.to_string()),
        }
    })();
    finish(result, json)
}

fn neutral(root: &Path, size: Option<&str>, out: Option<&str>, json: bool) -> Result<bool, String> {
    let size: u8 = match size {
        None => 32,
        Some(s) => s
            .parse()
            .ok()
            .filter(|n| (trino_core::post::LUT_MIN..=trino_core::post::LUT_MAX).contains(n))
            .ok_or_else(|| format!("--size {s}: 2..=64 texels per axis"))?,
    };
    let blob = trino_asset_pipeline::lut::neutral(size);
    let lut = Lut::parse(&blob).map_err(|e| format!("{e:?}"))?;
    let (w, h, rgba) = trino_asset_pipeline::lut::to_strip(&lut);
    let out = out.map_or_else(
        || root.join("assets/shared/luts/neutral.png"),
        |o| root.join(o),
    );
    crate::ui_render::write_png(&out, w, h, &rgba)?;
    if json {
        println!(
            "{}",
            json!({ "out": out.display().to_string(), "size": size, "width": w, "height": h })
        );
    } else {
        println!(
            "wrote {} ({w}x{h}: {size} slices of blue, red along x, green down y)",
            out.display()
        );
    }
    Ok(true)
}

fn manifest(root: &Path) -> Result<Manifest, String> {
    Manifest::load(&root.join("assets/manifest.toml"))
}

/// A LUT blob from a master file or a manifest name.
fn lut_bytes(root: &Path, what: &str) -> Result<Vec<u8>, String> {
    if what.starts_with("luts/") && !Path::new(what).exists() {
        let m = manifest(root)?;
        return m.lut_blob(&root.join("assets"), Platform::Pc, what);
    }
    let p = PathBuf::from(what);
    let p = if p.is_absolute() || p.exists() {
        p
    } else {
        root.join(p)
    };
    trino_asset_pipeline::lut::bake(&p)
}

fn inspect(root: &Path, what: &str, json: bool) -> Result<bool, String> {
    let blob = lut_bytes(root, what)?;
    let lut = Lut::parse(&blob).map_err(|e| format!("{what}: {e:?}"))?;
    // How far it moves colors: the largest change over a 17³ grid.
    let mut moved = 0u8;
    for b in 0..17u32 {
        for g in 0..17u32 {
            for r in 0..17u32 {
                let c = Color::rgb(
                    (r * 255 / 16) as u8,
                    (g * 255 / 16) as u8,
                    (b * 255 / 16) as u8,
                );
                let o = lut.apply_color(c);
                moved = moved
                    .max(o.r.abs_diff(c.r))
                    .max(o.g.abs_diff(c.g))
                    .max(o.b.abs_diff(c.b));
            }
        }
    }
    let samples: Vec<([u8; 3], [u8; 3])> = [
        [0u8, 0, 0],
        [255, 255, 255],
        [128, 128, 128],
        [200, 60, 20],
        [30, 120, 220],
        [90, 200, 120],
    ]
    .into_iter()
    .map(|c| {
        let o = lut.apply_color(Color::rgb(c[0], c[1], c[2]));
        (c, [o.r, o.g, o.b])
    })
    .collect();
    if json {
        println!(
            "{}",
            json!({
                "lut": what,
                "size": lut.size(),
                "bytes": blob.len(),
                "identity": moved <= 1,
                "max_change": moved,
                "samples": samples.iter().map(|(a, b)| json!({ "in": a, "out": b })).collect::<Vec<_>>(),
            })
        );
    } else {
        println!(
            "{what}: {}³ texels, {} bytes as TLUT, largest change {moved}/255{}",
            lut.size(),
            blob.len(),
            if moved <= 1 { " (the identity)" } else { "" }
        );
        for (a, b) in samples {
            println!("  {a:?} -> {b:?}");
        }
    }
    Ok(true)
}

pub fn run_post(root: &Path, args: &[&str]) -> ExitCode {
    let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let json = crate::web::take_flag(&mut args, "--json");
    let rest: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match rest.as_slice() {
        ["check"] => check(root, json),
        ["check", dir] => check(&root.join(dir), json),
        _ => Err(POST_USAGE.to_string()),
    };
    finish(result, json)
}

/// One `post check` finding.
struct Finding {
    error: bool,
    what: String,
    message: String,
}

fn check(root: &Path, json: bool) -> Result<bool, String> {
    let m = manifest(root)?;
    let assets = root.join("assets");
    let mut found: Vec<Finding> = Vec::new();
    let mut add = |error: bool, what: String, message: String| {
        found.push(Finding {
            error,
            what,
            message,
        })
    };
    let mut luts = Vec::new();
    for name in m.luts.keys() {
        let logical = format!("luts/{name}");
        for platform in [Platform::Pc, Platform::Web, Platform::N64, Platform::N3ds] {
            match m.lut_blob(&assets, platform, &logical) {
                Ok(b) => {
                    if platform == Platform::Pc {
                        let size = Lut::parse(&b).map(|l| l.size()).unwrap_or(0);
                        luts.push(json!({ "lut": logical, "size": size, "bytes": b.len() }));
                        if size < 16 {
                            add(
                                false,
                                logical.clone(),
                                format!(
                                    "{size}³ texels: grades with visible steps below 16 \
                                     (32 is the usual)"
                                ),
                            );
                        }
                    }
                }
                Err(e) => add(true, logical.clone(), format!("{}: {e}", platform.key())),
            }
        }
    }
    // Scenes: their grading is declared, `bake_grading` has one, and the
    // models they draw are graded the same for the consoles.
    let lib = trino_scene::Library::load(root);
    let declared = |l: &str| {
        l.strip_prefix("luts/")
            .is_some_and(|n| m.luts.contains_key(n))
    };
    let mut scenes: Vec<(String, PathBuf, bool)> = m
        .scenes
        .iter()
        .map(|(n, d)| (format!("scenes/{n}"), root.join(&d.file), d.bake_grading))
        .collect();
    if let Ok(dir) = std::fs::read_dir(root.join("scenes")) {
        for e in dir.flatten() {
            let p = e.path();
            let listed = scenes.iter().any(|(_, f, _)| f == &p);
            if !listed && p.to_string_lossy().ends_with(".scene.ron") {
                scenes.push((p.display().to_string(), p, false));
            }
        }
    }
    for (what, file, bake) in &scenes {
        let scene = match trino_scene::Scene::load(file) {
            Ok(s) => s,
            Err(e) => {
                add(true, what.clone(), e);
                continue;
            }
        };
        let grading = scene
            .environment
            .post
            .as_ref()
            .and_then(|p| p.grading.clone());
        if let Some(g) = &grading
            && !declared(g)
        {
            add(
                true,
                what.clone(),
                format!("post.grading `{g}` is not a LUT of the manifest"),
            );
        }
        if *bake && grading.is_none() {
            add(
                true,
                what.clone(),
                "bake_grading needs `environment.post.grading`".into(),
            );
        }
        if let (true, Some(g)) = (*bake, &grading) {
            let r = trino_scene::resolve_for(&scene, &lib, 0.0, trino_scene::Target::N64);
            let mut models: Vec<String> = r
                .items
                .iter()
                .filter_map(|i| i.model.as_ref().map(|(_, l)| l.clone()))
                .collect();
            models.sort();
            models.dedup();
            for logical in models {
                let decl = logical
                    .strip_prefix("models/")
                    .and_then(|n| m.models.get(n));
                let theirs = decl.and_then(|d| d.grading.as_deref());
                if theirs != Some(g.as_str()) {
                    add(
                        false,
                        what.clone(),
                        format!(
                            "draws `{logical}`, graded {} on the consoles, not `{g}`: its \
                             colors keep another mood there",
                            theirs.map_or("by no LUT".to_string(), |t| format!("by `{t}`"))
                        ),
                    );
                }
            }
        }
    }
    let errors = found.iter().filter(|f| f.error).count();
    let warnings = found.len() - errors;
    if json {
        println!(
            "{}",
            json!({
                "ok": errors == 0,
                "luts": luts,
                "errors": errors,
                "warnings": warnings,
                "findings": found.iter().map(|f| json!({
                    "level": if f.error { "error" } else { "warning" },
                    "asset": f.what,
                    "message": f.message,
                })).collect::<Vec<_>>(),
            })
        );
    } else {
        for f in &found {
            let level = if f.error { "error" } else { "warning" };
            println!("{level}: {}: {}", f.what, f.message);
        }
        println!(
            "post check: {} LUT(s), {} scene(s): {errors} error(s), {warnings} warning(s)",
            m.luts.len(),
            scenes.len()
        );
    }
    Ok(errors == 0)
}

fn family_of(s: &str) -> Result<Family, String> {
    match s {
        "pc" => Ok(Family::Pc),
        "web" => Ok(Family::Web),
        "n64" => Ok(Family::N64),
        "3ds" | "n3ds" => Ok(Family::N3ds),
        other => Err(format!("--platform {other}: pc, web, n64 or 3ds")),
    }
}

/// `scene shot`.
pub fn scene_shot_cmd(root: &Path, args: &[&str]) -> ExitCode {
    let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let json = crate::web::take_flag(&mut args, "--json");
    let result = (|| {
        let family = match crate::web::take_value(&mut args, "--platform")? {
            Some(p) => family_of(&p)?,
            None => Family::Pc,
        };
        let view = match crate::web::take_value(&mut args, "--post")? {
            None => PostView::Game,
            Some(v) => {
                PostView::parse(&v).ok_or_else(|| format!("--post {v}: game, on, off or bloom"))?
            }
        };
        let size = match crate::web::take_value(&mut args, "--size")? {
            None => None,
            Some(s) => Some(
                s.split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                    .ok_or_else(|| format!("--size {s}: WxH"))?,
            ),
        };
        let out = crate::web::take_value(&mut args, "--out")?;
        let [file] = args.as_slice() else {
            return Err(SHOT_USAGE.to_string());
        };
        let path = root.join(file);
        let (w, h, rgba, stats) = shot(root, &path, family, view, size)?;
        let stem = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.trim_end_matches(".scene.ron").to_string())
            .unwrap_or_else(|| "scene".into());
        let out = out.map_or_else(
            || {
                root.join("target/post").join(format!(
                    "{stem}-{}-post-{}.png",
                    family.key(),
                    view.key()
                ))
            },
            |o| root.join(o),
        );
        crate::ui_render::write_png(&out, w, h, &rgba)?;
        if json {
            println!(
                "{}",
                json!({
                    "scene": file,
                    "platform": family.key(),
                    "post": view.key(),
                    "out": out.display().to_string(),
                    "width": w,
                    "height": h,
                    "post_passes": stats.post_passes,
                    "bloom_levels": stats.bloom_levels,
                    "glows": stats.glows,
                    "graded_models": stats.graded_models,
                    "graded_environment": stats.graded_env,
                })
            );
        } else {
            println!(
                "wrote {} ({w}x{h}; post passes {}, bloom levels {}, glows {}, graded models {}{})",
                out.display(),
                stats.post_passes,
                stats.bloom_levels,
                stats.glows,
                stats.graded_models,
                if stats.graded_env {
                    ", graded environment"
                } else {
                    ""
                }
            );
        }
        Ok(true)
    })();
    finish(result, json)
}

/// What a shot drew.
#[derive(Default)]
pub struct ShotStats {
    pub post_passes: u32,
    pub bloom_levels: u32,
    pub glows: u32,
    pub graded_models: u32,
    pub graded_env: bool,
}

/// `file` rendered headless as `family` shows it, with the post stack as
/// `view` says. The console families draw their bake's graded models and,
/// for a scene the manifest bakes with `bake_grading`, its graded
/// environment.
pub fn shot(
    root: &Path,
    file: &Path,
    family: Family,
    view: PostView,
    size: Option<(u32, u32)>,
) -> Result<(u32, u32, Vec<u8>, ShotStats), String> {
    let platform = crate::ui_render::pipeline_platform(family);
    let baked = root
        .join("target/assets")
        .join(format!("post-{}", family.key()));
    trino_asset_pipeline::bake_all_with(
        &root.join("assets"),
        platform,
        &baked,
        crate::threads::build(),
    )
    .map_err(|e| e.to_string())?;
    let assets = trino_asset_pipeline::load_dir(&baked, None)?;
    let mut r = pollster::block_on(PcRenderer::new_headless(crate::ui_render::profile(family)))
        .map_err(|e| format!("no GPU renderer: {e}"))?;
    if matches!(family, Family::Pc | Family::Web) {
        let (w, h) = size.unwrap_or((1280, 720));
        r.set_internal_size(w, h);
    }
    r.set_post_view(view);
    let m = manifest(root)?;
    let mut stats = ShotStats::default();
    for s in assets.sprites {
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for model in assets.models {
        let graded = m
            .models
            .get(model.logical.trim_start_matches("models/"))
            .is_some_and(|d| d.grading.is_some());
        stats.graded_models += (graded && matches!(family, Family::N64 | Family::N3ds)) as u32;
        r.upload_mesh(ModelId(model.id), model.tmdl);
    }
    for b in assets.blobs {
        r.take_lut_blob(b.id, &b.bytes);
    }
    let mut scene = trino_scene::Scene::load(file)?;
    // The consoles' bake grades a `bake_grading` scene's colors.
    if matches!(family, Family::N64 | Family::N3ds)
        && let Some((_, decl)) = m
            .scenes
            .iter()
            .find(|(_, d)| root.join(&d.file) == file || Path::new(&d.file) == file)
        && let Some(lut) =
            trino_asset_pipeline::model::scene_grading(&root.join("assets"), &m, decl, platform)?
    {
        let lut = Lut::parse(&lut).map_err(|e| format!("{e:?}"))?;
        scene.environment = scene.environment.graded(&lut);
        stats.graded_env = true;
    }
    let mut lib = trino_scene::Library::load(root);
    lib.model_bounds = trino_asset_pipeline::manifest_model_bounds(&root.join("assets"));
    let target = match family {
        Family::N64 => trino_scene::Target::N64,
        Family::N3ds => trino_scene::Target::N3ds,
        Family::Pc => trino_scene::Target::Pc,
        Family::Web => trino_scene::Target::Web,
    };
    let resolved = trino_scene::resolve_for(&scene, &lib, 0.0, target);
    let env = &scene.environment;
    let camera = env.core_camera();
    let missing: Vec<LutId> = resolved
        .post
        .and_then(|p| p.grading)
        .filter(|l| matches!(family, Family::Pc | Family::Web) && !r.has_lut(*l))
        .into_iter()
        .collect();
    if let Some(l) = missing.first() {
        return Err(format!(
            "{}: its grading LUT ({:#010x}) is not in the manifest",
            file.display(),
            l.0
        ));
    }
    for _ in 0..3 {
        let c = env.clear;
        r.begin_frame(Color::rgb(c[0], c[1], c[2]));
        r.set_camera(&camera);
        r.set_light(&env.core_light());
        r.set_ambient_occlusion(env.core_ao().as_ref());
        r.set_shadows(resolved.shadows.as_ref());
        r.set_shadow_receivers(&resolved.receivers());
        r.set_post(resolved.post.as_ref());
        stats.glows = 0;
        for item in &resolved.items {
            if let Some(id) = item.model_from(camera.eye) {
                r.draw_model_matrix(id, &item.matrix, Material::VertexLit, &item.params);
            }
            if let Some(g) = &item.glow {
                let p = item.matrix.transform_point(trino_core::Vec3::ZERO);
                stats.glows += trino_core::draw_glow(&mut r, g, p) as u32;
            }
        }
        r.end_frame();
    }
    let t = r.frame_times();
    stats.post_passes = t.post_passes;
    stats.bloom_levels = t.bloom_levels;
    let (w, h) = r.internal_size();
    Ok((w, h, r.read_offscreen(), stats))
}
