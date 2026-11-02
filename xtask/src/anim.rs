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
  cargo xtask anim new <name> --model models/x
  cargo xtask anim export <name> --out <file.tang> [--platform p]   # the baked blob, for a game that embeds it
  cargo xtask anim export <name> --rust <file.rs> [--static NAME] [--platform p]   # its decoded tables as static Rust (no parser linked)
  (every command takes --dir <project> for another project's assets/)";

pub fn run(root: &Path, args: &[&str]) -> ExitCode {
    let json = args.contains(&"--json");
    let mut dir: Option<std::path::PathBuf> = None;
    let mut names = Vec::new();
    let mut platform = None;
    let mut script = None;
    let mut out = None;
    let mut rust = None;
    let mut static_name = None;
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
            "--dir" => dir = it.next().map(std::path::PathBuf::from),
            "--out" => out = it.next().map(|s| s.to_string()),
            "--rust" => rust = it.next().map(|s| s.to_string()),
            "--static" => static_name = it.next().map(|s| s.to_string()),
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
    let root = dir.as_deref().unwrap_or(root);
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
        Some("export") if rust.is_some() => {
            let (Some(name), Some(rs)) = (names.first(), rust) else {
                return fail(
                    &format!("export needs <name> --rust <file.rs>\n{USAGE}"),
                    json,
                );
            };
            let ident = static_name.unwrap_or_else(|| {
                format!("{}_GRAPH", name.to_uppercase().replace(['-', ' '], "_"))
            });
            export_rust(
                root,
                &manifest,
                name,
                Path::new(&rs),
                &ident,
                platform.unwrap_or(Platform::Pc),
                json,
            )
        }
        Some("export") => {
            let (Some(name), Some(out)) = (names.first(), out) else {
                return fail(
                    &format!("export needs <name> --out <file.tang>\n{USAGE}"),
                    json,
                );
            };
            export(
                root,
                &manifest,
                name,
                Path::new(&out),
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

/// Write graph `name`'s baked `TANG` for `platform` to `out` (a game that
/// embeds its graph with `include_bytes!`, as Solid Eleven does).
fn export(
    root: &Path,
    manifest: &Manifest,
    name: &str,
    out: &Path,
    platform: Platform,
    json: bool,
) -> ExitCode {
    let file = anim::graph_file(root, name);
    let def = match AnimGraphDef::load(&file) {
        Ok(d) => d,
        Err(e) => return fail(&format!("{}: {e}", file.display()), json),
    };
    let bytes = match anim_graph::bake(root, manifest, platform, &def) {
        Ok(Some(b)) => b,
        Ok(None) => {
            return fail(
                &format!("anim/{name}: its model is not baked for {}", platform.key()),
                json,
            );
        }
        Err(e) => return fail(&format!("anim/{name}: {e}"), json),
    };
    if let Err(e) = std::fs::write(out, &bytes) {
        return fail(&format!("{}: {e}", out.display()), json);
    }
    if json {
        println!(
            "{}",
            json!({ "ok": true, "graph": format!("anim/{name}"), "out": out.display().to_string(), "bytes": bytes.len() })
        );
    } else {
        println!("anim/{name}: {} bytes -> {}", bytes.len(), out.display());
    }
    ExitCode::SUCCESS
}

/// Write graph `name`'s decoded tables for `platform` to `out` as static
/// Rust (issue #138): `pub static <ident>: GraphTables<'static>`.
fn export_rust(
    root: &Path,
    manifest: &Manifest,
    name: &str,
    out: &Path,
    ident: &str,
    platform: Platform,
    json: bool,
) -> ExitCode {
    let file = anim::graph_file(root, name);
    let def = match AnimGraphDef::load(&file) {
        Ok(d) => d,
        Err(e) => return fail(&format!("{}: {e}", file.display()), json),
    };
    let bytes = match anim_graph::bake(root, manifest, platform, &def) {
        Ok(Some(b)) => b,
        Ok(None) => {
            return fail(
                &format!("anim/{name}: its model is not baked for {}", platform.key()),
                json,
            );
        }
        Err(e) => return fail(&format!("anim/{name}: {e}"), json),
    };
    let code = match anim::tables_to_rust(&bytes, ident, name) {
        Ok(c) => c,
        Err(e) => return fail(&format!("anim/{name}: {e}"), json),
    };
    if let Err(e) = std::fs::write(out, &code) {
        return fail(&format!("{}: {e}", out.display()), json);
    }
    if json {
        println!(
            "{}",
            json!({ "ok": true, "graph": format!("anim/{name}"), "out": out.display().to_string(), "static": ident, "bytes": code.len() })
        );
    } else {
        println!("anim/{name}: static {ident} -> {}", out.display());
    }
    ExitCode::SUCCESS
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

/// A platform's tiers and their caps (what poses each folds to).
fn tiers(p: Platform) -> Vec<(&'static str, trino_core::Caps)> {
    use trino_core::Caps;
    match p {
        Platform::Pc => vec![("pc", Caps::PC)],
        Platform::Web => vec![("web", Caps::WEB)],
        Platform::N64 => vec![("n64", Caps::N64), ("n64_expanded", Caps::N64_EXPANDED)],
        Platform::N3ds => vec![("3ds", Caps::N3DS), ("n3ds_new", Caps::N3DS_NEW)],
    }
}

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
            let joints = anim_graph::model_joints(root, manifest, p, &def.model_path())
                .ok()
                .flatten();
            let issues = anim::check_with(&def, clips.as_deref(), joints.as_deref(), &|s| {
                anim_graph::has_sound(manifest, s)
            });
            let errors = issues.iter().filter(|i| i.level == Level::Error).count();
            let bytes = clips.as_ref().filter(|_| errors == 0).and_then(|c| {
                anim::to_tang_with(&def, c, joints.as_deref(), &|s| {
                    anim_graph::has_sound(manifest, s)
                })
                .ok()
            });
            // What it costs at run time (issue #138).
            let runtime = bytes.as_deref().and_then(|b| anim::runtime_bytes(b).ok());
            let bytes = bytes.map(|b| b.len());
            // What each of the platform's tiers folds (issue #112).
            let folds: Vec<serde_json::Value> = tiers(p)
                .into_iter()
                .map(|(tier, caps)| {
                    let notes = anim::fold_report(&def, caps.anim_layers);
                    json!({
                        "tier": tier,
                        "layers": caps.anim_layers.layers,
                        "clips": caps.anim_layers.clips,
                        "notes": notes,
                    })
                })
                .collect();
            ok &= errors == 0;
            if !json {
                let status = if errors == 0 { "ok" } else { "FAILED" };
                println!(
                    "anim/{name} [{}]: {status}{}",
                    p.key(),
                    bytes.map_or(String::new(), |b| format!(" — TANG {b} bytes"))
                );
                if let Some((t, full, crowd)) = runtime {
                    println!(
                        "  run time (host): tables {t} B, AnimGraphInstance {full} B, \
                         a crowd GraphInstance {crowd} B"
                    );
                }
                for i in &issues {
                    println!("  {i}");
                }
                for f in &folds {
                    let notes = f["notes"].as_array().map_or(&[][..], |v| v.as_slice());
                    if notes.is_empty() {
                        continue;
                    }
                    println!(
                        "  fold on {} ({} layer(s), {} clips):",
                        f["tier"].as_str().unwrap_or(""),
                        f["layers"],
                        f["clips"]
                    );
                    for n in notes {
                        println!(
                            "    {}: {}",
                            n["at"].as_str().unwrap_or(""),
                            n["message"].as_str().unwrap_or("")
                        );
                    }
                }
            }
            rows.push(json!({
                "graph": name,
                "platform": p.key(),
                "ok": errors == 0,
                "bytes": bytes,
                "runtime": runtime.map(|(t, full, crowd)| json!({
                    "tables_bytes": t,
                    "instance_bytes": full,
                    "crowd_instance_bytes": crowd,
                })),
                "issues": issues,
                "folds": folds,
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
