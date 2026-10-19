//! `cargo xtask ui <check|shot|export|list>` (issue #58) — the text-first
//! loop for UI screens (`ui/*.ui.ron`), for people, agents and CI:
//!
//! - `ui check [screen..] [--platform p] [--lang l] [--json]` validates
//!   every screen, then lays it out on each target the project builds for,
//!   in the default texts and each language of `assets/i18n/`, and reports
//!   overflowing text, controls the pad cannot reach, glyphs the fonts
//!   lack, keys a language lacks and the node and draw budgets
//!   (`Caps::max_ui_nodes` / `max_ui_draws`). Non-zero exit on any issue.
//! - `ui shot <screen> [--platform p] [--size WxH] [--lang l] [--scale s]
//!   [--focus id] [--out x.png] [--json]` renders the screen headless as
//!   the target shows it (the 3DS's two screens stacked).
//! - `ui export <screen> <out.rs> [--platform p] [--json]` writes it as
//!   `no_std` Rust (`pub static SCREEN: UiScreen<'static>`).
//! - `ui list [--json]` lists the screens and the languages.
//!
//! A screen is a name (`pause` → `ui/pause.ui.ron`) or a path.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::json;
use trino_asset_pipeline::Manifest;
use trino_core::{Caps, Family};
use trino_scene::ui::{
    Flat, Issue, Langs, Level, ScreenDef, Target, audit, missing_assets, to_rust, validate,
};

use crate::project::{Project, Target as Platform};
use crate::ui_render::{self, ShotOptions};

const USAGE: &str = "xtask: usage:\n  \
    cargo xtask ui check [screen..] [--platform n64|3ds|pc|web] [--lang <lang>] [--json]\n  \
    cargo xtask ui shot <screen> [--platform n64|3ds|pc|web] [--size WxH] [--lang <lang>] \
    [--scale S] [--focus <id>] [--out <file.png>] [--json]\n  \
    cargo xtask ui export <screen> <out.rs> [--platform n64|3ds|pc|web] [--json]\n  \
    cargo xtask ui list [--json]";

