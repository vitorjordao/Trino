//! `cargo xtask replay run|dump|check` (issue #67, ADR 0036) and the
//! emulator harnesses' checks of the `players` and `replay` self-test
//! steps.
//!
//! - `replay dump <file.trec> [--json]`: the header and every frame (dt,
//!   connected players, each player's held buttons, sticks, triggers,
//!   pointer, keys, the recorded hash) — a recording as an agent reads it.
//! - `replay run <file.trec> --platform pc|n64|3ds|web [--until-frame N]
//!   [--shot out.png] [--window] [--json]`: play it on that target and
//!   print where it diverged. The PC runs the game's sim headless in this
//!   process (castle64, the showcase, Solid Eleven), or in its real window
//!   with `--shot`/`--window`; the consoles build an image with the
//!   recording in it (`replay.trec`) and boot it in ares or Azahar; the Web
//!   opens `?replay=` in Chromium through the Playwright harness.
//! - `replay record <out.trec> --platform n64|3ds`: boot the game in its
//!   emulator with a `record` file in the image; it streams the live input as
//!   `TRINO_REC:` lines over its debug channel, which land in `out.trec`
//!   when the emulator closes (or its timeout, `TRINO_EMU_TIMEOUT`). Play in
//!   the emulator's window (a local ares or Azahar, not `TRINO_ARES=docker`).
//!   The PC records with `TRINO_RECORD=<file>`, the Web with `?record`.
//! - `replay check <file.trec>... [--platforms pc,n64,3ds,web] [--json]`:
//!   `run` on each target; every one must replay without a divergence and
//!   end on the same hash (the cross-platform determinism gate for any
//!   recording; CI's gate is the self-test's `replay` step).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::json;
use trino_core::replay::{Reader, Session};
use trino_core::{Button, Game, Key, PlayersInput};

use crate::project::Project;

const USAGE: &str = "xtask: usage:\n  cargo xtask replay dump <file.trec> [--json]\n  \
     cargo xtask replay record <out.trec> --platform n64|3ds\n  \
     cargo xtask replay run <file.trec> --platform pc|n64|3ds|web [--until-frame N] \
     [--shot out.png] [--window] [--json]\n  \
     cargo xtask replay check <file.trec>... [--platforms pc,n64,3ds,web] [--json]";

pub fn run(project: &Project, args: &[&str]) -> ExitCode {
    let json = args.contains(&"--json");
    let result = match args {
        ["dump", file, ..] => dump(Path::new(file), json),
        ["run", file, rest @ ..] => run_cmd(project, Path::new(file), rest, json),
        ["check", rest @ ..] => check(project, rest, json),
        ["record", file, rest @ ..] => record(project, Path::new(file), rest),
        _ => Err(USAGE.to_string()),
    };
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

/// A flag's value (`--platform n64`).
fn value<'a>(args: &[&'a str], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| *a == flag)
        .and_then(|i| args.get(i + 1).copied())
}

/// The games this checkout can replay headless, by the header's id.
const GAMES: [&str; 3] = ["castle64", "showcase", "solid-eleven"];

/// The game a recording names (one of [`GAMES`], or the project's own).
pub fn game_of(header: &trino_core::replay::Header, project: &Project) -> Option<String> {
    GAMES
        .iter()
        .map(|g| g.to_string())
        .chain([project.name.clone()])
        .find(|g| trino_core::asset_id(g) == header.game)
}

fn button_names(s: &trino_core::InputState) -> Vec<String> {
    Button::ALL
        .iter()
        .filter(|b| s.is_down(**b))
        .map(|b| format!("{b:?}"))
        .collect()
}

fn key_names(s: &trino_core::InputState) -> Vec<String> {
    Key::ALL
        .iter()
        .filter(|k| s.key(**k))
        .map(|k| format!("{k:?}"))
        .collect()
}

