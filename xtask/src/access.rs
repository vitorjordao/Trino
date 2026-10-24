//! `cargo xtask access <check|shot>` (issue #60, ADR 0027) — accessibility
//! as a text-first loop, for people, agents and CI:
//!
//! - `access check [dir] [--platform p] [--json]`: the project's semantic
//!   palettes stay apart and readable in every color-vision mode
//!   (`assets/palettes.ron`), its high-contrast theme keeps 7:1, every
//!   caption key is in every language and every `important` sound has
//!   one, and every UI screen fits at 200 % text on each target and in each
//!   language. Non-zero exit on any finding. `dir` checks an example with
//!   its own assets (`examples/solid-eleven`).
//! - `access shot <screen|scene.ron> [--cvd mode] [--view mode] [--text N]
//!   [--contrast] [--platform p] [--size WxH] [--lang l] [--out x.png]
//!   [--json]`: renders a UI screen (`ui/<name>.ui.ron`) or a scene
//!   headless as a player with those settings gets it — `--cvd` turns the
//!   player's mode on (icons) and shows the picture as that viewer sees it
//!   (`--view` overrides the simulation, `--view off` shows it as drawn).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::json;
use trino_core::ui::UiAccess;
use trino_core::{Color, CvdMode, Family, Material, ModelId, Renderer};
use trino_render_wgpu::PcRenderer;

use crate::project::Project;
use crate::ui::{families, family, manifest, root_of, screen_path};
use crate::ui_render::{self, ShotOptions};

const USAGE: &str = "xtask: usage:\n  \
    cargo xtask access check [dir] [--platform n64|3ds|pc|web] [--json]\n  \
    cargo xtask access shot <screen|scene.ron> [--cvd protanopia|deuteranopia|tritanopia|grayscale] \
    [--view <mode>|off] [--text 100..200] [--contrast] [--platform n64|3ds|pc|web] [--size WxH] \
    [--lang <lang>] [--out <file.png>] [--json]";