pub fn run(project: &Project, args: &[&str]) -> ExitCode {
    let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let json = crate::web::take_flag(&mut args, "--json");
    let opts = match Opts::take(&mut args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("xtask: {e}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let rest: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match rest.as_slice() {
        ["check", screens @ ..] => check(project, screens, &opts, json),
        ["shot", screen] => shot(project, screen, &opts, json),
        ["export", screen, out] => export(project, screen, Path::new(out), &opts, json),
        ["list"] => list(project, json),
        _ => Err(USAGE.to_string()),
    };
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
    scale: Option<f32>,
    focus: Option<String>,
    out: Option<PathBuf>,
}

impl Opts {
    fn take(args: &mut Vec<String>) -> Result<Opts, String> {
        let platform = match crate::web::take_value(args, "--platform")? {
            Some(p) => {
                Some(family(&p).ok_or(format!("--platform {p}: needs n64, 3ds, pc or web"))?)
            }
            None => None,
        };
        let size = match crate::web::take_value(args, "--size")? {
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
        let scale = match crate::web::take_value(args, "--scale")? {
            Some(s) => Some(
                s.parse::<f32>()
                    .ok()
                    .filter(|v| *v > 0.0)
                    .ok_or("--scale needs a positive number")?,
            ),
            None => None,
        };
        Ok(Opts {
            platform,
            size,
            lang: crate::web::take_value(args, "--lang")?,
            scale,
            focus: crate::web::take_value(args, "--focus")?,
            out: crate::web::take_value(args, "--out")?.map(PathBuf::from),
        })
    }
}

fn family(name: &str) -> Option<Family> {
    match name {
        "pc" => Some(Family::Pc),
        "n64" => Some(Family::N64),
        "3ds" | "n3ds" => Some(Family::N3ds),
        "web" => Some(Family::Web),
        _ => None,
    }
}

/// The targets the project builds for, as UI families.
fn families(project: &Project) -> Vec<Family> {
    project
        .platforms
        .iter()
        .map(|p| match p {
            Platform::Pc => Family::Pc,
            Platform::N64 => Family::N64,
            Platform::N3ds => Family::N3ds,
            Platform::Web => Family::Web,
        })
        .collect()
}

/// The project a screen file belongs to: the folder holding its `ui/` and
/// an `assets/manifest.toml` (an example's own assets), else `root`.
fn root_of(root: &Path, file: &Path) -> PathBuf {
    let dir = file
        .parent()
        .filter(|d| d.file_name().is_some_and(|n| n == "ui"));
    match dir.and_then(Path::parent) {
        Some(p) if p.join("assets/manifest.toml").is_file() => p.to_path_buf(),
        _ => root.to_path_buf(),
    }
}

/// `pause` → `<root>/ui/pause.ui.ron`; a path stays a path.
fn screen_path(root: &Path, screen: &str) -> PathBuf {
    if screen.ends_with(".ron") {
        let p = PathBuf::from(screen);
        if p.is_file() || p.is_absolute() {
            p
        } else {
            root.join(p)
        }
    } else {
        root.join(trino_asset_pipeline::ui::UI_DIR)
            .join(format!("{screen}.ui.ron"))
    }
}

fn manifest(root: &Path) -> Result<Manifest, String> {
    Manifest::load(&root.join("assets").join("manifest.toml"))
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).display().to_string()
}

fn check(project: &Project, screens: &[&str], opts: &Opts, json: bool) -> Result<bool, String> {
    let root = &project.root;
    let files: Vec<PathBuf> = if screens.is_empty() {
        trino_asset_pipeline::ui::screen_files(root)
    } else {
        screens.iter().map(|s| screen_path(root, s)).collect()
    };
    let families: Vec<Family> = match opts.platform {
        Some(f) => vec![f],
        None => families(project),
    };
    let mut reports = Vec::new();
    let mut languages = std::collections::BTreeSet::new();
    let (mut errors, mut warnings) = (0usize, 0usize);
    for file in &files {
        // A screen of an example with its own assets checks against those.
        let froot = root_of(root, file);
        let manifest = manifest(&froot)?;
        let langs = trino_asset_pipeline::ui::langs(&froot)?;
        let lang_list: Vec<Option<String>> = match &opts.lang {
            Some(l) => {
                if !langs.tables.contains_key(l) {
                    return Err(format!(
                        "--lang {l}: no assets/i18n/{l}.ron (have: {})",
                        langs.names().join(", ")
                    ));
                }
                vec![Some(l.clone())]
            }
            None => std::iter::once(None)
                .chain(langs.names().into_iter().map(Some))
                .collect(),
        };
        languages.extend(langs.names());
        let (report, ok_text) =
            check_one(root, &froot, &manifest, &langs, file, &families, &lang_list);
        for i in report["issues"].as_array().into_iter().flatten() {
            if i["level"] == "error" {
                errors += 1;
            } else {
                warnings += 1;
            }
        }
        if !json {
            println!("{ok_text}");
        }
        reports.push(report);
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "screens": reports,
                "languages": languages,
                "errors": errors,
                "warnings": warnings,
            }))
            .unwrap()
        );
    } else if files.is_empty() {
        println!("ui check: no screens in {}", root.join("ui").display());
    } else {
        println!(
            "ui check: {} screen(s), {} target(s), {} language(s): {errors} error(s), \
             {warnings} warning(s)",
            files.len(),
            families.len(),
            languages.len()
        );
    }
    Ok(errors == 0 && warnings == 0)
}