/// `replay dump`.
fn dump(file: &Path, json_out: bool) -> Result<bool, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut r = Reader::new(&bytes).map_err(|e| format!("{}: {}", file.display(), e.message()))?;
    let h = *r.header();
    let mut frames = Vec::new();
    let mut text = String::new();
    let mut prev: Option<PlayersInput> = None;
    let mut seconds = 0f64;
    while let Some(f) = r.next_frame() {
        let f = f.map_err(|e| format!("{}: {}", file.display(), e.message()))?;
        seconds += f.dt as f64;
        let players: Vec<serde_json::Value> = (0..h.players as usize)
            .map(|p| {
                let s = &f.players.players[p];
                json!({
                    "buttons": button_names(s),
                    "stick": [s.stick.x, s.stick.y],
                    "right_stick": [s.right_stick.x, s.right_stick.y],
                    "triggers": [s.triggers.x, s.triggers.y],
                    "pointer": s.pointer.map(|v| vec![v.x, v.y]),
                    "pointer_screen": s.pointer_screen,
                    "pointer_buttons": s.pointer_bits(),
                    "wheel": s.wheel,
                    "keys": key_names(s),
                    "pad": s.pad,
                })
            })
            .collect();
        frames.push(json!({
            "frame": f.index,
            "dt": f.dt,
            "connected": f.players.connected,
            "players": players,
            "hash": f.hash.map(|v| format!("{v:#018x}")),
        }));
        if !json_out && prev.is_none_or(|p| p != f.players) {
            let mut line = format!("frame {:>6} t={seconds:>8.3}s", f.index);
            for p in 0..h.players as usize {
                if f.players.connected & (1 << p) == 0 && p > 0 {
                    continue;
                }
                let s = &f.players.players[p];
                line.push_str(&format!(
                    "  P{} [{}] stick={:.2},{:.2}",
                    p + 1,
                    button_names(s).join("+"),
                    s.stick.x,
                    s.stick.y
                ));
                if let Some(v) = s.pointer {
                    line.push_str(&format!(" pointer={:.0},{:.0}", v.x, v.y));
                }
                let k = key_names(s);
                if !k.is_empty() {
                    line.push_str(&format!(" keys={}", k.join("+")));
                }
            }
            text.push_str(&line);
            text.push('\n');
        }
        prev = Some(f.players);
    }
    let header = json!({
        "version": h.version,
        "finished": h.finished(),
        "game_id": format!("{:#010x}", h.game),
        "game": GAMES.iter().find(|g| trino_core::asset_id(g) == h.game),
        "screen": [h.screen.0, h.screen.1],
        "players": h.players,
        "hash_every": h.hash_every,
        "start_frame": h.start_frame,
        "frames": frames.len(),
        "seconds": seconds,
        "start_hash": format!("{:#018x}", h.start_hash),
        "bytes": bytes.len(),
    });
    if json_out {
        println!("{}", json!({ "header": header, "frames": frames }));
    } else {
        println!(
            "{}: {} frames ({seconds:.2} s), {} player(s), screen {}x{}, hash every {} frame(s), \
             start hash {:#018x}, {} bytes{}",
            file.display(),
            frames.len(),
            h.players,
            h.screen.0,
            h.screen.1,
            h.hash_every,
            h.start_hash,
            bytes.len(),
            if h.start_frame > 0 {
                format!(" (a tail from session frame {})", h.start_frame)
            } else {
                String::new()
            }
        );
        print!("{text}");
    }
    Ok(true)
}

/// What a replay ended with on one target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub platform: String,
    pub ok: bool,
    pub frames: u32,
    pub checked: u32,
    pub hash: u64,
    /// The first divergence: frame, expected, got.
    pub diverged: Option<(u32, u64, u64)>,
}

impl Verdict {
    fn json(&self) -> serde_json::Value {
        json!({
            "platform": self.platform,
            "ok": self.ok,
            "frames": self.frames,
            "checked": self.checked,
            "hash": format!("{:#018x}", self.hash),
            "diverged": self.diverged.map(|(f, e, g)| json!({
                "frame": f,
                "expected": format!("{e:#018x}"),
                "got": format!("{g:#018x}"),
            })),
        })
    }
}

fn hex(s: &str) -> Option<u64> {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}

