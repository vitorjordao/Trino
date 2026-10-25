//! `cargo xtask anim <list|check|simulate|new>` — animation graphs as text
//! (issue #65, `trino_scene::anim`). The editor's Animation tab writes the
//! same `assets/anim/<name>.anim.ron` files and runs the same checks.
//!
//! - `anim list [--json]`: every graph, its model, states, parameters.
//! - `anim check [name..] [--platform p] [--json]`: names, kinds, limits,
//!   blend points, events past their clip's end, unreachable states and dead
//!   ends, against the clips the model bakes to on each platform; bakes the
//!   `TANG` blob and reports its size. Fails on errors.
//! - `anim simulate <name> --script <file.ron> [--out trace.json] [--sample N]
//!   [--platform p] [--json]`: runs the script's parameter changes through
//!   the runtime itself and writes the state timeline, transitions, events,
//!   root motion and pose samples (what an agent reads instead of watching).
//! - `anim new <name> --model models/x`: a starter graph, one state per clip.

use std::path::Path;
use std::process::ExitCode;

use serde_json::json;
use trino_asset_pipeline::anim_graph;
use trino_asset_pipeline::{Manifest, Platform};
use trino_scene::anim::{self, AnimGraphDef, Level, ScriptDef};

const USAGE: &str = "xtask: usage:
  cargo xtask anim list [--json]
  cargo xtask anim check [name..] [--platform pc|n64|3ds|web] [--json]
  cargo xtask anim simulate <name> --script <file.ron> [--out trace.json] [--sample N] [--platform p] [--json]
  cargo xtask anim new <name> --model models/x";

