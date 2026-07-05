//! `cargo xtask <command> [platform] [args...]`
//!
//! The single entry point for building, running and testing Trino on every
//! target. Wraps cargo, the asset pipeline, Docker (N64) and emulators so
//! contributors and CI never memorize per-platform incantations.

mod n3ds;
mod n64;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use trino_asset_pipeline::{Platform, bake_all};

const HELP: &str = "\
cargo xtask <command> [platform] [-- extra args]

commands:
  build <pc|n64|3ds>   compile the app for a platform
  run   <pc|n64|3ds>   build and launch (emulator for consoles)
  test  [pc|n64|3ds] [--bless]
                       run the test suite (default: everything testable);
                       --bless regenerates golden images
  assets <pc|n64|3ds>  bake assets into target/assets/<platform>
  watch <pc|n64>       live-reload session (pc: dylib hot swap; n64: rebuild
                       ROM + relaunch ares)
  editor               launch the Trino editor
  new <name>           scaffold a new game project        (Fase 8)
  gen-assets           regenerate the sample master assets (dev utility)
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
        (Some("assets"), Some("n64")) => match n64::bake_assets(&n64::repo_root(), false) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("[n64] {e}");
                ExitCode::FAILURE
            }
        },
        (Some("assets"), Some("3ds" | "n3ds")) => {
            match n3ds::bake_assets(&n3ds::repo_root(), false) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("[3ds] {e}");
                    ExitCode::FAILURE
                }
            }
        }
        (Some("assets"), Some(platform)) => match Platform::parse(platform) {
            Some(p) => assets(p),
            None => {
                eprintln!("xtask: unknown platform `{platform}`");
                ExitCode::FAILURE
            }
        },
        (Some("watch"), Some("pc")) => watch_pc(),
        (Some("editor"), _) => cargo(&["run", "-p", "trino-editor"], &[]),
        (Some("gen-assets"), _) => gen_assets(),

        (Some("build"), Some("n64")) => n64::build(false),
        (Some("run"), Some("n64")) => n64::run(),
        (Some("test"), Some("n64")) => n64::test(),
        (Some("watch"), Some("n64")) => n64::watch(),

        (Some("build"), Some("3ds")) => n3ds::build(false),
        (Some("run"), Some("3ds")) => n3ds::run(),
        (Some("test"), Some("3ds")) => n3ds::test(),
        (Some("watch"), Some("3ds")) => n3ds::watch(),
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

fn repo_root() -> PathBuf {
    // xtask always runs from the workspace via `cargo xtask`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn assets(platform: Platform) -> ExitCode {
    let root = repo_root();
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
///
/// Only `examples/` is watched on purpose: changes to `crates/core` or other
/// host-linked crates can change type layouts, and swapping a dylib across a
/// layout change is undefined behavior. Those changes require a restart.
fn watch_pc() -> ExitCode {
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
                && !events.is_empty()
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
        "xtask watch: editing examples/ rebuilds the game dylib; Ctrl+C or close the window to stop"
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
                .args(["build", "-p", "platformer"])
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
    let root = repo_root();
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

    println!("sample assets regenerated under assets/shared/");
    ExitCode::SUCCESS
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

fn not_yet(what: &str, phase: &str) -> ExitCode {
    eprintln!("xtask: `{what}` arrives in {phase} — see PLANO_EXECUCAO_TRINO.md");
    ExitCode::FAILURE
}