/// One screen's report (JSON) and its text form.
fn check_one(
    root: &Path,
    froot: &Path,
    manifest: &Manifest,
    langs: &Langs,
    file: &Path,
    families: &[Family],
    lang_list: &[Option<String>],
) -> (serde_json::Value, String) {
    let name = rel(root, file);
    let def = match ScreenDef::load(file) {
        Ok(d) => d,
        Err(e) => {
            // `load` names the file; the report already does.
            let prefix = format!("{}: ", file.display());
            let e = e.strip_prefix(&prefix).unwrap_or(&e).to_string();
            let issue = json!({"level": "error", "node": null, "label": "screen", "message": e});
            return (
                json!({"file": name, "issues": [issue]}),
                format!("{name}:\n  error: {e}"),
            );
        }
    };
    let mut issues: Vec<Issue> = validate(&def);
    let structural = issues.iter().any(|i| i.level == Level::Error);
    issues.extend(missing_assets(
        &def,
        &|path: &str| match path.split_once('/') {
            Some(("sprites", name)) => manifest.sprites.contains_key(name),
            Some(("models", name)) => manifest.models.contains_key(name),
            _ => false,
        },
    ));
    let mut targets = Vec::new();
    if !structural {
        for &f in families {
            let platform = ui_render::pipeline_platform(f);
            let fonts = trino_asset_pipeline::ui::fonts(froot, manifest, platform);
            let sizes =
                |s: &str| trino_asset_pipeline::ui::sprite_size(froot, manifest, platform, s);
            let flat = Flat::new(&def, &sizes);
            for lang in lang_list {
                let report = audit(
                    &flat,
                    Target {
                        caps: caps_of(f),
                        lang: lang.as_deref(),
                        interface_scale: 1.0,
                    },
                    &fonts,
                    langs,
                );
                targets.push(json!({
                    "platform": f.key(),
                    "lang": lang,
                    "nodes": report.stats.nodes,
                    "draws": report.stats.draws,
                    "overflow": report.overflow,
                    "max_nodes": caps_of(f).max_ui_nodes,
                    "max_draws": caps_of(f).max_ui_draws,
                }));
                for i in report.issues {
                    if !issues.contains(&i) {
                        issues.push(i);
                    }
                }
            }
        }
    }
    let mut text = String::new();
    if issues.is_empty() {
        let budgets: Vec<String> = targets
            .iter()
            .filter(|t| t["lang"].is_null())
            .map(|t| {
                format!(
                    "{} {}/{} nodes {}/{} draws",
                    t["platform"].as_str().unwrap_or(""),
                    t["nodes"],
                    t["max_nodes"],
                    t["draws"],
                    t["max_draws"]
                )
            })
            .collect();
        text = format!("{name}: ok ({})", budgets.join(", "));
    } else {
        text.push_str(&format!("{name}:"));
        for i in &issues {
            text.push_str(&format!("\n  {i}"));
        }
    }
    (
        json!({
            "file": name,
            "name": def.name,
            "nodes": def.nodes().len(),
            "targets": targets,
            "issues": issues,
        }),
        text,
    )
}

fn caps_of(f: Family) -> Caps {
    f.caps()
}

fn load(root: &Path, screen: &str) -> Result<(PathBuf, ScreenDef), String> {
    let path = screen_path(root, screen);
    let def = ScreenDef::load(&path)?;
    let errors: Vec<String> = validate(&def)
        .into_iter()
        .filter(|i| i.level == Level::Error)
        .map(|i| i.to_string())
        .collect();
    if !errors.is_empty() {
        return Err(format!("{}:\n  {}", path.display(), errors.join("\n  ")));
    }
    Ok((path, def))
}

