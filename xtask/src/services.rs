//! `cargo xtask services <check|lock|list|export steam>` (issue #61, ADR
//! 0029) — a project's achievements, stats and presence keys
//! (`assets/achievements.ron`) as a text-first loop:
//!
//! - `services check [dir] [--json]`: ids, limits, stat references, the
//!   icons, every name/description/presence key in every language, the
//!   shipped bit order (`assets/achievements.lock`) and whether a Steam
//!   export on disk is stale. Non-zero exit on an error.
//! - `services lock [dir]`: write the current order as the lock (do it
//!   before the first release; later edits may only append).
//! - `services list [dir] [--json]`: the table as the bake writes it.
//! - `services export steam [dir] [--out <dir>] [--json]`: the files the
//!   Steamworks partner site and the Steam client take (`dist/steam-config`
//!   by default): `achievements.csv`, `achievements_loc.vdf`,
//!   `rich_presence/<language>.vdf`, `game_actions_<app id>.vdf` and
//!   64 × 64 `icons/`. The editor's Achievements tab writes the same.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::json;
use trino_scene::services::{self as svc, AchievementsDef};
use trino_scene::ui::Level;

use crate::project::Project;

const USAGE: &str = "xtask: usage:\n  \
    cargo xtask services check [dir] [--json]\n  \
    cargo xtask services lock [dir]\n  \
    cargo xtask services list [dir] [--json]\n  \
    cargo xtask services export steam [dir] [--out <dir>] [--json]";

/// Where `export steam` writes unless `--out` says otherwise.
pub const STEAM_CONFIG_DIR: &str = trino_asset_pipeline::services::STEAM_CONFIG_DIR;

pub fn run(project: &Project, args: &[&str]) -> ExitCode {
    let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let json = crate::web::take_flag(&mut args, "--json");
    let result = crate::web::take_value(&mut args, "--out").and_then(|out| {
        let rest: Vec<&str> = args.iter().map(String::as_str).collect();
        let dir = |d: Option<&&str>| d.map_or(project.root.clone(), |d| project.root.join(d));
        match rest.as_slice() {
            ["check", d @ ..] if d.len() <= 1 => check(project, &dir(d.first()), json),
            ["lock", d @ ..] if d.len() <= 1 => lock(&dir(d.first())),
            ["list", d @ ..] if d.len() <= 1 => list(&dir(d.first()), json),
            ["export", "steam", d @ ..] if d.len() <= 1 => {
                let root = dir(d.first());
                let out = out
                    .map(PathBuf::from)
                    .unwrap_or_else(|| root.join(STEAM_CONFIG_DIR));
                export_steam(project, &root, &out, json).map(|_| true)
            }
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

fn load(root: &Path) -> Result<AchievementsDef, String> {
    svc::load(root)?.ok_or_else(|| {
        format!(
            "{}: no achievements (write one, or use the editor's Achievements tab)",
            svc::path(root).display()
        )
    })
}

/// Every finding for the project at `root`, the Steam export's staleness
/// included (when `dist/steam-config` exists).
pub fn findings(project: &Project, root: &Path) -> Result<Vec<svc::Finding>, String> {
    let def = load(root)?;
    let manifest = crate::ui::manifest(root)?;
    let langs = trino_asset_pipeline::ui::langs(root)?;
    let lock = svc::load_lock(root);
    let has_sprite = |s: &str| {
        s.strip_prefix("sprites/")
            .is_some_and(|n| manifest.sprites.contains_key(n))
    };
    let mut out = svc::check(&def, &langs, lock.as_deref(), &has_sprite);
    let export = root.join(STEAM_CONFIG_DIR);
    if export.is_dir() {
        for (rel, want) in text_files(project, &def, &langs) {
            let have = std::fs::read_to_string(export.join(&rel)).unwrap_or_default();
            if have != want {
                out.push(svc::Finding {
                    level: Level::Warning,
                    area: "steam",
                    subject: rel,
                    lang: None,
                    message: format!(
                        "differs from what `cargo xtask services export steam` writes now \
                         ({STEAM_CONFIG_DIR} is stale)"
                    ),
                });
            }
        }
    }
    Ok(out)
}

fn check(project: &Project, root: &Path, json: bool) -> Result<bool, String> {
    let f = findings(project, root)?;
    let errors = f.iter().filter(|x| x.level == Level::Error).count();
    let warnings = f.len() - errors;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "file": svc::path(root),
                "errors": errors,
                "warnings": warnings,
                "findings": f,
            }))
            .unwrap()
        );
    } else {
        for x in &f {
            println!("{x}");
        }
        println!("services check: {errors} error(s), {warnings} warning(s)");
    }
    Ok(errors == 0)
}