pub fn run(project: &Project, args: &[&str]) -> ExitCode {
    let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let json = crate::web::take_flag(&mut args, "--json");
    let result = Opts::take(&mut args).and_then(|opts| {
        let rest: Vec<&str> = args.iter().map(String::as_str).collect();
        match rest.as_slice() {
            ["check"] => check(project, &project.root, &opts, json),
            ["check", dir] => check(project, &project.root.join(dir), &opts, json),
            ["shot", what] => shot(project, what, &opts, json),
            _ => Err(USAGE.to_string()),
        }
    });
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

#[derive(Default)]
struct Opts {
    platform: Option<Family>,
    size: Option<(u32, u32)>,
    lang: Option<String>,
    cvd: Option<CvdMode>,
    view: Option<CvdMode>,
    text: Option<u8>,
    contrast: bool,
    out: Option<PathBuf>,
}

impl Opts {
    fn take(args: &mut Vec<String>) -> Result<Opts, String> {
        let take = |args: &mut Vec<String>, f: &str| crate::web::take_value(args, f);
        let mode = |v: Option<String>, flag: &str| -> Result<Option<CvdMode>, String> {
            v.map(|m| {
                CvdMode::parse(&m).ok_or(format!(
                    "{flag} {m}: off, protanopia, deuteranopia, tritanopia or grayscale"
                ))
            })
            .transpose()
        };
        let platform = match take(args, "--platform")? {
            Some(p) => Some(family(&p).ok_or(format!("--platform {p}: n64, 3ds, pc or web"))?),
            None => None,
        };
        let size = match take(args, "--size")? {
            Some(s) => {
                let (w, h) = s.split_once('x').ok_or("--size needs WxH (1920x1080)")?;
                let parse = |v: &str| v.parse::<u32>().ok().filter(|v| (16..=8192).contains(v));
                Some(
                    parse(w)
                        .zip(parse(h))
                        .ok_or("--size needs WxH between 16 and 8192")?,
                )
            }
            None => None,
        };
        let text = match take(args, "--text")? {
            Some(t) => Some(
                t.trim_end_matches('%')
                    .parse::<u8>()
                    .ok()
                    .filter(|v| (100..=200).contains(v))
                    .ok_or(format!("--text {t}: a percentage from 100 to 200"))?,
            ),
            None => None,
        };
        Ok(Opts {
            platform,
            size,
            lang: take(args, "--lang")?,
            cvd: mode(take(args, "--cvd")?, "--cvd")?,
            view: mode(take(args, "--view")?, "--view")?,
            text,
            contrast: crate::web::take_flag(args, "--contrast"),
            out: take(args, "--out")?.map(PathBuf::from),
        })
    }

    /// What the player's settings change in a UI screen.
    fn ui_access(&self, contrast: Option<trino_core::ui::Theme>) -> UiAccess {
        UiAccess {
            text_scale: self.text.unwrap_or(100) as f32 / 100.0,
            high_contrast: self.contrast,
            contrast,
            cvd: self.cvd.unwrap_or(CvdMode::Off),
        }
    }

    /// The simulation view: `--view`, else `--cvd`'s mode.
    fn simulate(&self) -> Option<CvdMode> {
        self.view.or(self.cvd).filter(|m| *m != CvdMode::Off)
    }
}

fn check(project: &Project, root: &Path, opts: &Opts, json: bool) -> Result<bool, String> {
    let manifest = manifest(root)?;
    let fams = match opts.platform {
        Some(f) => vec![f],
        None if root == project.root => families(project),
        None => vec![Family::N64, Family::N3ds, Family::Pc, Family::Web],
    };
    let report = trino_asset_pipeline::access::check_project(root, &manifest, &fams)?;
    let ok = report.findings.is_empty();
    if json {
        let mut v = serde_json::to_value(&report).map_err(|e| e.to_string())?;
        v["errors"] = json!(report.errors());
        v["warnings"] = json!(report.warnings());
        v["platforms"] = json!(fams.iter().map(|f| f.key()).collect::<Vec<_>>());
        println!(
            "{}",
            serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?
        );
    } else {
        for line in &report.checklist {
            println!(
                "{} [{}] {}",
                if line.passed { "ok  " } else { "FAIL" },
                line.area,
                line.check
            );
        }
        for f in &report.findings {
            println!("  {f}");
        }
        println!(
            "access check: {} colors ({} groups, {} pairs), {} captions ({} important), \
             {} icons, {} theme, {} screens at 200 % on {}: {} errors, {} warnings",
            report.colors,
            report.distinct_groups,
            report.contrast_pairs,
            report.captions,
            report.important,
            report.icons,
            if report.theme { "own" } else { "engine" },
            report.screens,
            fams.iter().map(|f| f.key()).collect::<Vec<_>>().join("/"),
            report.errors(),
            report.warnings()
        );
    }
    Ok(ok)
}

fn shot(project: &Project, what: &str, opts: &Opts, json: bool) -> Result<bool, String> {
    let family = opts.platform.unwrap_or(Family::Pc);
    let is_scene = what.ends_with(".scene.ron");
    let (root, file) = if is_scene {
        let p = PathBuf::from(what);
        let p = if p.is_absolute() || p.is_file() {
            p
        } else {
            project.root.join(p)
        };
        (project.root.clone(), p)
    } else {
        let file = screen_path(&project.root, what);
        (root_of(&project.root, &file), file)
    };
    let (w, h, rgba, stem) = if is_scene {
        let (w, h, rgba) = scene_shot(&root, &file, family, opts)?;
        let stem = file
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.trim_end_matches(".scene.ron").to_string())
            .unwrap_or_else(|| "scene".into());
        (w, h, rgba, stem)
    } else {
        let manifest = manifest(&root)?;
        let langs = trino_asset_pipeline::ui::langs(&root)?;
        let def = trino_scene::ui::ScreenDef::load(&file)?;
        let contrast = trino_scene::access::load_theme(&root)?
            .map(|t| trino_scene::access::contrast_theme(&t));
        let shot = ui_render::render(
            &root,
            &manifest,
            &langs,
            &def,
            &ShotOptions {
                family,
                size: opts.size,
                lang: opts.lang.as_deref(),
                interface_scale: 1.0,
                focus: None,
                access: opts.ui_access(contrast),
                simulate: opts.simulate(),
                stereo: None,
            },
        )?;
        (shot.width, shot.height, shot.rgba, def.name.clone())
    };
    let tag = |o: Option<CvdMode>| o.map(|m| format!("-{}", m.key())).unwrap_or_default();
    let out = opts.out.clone().unwrap_or_else(|| {
        let mut name = format!("{stem}-{}{}", family.key(), tag(opts.cvd));
        if let Some(t) = opts.text {
            name += &format!("-text{t}");
        }
        if opts.contrast {
            name += "-contrast";
        }
        root.join("target/access").join(format!("{name}.png"))
    });
    ui_render::write_png(&out, w, h, &rgba)?;
    if json {
        println!(
            "{}",
            json!({
                "file": what,
                "platform": family.key(),
                "cvd": opts.cvd.map(CvdMode::key),
                "view": opts.simulate().map(CvdMode::key),
                "text_scale": opts.text.unwrap_or(100),
                "high_contrast": opts.contrast,
                "out": out.display().to_string(),
                "width": w,
                "height": h,
            })
        );
    } else {
        println!("wrote {} ({w}x{h})", out.display());
    }
    Ok(true)
}