fn shot(project: &Project, screen: &str, opts: &Opts, json: bool) -> Result<bool, String> {
    let (path, def) = load(&project.root, screen)?;
    let root = &root_of(&project.root, &path);
    let manifest = manifest(root)?;
    let langs = trino_asset_pipeline::ui::langs(root)?;
    if let Some(l) = &opts.lang
        && !langs.tables.contains_key(l)
    {
        return Err(format!("--lang {l}: no assets/i18n/{l}.ron"));
    }
    let family = opts.platform.unwrap_or(Family::Pc);
    let shot = ui_render::render(
        root,
        &manifest,
        &langs,
        &def,
        &ShotOptions {
            family,
            size: opts.size,
            lang: opts.lang.as_deref(),
            interface_scale: opts.scale.unwrap_or(1.0),
            focus: opts.focus.as_deref(),
        },
    )?;
    let out = opts.out.clone().unwrap_or_else(|| {
        let lang = opts
            .lang
            .as_deref()
            .map(|l| format!("-{l}"))
            .unwrap_or_default();
        root.join("target")
            .join("ui")
            .join(format!("{}-{}{lang}.png", def.name, family.key()))
    });
    ui_render::write_png(&out, shot.width, shot.height, &shot.rgba)?;
    if json {
        println!(
            "{}",
            json!({
                "screen": def.name,
                "platform": family.key(),
                "lang": opts.lang,
                "out": out.display().to_string(),
                "width": shot.width,
                "height": shot.height,
                "nodes": shot.stats.nodes,
                "draws": shot.stats.draws,
                "overflow": shot.stats.overflow,
            })
        );
    } else {
        println!(
            "ui shot: {} on {} ({}×{}, {} nodes, {} draws) → {}",
            def.name,
            family.key(),
            shot.width,
            shot.height,
            shot.stats.nodes,
            shot.stats.draws,
            out.display()
        );
    }
    Ok(true)
}

fn export(
    project: &Project,
    screen: &str,
    out: &Path,
    opts: &Opts,
    json: bool,
) -> Result<bool, String> {
    let (path, def) = load(&project.root, screen)?;
    let root = &root_of(&project.root, &path);
    let manifest = manifest(root)?;
    let platform = ui_render::pipeline_platform(opts.platform.unwrap_or(Family::Pc));
    let sizes = |s: &str| trino_asset_pipeline::ui::sprite_size(root, &manifest, platform, s);
    let flat = Flat::new(&def, &sizes);
    let code = to_rust(&flat);
    if let Some(dir) = out.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(out, &code).map_err(|e| format!("{}: {e}", out.display()))?;
    if json {
        println!(
            "{}",
            json!({
                "screen": def.name,
                "from": rel(root, &path),
                "out": out.display().to_string(),
                "nodes": flat.nodes.len(),
                "bytes": code.len(),
            })
        );
    } else {
        println!(
            "ui export: {} ({} nodes) → {}",
            rel(root, &path),
            flat.nodes.len(),
            out.display()
        );
    }
    Ok(true)
}

fn list(project: &Project, json: bool) -> Result<bool, String> {
    let root = &project.root;
    let langs = trino_asset_pipeline::ui::langs(root)?;
    let mut screens = Vec::new();
    for file in trino_asset_pipeline::ui::screen_files(root) {
        match ScreenDef::load(&file) {
            Ok(def) => {
                let nodes = def.nodes();
                let controls = nodes.iter().filter(|(_, n)| n.kind.is_focusable()).count();
                screens.push(json!({
                    "name": def.name,
                    "file": rel(root, &file),
                    "nodes": nodes.len(),
                    "controls": controls,
                    "reference": [def.reference.0, def.reference.1],
                }));
            }
            Err(e) => screens.push(json!({"file": rel(root, &file), "error": e})),
        }
    }
    let languages: Vec<_> = langs
        .tables
        .values()
        .map(|t| json!({"lang": t.lang, "fallback": t.fallback, "entries": t.entries.len()}))
        .collect();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"screens": screens, "languages": languages}))
                .unwrap()
        );
    } else {
        for s in &screens {
            match s.get("error") {
                Some(e) => println!("{}: {}", s["file"].as_str().unwrap_or(""), e),
                None => println!(
                    "{:<16} {:>3} nodes {:>3} controls  {}",
                    s["name"].as_str().unwrap_or(""),
                    s["nodes"],
                    s["controls"],
                    s["file"].as_str().unwrap_or("")
                ),
            }
        }
        for l in &languages {
            println!(
                "lang {:<10} {:>4} strings{}",
                l["lang"].as_str().unwrap_or(""),
                l["entries"],
                match l["fallback"].as_str() {
                    Some("") | None => String::new(),
                    Some(f) => format!(" (fallback {f})"),
                }
            );
        }
    }
    Ok(true)
}