/// A `TRINO_REPLAY: ok …` / `TRINO_REPLAY: diverged at frame …` line.
pub fn parse_verdict(line: &str) -> Option<Verdict> {
    let rest = line.split_once("TRINO_REPLAY: ")?.1.trim();
    let platform = rest.rsplit_once(" on ")?.1.to_string();
    if let Some(r) = rest.strip_prefix("ok ") {
        let field = |k: &str| r.split_whitespace().find_map(|w| w.strip_prefix(k));
        return Some(Verdict {
            platform,
            ok: true,
            frames: field("frames=")?.parse().ok()?,
            checked: field("checked=")?.parse().ok()?,
            hash: hex(field("hash=")?)?,
            diverged: None,
        });
    }
    let r = rest.strip_prefix("diverged at frame ")?;
    let (frame, r) = r.split_once(' ')?;
    let expected = r.split_once("expected ")?.1.split(',').next()?;
    let got = r.split_once("got ")?.1.split(')').next()?;
    let (frame, expected, got) = (frame.parse().ok()?, hex(expected)?, hex(got)?);
    Some(Verdict {
        platform,
        ok: false,
        frames: frame,
        checked: 0,
        hash: got,
        diverged: Some((frame, expected, got)),
    })
}

/// A platform without devices or services: what a recorded game's update
/// sees in the headless PC run.
struct Headless {
    players: trino_core::HostedPlayers,
    start: std::time::Instant,
}

impl trino_core::Platform for Headless {
    fn now_micros(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }
    fn log(&self, _: &str) {}
    fn hosted_players(&self) -> Option<&trino_core::HostedPlayers> {
        Some(&self.players)
    }
    fn hosted_players_mut(&mut self) -> Option<&mut trino_core::HostedPlayers> {
        Some(&mut self.players)
    }
}

