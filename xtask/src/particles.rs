//! `cargo xtask particles <new|check|inspect>` — the text-first side of
//! particle effects (`trino_asset_pipeline::particles`). The editor's
//! particle inspector writes the same files and runs the same simulation.
//!
//! - `particles new <name> --sprite sprites/x` writes
//!   `assets/shared/particles/<name>.particles.ron` and declares it.
//! - `particles check` parses and validates every declared effect.
//! - `particles inspect <name> [--seconds S] [--seed N] [--json]` runs it
//!   deterministically on each platform: live and peak particles, peak
//!   triangles and estimated N64 time, and a position hash (the same seed
//!   gives the same particles on every target).

use std::path::Path;
use std::process::ExitCode;

use trino_asset_pipeline::Platform;
use trino_asset_pipeline::particles::{self, ParticlesDef};

const USAGE: &str = "xtask: usage:
  cargo xtask particles new <name> --sprite sprites/x
  cargo xtask particles check
  cargo xtask particles inspect <name> [--seconds S] [--seed N] [--json]";

pub fn run(root: &Path, args: &[&str]) -> ExitCode {
    let assets = root.join("assets");
    match args {
        ["new", name, "--sprite", sprite] => {
            match particles::save(&assets, name, &ParticlesDef::new(*sprite)) {
                Ok(declared) => {
                    println!(
                        "wrote {}{}",
                        particles::def_file(&assets, name).display(),
                        if declared {
                            format!(" — declared [particles.{name}]")
                        } else {
                            String::new()
                        }
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => fail(&e),
            }
        }
        ["check"] => check(&assets),
        ["inspect", name, rest @ ..] => inspect(&assets, name, rest),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn fail(e: &str) -> ExitCode {
    eprintln!("error: {e}");
    ExitCode::FAILURE
}

/// An effect's name and its settings (or why they don't parse).
type Declared = (String, Result<ParticlesDef, String>);

/// Every declared effect's settings — what `check` validates.
fn declared(assets: &Path) -> Result<Vec<Declared>, String> {
    let manifest = trino_asset_pipeline::Manifest::load(&assets.join("manifest.toml"))?;
    Ok(manifest
        .particles
        .iter()
        .map(|(name, decl)| {
            let def = trino_asset_pipeline::resolve_source(assets, Platform::Pc, &decl.file)
                .and_then(|p| {
                    std::fs::read_to_string(&p)
                        .map_err(|e| format!("{}: {e}", p.display()))
                        .and_then(|t| {
                            ParticlesDef::from_ron(&t).map_err(|e| format!("{}: {e}", p.display()))
                        })
                });
            (name.clone(), def)
        })
        .collect())
}

fn check(assets: &Path) -> ExitCode {
    let list = match declared(assets) {
        Ok(l) => l,
        Err(e) => return fail(&e),
    };
    let mut failed = false;
    for (name, def) in &list {
        match def {
            Ok(d) => println!(
                "ok  {name}: {} · max PC {} / N64 {} / 3DS {}",
                d.sprite,
                d.cap(Platform::Pc),
                d.cap(Platform::N64),
                d.cap(Platform::N3ds)
            ),
            Err(e) => {
                eprintln!("error: {name}: {e}");
                failed = true;
            }
        }
    }
    println!("{} effect(s)", list.len());
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn inspect(assets: &Path, name: &str, rest: &[&str]) -> ExitCode {
    let (mut seconds, mut seed, mut json) = (3.0f32, 1u32, false);
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        match *flag {
            "--json" => json = true,
            "--seconds" | "--seed" => {
                let Some(v) = it.next() else {
                    eprintln!("xtask: {flag} needs a value\n{USAGE}");
                    return ExitCode::FAILURE;
                };
                let ok = if *flag == "--seconds" {
                    v.parse().map(|x| seconds = x).is_ok()
                } else {
                    v.parse().map(|x| seed = x).is_ok()
                };
                if !ok {
                    eprintln!("xtask: bad value `{v}` for {flag}");
                    return ExitCode::FAILURE;
                }
            }
            other => {
                eprintln!("xtask: unknown flag `{other}`\n{USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }
    let list = match declared(assets) {
        Ok(l) => l,
        Err(e) => return fail(&e),
    };
    let Some((_, def)) = list.into_iter().find(|(n, _)| n == name) else {
        return fail(&format!("no [particles.{name}] in the manifest"));
    };
    let def = match def {
        Ok(d) => d,
        Err(e) => return fail(&e),
    };
    let reports: Vec<particles::SimReport> = Platform::ALL
        .iter()
        .map(|p| particles::simulate(&def, *p, seconds, seed))
        .collect();
    if json {
        let rows: Vec<String> = reports
            .iter()
            .map(|r| {
                format!(
                    "{{\"platform\":\"{}\",\"cap\":{},\"live\":{},\"peak\":{},\"peak_tris\":{},\"peak_n64_us\":{},\"hash\":{}}}",
                    r.platform.key(), r.cap, r.live, r.peak, r.peak_tris, r.peak_n64_us, r.hash
                )
            })
            .collect();
        println!(
            "{{\"name\":\"{name}\",\"seconds\":{seconds},\"seed\":{seed},\"platforms\":[{}]}}",
            rows.join(",")
        );
    } else {
        println!("{name}: {} for {seconds} s, seed {seed}", def.sprite);
        for r in &reports {
            println!(
                "  {:4} cap {:3}  live {:3}  peak {:3}  {:4} tris  N64 ≈{:.2} ms  hash {:08x}",
                r.platform.key(),
                r.cap,
                r.live,
                r.peak,
                r.peak_tris,
                r.peak_n64_us as f32 / 1000.0,
                r.hash
            );
        }
    }
    ExitCode::SUCCESS
}
