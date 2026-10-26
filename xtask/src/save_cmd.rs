//! `cargo xtask save inspect|corrupt` (issue #68): look inside a save the
//! way the engine reads it, and damage one on purpose to try a game's
//! recovery by hand.
//!
//! What a save file can be: `trino_asset_pipeline::saves`.

use std::path::PathBuf;
use std::process::ExitCode;

use trino_asset_pipeline::saves::{corrupt, inspect};

pub fn run(rest: &[&str]) -> ExitCode {
    let json = rest.contains(&"--json");
    let medium = rest.contains(&"--medium");
    let value = |flag: &str| {
        rest.iter()
            .position(|a| *a == flag)
            .and_then(|i| rest.get(i + 1))
            .copied()
    };
    let positional: Vec<&str> = {
        let mut v = Vec::new();
        let mut skip = false;
        for a in rest.iter().skip(1) {
            if skip {
                skip = false;
                continue;
            }
            if ["--mode", "--slot", "--at"].contains(a) {
                skip = true;
                continue;
            }
            if !a.starts_with("--") {
                v.push(*a);
            }
        }
        v
    };
    let Some(path) = positional.first().map(PathBuf::from) else {
        eprintln!("usage: cargo xtask save inspect <save> [--medium] [--json]");
        eprintln!(
            "       cargo xtask save corrupt <save> --mode flip|truncate [--slot newest|a|b|all] [--at <byte>] [--medium]"
        );
        return ExitCode::FAILURE;
    };
    let r = match rest.first().copied() {
        Some("inspect") => inspect(&path, medium).map(|report| {
            if json {
                println!("{}", report.json());
            } else {
                print!("{}", report.text());
            }
        }),
        Some("corrupt") => {
            let mode = value("--mode").unwrap_or("flip");
            let slot = value("--slot").unwrap_or("newest");
            let at = value("--at").and_then(|v| v.parse().ok());
            corrupt(&path, medium, mode, slot, at).map(|what| {
                if json {
                    println!(
                        "{{\"file\":{:?},\"done\":{:?}}}",
                        path.display().to_string(),
                        what
                    );
                } else {
                    println!("{what}");
                }
            })
        }
        _ => Err("save: inspect or corrupt".into()),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask save: {e}");
            ExitCode::FAILURE
        }
    }
}