fn lock(root: &Path) -> Result<bool, String> {
    let def = load(root)?;
    if let Some(old) = svc::load_lock(root) {
        let ids: Vec<&str> = def.achievements.iter().map(|a| a.id.as_str()).collect();
        if !ids.starts_with(&old.iter().map(String::as_str).collect::<Vec<_>>()) {
            return Err(format!(
                "{}: the achievements no longer start with the locked order; \
                 restore it (saves hold those bits) instead of relocking",
                svc::lock_path(root).display()
            ));
        }
    }
    let p = svc::write_lock(root, &def)?;
    println!(
        "services lock: {} achievement(s) in {}",
        def.achievements.len(),
        p.display()
    );
    Ok(true)
}

fn list(root: &Path, json: bool) -> Result<bool, String> {
    let def = load(root)?;
    let blob = def.to_tach()?;
    let t = trino_core::AchievementTable::parse(&blob).map_err(|e| format!("{e:?}"))?;
    let packed = trino_core::AchievementState::packed_len(t.len(), t.stat_count());
    if json {
        let achievements: Vec<_> = t
            .iter()
            .map(|a| {
                json!({
                    "bit": a.index, "id": a.api, "hash": format!("{:08x}", a.id.0),
                    "name": a.name, "description": a.description, "hidden": a.hidden,
                    "icon": def.achievements[a.index].icon,
                    "stat": a.stat.and_then(|s| t.stat(s)).map(|s| s.api),
                    "threshold": a.stat.map(|_| a.threshold),
                })
            })
            .collect();
        let stats: Vec<_> = t
            .stats()
            .map(|s| json!({"id": s.api, "min": s.min, "max": s.max, "increment_only": s.increment_only}))
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "achievements": achievements,
                "stats": stats,
                "presence": def.presence,
                "blob_bytes": blob.len(),
                "save_bytes": packed,
            }))
            .unwrap()
        );
    } else {
        for a in t.iter() {
            let stat = a
                .stat
                .and_then(|s| t.stat(s))
                .map(|s| format!("  ({} >= {})", s.api, a.threshold))
                .unwrap_or_default();
            let hidden = if a.hidden { "  hidden" } else { "" };
            println!("{:3}  {:<24} {}{stat}{hidden}", a.index, a.api, a.name);
        }
        for s in t.stats() {
            let inc = if s.increment_only {
                ", increment only"
            } else {
                ""
            };
            println!("stat {:<20} {}..={}{inc}", s.api, s.min, s.max);
        }
        println!(
            "{} achievement(s), {} stat(s), {} presence key(s): {} B blob, {} B in a console save",
            t.len(),
            t.stat_count(),
            t.presence_count(),
            blob.len(),
            packed
        );
    }
    Ok(true)
}

/// The text files of a Steam export (relative path, contents).
pub fn text_files(
    project: &Project,
    def: &AchievementsDef,
    langs: &trino_scene::i18n::Langs,
) -> Vec<(String, String)> {
    trino_asset_pipeline::services::steam_text_files(
        def,
        langs,
        project.steam_app_id,
        &project.title,
    )
}

/// Write the Steam files into `out` (the editor's Export button calls the
/// same function); returns them.
pub fn export_steam(
    project: &Project,
    root: &Path,
    out: &Path,
    json: bool,
) -> Result<Vec<PathBuf>, String> {
    let written = trino_asset_pipeline::services::export_steam(
        root,
        project.steam_app_id,
        &project.title,
        out,
    )?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "out": out, "files": written })).unwrap()
        );
    } else {
        for p in &written {
            println!("wrote {}", p.display());
        }
        if project.steam_app_id.is_none() {
            println!(
                "note: no [steam] app_id in trino.toml: the action manifest is game_actions_0.vdf"
            );
        }
    }
    Ok(written)
}
