//! `cargo xtask <command> [platform] [args...]`
//!
//! The single entry point for building, running and testing Trino on every
//! target. Wraps cargo, the asset pipeline, Docker (N64) and emulators so
//! contributors and CI never memorize per-platform incantations.

mod agents;
mod assets_cmd;
mod audio;
mod blender;
mod castle64_assets;
mod console_groups;
mod emulators;
mod fbgolden;
mod font_assets;
mod hardware;
mod hwtier;
mod impostor;
mod model;
mod n3ds;
mod n64;
mod particles;
mod perf;
mod project;
mod quality;
mod river_assets;
mod scene;
mod showcase_assets;
mod standalone;
mod texture;
mod web;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use project::Target;
use trino_asset_pipeline::{Platform, bake_all};

const HELP: &str = "\
cargo xtask <command> [platform] [-- extra args]

commands:
  build <pc|n64|3ds|web> compile the app for a platform (--selftest: the
                       self-test build `test 3ds --no-build` boots)
  build [all]          every platform the project targets, in turn
  [--project <dir>]    package an external project (its trino.toml; also
                       $TRINO_PROJECT or a trino.toml in the current
                       directory) instead of the engine's demo. Its
                       [project] platforms = [\"pc\", \"n64\", \"3ds\", \"web\"]
                       (default: pc, n64, 3ds) gates every platform command:
                       batch commands skip the others, naming one fails
  build web [--release] [--out <dir>]
                       the browser build (issue #47): wasm + wasm-bindgen +
                       KTX2 asset packs -> target/web/{index.html, trino.js,
                       trino.wasm, assets/} (--out: an export folder)
  run web [--release] [--port N] [--query \"?game=solid-eleven\"]
                       serve target/web on 127.0.0.1 (COOP/COEP headers) and
                       open the browser (TRINO_NO_BROWSER=1: serve only)
  test web [--headed] [--browsers chromium,firefox,webkit] [--no-build]
                       the browser self-test in each browser (Playwright,
                       tools/web-test): TRINO_TEST_PASS, KTX2, saves and the
                       golden frames in tests/golden/web/ (--bless rewrites)
  run   <pc|n64|3ds|web> build and launch (emulator for consoles, a
                       local server + the browser for web)
  run pc --quality <low|medium|high|ultra|auto>
                       the native profile at that quality preset for this
                       run (TRINO_QUALITY; auto calibrates; never saved)
  run n64 --device sc64|everdrive
  run 3ds --device network [--ip <IP>] [--gdb [--gdb-port N]]
                       real hardware instead of the emulator (flashcart
                       over USB / 3dslink): deploy, then print the game's
                       debug log here; --no-build deploys target/'s file,
                       --selftest the self-test build; `run --target n64`
                       also works (docs/HARDWARE.md)
  run|test|watch 3ds --accurate-timing
                       Azahar with \"Simulate 3DS GPU timings\" on for this
                       session (also TRINO_AZAHAR_GPU_TIMING=1); your own
                       setting comes back when Azahar exits
  run|test|watch n64 --expansion-pak on|off   (test: also both)
                       ares with or without the Expansion Pak (test: on by
                       default; it checks the ROM's TRINO_HW line, 8M/4M)
  run|test|watch 3ds --new3ds on|off          (test: also both)
                       Azahar as a New 3DS or an old one (test: New by
                       default); your own setting comes back when it exits
  build|run|test|watch 3ds --speedup on|off
                       the New 3DS's 804 MHz mode for this build, over
                       trino.toml's [3ds] speedup (default on); off keeps a
                       New 3DS on the base tier
  test  [pc|n64|3ds|web] [--bless] [--no-build]
                       run the test suite (default: the workspace's host
                       tests); --bless regenerates golden images
  test all             the host suite, then each targeted console's
  assets <pc|n64|3ds|web> bake assets into target/assets/<platform>
  assets [all]         bake for every platform the project targets
  assets report [--platform n64|3ds|pc|web] [--tier base|expanded] [--json]
                       RAM per console tier: what is left for assets
                       ([memory]), each group's bytes loaded and baked,
                       boot / boot + group / each scene's groups (fails
                       when one does not fit, like strict mode)
  assets check [--json]
                       scenes' `requires` and cell groups are declared, what
                       they draw is in boot or one of them, and they fit
  assets group <sprites/x|models/x|..> <group[,group..]|boot> [--json]
                       put an asset in groups (declaring new ones)
  watch <pc|n64>       live-reload session (pc: dylib hot swap; n64: rebuild
                       ROM + relaunch ares). pc: --game <crate> picks the
                       game dylib to rebuild (default: castle64)
  editor [<dir>]       launch the Trino editor: the project launcher
                       (recent projects, open, create from a template), or
                       straight into the project at <dir> / --project
  emulators setup ares|azahar
                       provision the self-test emulators (ares Docker image
                       / pinned Azahar AppImage into target/emulators)
  new <name>           scaffold a new game crate under examples/
  new <name> --standalone <dir> [--git <url> [--rev <r>|--branch <b>]]
      [--template blank|castle64|showcase|football]
                       a game project outside this repo (own workspace,
                       PC host, assets); engine via path (default) or git;
                       the game starts blank or as a copy of a demo
  new                  the same, asking for name, folder and template
  gen-assets           regenerate the sample master assets (dev utility)
  scene check [file..] validate scenes/prefabs/materials (text-first loop)
  scene export [--platform pc|n64|3ds] <scene.ron> <out.rs>
                       bake a scene into no_std Rust data for games
  impostor new <name> --model models/x [--views N] [--pitch DEG] [--tile PX]
  impostor bake <name>.. | --all
                       pre-render a model from N yaws into a billboard atlas
                       (PC/3DS RGBA + N64 CI4 within TMEM) + its TIMP table
  particles new <name> --sprite sprites/x | check
  particles inspect <name> [--seconds S] [--seed N] [--json]
                       particle effects as text: validate, and simulate each
                       platform's cap (peak tris, N64 time, position hash)
  impostor check [name..] | list [--json]
                       spot missing or stale impostors (model changed)
  texture inspect <sprites/x|fonts/x> [--platform pc|n64|3ds] [--json]
                       format and why (auto's candidates), bytes, PSNR vs the
                       master and TMEM / 3DS texture-memory share per platform
  texture report [--platform pc|n64|3ds] [--json]
                       every texture, biggest first, with totals vs RGBA8;
                       fails when a bake would (min_psnr)
  audio inspect <sounds/x|music/x> [--platform pc|n64|3ds|web] [--json]
                       codec and why, bytes vs PCM16, SNR vs the master, RAM
                       resident/playing and who decodes, per platform
  audio report [--platform ..] [--json]
                       every sound and track, biggest first, with totals
  audio preview <sounds/x|music/x> [--platform ..] [--out <dir>] [--json]
                       each platform's decoded result as a WAV (+ the master)
  audio mix [--json]   validate assets/audio.toml and print the mix
  quality show [--json] | check
                       the project's quality presets (trino.toml [pc.quality],
                       assets/quality.ron over the built-in Low..Ultra)
  quality set <preset> <key>=<value>.. [--json] | reset [<preset>..]
                       change (or drop) a preset's values in assets/quality.ron
  quality bench [--json]
                       the first launch's calibration on this machine: each
                       preset's frame time on the bench scene, and the pick
  model inspect <file> [--fps N] [--once a,b] [--anim] [--json]
                       bake a .glb/.gltf/.prim.ron like the engine and report
                       tris, joints, clips, bounds, N64 budget (art gate)
  blender smoke [--no-render]
                       run the Blender toolkit end to end (prop + rigid
                       character -> export -> model inspect gates); skips
                       when Blender is not installed
  blender castle64 | solid-eleven [--only a,b] [--no-render]
                       rebuild a demo's Blender-authored models with their
                       triangle gates (castle64: doors, player parts, boar;
                       solid-eleven: pitch, stadium, players, rig + clips)
  agents <sync|check>  mirror .agents/skills/ into tool-specific folders
                       (.claude/skills/) / verify the mirrors are in sync
";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let bless = args.iter().any(|a| a == "--bless");
    args.retain(|a| a != "--bless");
    // Console CI split: build the self-test app in the toolchain container,
    // boot it elsewhere (`test 3ds --no-build`).
    let selftest = args.iter().any(|a| a == "--selftest");
    args.retain(|a| a != "--selftest");
    let no_build = args.iter().any(|a| a == "--no-build");
    args.retain(|a| a != "--no-build");
    // Azahar's "Simulate 3DS GPU timings" for this session (or
    // TRINO_AZAHAR_GPU_TIMING=1).
    let accurate_flag = args.iter().any(|a| a == "--accurate-timing");
    args.retain(|a| a != "--accurate-timing");
    let accurate = n3ds::accurate_timing(accurate_flag);
    // The consoles' hardware tiers (issue #54): `--expansion-pak`,
    // `--new3ds`, `--speedup`.
    let hw = match hwtier::parse(&mut args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    // `watch pc --game <crate>`: which game crate feeds the hot-reload dylib.
    let game = match args.iter().position(|a| a == "--game") {
        Some(i) => {
            if i + 1 >= args.len() {
                eprintln!("xtask: --game needs a crate name (e.g. --game platformer)");
                return ExitCode::FAILURE;
            }
            let name = args.remove(i + 1);
            args.remove(i);
            name
        }
        // Must match the game crate apps/pc links (the 3D showcase).
        None => "castle64".into(),
    };
    // Console builds package the engine's demo or an external project
    // (`--project <dir>`, $TRINO_PROJECT, or a trino.toml in the cwd).
    let mut project = match project::Project::select(&mut args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(on) = hw.speedup {
        project.n3ds_speedup = on;
    }
    let project = project;
    // Real hardware (`run n64 --device sc64`, `run --target n64 ...`), parsed
    // after `--project <dir>` is gone so it may come first.
    let device = match hardware::parse(&mut args) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(d) = &device
        && let Err(e) = hardware::check_invocation(
            args.first().map(String::as_str),
            args.get(1).map(String::as_str),
            d,
        )
    {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    if accurate_flag
        && let Err(e) = n3ds::check_accurate_timing(
            args.first().map(String::as_str),
            args.get(1).map(String::as_str),
            device.is_some(),
        )
    {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    if let Err(e) = hwtier::check(
        &hw,
        args.first().map(String::as_str),
        args.get(1).map(String::as_str),
    ) {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    if device.is_some() && hw.expansion_pak.is_some() {
        eprintln!("xtask: --expansion-pak sets ares's; real hardware has the Pak it has");
        return ExitCode::FAILURE;
    }
    if device.is_some() && hw.new3ds.is_some() {
        eprintln!("xtask: --new3ds sets Azahar's; real hardware is the console it is");
        return ExitCode::FAILURE;
    }
    if device.is_some() && selftest && no_build {
        eprintln!(
            "xtask: --selftest builds the self-test app, --no-build sends what is in \
             target/ — use one (`build n64|3ds --selftest` first, then --no-build)"
        );
        return ExitCode::FAILURE;
    }
    // `[project] platforms` in trino.toml: a platform command on a platform
    // the project leaves out fails here, before any toolchain runs (after
    // `hardware::parse`, so `run --target n64` is gated too).
    if let Err(e) = gate(&project, &args) {
        eprintln!("xtask: {e}");
        return ExitCode::FAILURE;
    }
    // The Web's options (`build|run|test web`).
    let web_build = web::BuildOptions {
        release: web::take_flag(&mut args, "--release"),
        out: match web::take_value(&mut args, "--out") {
            Ok(v) => v.map(std::path::PathBuf::from),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        },
    };
    let web_port: u16 = match web::take_value(&mut args, "--port") {
        Ok(Some(p)) => match p.parse() {
            Ok(p) => p,
            Err(_) => {
                eprintln!("xtask: --port needs a number");
                return ExitCode::FAILURE;
            }
        },
        Ok(None) => 8080,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let web_query = web::take_value(&mut args, "--query")
        .ok()
        .flatten()
        .unwrap_or_default();
    let web_test = web::TestOptions {
        build: !no_build,
        bless,
        headed: web::take_flag(&mut args, "--headed"),
        browsers: web::take_value(&mut args, "--browsers")
            .ok()
            .flatten()
            .unwrap_or_else(|| "chromium,firefox,webkit".into())
            .split(',')
            .map(String::from)
            .collect(),
    };
    // `run pc --quality <preset|auto>` (issue #55): TRINO_QUALITY for the run.
    let pc_quality = match web::take_value(&mut args, "--quality") {
        Ok(Some(q)) => {
            if quality_choice(&q).is_none() {
                eprintln!("xtask: --quality needs low, medium, high, ultra or auto");
                return ExitCode::FAILURE;
            }
            if args.first().map(String::as_str) != Some("run")
                || args.get(1).map(String::as_str) != Some("pc")
            {
                eprintln!("xtask: --quality goes with `run pc`");
                return ExitCode::FAILURE;
            }
            Some(q.to_ascii_lowercase())
        }
        Ok(None) => None,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let mut it = args.iter().map(String::as_str);

    match (it.next(), it.next()) {
        (Some("build"), Some("pc")) => build_pc(&project, "build", &[]),
        (Some("build"), Some("web")) => web::build(&project, &web_build),
        (Some("run"), Some("web")) => web::run(&project, &web_build, web_port, &web_query),
        (Some("test"), Some("web")) => web::test(&project, &web_test),
        (Some("assets"), Some("web")) => assets_for(&project, Target::Web),
        (Some("run"), Some("pc")) => {
            let extra: Vec<&str> = it.collect();
            let envs: Vec<(&str, &str)> = pc_quality
                .as_deref()
                .map(|q| ("TRINO_QUALITY", q))
                .into_iter()
                .collect();
            build_pc_with(&project, "run", &extra, &envs)
        }
        (Some("test"), None | Some("pc")) => test_pc(&project, bless),
        // Batch commands: every platform `[project] platforms` lists, in
        // order pc, n64, 3ds; the others are skipped with a line saying so.
        (Some("build"), None | Some("all")) => batch(&project, "build", |t| match t {
            Target::Pc => build_pc(&project, "build", &[]),
            Target::N64 => n64::build(&project, selftest),
            Target::N3ds => n3ds::build(&project, selftest),
            Target::Web => web::build(&project, &web_build),
        }),
        (Some("assets"), None | Some("all")) => {
            batch(&project, "assets", |t| assets_for(&project, t))
        }
        (Some("test"), Some("all")) => batch(&project, "test", |t| match t {
            Target::Pc => test_pc(&project, bless),
            Target::N64 => n64::test(&project, bless, hw.pak_tests()),
            Target::N3ds => n3ds::test(&project, !no_build, bless, accurate, hw.new3ds_tests()),
            Target::Web => web::test(&project, &web_test),
        }),
        (Some("assets"), Some("report" | "check" | "group")) => {
            let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
            assets_cmd::run(&project.root, &rest)
        }
        (Some("assets"), Some("n64")) => assets_for(&project, Target::N64),
        (Some("assets"), Some("3ds" | "n3ds")) => assets_for(&project, Target::N3ds),
        (Some("assets"), Some(platform)) => match Platform::parse(platform) {
            Some(Platform::Pc) => assets_for(&project, Target::Pc),
            Some(p) => assets(&project.root, p),
            None => {
                eprintln!("xtask: unknown platform `{platform}`");
                ExitCode::FAILURE
            }
        },
        (Some("watch"), Some("pc")) => watch_pc(&game),
        (Some("editor"), _) => editor(&project, args.get(1).map(String::as_str)),
        (Some("gen-assets"), _) => gen_assets(),
        (Some("agents"), sub) => agents::run(&repo_root(), sub),
        (Some("emulators"), _) => {
            let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
            emulators::run(&repo_root(), &rest)
        }
        (Some("blender"), _) => {
            let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
            blender::run(&repo_root(), &rest)
        }
        (Some("model"), _) => {
            let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
            // `model pose` writes into the selected project's assets.
            model::run(&project.root, &rest)
        }
        (Some("scene"), _) => {
            let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
            scene::run(&repo_root(), &rest)
        }
        (Some("particles"), _) => {
            let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
            particles::run(&project.root, &rest)
        }
        (Some("texture"), _) => {
            let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
            texture::run(&project.root, &rest)
        }
        (Some("quality"), _) => {
            let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
            quality::run(&project, &rest)
        }
        (Some("audio"), _) => {
            let mut rest: Vec<String> = args.iter().skip(1).cloned().collect();
            // `--out` was taken with the Web's options above.
            if let Some(out) = &web_build.out {
                rest.extend(["--out".to_string(), out.display().to_string()]);
            }
            let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
            audio::run(&project.root, &rest)
        }
        (Some("impostor"), _) => {
            let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
            // An external project's own assets when one is selected.
            impostor::run(&project.root, &rest)
        }

        (Some("build"), Some("n64")) => n64::build(&project, selftest),
        (Some("run"), Some("n64")) => match &device {
            Some(d) => hardware::deploy_n64(&project, d, !no_build, selftest),
            None => n64::run(&project, &hw),
        },
        (Some("test"), Some("n64")) => n64::test(&project, bless, hw.pak_tests()),
        (Some("watch"), Some("n64")) => n64::watch(&project, &hw),

        (Some("build"), Some("3ds")) => n3ds::build(&project, selftest),
        (Some("run"), Some("3ds")) => match &device {
            Some(d) => hardware::deploy_3ds(&project, d, !no_build, selftest),
            None => n3ds::run(&project, accurate, &hw),
        },
        (Some("test"), Some("3ds")) => {
            n3ds::test(&project, !no_build, bless, accurate, hw.new3ds_tests())
        }
        (Some("watch"), Some("3ds")) => n3ds::watch(&project, accurate, &hw),
        (Some("new"), Some(name)) => {
            let rest: Vec<&str> = args.iter().skip(2).map(String::as_str).collect();
            new_game(name, &rest)
        }
        // No name: ask on stdin (name, folder, template). A closed or empty
        // stdin prints the usage, as before the prompts existed.
        (Some("new"), None) => {
            let root = repo_root();
            let stdin = std::io::stdin();
            match standalone::prompt(&mut stdin.lock(), &mut std::io::stdout(), &root) {
                Some((names, s)) => make_standalone(&root, &names, &s),
                None => {
                    eprintln!("\n{NEW_USAGE}");
                    ExitCode::FAILURE
                }
            }
        }

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

/// True when a debounced batch holds a real filesystem change. Reading a
/// file emits inotify *access* events; reacting to those turns every build
/// (which reads the sources) into the trigger for the next one.
pub fn real_change(events: &[notify_debouncer_full::DebouncedEvent]) -> bool {
    events.iter().any(|e| {
        !matches!(
            e.event.kind,
            notify_debouncer_full::notify::EventKind::Access(_)
        )
    })
}

fn repo_root() -> PathBuf {
    // xtask always runs from the workspace via `cargo xtask`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Fails when `args` (`<command> <platform> ...`) runs a platform command
/// on a platform the project's `[project] platforms` leaves out.
fn gate(project: &project::Project, args: &[String]) -> Result<(), String> {
    let (Some(cmd), Some(platform)) = (args.first(), args.get(1)) else {
        return Ok(());
    };
    if !matches!(cmd.as_str(), "build" | "run" | "test" | "watch" | "assets") {
        return Ok(());
    }
    match Target::parse(platform) {
        Some(t) => project.require(t),
        None => Ok(()),
    }
}

/// Run `one` on each platform of the project, skipping the platforms it
/// does not target; stops at the first failure.
fn batch(
    project: &project::Project,
    what: &str,
    mut one: impl FnMut(Target) -> ExitCode,
) -> ExitCode {
    for (t, runs) in project.batch() {
        if !runs {
            println!(
                "xtask {what}: skipping {} (not in [project] platforms)",
                t.key()
            );
            continue;
        }
        println!("xtask {what}: {}", t.key());
        let code = one(t);
        if code != ExitCode::SUCCESS {
            eprintln!("xtask {what}: {} failed", t.key());
            return code;
        }
    }
    ExitCode::SUCCESS
}

/// `assets <platform>` for the selected project (the engine's own
/// `assets/` when none is).
fn assets_for(project: &project::Project, t: Target) -> ExitCode {
    let (tag, made) = match t {
        Target::Pc => return assets(&project.root, Platform::Pc),
        Target::N64 => ("n64", n64::bake_assets(project, false)),
        Target::N3ds => ("3ds", n3ds::bake_assets(project, false)),
        Target::Web => return assets(&project.root, Platform::Web),
    };
    match made {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[{tag}] {e}");
            ExitCode::FAILURE
        }
    }
}

/// `build pc` / `run pc`: the engine's `trino-app-pc`, or an external
/// project's desktop host (`[pc] package`, default `<name>-pc`).
fn build_pc(project: &project::Project, verb: &str, extra: &[&str]) -> ExitCode {
    build_pc_with(project, verb, extra, &[])
}

/// [`build_pc`] with environment variables (`run pc --quality`).
fn build_pc_with(
    project: &project::Project,
    verb: &str,
    extra: &[&str],
    envs: &[(&str, &str)],
) -> ExitCode {
    let manifest = project.root.join("Cargo.toml");
    let manifest = manifest.to_string_lossy();
    let mut cmd = vec![verb];
    if !project.is_engine {
        cmd.extend(["--manifest-path", manifest.as_ref()]);
    }
    cmd.extend(["-p", project.pc_package.as_str()]);
    cmd.extend(extra);
    cargo(&cmd, envs)
}

/// `test` / `test pc`: the workspace suite (the project's own workspace
/// when one is selected).
fn test_pc(project: &project::Project, bless: bool) -> ExitCode {
    let envs: &[(&str, &str)] = if bless { &[("TRINO_BLESS", "1")] } else { &[] };
    let manifest = project.root.join("Cargo.toml");
    let manifest = manifest.to_string_lossy();
    let mut cmd = vec!["test", "--workspace"];
    if !project.is_engine {
        cmd.extend(["--manifest-path", manifest.as_ref()]);
    }
    cargo(&cmd, envs)
}

/// `editor [<dir>]`: the editor's project launcher, or the project at
/// `<dir>` (else the selected external project). Runs from the engine's
/// manifest so `cargo trino editor` works inside a project too.
fn editor(project: &project::Project, dir: Option<&str>) -> ExitCode {
    let manifest = project.engine.join("Cargo.toml");
    let manifest = manifest.to_string_lossy();
    let mut cmd = vec![
        "run",
        "--manifest-path",
        manifest.as_ref(),
        "-p",
        "trino-editor",
    ];
    let root = project.root.to_string_lossy();
    match dir {
        Some(d) => cmd.extend(["--", d]),
        None if !project.is_engine => cmd.extend(["--", root.as_ref()]),
        None => {}
    }
    cargo(&cmd, &[])
}

fn assets(root: &Path, platform: Platform) -> ExitCode {
    let out = root.join("target/assets").join(platform.key());
    match bake_all(&root.join("assets"), platform, &out) {
        Ok(report) => {
            println!(
                "baked {} asset(s) for {} into {}",
                report.entries.len(),
                platform.key(),
                out.display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Live-reload session: launches the app with the `reload` feature (which
/// watches assets itself) and rebuilds the game dylib whenever game source
/// changes — hot-lib-reloader inside the app picks up the new library.
/// `game` is the crate that produces the dylib (`--game`, default
/// `platformer`) — it must match the crate `apps/pc` links and hot-loads.
///
/// Only `examples/` is watched on purpose: changes to `crates/core` or other
/// host-linked crates can change type layouts, and swapping a dylib across a
/// layout change is undefined behavior. Those changes require a restart.
fn watch_pc(game: &str) -> ExitCode {
    use std::sync::mpsc;
    use std::time::Duration;

    use notify_debouncer_full::notify::RecursiveMode;
    use notify_debouncer_full::{DebounceEventResult, new_debouncer};

    let root = repo_root();

    let mut app = match Command::new(cargo_bin())
        .args(["run", "-p", "trino-app-pc", "--features", "reload"])
        .current_dir(&root)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            eprintln!("xtask: failed to launch app: {e}");
            return ExitCode::FAILURE;
        }
    };

    let (tx, rx) = mpsc::channel();
    let mut debouncer = match new_debouncer(
        Duration::from_millis(200),
        None,
        move |result: DebounceEventResult| {
            if let Ok(events) = result
                && crate::real_change(&events)
            {
                let _ = tx.send(());
            }
        },
    ) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("xtask: failed to start watcher: {e}");
            let _ = app.kill();
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = debouncer.watch(root.join("examples"), RecursiveMode::Recursive) {
        eprintln!("xtask: failed to watch examples/: {e}");
        let _ = app.kill();
        return ExitCode::FAILURE;
    }

    println!(
        "xtask watch: editing examples/ rebuilds the `{game}` dylib; Ctrl+C or close the window to stop"
    );
    loop {
        // Stop when the app window is closed.
        if let Ok(Some(status)) = app.try_wait() {
            return if status.success() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
        if rx.recv_timeout(Duration::from_millis(300)).is_ok() {
            // Drain queued events, then rebuild once.
            while rx.try_recv().is_ok() {}
            println!("xtask watch: game source changed, rebuilding dylib...");
            let status = Command::new(cargo_bin())
                .args(["build", "-p", game])
                .current_dir(&root)
                .status();
            match status {
                Ok(s) if s.success() => println!("xtask watch: dylib rebuilt, hot reload incoming"),
                Ok(_) => eprintln!("xtask watch: build failed (fix and save again)"),
                Err(e) => eprintln!("xtask watch: cargo failed: {e}"),
            }
        }
    }
}

/// Regenerate the sample master assets (committed to the repo). Dev utility:
/// keeps binary fixtures reproducible from code.
fn gen_assets() -> ExitCode {
    gen_assets_into(&repo_root());
    println!("sample assets regenerated under assets/shared/");
    ExitCode::SUCCESS
}

/// Write every generated master under `root/assets/shared/` (the repo root
/// for `gen-assets`; a temp dir in tests).
fn gen_assets_into(root: &Path) {
    let sprites = root.join("assets/shared/sprites");
    let sounds = root.join("assets/shared/sounds");
    let music = root.join("assets/shared/music");
    std::fs::create_dir_all(&sprites).unwrap();
    std::fs::create_dir_all(&sounds).unwrap();
    std::fs::create_dir_all(&music).unwrap();

    // player.png: 32x32 red/white checkerboard (hello-sprite's sprite).
    let size = 32u32;
    let cell = size / 4;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let pick = ((x / cell) + (y / cell)).is_multiple_of(2);
            pixels.extend_from_slice(if pick {
                &[230, 70, 70, 255]
            } else {
                &[250, 250, 250, 255]
            });
        }
    }
    write_png(&sprites.join("player.png"), size, size, &pixels);

    gen_platformer_sprites(&sprites);
    gen_wav(&sounds.join("beep.wav"), &[(440.0, 0.15, Wave::Sine)], 0.25);
    gen_wav(
        &sounds.join("jump.wav"),
        &[
            (220.0, 0.05, Wave::SweepTo(660.0)),
            (660.0, 0.08, Wave::Square),
        ],
        0.2,
    );
    gen_wav(
        &sounds.join("coin.wav"),
        &[(988.0, 0.06, Wave::Square), (1319.0, 0.1, Wave::Square)],
        0.18,
    );
    gen_wav(
        &sounds.join("win.wav"),
        &[
            (523.0, 0.12, Wave::Square),
            (659.0, 0.12, Wave::Square),
            (784.0, 0.12, Wave::Square),
            (1047.0, 0.3, Wave::Square),
        ],
        0.2,
    );
    gen_theme(&music.join("theme.wav"));
    gen_water_loop(&sounds.join("water.wav"));

    let models = root.join("assets/shared/models");
    std::fs::create_dir_all(&models).unwrap();
    gen_cube_glb(&models.join("cube.glb"));

    // castle64 (the 3D showcase): blocks, articulated player, boar, doors.
    castle64_assets::gen_all(root, &|path, w, h, rgba| write_png(path, w, h, rgba));
    showcase_assets::gen_all(root, &|path, w, h, rgba| write_png(path, w, h, rgba));
    // The showcase's river: carved terrain, water ribbon, bridge, layers.
    river_assets::gen_all(root, &|path, w, h, rgba| write_png(path, w, h, rgba));
    // trino5x7: the default pixel font (bitmap text on every target).
    font_assets::gen_all(root, &|path, w, h, rgba| write_png(path, w, h, rgba));
}

/// glTF `COLOR_0` is linear: display (sRGB) color bytes -> little-endian
/// linear `f32`s (accessor `componentType` 5126). The bake converts back
/// to the exact same bytes (`asset-pipeline` `color0_to_display`).
pub(crate) fn color0_linear(display: &[u8]) -> Vec<u8> {
    let lin = |b: u8| {
        let c = b as f64 / 255.0;
        if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let mut out = Vec::with_capacity(display.len() * 4);
    for px in display.chunks(4) {
        for (k, &b) in px.iter().enumerate() {
            let v = if k == 3 { b as f64 / 255.0 } else { lin(b) };
            out.extend_from_slice(&(v as f32).to_le_bytes());
        }
    }
    out
}

/// cube.glb: a unit cube with per-face normals and colors — the 3D pipeline
/// master. Hand-assembled GLB (JSON + BIN chunks) so the sample stays
/// reproducible from code.
fn gen_cube_glb(path: &Path) {
    use trino_core::Vec3;

    let faces: [(Vec3, [u8; 4]); 6] = [
        (Vec3::new(0.0, 0.0, -1.0), [230, 80, 80, 255]),
        (Vec3::new(0.0, 0.0, 1.0), [80, 230, 80, 255]),
        (Vec3::new(-1.0, 0.0, 0.0), [80, 80, 230, 255]),
        (Vec3::new(1.0, 0.0, 0.0), [230, 230, 80, 255]),
        (Vec3::new(0.0, -1.0, 0.0), [230, 80, 230, 255]),
        (Vec3::new(0.0, 1.0, 0.0), [80, 230, 230, 255]),
    ];
    let mut positions: Vec<f32> = Vec::new();
    let mut normals: Vec<f32> = Vec::new();
    let mut colors: Vec<u8> = Vec::new();
    let mut indices: Vec<u16> = Vec::new();
    for (f, (n, c)) in faces.iter().enumerate() {
        let u = if n.y.abs() > 0.9 {
            Vec3::new(1.0, 0.0, 0.0)
        } else {
            Vec3::new(0.0, 1.0, 0.0).cross(*n).normalized()
        };
        let v = n.cross(u);
        let base = (f * 4) as u16;
        for (su, sv) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            let p = *n * 0.5 + u * su + v * sv;
            positions.extend_from_slice(&[p.x, p.y, p.z]);
            normals.extend_from_slice(&[n.x, n.y, n.z]);
            colors.extend_from_slice(c);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    let mut bin: Vec<u8> = Vec::new();
    for v in positions.iter().chain(normals.iter()) {
        bin.extend_from_slice(&v.to_le_bytes());
    }
    let colors = color0_linear(&colors);
    let colors_offset = bin.len();
    bin.extend_from_slice(&colors);
    let indices_offset = bin.len();
    for i in &indices {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }

    let vcount = positions.len() / 3;
    let normals_offset = positions.len() * 4;
    let json = format!(
        r#"{{"asset":{{"version":"2.0","generator":"trino gen-assets"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0,"name":"cube"}}],"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1,"COLOR_0":2}},"indices":3}}]}}],"buffers":[{{"byteLength":{}}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":{}}},{{"buffer":0,"byteOffset":{normals_offset},"byteLength":{}}},{{"buffer":0,"byteOffset":{colors_offset},"byteLength":{}}},{{"buffer":0,"byteOffset":{indices_offset},"byteLength":{}}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":{vcount},"type":"VEC3","min":[-0.5,-0.5,-0.5],"max":[0.5,0.5,0.5]}},{{"bufferView":1,"componentType":5126,"count":{vcount},"type":"VEC3"}},{{"bufferView":2,"componentType":5126,"count":{vcount},"type":"VEC4"}},{{"bufferView":3,"componentType":5123,"count":{},"type":"SCALAR"}}]}}"#,
        bin.len(),
        positions.len() * 4,
        normals.len() * 4,
        colors.len(),
        indices.len() * 2,
        indices.len(),
    );
    let mut json_bytes = json.into_bytes();
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }

    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut glb: Vec<u8> = Vec::with_capacity(total);
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"JSON");
    glb.extend_from_slice(&json_bytes);
    glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"BIN\0");
    glb.extend_from_slice(&bin);
    std::fs::write(path, glb).unwrap();
}

/// 16x16 sprites for the platformer, drawn as readable ASCII art.
fn gen_platformer_sprites(dir: &Path) {
    let put = |name: &str, art: [&str; 16], palette: &[(u8, [u8; 4])]| {
        let mut rgba = Vec::with_capacity(16 * 16 * 4);
        for row in art {
            assert_eq!(row.len(), 16, "{name}: row width");
            for b in row.bytes() {
                let color = palette
                    .iter()
                    .find(|(k, _)| *k == b)
                    .map(|(_, c)| *c)
                    .unwrap_or([0, 0, 0, 0]); // '.' and unknown = transparent
                rgba.extend_from_slice(&color);
            }
        }
        write_png(&dir.join(name), 16, 16, &rgba);
    };

    put(
        "hero.png",
        [
            "................",
            "....HHHHHHH.....",
            "...HHHHHHHHH....",
            "...HSSSSSSSH....",
            "...SSKSSKSSS....",
            "...SSSSSSSSS....",
            "....SSSSSS......",
            "...BBBBBBBBB....",
            "..BBBYBBYBBBB...",
            "..SBBBBBBBBS....",
            "..SBBBBBBBBS....",
            "...BBBBBBBB.....",
            "...BBB..BBB.....",
            "...BBB..BBB.....",
            "..GGGG..GGGG....",
            "................",
        ],
        &[
            (b'H', [84, 50, 25, 255]),
            (b'S', [255, 205, 148, 255]),
            (b'K', [20, 20, 20, 255]),
            (b'B', [40, 80, 200, 255]),
            (b'Y', [250, 210, 60, 255]),
            (b'G', [90, 60, 30, 255]),
        ],
    );
    put(
        "ground.png",
        [
            "GGGGGGGGGGGGGGGG",
            "GgGGGGgGGGGGGgGG",
            "DDDDDDDDDDDDDDDD",
            "DDDdDDDDDDDdDDDD",
            "DDDDDDDdDDDDDDDD",
            "DdDDDDDDDDDDDdDD",
            "DDDDDDdDDDdDDDDD",
            "DDdDDDDDDDDDDDDD",
            "DDDDDdDDDDDDdDDD",
            "DDDDDDDDDdDDDDDD",
            "DdDDDdDDDDDDDDdD",
            "DDDDDDDDdDDDDDDD",
            "DDdDDDDDDDDdDDDD",
            "DDDDDdDDDDDDDDDD",
            "DDDDDDDDDdDDDDdD",
            "DdDDDDDdDDDDDDDD",
        ],
        &[
            (b'G', [106, 190, 48, 255]),
            (b'g', [140, 214, 80, 255]),
            (b'D', [143, 86, 59, 255]),
            (b'd', [102, 57, 49, 255]),
        ],
    );
    // brick.png: generated in code (regular pattern beats hand-typed art).
    let mut brick = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16u32 {
        for x in 0..16u32 {
            let mortar_h = y % 4 == 3;
            let offset = if (y / 4) % 2 == 0 { 0 } else { 4 };
            let mortar_v = (x + offset) % 8 == 7;
            let c: [u8; 4] = if mortar_h || mortar_v {
                [96, 44, 44, 255]
            } else {
                [172, 50, 50, 255]
            };
            brick.extend_from_slice(&c);
        }
    }
    write_png(&dir.join("brick.png"), 16, 16, &brick);

    // coin.png: circle with a rim and a highlight.
    let mut coin = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16i32 {
        for x in 0..16i32 {
            let dx = x - 8;
            let dy = y - 8;
            let d2 = dx * dx + dy * dy;
            let c: [u8; 4] = if d2 <= 25 {
                if dx <= -2 && dy <= -2 {
                    [255, 246, 160, 255] // highlight
                } else {
                    [252, 224, 40, 255]
                }
            } else if d2 <= 36 {
                [216, 140, 20, 255] // rim
            } else {
                [0, 0, 0, 0]
            };
            coin.extend_from_slice(&c);
        }
    }
    write_png(&dir.join("coin.png"), 16, 16, &coin);

    put(
        "flag.png",
        [
            "..M.............",
            "..MFFFFFFF......",
            "..MFFFFFFFFF....",
            "..MFFFFFFFFFFF..",
            "..MFFFFFFFFF....",
            "..MFFFFFFF......",
            "..M.............",
            "..M.............",
            "..M.............",
            "..M.............",
            "..M.............",
            "..M.............",
            "..M.............",
            "..M.............",
            ".MMM............",
            "................",
        ],
        &[(b'M', [120, 120, 130, 255]), (b'F', [60, 200, 80, 255])],
    );
}

enum Wave {
    Sine,
    Square,
    /// Linear frequency sweep from the note's own frequency to this one.
    SweepTo(f32),
}

/// Note sequence -> mono 16-bit wav at 44.1 kHz with a soft per-note decay.
fn gen_wav(path: &Path, notes: &[(f32, f32, Wave)], volume: f32) {
    let rate = 44_100u32;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for (freq, dur, wave) in notes {
        let frames = (rate as f32 * dur) as usize;
        let mut phase = 0.0f32;
        for i in 0..frames {
            let t = i as f32 / frames as f32;
            let f = match wave {
                Wave::SweepTo(to) => freq + (to - freq) * t,
                _ => *freq,
            };
            phase += f * std::f32::consts::TAU / rate as f32;
            let s = match wave {
                Wave::Sine => phase.sin(),
                _ => {
                    if phase.sin() >= 0.0 {
                        1.0
                    } else {
                        -1.0
                    }
                }
            };
            let fade = 1.0 - t * t;
            writer
                .write_sample((s * volume * fade * i16::MAX as f32) as i16)
                .unwrap();
        }
    }
    writer.finalize().unwrap();
}

/// water.wav: a 2 s seamless water/stream loop for positional ambience —
/// lowpassed noise with slow swells and soft "bubble" chirps; the last
/// 0.25 s is crossfaded into the start so the loop point is inaudible.
fn gen_water_loop(path: &Path) {
    let rate = 22_050u32;
    let n = rate as usize * 2;
    let fade = rate as usize / 4;
    let mut seed = 0x1234_5678u32;
    let mut noise = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
    };
    let total = n + fade;
    let mut raw = Vec::with_capacity(total);
    let (mut lp1, mut lp2) = (0.0f32, 0.0f32);
    for i in 0..total {
        let t = i as f32 / rate as f32;
        lp1 += (noise() - lp1) * 0.08;
        lp2 += (lp1 - lp2) * 0.15;
        let swell = 0.7 + 0.3 * (t * std::f32::consts::TAU * 0.5).sin();
        // Bubbles: short damped sine chirps at fixed times.
        let mut bubble = 0.0;
        for &(at, f) in &[
            (0.31f32, 900.0f32),
            (0.87, 1300.0),
            (1.42, 1050.0),
            (1.77, 760.0),
        ] {
            let dt = t - at;
            if (0.0..0.06).contains(&dt) {
                bubble += (dt * f * (1.0 + dt * 8.0) * std::f32::consts::TAU).sin()
                    * (1.0 - dt / 0.06)
                    * 0.25;
            }
        }
        raw.push(lp2 * 2.2 * swell + bubble);
    }
    // Crossfade the tail over the head: sample i (< fade) mixes in i + n.
    let mut out = raw[..n].to_vec();
    for i in 0..fade {
        let k = i as f32 / fade as f32;
        out[i] = out[i] * k + raw[n + i] * (1.0 - k);
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for s in out {
        writer
            .write_sample((s.clamp(-1.0, 1.0) * 0.6 * i16::MAX as f32) as i16)
            .unwrap();
    }
    writer.finalize().unwrap();
}

/// theme.wav: a 3.2 s seamless-ish chiptune loop (square lead + pulse bass).
fn gen_theme(path: &Path) {
    let rate = 44_100u32;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    // (lead Hz, bass Hz) per 0.2 s step, 16 steps.
    let steps: [(f32, f32); 16] = [
        (262.0, 131.0),
        (330.0, 131.0),
        (392.0, 165.0),
        (330.0, 165.0),
        (440.0, 175.0),
        (392.0, 175.0),
        (330.0, 165.0),
        (294.0, 165.0),
        (262.0, 131.0),
        (330.0, 131.0),
        (392.0, 165.0),
        (523.0, 165.0),
        (440.0, 175.0),
        (494.0, 196.0),
        (523.0, 196.0),
        (392.0, 131.0),
    ];
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    let step_frames = (rate as f32 * 0.2) as usize;
    let mut lead_phase = 0.0f32;
    let mut bass_phase = 0.0f32;
    for (lead, bass) in steps {
        for i in 0..step_frames {
            let t = i as f32 / step_frames as f32;
            lead_phase += lead * std::f32::consts::TAU / rate as f32;
            bass_phase += bass * std::f32::consts::TAU / rate as f32;
            let l = if lead_phase.sin() >= 0.0 { 1.0 } else { -1.0 };
            // Narrow pulse for the bass (25% duty).
            let b = if (bass_phase / std::f32::consts::TAU).fract() < 0.25 {
                1.0
            } else {
                -1.0
            };
            let env = if t < 0.05 { t / 0.05 } else { 1.0 - t * 0.3 };
            let s = (l * 0.10 + b * 0.05) * env;
            writer.write_sample((s * i16::MAX as f32) as i16).unwrap();
        }
    }
    writer.finalize().unwrap();
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) {
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(rgba).unwrap();
}

fn cargo_bin() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

/// A preset name or `auto` (what `TRINO_QUALITY` takes).
fn quality_choice(s: &str) -> Option<()> {
    let s = s.trim();
    (s.eq_ignore_ascii_case("auto")
        || trino_core::QualityPreset::parse(s)
            .is_some_and(|p| p != trino_core::QualityPreset::Custom))
    .then_some(())
}

fn cargo(args: &[&str], envs: &[(&str, &str)]) -> ExitCode {
    let status = Command::new(cargo_bin())
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

/// Scaffold a new game crate from `templates/new-game/` into `examples/`.
/// The workspace member glob picks it up automatically; every generated
/// project ships its own AGENTS.md (AI-friendly by construction).
fn new_game(name: &str, rest: &[&str]) -> ExitCode {
    let root = repo_root();
    match standalone::parse_args(rest, &root) {
        Ok(Some(s)) => {
            return match Names::new(name) {
                Ok(names) => make_standalone(&root, &names, &s),
                Err(e) => {
                    eprintln!("xtask: {e}");
                    ExitCode::FAILURE
                }
            };
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("xtask: {e}");
            return ExitCode::FAILURE;
        }
    }
    match scaffold_game(&root, name) {
        Ok(dir) => {
            println!("created {}", dir.display());
            println!("\nnext steps:");
            println!("  cargo test -p {name}     # its unit tests join the workspace suite");
            println!(
                "  edit apps/* to launch {name} instead of the platformer (see its AGENTS.md)"
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

const NEW_USAGE: &str = "xtask: usage: cargo xtask new <kebab-case-name> \
     [--standalone <dir> [--git <url> [--rev <r> | --branch <b>]] \
     [--template blank|castle64|showcase|football]]";

/// Render a standalone project and print what to do next.
fn make_standalone(root: &Path, names: &Names, s: &standalone::Standalone) -> ExitCode {
    match standalone::scaffold(root, &s.dir, names, &s.source, s.template) {
        Ok(()) => {
            let dir = s.dir.display();
            println!(
                "created standalone project {dir} ({} template)",
                s.template.key()
            );
            println!("\nnext steps:");
            println!("  cd {dir}");
            println!(
                "  cargo run -p {}-pc   # play (TRINO_SIM=3ds for the 3DS look)",
                names.kebab
            );
            println!("  cargo test               # the game's unit tests");
            println!("  cargo trino editor       # open it in the editor");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

pub use trino_scaffold::Names;

/// Testable core of `new_game`: render the template into `<root>/examples/<name>`.
fn scaffold_game(root: &Path, name: &str) -> Result<PathBuf, String> {
    let names = Names::new(name)?;
    let dest = root.join("examples").join(name);
    if dest.exists() {
        return Err(format!("{} already exists", dest.display()));
    }
    copy_template(
        &root.join("templates/new-game"),
        &dest,
        &names.kebab,
        &names.snake,
        &names.camel,
    )?;
    Ok(dest)
}

use trino_scaffold::template_target_name;

fn copy_template(
    from: &Path,
    to: &Path,
    name: &str,
    snake: &str,
    camel: &str,
) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let src = entry.path();
        let dst = to.join(template_target_name(&entry.file_name()));
        if src.is_dir() {
            copy_template(&src, &dst, name, snake, camel)?;
        } else {
            let text =
                std::fs::read_to_string(&src).map_err(|e| format!("{}: {e}", src.display()))?;
            let rendered = text
                .replace("{{name}}", name)
                .replace("{{name_snake}}", snake)
                .replace("{{name_camel}}", camel);
            std::fs::write(&dst, rendered).map_err(|e| format!("{}: {e}", dst.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{copy_template, gen_assets_into, repo_root, scaffold_game};

    /// Every file under `dir`, relative to it.
    fn files(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files(&path, base, out);
            } else {
                out.push(path.strip_prefix(base).unwrap().to_path_buf());
            }
        }
    }

    fn png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
        let mut dec =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
        dec.set_transformations(png::Transformations::EXPAND);
        let mut reader = dec.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        (info.width, info.height, buf)
    }

    fn wav(path: &std::path::Path) -> (hound::WavSpec, Vec<i32>) {
        let mut r = hound::WavReader::open(path).unwrap();
        let spec = r.spec();
        (spec, r.samples::<i32>().map(Result::unwrap).collect())
    }

    /// Generated and committed masters agree: PNG pixels and WAV samples
    /// within one step, models bake to the same mesh (counts, indices,
    /// positions within 1e-4, colors within one step). Tolerant on purpose:
    /// `sin`/`powf` may round differently in another OS's libm.
    fn same_master(
        rel: &std::path::Path,
        generated: &std::path::Path,
        committed: &std::path::Path,
    ) {
        let ext = rel.extension().and_then(|e| e.to_str()).unwrap_or("");
        let close = |a: &[u8], b: &[u8]| {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.abs_diff(*y) <= 1)
        };
        match ext {
            "png" => {
                let (a, b) = (png(generated), png(committed));
                assert_eq!((a.0, a.1), (b.0, b.1), "{}: size", rel.display());
                assert!(close(&a.2, &b.2), "{}: pixels differ", rel.display());
            }
            "wav" => {
                let (a, b) = (wav(generated), wav(committed));
                assert_eq!(a.0, b.0, "{}: wav spec", rel.display());
                assert_eq!(a.1.len(), b.1.len(), "{}: sample count", rel.display());
                assert!(
                    a.1.iter().zip(&b.1).all(|(x, y)| (x - y).abs() <= 1),
                    "{}: samples differ",
                    rel.display()
                );
            }
            "glb" | "gltf" => {
                let opts = trino_asset_pipeline::ModelOptions::default();
                let ta = trino_asset_pipeline::bake_model(generated, &opts).unwrap();
                let tb = trino_asset_pipeline::bake_model(committed, &opts).unwrap();
                let (a, b) = (
                    trino_core::Mesh::from_tmdl(&ta).unwrap(),
                    trino_core::Mesh::from_tmdl(&tb).unwrap(),
                );
                let what = rel.display();
                assert_eq!(a.vertex_count, b.vertex_count, "{what}: vertices");
                assert_eq!(a.index_count, b.index_count, "{what}: indices");
                assert_eq!(a.joint_count(), b.joint_count(), "{what}: joints");
                assert_eq!(a.clip_count(), b.clip_count(), "{what}: clips");
                for i in 0..a.index_count {
                    assert_eq!(a.index(i), b.index(i), "{what}: index {i}");
                }
                for i in 0..a.vertex_count {
                    let (p, q) = (a.position(i), b.position(i));
                    let d = (p.x - q.x)
                        .abs()
                        .max((p.y - q.y).abs())
                        .max((p.z - q.z).abs());
                    assert!(d < 1e-4, "{what}: vertex {i} moved {d}");
                    let (c, k) = (a.color(i), b.color(i));
                    assert!(
                        close(&[c.r, c.g, c.b, c.a], &[k.r, k.g, k.b, k.a]),
                        "{what}: vertex {i} color"
                    );
                }
            }
            other => panic!("{}: no comparison for `.{other}` masters", rel.display()),
        }
    }

    /// `cargo xtask gen-assets` keeps the committed binary masters
    /// reproducible from code: generating them into a temp dir gives the
    /// same sprites, sounds, music, models and font the repo ships.
    #[test]
    fn gen_assets_reproduces_the_committed_masters() {
        let tmp = tempfile::tempdir().unwrap();
        gen_assets_into(tmp.path());
        let mut out = Vec::new();
        files(tmp.path(), tmp.path(), &mut out);
        out.sort();
        assert!(out.len() >= 30, "generated only {} files", out.len());
        for kind in ["sprites", "sounds", "music", "models", "fonts"] {
            assert!(
                out.iter()
                    .any(|r| r.starts_with(format!("assets/shared/{kind}"))),
                "nothing generated under {kind}/"
            );
        }
        for rel in &out {
            let committed = repo_root().join(rel);
            assert!(
                committed.is_file(),
                "{} is generated but not committed",
                rel.display()
            );
            same_master(rel, &tmp.path().join(rel), &committed);
        }
    }

    /// Watch loops rebuild on writes, creates and removes, never on the
    /// access events that reading the sources emits.
    #[test]
    fn only_real_filesystem_changes_trigger_a_rebuild() {
        use notify_debouncer_full::DebouncedEvent;
        use notify_debouncer_full::notify::event::{AccessKind, CreateKind, Event, EventKind};
        let ev = |kind| DebouncedEvent::new(Event::new(kind), std::time::Instant::now());
        let read = ev(EventKind::Access(AccessKind::Any));
        let made = ev(EventKind::Create(CreateKind::File));
        assert!(!super::real_change(&[]));
        assert!(!super::real_change(std::slice::from_ref(&read)));
        assert!(super::real_change(&[read, made]));
    }

    /// The platform gate stops platform commands on a platform the
    /// project leaves out, and nothing else.
    #[test]
    fn the_platform_gate_only_stops_left_out_platforms() {
        let mut p = crate::project::Project::engine();
        p.platforms = vec![crate::project::Target::Pc, crate::project::Target::N3ds];
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        for ok in [
            &["build", "pc"][..],
            &["run", "3ds"],
            &["test", "n3ds"],
            &["assets", "all"],
            &["build"],
            &["test"],
            &["scene", "check"],
            &["model", "inspect", "n64"],
            &["new", "n64"],
        ] {
            super::gate(&p, &args(ok)).unwrap_or_else(|e| panic!("{ok:?}: {e}"));
        }
        for cmd in ["build", "run", "test", "watch", "assets"] {
            let e = super::gate(&p, &args(&[cmd, "n64"])).unwrap_err();
            assert!(e.contains("does not target n64"), "{cmd}: {e}");
        }
    }

    #[test]
    fn scaffold_renders_placeholders() {
        let temp = std::env::temp_dir().join(format!("trino-scaffold-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        // Give the fake root a verbatim copy of the real template.
        copy_template(
            &repo_root().join("templates/new-game"),
            &temp.join("templates/new-game"),
            "{{name}}",
            "{{name_snake}}",
            "{{name_camel}}",
        )
        .unwrap();

        let dir = scaffold_game(&temp, "my-game").unwrap();
        let lib = std::fs::read_to_string(dir.join("src/lib.rs")).unwrap();
        assert!(lib.contains("MyGameGame"), "camel-case game struct");
        assert!(!lib.contains("{{"), "no placeholders left");
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("name = \"my-game\""));
        assert!(dir.join("AGENTS.md").exists());

        assert!(scaffold_game(&temp, "my-game").is_err(), "already exists");
        assert!(scaffold_game(&temp, "Bad_Name").is_err(), "invalid name");
        let _ = std::fs::remove_dir_all(&temp);
    }
}