struct Mute;
impl trino_core::Audio for Mute {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// A fresh game of `name` at `screen` (the headless PC run).
pub fn make_game(name: &str, screen: trino_core::Vec2) -> Option<Box<dyn Game>> {
    Some(match name {
        "castle64" => Box::new(castle64::Castle64Game::new(screen)),
        "showcase" => Box::new(showcase::ShowcaseGame::with_screen(screen)),
        "solid-eleven" => Box::new(solid_eleven::SolidElevenGame::new(screen)),
        _ => return None,
    })
}

/// The recording on the PC, in this process, without a window: the
/// game's sim only (rendering never changes it).
pub fn run_headless(bytes: &[u8], name: &str, until: Option<u32>) -> Result<Verdict, String> {
    let header = *Reader::new(bytes).map_err(|e| e.message())?.header();
    let screen = trino_core::Vec2::new(header.screen.0 as f32, header.screen.1 as f32);
    let mut game = make_game(name, screen)
        .ok_or_else(|| format!("`{name}` can't run headless here: use --window"))?;
    let mut platform = Headless {
        players: trino_core::HostedPlayers::new(),
        start: std::time::Instant::now(),
    };
    let mut session = Session::replay(bytes, name, until).map_err(|e| e.message())?;
    session.start(game.as_ref());
    while let Some((players, dt)) = session.input(&PlayersInput::NONE, 0.0) {
        platform.players.publish_players(&players);
        game.update_with(&mut platform, &players.players[0], &mut Mute, dt);
        session.after(game.as_ref(), &players, dt, &mut |_: &[u8]| {});
    }
    let mut line = String::new();
    let _ = session.verdict("pc", &mut line);
    parse_verdict(&line).ok_or_else(|| format!("no verdict ({line})"))
}

/// The engine's PC app (castle64, the showcase, Solid Eleven) or a
/// project's PC host, replaying in its window; `shot` gets its last frame.
fn run_window(
    project: &Project,
    file: &Path,
    name: &str,
    until: Option<u32>,
    shot: Option<&Path>,
) -> Result<Verdict, String> {
    let mut cmd = std::process::Command::new(std::env::var("CARGO").unwrap_or("cargo".into()));
    cmd.current_dir(&project.root)
        .arg("run")
        .arg("--quiet")
        .arg("-p");
    if project.is_engine {
        cmd.arg("trino-app-pc");
        if name != "castle64" {
            cmd.env("TRINO_GAME", name);
        }
    } else {
        cmd.arg(&project.pc_package);
    }
    cmd.env("TRINO_REPLAY", file).env("TRINO_WINDOWED", "1");
    if let Some(u) = until {
        cmd.env("TRINO_REPLAY_UNTIL", u.to_string());
    }
    if let Some(s) = shot {
        cmd.env("TRINO_SCREENSHOT", s);
    }
    let out = cmd
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|e| format!("cargo run: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .lines()
        .find_map(parse_verdict)
        .ok_or_else(|| format!("the PC app printed no TRINO_REPLAY line ({})", out.status))
}

/// Where a staged session goes in a console image: set by [`run_console`]
/// and the `--record` runs, read by the bakes ([`stage`]).
const STAGE_REPLAY: &str = "TRINO_REPLAY_STAGE";
const STAGE_UNTIL: &str = "TRINO_REPLAY_STAGE_UNTIL";
const STAGE_SHOT: &str = "TRINO_REPLAY_STAGE_SHOT";
const STAGE_RECORD: &str = "TRINO_RECORD_STAGE";

/// Put the session `replay run` / `run --record` asked for into a console
/// image's file system (`dir`: the N64's DFS, the 3DS's RomFS).
pub fn stage(dir: &Path) -> Result<(), String> {
    if let Ok(src) = std::env::var(STAGE_REPLAY) {
        std::fs::copy(&src, dir.join("replay.trec")).map_err(|e| format!("{src}: {e}"))?;
        if let Ok(u) = std::env::var(STAGE_UNTIL) {
            std::fs::write(dir.join("replay_until"), u).map_err(|e| e.to_string())?;
        }
        if std::env::var_os(STAGE_SHOT).is_some() {
            std::fs::write(dir.join("replay_shot"), "1").map_err(|e| e.to_string())?;
        }
    }
    if let Ok(game) = std::env::var(STAGE_RECORD) {
        std::fs::write(dir.join("record"), game).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Stage a session for the next console build (this process only).
pub fn set_stage(replay: Option<&Path>, until: Option<u32>, shot: bool, record: Option<&str>) {
    // SAFETY: xtask is single-threaded here (before any build or boot).
    unsafe {
        for k in [STAGE_REPLAY, STAGE_UNTIL, STAGE_SHOT, STAGE_RECORD] {
            std::env::remove_var(k);
        }
        if let Some(r) = replay {
            std::env::set_var(STAGE_REPLAY, r);
        }
        if let Some(u) = until {
            std::env::set_var(STAGE_UNTIL, u.to_string());
        }
        if shot {
            std::env::set_var(STAGE_SHOT, "1");
        }
        if let Some(g) = record {
            std::env::set_var(STAGE_RECORD, g);
        }
    }
}

/// Build the console image with the recording in it, boot it in its
/// emulator, read the verdict (and the last frame's dump with `shot`).
fn run_console(
    project: &Project,
    platform: &str,
    file: &Path,
    until: Option<u32>,
    shot: Option<&Path>,
) -> Result<Verdict, String> {
    let file = file
        .canonicalize()
        .map_err(|e| format!("{}: {e}", file.display()))?;
    set_stage(Some(&file), until, shot.is_some(), None);
    let built = match platform {
        "n64" => crate::n64::build(project, false),
        _ => crate::n3ds::build(project, false),
    };
    set_stage(None, None, false, None);
    if built != ExitCode::SUCCESS {
        return Err(format!("the {platform} build failed"));
    }
    let mut verdict = None;
    let mut fb = crate::fbgolden::FbCapture::default();
    let mut shot_img = None;
    let mut on_line = |line: &str| {
        if fb.feed(line) {
            if fb.done() {
                shot_img = fb.image().ok();
                fb = crate::fbgolden::FbCapture::default();
            }
            return false;
        }
        if line.contains("TRINO_REPLAY") {
            println!("[{platform}] {}", line.trim());
        }
        if let Some(v) = parse_verdict(line) {
            verdict = Some(v);
        }
        line.contains("TRINO_REPLAY_DONE")
    };
    match platform {
        "n64" => crate::n64::boot_for_lines(project, true, &mut on_line)?,
        _ => crate::n3ds::boot_for_lines(&project.app_3dsx(), &mut on_line)?,
    }
    if let (Some(path), Some((w, h, rgba))) = (shot, shot_img) {
        crate::fbgolden::write_png(path, w, h, &rgba)?;
        println!("[{platform}] last frame: {}", path.display());
    }
    verdict.ok_or_else(|| format!("the {platform} run printed no TRINO_REPLAY verdict"))
}

/// `replay record`: the console's own recording of a play session.
fn record(project: &Project, out: &Path, args: &[&str]) -> Result<bool, String> {
    let platform = value(args, "--platform").ok_or("replay record needs --platform n64|3ds")?;
    let name = if project.is_engine {
        "castle64".to_string()
    } else {
        project.name.clone()
    };
    set_stage(None, None, false, Some(&name));
    let built = match platform {
        "n64" => crate::n64::build(project, false),
        "3ds" => crate::n3ds::build(project, false),
        p => {
            return Err(format!(
                "replay record: `{p}` (n64 or 3ds; the PC: TRINO_RECORD)"
            ));
        }
    };
    set_stage(None, None, false, None);
    if built != ExitCode::SUCCESS {
        return Err(format!("the {platform} build failed"));
    }
    println!("[{platform}] recording {name}: play in the emulator, close it to stop");
    let mut rec = Rerecord::default();
    let mut on_line = |line: &str| {
        rec.feed(line);
        false
    };
    let run = match platform {
        "n64" => crate::n64::boot_for_lines(project, true, &mut on_line),
        _ => crate::n3ds::boot_for_lines(&project.app_3dsx(), &mut on_line),
    };
    if rec.bytes.is_empty() {
        return Err(format!(
            "no recording came back ({})",
            run.err().unwrap_or_default()
        ));
    }
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(out, &rec.bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    let frames = Reader::new(&rec.bytes).map(|r| {
        let mut r = r;
        let mut n = 0;
        while let Some(Ok(_)) = r.next_frame() {
            n += 1;
        }
        n
    });
    println!(
        "[{platform}] {}: {} frames, {} bytes",
        out.display(),
        frames.unwrap_or(0),
        rec.bytes.len()
    );
    Ok(true)
}

/// The Web build with the recording beside it, `?replay=` in Chromium.
fn run_web(
    project: &Project,
    file: &Path,
    name: &str,
    until: Option<u32>,
    shot: Option<&Path>,
) -> Result<Verdict, String> {
    let web = project.root.join("target/web");
    crate::web::build_inner(
        project,
        &crate::web::BuildOptions {
            release: true,
            out: Some(web.clone()),
        },
    )?;
    std::fs::copy(file, web.join("replay.trec")).map_err(|e| format!("{}: {e}", file.display()))?;
    let harness = project.engine.join("tools/web-test");
    crate::web::ensure_harness(&harness, &["chromium".to_string()])?;
    let port = crate::web::serve(web, 0)?;
    let out = project.root.join("target/web-replay");
    let _ = std::fs::remove_dir_all(&out);
    let mut page = format!("?game={name}&replay=replay.trec");
    if let Some(u) = until {
        page.push_str(&format!("&replay_until={u}"));
    }
    let mut cmd = std::process::Command::new("node");
    cmd.arg(harness.join("run.mjs"))
        .arg("--url")
        .arg(format!("http://127.0.0.1:{port}/"))
        .arg("--out")
        .arg(&out)
        .args(["--browsers", "chromium", "--page", &page]);
    if let Some(s) = shot {
        let abs = std::path::absolute(s).map_err(|e| e.to_string())?;
        cmd.arg("--shot").arg(abs);
    }
    // The harness's own lines go to stderr: `--json` stays JSON.
    let _ = cmd.stdout(std::process::Stdio::inherit()).status();
    let console =
        std::fs::read_to_string(out.join("chromium-webgl2/console.log")).unwrap_or_default();
    console.lines().find_map(parse_verdict).ok_or_else(|| {
        format!(
            "the page printed no TRINO_REPLAY verdict ({})",
            out.display()
        )
    })
}

/// `run` on one platform.
fn run_on(
    project: &Project,
    platform: &str,
    file: &Path,
    until: Option<u32>,
    shot: Option<&Path>,
    window: bool,
) -> Result<Verdict, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let header = *Reader::new(&bytes)
        .map_err(|e| format!("{}: {}", file.display(), e.message()))?
        .header();
    let name = game_of(&header, project).ok_or_else(|| {
        format!(
            "{}: a recording of a game this project doesn't have",
            file.display()
        )
    })?;
    match platform {
        "pc" if window || shot.is_some() => run_window(project, file, &name, until, shot),
        "pc" => run_headless(&bytes, &name, until),
        "n64" | "3ds" => {
            if project.is_engine && name != "castle64" {
                return Err(format!(
                    "the engine's {platform} image runs castle64; replay `{name}` from its project"
                ));
            }
            run_console(project, platform, file, until, shot)
        }
        "web" => run_web(project, file, &name, until, shot),
        _ => Err(format!("unknown platform `{platform}` (pc, n64, 3ds, web)")),
    }
}

/// `replay run`.
fn run_cmd(project: &Project, file: &Path, args: &[&str], json_out: bool) -> Result<bool, String> {
    let platform = value(args, "--platform").unwrap_or("pc");
    let until = value(args, "--until-frame")
        .map(|v| {
            v.parse::<u32>()
                .map_err(|_| format!("--until-frame {v}: not a number"))
        })
        .transpose()?;
    let shot = value(args, "--shot").map(PathBuf::from);
    let v = run_on(
        project,
        platform,
        file,
        until,
        shot.as_deref(),
        args.contains(&"--window"),
    )?;
    if json_out {
        println!("{}", v.json());
    } else {
        match v.diverged {
            None => println!(
                "replay: ok on {} — {} frames, {} hashes checked, final hash {:#018x}",
                v.platform, v.frames, v.checked, v.hash
            ),
            Some((f, e, g)) => println!(
                "replay: diverged at frame {f} on {} (expected {e:#018x}, got {g:#018x})",
                v.platform
            ),
        }
    }
    Ok(v.ok)
}

/// `replay check`: every platform, the same verdict.
fn check(project: &Project, args: &[&str], json_out: bool) -> Result<bool, String> {
    let platforms: Vec<&str> = value(args, "--platforms")
        .unwrap_or("pc,n64,3ds,web")
        .split(',')
        .collect();
    let files: Vec<&str> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| !a.starts_with("--") && (*i == 0 || args[i - 1] != "--platforms"))
        .map(|(_, a)| *a)
        .collect();
    if files.is_empty() {
        return Err(USAGE.to_string());
    }
    let mut all_ok = true;
    let mut report = Vec::new();
    for file in &files {
        let mut verdicts = Vec::new();
        for p in &platforms {
            let v = run_on(project, p, Path::new(file), None, None, false).unwrap_or_else(|e| {
                Verdict {
                    platform: p.to_string(),
                    ok: false,
                    frames: 0,
                    checked: 0,
                    hash: 0,
                    diverged: None,
                }
                .with_error(&e)
            });
            verdicts.push(v);
        }
        let first = verdicts.first().map(|v| v.hash);
        let same = verdicts.iter().all(|v| v.ok && Some(v.hash) == first);
        all_ok &= same;
        if !json_out {
            for v in &verdicts {
                println!(
                    "{file}: {:<4} {} frames={} checked={} hash={:#018x}{}",
                    v.platform,
                    if v.ok { "ok  " } else { "FAIL" },
                    v.frames,
                    v.checked,
                    v.hash,
                    v.diverged
                        .map(|(f, _, _)| format!(" diverged at frame {f}"))
                        .unwrap_or_default()
                );
            }
            println!(
                "{file}: {}",
                if same {
                    "every platform replayed it to the same hash"
                } else {
                    "the platforms disagree"
                }
            );
        }
        report.push(json!({
            "file": file,
            "same": same,
            "runs": verdicts.iter().map(Verdict::json).collect::<Vec<_>>(),
        }));
    }
    if json_out {
        println!("{}", json!({ "ok": all_ok, "recordings": report }));
    }
    Ok(all_ok)
}

impl Verdict {
    fn with_error(self, e: &str) -> Verdict {
        eprintln!("[{}] {e}", self.platform);
        self
    }
}

/// The self-test's `TRINO_STEP:players connected=<mask> players=<n>` line
/// against the controllers the emulator plugs in (ares: all four ports;
/// Azahar: the one player). Returns the line's summary.
pub fn check_players_step(step: Option<&str>, want_mask: &str) -> Result<String, String> {
    let step =
        step.ok_or("players: the self-test printed no `TRINO_STEP:players connected=` line")?;
    let rest = step
        .split_once("TRINO_STEP:players ")
        .map(|(_, r)| r.trim())
        .ok_or_else(|| format!("players: bad line `{step}`"))?;
    let mask = rest
        .split_whitespace()
        .find_map(|w| w.strip_prefix("connected="))
        .ok_or_else(|| format!("players: no mask in `{rest}`"))?;
    if mask != want_mask {
        return Err(format!(
            "players: the emulator plugs in {want_mask} but the self-test saw `{rest}`"
        ));
    }
    Ok(rest.to_string())
}

/// The self-test's `TRINO_STEP:replay ok frames=… checked=… hash=…` line:
/// present, with every frame checked. Returns its summary.
pub fn check_replay_step(step: Option<&str>) -> Result<String, String> {
    let step = step.ok_or("replay: the self-test printed no `TRINO_STEP:replay ok` line")?;
    let rest = step
        .split_once("TRINO_STEP:replay ok ")
        .map(|(_, r)| r.trim())
        .ok_or_else(|| format!("replay: bad line `{step}`"))?;
    let field = |k: &str| {
        rest.split_whitespace()
            .find_map(|w| w.strip_prefix(k))
            .and_then(|v| v.parse::<u32>().ok())
    };
    match (field("frames="), field("checked=")) {
        (Some(f), Some(c)) if f > 0 && c == f => Ok(rest.to_string()),
        _ => Err(format!("replay: not every frame was checked: `{rest}`")),
    }
}

/// The castle64 fixture the `replay` step plays (the PC's recording).
pub const FIXTURE: &str = "assets/shared/replays/castle64_green.trec";

/// What a target sent back over its debug channel while it replayed the
/// fixture (`TRINO_REC:` lines, `TRINO_REC_END`): the frames it fed and
/// the hashes it computed, recorded again by its own recorder.
#[derive(Default)]
pub struct Rerecord {
    pub bytes: Vec<u8>,
    pub ended: bool,
}

impl Rerecord {
    /// Take a `TRINO_REC` line (true), or leave any other line (false).
    pub fn feed(&mut self, line: &str) -> bool {
        if line.contains("TRINO_REC_END") {
            self.ended = true;
            return true;
        }
        trino_core::replay::decode_line(line, &mut |b: &[u8]| self.bytes.extend_from_slice(b))
            .is_some()
    }

    /// The re-recording against the PC's file, byte for byte; written to
    /// `out` for a look when it differs (or always). Returns a summary.
    pub fn check(&self, engine: &std::path::Path, out: &std::path::Path) -> Result<String, String> {
        if !self.ended {
            return Err("replay: the target's re-recording never ended (TRINO_REC_END)".into());
        }
        if let Some(dir) = out.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(out, &self.bytes);
        let fixture = std::fs::read(engine.join(FIXTURE)).map_err(|e| format!("{FIXTURE}: {e}"))?;
        if self.bytes != fixture {
            let at = self
                .bytes
                .iter()
                .zip(&fixture)
                .position(|(a, b)| a != b)
                .unwrap_or(self.bytes.len().min(fixture.len()));
            return Err(format!(
                "replay: the target's re-recording ({} bytes, {}) differs from the PC's \
                 ({} bytes) at byte {at}",
                self.bytes.len(),
                out.display(),
                fixture.len()
            ));
        }
        Ok(format!("{} bytes identical to the PC's", self.bytes.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_lines_parse() {
        let ok = parse_verdict(
            "[ares] TRINO_REPLAY: ok frames=731 checked=731 hash=0x1f43636b3dddc909 on n64",
        )
        .unwrap();
        assert_eq!(
            (ok.platform.as_str(), ok.ok, ok.frames, ok.checked, ok.hash),
            ("n64", true, 731, 731, 0x1f43636b3dddc909)
        );
        let bad = parse_verdict(
            "TRINO_REPLAY: diverged at frame 101 (expected 0x00000000000000aa, got 0x00000000000000bb) on pc",
        )
        .unwrap();
        assert_eq!(bad.diverged, Some((101, 0xaa, 0xbb)));
        assert!(!bad.ok);
        assert_eq!(parse_verdict("TRINO_STEP:replay ok frames=1"), None);
    }

    #[test]
    fn the_harness_reads_the_players_and_replay_lines() {
        let line = "[ares] TRINO_STEP:players connected=1111 players=4";
        assert!(check_players_step(Some(line), "1111").is_ok());
        assert!(check_players_step(Some(line), "0001").is_err());
        assert!(check_players_step(None, "0001").is_err());
        let ok =
            "TRINO_STEP:replay ok frames=731 checked=731 hash=0x1f43636b3dddc909 load_frames=2";
        assert!(check_replay_step(Some(ok)).is_ok());
        let partial = "TRINO_STEP:replay ok frames=731 checked=700 hash=0x0";
        assert!(check_replay_step(Some(partial)).is_err());
        assert!(check_replay_step(None).is_err());
    }
}