pub fn run(root: &Path, args: &[&str]) -> ExitCode {
    let json = args.contains(&"--json");
    let mut names = Vec::new();
    let mut platform = None;
    let mut script = None;
    let mut out = None;
    let mut model = None;
    let mut sample = 6usize;
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        match *a {
            "--json" => {}
            "--platform" => match it.next().and_then(|p| Platform::parse(p)) {
                Some(p) => platform = Some(p),
                None => return fail("--platform needs pc, n64, 3ds or web", json),
            },
            "--script" => script = it.next().map(|s| s.to_string()),
            "--out" => out = it.next().map(|s| s.to_string()),
            "--model" => model = it.next().map(|s| s.to_string()),
            "--sample" => match it.next().and_then(|v| v.parse().ok()) {
                Some(n) => sample = n,
                None => return fail("--sample needs a number of updates", json),
            },
            flag if flag.starts_with("--") => {
                return fail(&format!("unknown flag `{flag}`\n{USAGE}"), json);
            }
            name => names.push(name.to_string()),
        }
    }
    let manifest = match Manifest::load(&root.join("assets/manifest.toml")) {
        Ok(m) => m,
        Err(e) => return fail(&e, json),
    };
    match args.first().copied() {
        Some("list") => list(root, json),
        Some("check") => check(root, &manifest, &names, platform, json),
        Some("simulate") => {
            let (Some(name), Some(script)) = (names.first(), script) else {
                return fail(
                    &format!("simulate needs <name> --script <file.ron>\n{USAGE}"),
                    json,
                );
            };
            simulate(
                root,
                &manifest,
                name,
                &script,
                out.as_deref(),
                sample,
                platform.unwrap_or(Platform::Pc),
                json,
            )
        }
        Some("new") => {
            let (Some(name), Some(model)) = (names.first(), model) else {
                return fail(&format!("new needs <name> --model models/x\n{USAGE}"), json);
            };
            new(root, &manifest, name, &model, json)
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn fail(e: &str, json: bool) -> ExitCode {
    if json {
        println!("{}", json!({ "ok": false, "error": e }));
    } else {
        eprintln!("error: {e}");
    }
    ExitCode::FAILURE
}

fn list(root: &Path, json: bool) -> ExitCode {
    let mut rows = Vec::new();
    for (name, path) in anim::graph_files(root) {
        match AnimGraphDef::load(&path) {
            Ok(d) => rows.push(json!({
                "name": name,
                "file": path.strip_prefix(root).unwrap_or(&path).display().to_string(),
                "model": d.model_path(),
                "states": d.states.iter().map(|(n, s)| json!({ "name": n, "kind": s.kind(), "clips": s.clips() })).collect::<Vec<_>>(),
                "params": d.params.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
                "transitions": d.transitions.len(),
            })),
            Err(e) => rows.push(json!({ "name": name, "error": e })),
        }
    }
    if json {
        println!("{}", json!({ "ok": true, "graphs": rows }));
    } else if rows.is_empty() {
        println!("no graphs under {}", anim::DIR);
    } else {
        for r in &rows {
            match r.get("error") {
                Some(e) => println!("{}: {}", r["name"].as_str().unwrap_or(""), e),
                None => println!(
                    "anim/{}  {}  {} states, {} params, {} transitions",
                    r["name"].as_str().unwrap_or(""),
                    r["model"].as_str().unwrap_or(""),
                    r["states"].as_array().map_or(0, |a| a.len()),
                    r["params"].as_array().map_or(0, |a| a.len()),
                    r["transitions"]
                ),
            }
        }
    }
    ExitCode::SUCCESS
}

const PLATFORMS: [Platform; 4] = [Platform::Pc, Platform::N64, Platform::N3ds, Platform::Web];

fn check(
    root: &Path,
    manifest: &Manifest,
    names: &[String],
    platform: Option<Platform>,
    json: bool,
) -> ExitCode {
    let files: Vec<(String, std::path::PathBuf)> = anim::graph_files(root)
        .into_iter()
        .filter(|(n, _)| names.is_empty() || names.contains(n))
        .collect();
    for n in names {
        if !files.iter().any(|(f, _)| f == n) {
            return fail(
                &format!("no graph `{n}` ({})", anim::graph_file(root, n).display()),
                json,
            );
        }
    }
    let platforms: Vec<Platform> = platform.map_or(PLATFORMS.to_vec(), |p| vec![p]);
    let mut ok = true;
    let mut rows = Vec::new();
    for (name, path) in &files {
        let def = match AnimGraphDef::load(path) {
            Ok(d) => d,
            Err(e) => {
                ok = false;
                rows.push(json!({ "graph": name, "error": e }));
                if !json {
                    println!("anim/{name}: error: {e}");
                }
                continue;
            }
        };
        for &p in &platforms {
            let clips = match anim_graph::model_clips(root, manifest, p, &def.model_path()) {
                Ok(c) => c,
                Err(e) => {
                    ok = false;
                    rows.push(json!({ "graph": name, "platform": p.key(), "error": e }));
                    continue;
                }
            };
            let declared = manifest
                .models
                .contains_key(def.model_path().trim_start_matches("models/"));
            if clips.is_none() && declared {
                // The model is not on this platform (a PC-only rig): no blob.
                rows.push(json!({ "graph": name, "platform": p.key(), "skipped": "model not baked here" }));
                if !json {
                    println!(
                        "anim/{name} [{}]: skipped (its model is not baked here)",
                        p.key()
                    );
                }
                continue;
            }
            let issues = anim::check(&def, clips.as_deref(), &|s| {
                anim_graph::has_sound(manifest, s)
            });
            let errors = issues.iter().filter(|i| i.level == Level::Error).count();
            let bytes = clips
                .as_ref()
                .filter(|_| errors == 0)
                .and_then(|c| anim::to_tang(&def, c, &|s| anim_graph::has_sound(manifest, s)).ok())
                .map(|b| b.len());
            ok &= errors == 0;
            if !json {
                let status = if errors == 0 { "ok" } else { "FAILED" };
                println!(
                    "anim/{name} [{}]: {status}{}",
                    p.key(),
                    bytes.map_or(String::new(), |b| format!(" — TANG {b} bytes"))
                );
                for i in &issues {
                    println!("  {i}");
                }
            }
            rows.push(json!({
                "graph": name,
                "platform": p.key(),
                "ok": errors == 0,
                "bytes": bytes,
                "issues": issues,
            }));
        }
    }
    if json {
        println!("{}", json!({ "ok": ok, "results": rows }));
    } else if files.is_empty() {
        println!("no graphs under {}", anim::DIR);
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[allow(clippy::too_many_arguments)]
fn simulate(
    root: &Path,
    manifest: &Manifest,
    name: &str,
    script: &str,
    out: Option<&str>,
    sample: usize,
    platform: Platform,
    json: bool,
) -> ExitCode {
    let def = match AnimGraphDef::load(&anim::graph_file(root, name)) {
        Ok(d) => d,
        Err(e) => return fail(&e, json),
    };
    let script_path = std::env::current_dir()
        .unwrap_or_else(|_| root.to_path_buf())
        .join(script);
    let script = match std::fs::read_to_string(&script_path)
        .map_err(|e| format!("{}: {e}", script_path.display()))
        .and_then(|t| {
            ScriptDef::from_ron(&t).map_err(|e| format!("{}: {e}", script_path.display()))
        }) {
        Ok(s) => s,
        Err(e) => return fail(&e, json),
    };
    let tang = match anim_graph::bake(root, manifest, platform, &def) {
        Ok(Some(b)) => b,
        Ok(None) => {
            return fail(
                &format!("{} is not baked for {}", def.model_path(), platform.key()),
                json,
            );
        }
        Err(e) => return fail(&e, json),
    };
    let short = def.model_path().trim_start_matches("models/").to_string();
    let decl = manifest.models.get(&short);
    let clip_names = decl
        .and_then(|d| {
            trino_asset_pipeline::resolve_source(&root.join("assets"), platform, &d.file).ok()
        })
        .map(|p| trino_asset_pipeline::clip_names(&p))
        .unwrap_or_default();
    let mut trace = match anim::simulate(&def, &tang, &clip_names, &script, sample) {
        Ok(t) => t,
        Err(e) => return fail(&e, json),
    };
    let known: Vec<String> = decl
        .map(|d| {
            d.events
                .values()
                .flatten()
                .map(|e| e.name.clone())
                .collect()
        })
        .unwrap_or_default();
    anim::name_events(&mut trace, &known);
    let full = serde_json::to_string_pretty(&trace).expect("trace serializes");
    if let Some(out) = out {
        let path = std::env::current_dir()
            .unwrap_or_else(|_| root.to_path_buf())
            .join(out);
        if let Err(e) = std::fs::write(&path, &full) {
            return fail(&format!("{}: {e}", path.display()), json);
        }
        if !json {
            println!("wrote {}", path.display());
        }
    }
    if json {
        // The trace itself, unless it went to a file (then a summary).
        if out.is_some() {
            println!(
                "{}",
                json!({ "ok": true, "out": out, "transitions": trace.transitions, "events": trace.events.len(), "digest": trace.digest, "root": trace.root })
            );
        } else {
            println!("{full}");
        }
    } else {
        println!(
            "anim/{name}: {} updates, digest {:#010x}",
            trace.updates, trace.digest
        );
        for s in &trace.segments {
            println!("  {:7.3} – {:7.3} s  {}", s.from, s.to, s.state);
        }
        println!(
            "  {} events, root motion x {:.3} z {:.3} yaw {:.3}",
            trace.events.len(),
            trace.root.0,
            trace.root.1,
            trace.root.2
        );
        for u in &trace.unknown {
            println!("  warning: the script sets `{u}`, which the graph lacks");
        }
    }
    ExitCode::SUCCESS
}

fn new(root: &Path, manifest: &Manifest, name: &str, model: &str, json: bool) -> ExitCode {
    if !trino_asset_pipeline::manifest::edit::valid_name(name) {
        return fail(
            &format!("graph name `{name}`: lowercase letters, digits, - and _"),
            json,
        );
    }
    let path = anim::graph_file(root, name);
    if path.exists() {
        return fail(&format!("{} exists", path.display()), json);
    }
    let model = if model.starts_with("models/") {
        model.to_string()
    } else {
        format!("models/{model}")
    };
    let Some(decl) = manifest.models.get(model.trim_start_matches("models/")) else {
        return fail(&format!("model `{model}` is not in the manifest"), json);
    };
    let clips =
        trino_asset_pipeline::resolve_source(&root.join("assets"), Platform::Pc, &decl.file)
            .map(|p| trino_asset_pipeline::clip_names(&p))
            .unwrap_or_default();
    if clips.is_empty() {
        return fail(&format!("`{model}` has no clips"), json);
    }
    let def = AnimGraphDef::starter(&model, &clips);
    if let Err(e) = def.save(&path) {
        return fail(&e, json);
    }
    if json {
        println!(
            "{}",
            json!({ "ok": true, "file": path.display().to_string(), "states": clips })
        );
    } else {
        println!(
            "wrote {} ({} states: {})",
            path.display(),
            clips.len(),
            clips.join(", ")
        );
    }
    ExitCode::SUCCESS
}
