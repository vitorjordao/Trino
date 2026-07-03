//! `cargo xtask <command> [platform] [args...]`
//!
//! The single entry point for building, running and testing Trino on every
//! target. Wraps cargo, Docker (N64) and emulators so contributors and CI
//! never memorize per-platform incantations.
//!
//! Fase 0 implements the PC paths; console platforms land in Fases 4/5 and
//! currently exit with a pointer to the roadmap.

use std::process::{Command, ExitCode};

const HELP: &str = "\
cargo xtask <command> [platform] [-- extra args]

commands:
  build <pc|n64|3ds>   compile the app for a platform
  run   <pc|n64|3ds>   build and launch (emulator for consoles)
  test  [pc|n64|3ds] [--bless]
                       run the test suite (default: everything testable);
                       --bless regenerates golden images

  assets <platform>    bake assets for a platform        (Fase 2)
  watch <platform>     watch + rebuild + live reload      (Fase 2)
  new <name>           scaffold a new game project        (Fase 8)
";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let bless = args.iter().any(|a| a == "--bless");
    args.retain(|a| a != "--bless");
    let mut it = args.iter().map(String::as_str);

    match (it.next(), it.next()) {
        (Some("build"), Some("pc")) => cargo(&["build", "-p", "trino-app-pc"], &[]),
        (Some("run"), Some("pc")) => {
            let extra: Vec<&str> = it.collect();
            let mut cmd = vec!["run", "-p", "trino-app-pc"];
            cmd.extend(extra);
            cargo(&cmd, &[])
        }
        (Some("test"), None | Some("pc")) => {
            let envs: &[(&str, &str)] = if bless { &[("TRINO_BLESS", "1")] } else { &[] };
            cargo(&["test", "--workspace"], envs)
        }

        (Some("build" | "run" | "test"), Some(p @ ("n64" | "3ds"))) => {
            not_yet(p, if p == "n64" { "Fase 4" } else { "Fase 5" })
        }
        (Some("assets" | "watch"), _) => not_yet("assets/watch", "Fase 2"),
        (Some("new"), _) => not_yet("new", "Fase 8"),

        (Some("help") | None, _) => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        (Some(cmd), platform) => {
            eprintln!(
                "xtask: unknown invocation `{cmd}{}`\n\n{HELP}",
                platform.map(|p| format!(" {p}")).unwrap_or_default()
            );
            ExitCode::FAILURE
        }
    }
}

fn cargo(args: &[&str], envs: &[(&str, &str)]) -> ExitCode {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args(args)
        .envs(envs.iter().copied())
        .status();
    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(s) => ExitCode::from(s.code().unwrap_or(1).min(255) as u8),
        Err(e) => {
            eprintln!("xtask: failed to spawn cargo: {e}");
            ExitCode::FAILURE
        }
    }
}

fn not_yet(what: &str, phase: &str) -> ExitCode {
    eprintln!("xtask: `{what}` arrives in {phase} — see PLANO_EXECUCAO_TRINO.md");
    ExitCode::FAILURE
}
