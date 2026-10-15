//! `cargo xtask impostor <new|bake|check|list>` — the text-first side of
//! the impostor bake (`trino_asset_pipeline::impostor`). The editor's model
//! inspector writes the same settings file and calls the same bake, so an
//! agent and a person get identical files.
//!
//! - `impostor new <name> --model models/x [--views N] [--pitch DEG]
//!   [--tile PX] [--n64-tile PX]` writes `assets/impostors/<name>.impostor.ron`
//!   and bakes it.
//! - `impostor bake <name>... | --all` re-renders from the settings.
//! - `impostor check [name...]` fails when a baked impostor is missing or
//!   stale (its model changed since the bake).
//! - `impostor list [--json]` prints every impostor with its settings,
//!   sizes and state.
//!
//! Runs on the selected project (`--project`, `$TRINO_PROJECT`, a
//! `trino.toml` in the cwd), else on the engine's own assets.

use std::path::Path;
use std::process::ExitCode;

use trino_asset_pipeline::impostor::{self, ImpostorDef};

const USAGE: &str = "xtask: usage:
  cargo xtask impostor new <name> --model models/x [--views N] [--pitch DEG] [--tile PX] [--n64-tile PX]
  cargo xtask impostor bake <name>... | --all
  cargo xtask impostor check [name...]
  cargo xtask impostor list [--json]";

pub fn run(root: &Path, args: &[&str]) -> ExitCode {
    let assets = root.join("assets");
    match args {
        ["new", name, rest @ ..] => new(&assets, name, rest),
        ["bake", "--all"] => {
            let names = impostor::names(&assets);
            if names.is_empty() {
                eprintln!(
                    "xtask: no impostors under {}",
                    assets.join("impostors").display()
                );
                return ExitCode::FAILURE;
            }
            bake(
                &assets,
                &names.iter().map(String::as_str).collect::<Vec<_>>(),
            )
        }
        ["bake", names @ ..] if !names.is_empty() => bake(&assets, names),
        ["check", names @ ..] => check(&assets, names),
        ["list", rest @ ..] => list(&assets, rest.contains(&"--json")),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn new(assets: &Path, name: &str, rest: &[&str]) -> ExitCode {
    let mut def = ImpostorDef::new("");
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        let Some(value) = it.next() else {
            eprintln!("xtask: {flag} needs a value\n{USAGE}");
            return ExitCode::FAILURE;
        };
        let num = |v: &str| v.parse::<f32>().ok();
        let ok = match *flag {
            "--model" => {
                def.model = value.to_string();
                true
            }
            "--views" => value.parse().map(|v| def.views = v).is_ok(),
            "--pitch" => num(value).map(|v| def.pitch = v).is_some(),
            "--tile" => value.parse().map(|v| def.tile = v).is_ok(),
            "--n64-tile" => value.parse().map(|v| def.n64_tile = Some(v)).is_ok(),
            _ => {
                eprintln!("xtask: unknown flag `{flag}`\n{USAGE}");
                return ExitCode::FAILURE;
            }
        };
        if !ok {
            eprintln!("xtask: bad value `{value}` for {flag}");
            return ExitCode::FAILURE;
        }
    }
    if def.model.is_empty() {
        eprintln!("xtask: impostor new needs --model models/<name>\n{USAGE}");
        return ExitCode::FAILURE;
    }
    match impostor::save_def(assets, name, &def) {
        Ok(path) => println!("wrote {}", path.display()),
        Err(e) => {
            eprintln!("xtask: {e}");
            return ExitCode::FAILURE;
        }
    }
    bake(assets, &[name])
}

fn bake(assets: &Path, names: &[&str]) -> ExitCode {
    let mut failed = false;
    for name in names {
        match impostor::bake(assets, name) {
            Ok(r) => {
                let i = &r.pc.impostor;
                println!(
                    "baked impostor `{name}`: {} views, {}x{} atlas ({} px tiles), \
                     N64 {}x{} CI4 ({} px tiles), quad {:.2} m",
                    i.views,
                    r.pc.width,
                    r.pc.height,
                    i.tile,
                    r.n64.width,
                    r.n64.height,
                    r.n64.impostor.tile,
                    i.size
                );
                for f in &r.written {
                    println!("  wrote assets/{f}");
                }
                if r.manifest_changed {
                    println!(
                        "  manifest: [sprites.impostor-{name}] + [blobs.impostor-{name}] — \
                         use it in a scene LOD level: impostor: Some(\"impostors/{name}\")"
                    );
                }
            }
            Err(e) => {
                eprintln!("error: impostor `{name}`: {e}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn check(assets: &Path, names: &[&str]) -> ExitCode {
    let all = impostor::names(assets);
    let names: Vec<&str> = if names.is_empty() {
        all.iter().map(String::as_str).collect()
    } else {
        names.to_vec()
    };
    let mut failed = false;
    for name in &names {
        match impostor::check(assets, name) {
            Ok(()) => println!("ok  {name}"),
            Err(e) => {
                eprintln!("error: {e}");
                failed = true;
            }
        }
    }
    println!("{} impostor(s)", names.len());
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn list(assets: &Path, json: bool) -> ExitCode {
    let mut rows = Vec::new();
    for name in impostor::names(assets) {
        let def = impostor::load_def(assets, &name);
        let state = match impostor::check(assets, &name) {
            Ok(()) => "fresh".to_string(),
            Err(e) if e.contains("not baked") => "not baked".to_string(),
            Err(e) if e.contains("stale") => "stale".to_string(),
            Err(e) => format!("error: {e}"),
        };
        rows.push((name, def, state));
    }
    if json {
        let items: Vec<String> = rows
            .iter()
            .map(|(name, def, state)| match def {
                Ok(d) => format!(
                    "{{\"name\":{},\"model\":{},\"views\":{},\"pitch\":{},\"tile\":{},\
                     \"n64_tile\":{},\"state\":{}}}",
                    q(name),
                    q(&d.model),
                    d.views,
                    d.pitch,
                    d.tile,
                    d.n64_tile
                        .unwrap_or_else(|| impostor::n64_tile(d.views, d.tile)),
                    q(state)
                ),
                Err(e) => format!("{{\"name\":{},\"error\":{}}}", q(name), q(e)),
            })
            .collect();
        println!("[{}]", items.join(","));
    } else {
        for (name, def, state) in &rows {
            match def {
                Ok(d) => println!(
                    "{name}: {} · {} views · pitch {}° · {} px (N64 {} px) · {state}",
                    d.model,
                    d.views,
                    d.pitch,
                    d.tile,
                    d.n64_tile
                        .unwrap_or_else(|| impostor::n64_tile(d.views, d.tile))
                ),
                Err(e) => println!("{name}: {e}"),
            }
        }
        println!("{} impostor(s)", rows.len());
    }
    ExitCode::SUCCESS
}

fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
