//! `cargo xtask <command> [platform] [args...]`
//!
//! The single entry point for building, running and testing Trino on every
//! target. Wraps cargo, the asset pipeline, Docker (N64) and emulators so
//! contributors and CI never memorize per-platform incantations.

mod castle64_assets;
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
                       ROM + relaunch ares). pc: --game <crate> picks the
                       game dylib to rebuild (default: castle64)
  editor               launch the Trino editor
  new <name>           scaffold a new game crate under examples/
  gen-assets           regenerate the sample master assets (dev utility)
";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let bless = args.iter().any(|a| a == "--bless");
    args.retain(|a| a != "--bless");
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
        (Some("watch"), Some("pc")) => watch_pc(&game),
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
        (Some("new"), Some(name)) => new_game(name),
        (Some("new"), None) => {
            eprintln!("xtask: usage: cargo xtask new <kebab-case-name>");
            ExitCode::FAILURE
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

    let models = root.join("assets/shared/models");
    std::fs::create_dir_all(&models).unwrap();
    gen_cube_glb(&models.join("cube.glb"));

    // castle64 (the 3D showcase): blocks, articulated player, boar, doors.
    castle64_assets::gen_all(&root, &|path, w, h, rgba| write_png(path, w, h, rgba));

    println!("sample assets regenerated under assets/shared/");
    ExitCode::SUCCESS
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
        r#"{{"asset":{{"version":"2.0","generator":"trino gen-assets"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0,"name":"cube"}}],"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1,"COLOR_0":2}},"indices":3}}]}}],"buffers":[{{"byteLength":{}}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":{}}},{{"buffer":0,"byteOffset":{normals_offset},"byteLength":{}}},{{"buffer":0,"byteOffset":{colors_offset},"byteLength":{}}},{{"buffer":0,"byteOffset":{indices_offset},"byteLength":{}}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":{vcount},"type":"VEC3","min":[-0.5,-0.5,-0.5],"max":[0.5,0.5,0.5]}},{{"bufferView":1,"componentType":5126,"count":{vcount},"type":"VEC3"}},{{"bufferView":2,"componentType":5121,"normalized":true,"count":{vcount},"type":"VEC4"}},{{"bufferView":3,"componentType":5123,"count":{},"type":"SCALAR"}}]}}"#,
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

/// Scaffold a new game crate from `templates/new-game/` into `examples/`.
/// The workspace member glob picks it up automatically; every generated
/// project ships its own AGENTS.md (AI-friendly by construction).
fn new_game(name: &str) -> ExitCode {
    match scaffold_game(&repo_root(), name) {
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

/// Testable core of `new_game`: render the template into `<root>/examples/<name>`.
fn scaffold_game(root: &Path, name: &str) -> Result<PathBuf, String> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase());
    if !valid {
        return Err(format!(
            "`{name}` is not a valid crate name — use kebab-case (e.g. my-game)"
        ));
    }
    let dest = root.join("examples").join(name);
    if dest.exists() {
        return Err(format!("{} already exists", dest.display()));
    }
    let snake = name.replace('-', "_");
    let camel: String = name
        .split('-')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                None => String::new(),
            }
        })
        .collect();

    copy_template(
        &root.join("templates/new-game"),
        &dest,
        name,
        &snake,
        &camel,
    )?;
    Ok(dest)
}

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
        let dst = to.join(entry.file_name());
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
    use super::{copy_template, repo_root, scaffold_game};

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