/// A scene rendered headless as `family` shows it, through the simulation
/// view: the models of its resolved draw list at its camera and light.
fn scene_shot(
    root: &Path,
    file: &Path,
    family: Family,
    opts: &Opts,
) -> Result<(u32, u32, Vec<u8>), String> {
    let platform = ui_render::pipeline_platform(family);
    let baked = root
        .join("target/assets")
        .join(format!("access-{}", family.key()));
    trino_asset_pipeline::bake_all_with(
        &root.join("assets"),
        platform,
        &baked,
        crate::threads::build(),
    )
    .map_err(|e| e.to_string())?;
    let assets = trino_asset_pipeline::load_dir(&baked, None)?;
    let mut r = pollster::block_on(PcRenderer::new_headless(ui_render::profile(family)))
        .map_err(|e| format!("no GPU renderer: {e}"))?;
    if matches!(family, Family::Pc | Family::Web) {
        let (w, h) = opts.size.unwrap_or((1280, 720));
        r.set_internal_size(w, h);
    }
    for s in assets.sprites {
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for m in assets.models {
        r.upload_mesh(ModelId(m.id), m.tmdl);
    }
    if let Some(mode) = opts.simulate() {
        r.set_cvd_view(mode);
    }
    let scene = trino_scene::Scene::load(file)?;
    let mut lib = trino_scene::Library::load(root);
    lib.model_bounds = trino_asset_pipeline::manifest_model_bounds(&root.join("assets"));
    let env = &scene.environment;
    let target = match family {
        Family::N64 => trino_scene::Target::N64,
        Family::N3ds => trino_scene::Target::N3ds,
        Family::Pc => trino_scene::Target::Pc,
        Family::Web => trino_scene::Target::Web,
    };
    let resolved = trino_scene::resolve_for(&scene, &lib, 0.0, target);
    let camera = env.core_camera();
    for _ in 0..3 {
        let c = env.clear;
        r.begin_frame(Color::rgb(c[0], c[1], c[2]));
        r.set_camera(&camera);
        r.set_light(&env.core_light());
        for item in &resolved.items {
            if let Some(id) = item.model_from(camera.eye) {
                r.draw_model_matrix(id, &item.matrix, Material::VertexLit, &item.params);
            }
        }
        r.end_frame();
    }
    let (w, h) = r.internal_size();
    Ok((w, h, r.read_offscreen()))
}
