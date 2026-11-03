//! `cargo xtask net lobby|local` (issue #67): the lobby server online
//! players meet at, and two local instances of a game playing online.
//!
//! - `net lobby [--udp 7400] [--ws 7401]` — rooms by code
//!   (`trino_net::lobby::Rooms`): PC games join over UDP
//!   (`TRINO_NET=lobby:<code>@<ip>:7400`), Web pages over a WebSocket
//!   (`?net=<code>&lobby=ws://<ip>:7401`), and the WebRTC offers, answers
//!   and candidates go through it. Runs until Ctrl+C.
//! - `net local [--game solid-eleven] [--sim latency=80,jitter=10,loss=3]
//!   [--port 7311] [--script file.trec] [--frames N] [--json]` — the PC
//!   app twice, a host and a guest on this machine, with the network's
//!   conditions simulated on the way in (the editor's Net panel runs this).
//!   With `--script` each plays its seat of a recording and both print the
//!   game's hash on the same frame: the run fails when they differ.
//!   `--lobby` meets in a room of an in-process lobby server instead of a
//!   host's (`--route relay` sends every datagram through its relay),
//!   `--watch` adds a spectator window (`--follow 2` follows player 2).
//! - `net config [--json]`, `net config set players.max=2 net.mode=delay ..`
//!   — `trino.toml`'s `[players]` and `[net]`, read and written in place
//!   (the editor's Net tab writes the same keys).
//! - `net 3ds [--no-build] [--docker <image>] [--seconds N] [--hash-at N]`
//!   (issue #117) — two 3DS consoles over local wireless (`uds`) in two
//!   Azahar instances that share an Azahar multiplayer room: a host build and
//!   a join build of castle64 (each its seat of the green run, input delay
//!   4), `tools/uds-test/two-azahar.sh` driving both windows under Xvfb with
//!   xdotool; both consoles must print the same hash on the same frame.
//!   Linux only; the tools come from the host or from `--docker <image>`
//!   (the CI's 3DS image plus xdotool, openbox and ImageMagick).
//! - `net web [--no-build] [--browser chromium|firefox] [--watch]` — the same in a
//!   browser: the Web build served, a lobby, two pages of Solid Eleven
//!   meeting in a room and playing over WebRTC (`tools/web-test/net.mjs`),
//!   each its seat of the two-player recording; both pages must log the
//!   same hash on frame 760.

use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex, mpsc};

use serde_json::json;
use trino_net::lobby::Rooms;

use crate::project::Project;

const USAGE: &str = "xtask: usage:\n  cargo xtask net lobby [--udp 7400] [--ws 7401]\n  \
     cargo xtask net local [--game solid-eleven] [--sim latency=80,jitter=10,loss=3] \
     [--mode rollback|delay] [--rollback N] [--delay N] [--port 7311] [--script file.trec] \
     [--hash-at N] [--frames N] [--lobby] [--route auto|direct|relay] [--watch] \
     [--follow N] [--json]\n  \
     cargo xtask net web [--no-build] [--browser chromium|firefox] [--watch]\n  \
     cargo xtask net 3ds [--no-build] [--docker <image>] [--seconds N] [--hash-at N]\n  \
     cargo xtask net config [--json] | net config set <key>=<value>..";

pub fn run(project: &Project, args: &[&str]) -> ExitCode {
    let value = |flag: &str| {
        args.iter()
            .position(|a| *a == flag)
            .and_then(|i| args.get(i + 1).copied())
    };
    let result = match args.first() {
        Some(&"lobby") => {
            let udp = value("--udp").and_then(|v| v.parse().ok()).unwrap_or(7400);
            let ws = value("--ws").and_then(|v| v.parse().ok()).unwrap_or(7401);
            lobby(udp, ws, None).map(|_| true)
        }
        Some(&"local") => local(project, args),
        Some(&"web") => web(project, args),
        Some(&"3ds") => n3ds(project, args),
        Some(&"config") => config(project, &args[1..]),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Serve the lobby on UDP `udp` and WebSocket `ws` (0: any free port).
/// With `ready`, the bound ports go there and the function returns once
/// serving started (tests, `test web --net`); without it, it serves for
/// ever.
pub fn lobby(udp: u16, ws: u16, ready: Option<mpsc::Sender<(u16, u16)>>) -> Result<(), String> {
    let rooms = Arc::new(Mutex::new(Rooms::new()));
    let sock = UdpSocket::bind(("0.0.0.0", udp)).map_err(|e| format!("UDP {udp}: {e}"))?;
    let listener = TcpListener::bind(("0.0.0.0", ws)).map_err(|e| format!("TCP {ws}: {e}"))?;
    let ports = (
        sock.local_addr().map_err(|e| e.to_string())?.port(),
        listener.local_addr().map_err(|e| e.to_string())?.port(),
    );
    println!(
        "[net] lobby: UDP {} (TRINO_NET=lobby:<code>@<this ip>:{}), WebSocket {} (?net=<code>&lobby=ws://<this ip>:{})",
        ports.0, ports.0, ports.1, ports.1
    );
    // UDP: one datagram, one answer (or a room's START to every member).
    let udp_rooms = rooms.clone();
    let udp_thread = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            let Ok((n, from)) = sock.recv_from(&mut buf) else {
                continue;
            };
            // Lobby lines, and the relay for peers that can't reach each
            // other directly (issue #117).
            let out = udp_rooms
                .lock()
                .map(|mut r| trino_net::lobby::serve_udp(&mut r, &from.to_string(), &buf[..n]))
                .unwrap_or_default();
            for (to, msg) in out {
                if let Ok(a) = to.parse::<SocketAddr>() {
                    let _ = sock.send_to(&msg, a);
                }
            }
        }
    });
    // WebSocket: a thread per page; members are `ws<n>`.
    let peers: Arc<Mutex<std::collections::HashMap<String, mpsc::Sender<String>>>> = Arc::default();
    let ws_thread = std::thread::spawn(move || {
        for (n, stream) in listener.incoming().enumerate() {
            let Ok(stream) = stream else { continue };
            let rooms = rooms.clone();
            let peers = peers.clone();
            std::thread::spawn(move || serve_ws(stream, format!("ws{n}"), rooms, peers));
        }
    });
    match ready {
        Some(tx) => {
            let _ = tx.send(ports);
            Ok(())
        }
        None => {
            let _ = udp_thread.join();
            let _ = ws_thread.join();
            Ok(())
        }
    }
}

type Peers = Arc<Mutex<std::collections::HashMap<String, mpsc::Sender<String>>>>;

/// One page's WebSocket: its lines to the rooms, the rooms' lines to it.
fn serve_ws(stream: std::net::TcpStream, me: String, rooms: Arc<Mutex<Rooms>>, peers: Peers) {
    use tungstenite::Message;
    let Ok(mut ws) = tungstenite::accept(stream) else {
        return;
    };
    let _ = ws
        .get_mut()
        .set_read_timeout(Some(std::time::Duration::from_millis(20)));
    let (tx, rx) = mpsc::channel::<String>();
    if let Ok(mut p) = peers.lock() {
        p.insert(me.clone(), tx);
    }
    loop {
        while let Ok(line) = rx.try_recv() {
            if ws.send(Message::text(line)).is_err() {
                break;
            }
        }
        match ws.read() {
            Ok(Message::Text(t)) => {
                let out = rooms
                    .lock()
                    .map(|mut r| r.handle(&me, t.as_str(), ""))
                    .unwrap_or_default();
                if let Ok(p) = peers.lock() {
                    for (to, line) in out {
                        if let Some(tx) = p.get(&to) {
                            let _ = tx.send(line);
                        }
                    }
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    if let Ok(mut r) = rooms.lock() {
        r.leave(&me);
    }
    if let Ok(mut p) = peers.lock() {
        p.remove(&me);
    }
}

/// `net local`: two instances of the PC app, a host and a guest.
fn local(project: &Project, args: &[&str]) -> Result<bool, String> {
    let value = |flag: &str| {
        args.iter()
            .position(|a| *a == flag)
            .and_then(|i| args.get(i + 1).copied())
    };
    let json_out = args.contains(&"--json");
    let game = value("--game").unwrap_or("solid-eleven");
    let sim = value("--sim").unwrap_or("latency=40,jitter=10,loss=3");
    let port = value("--port").unwrap_or("7311");
    let script = value("--script");
    let hash_at = value("--hash-at").unwrap_or("760");
    let frames = value("--frames");
    let mode = value("--mode");
    let rollback = value("--rollback");
    let delay = value("--delay");
    let use_lobby = args.contains(&"--lobby");
    let route = value("--route");
    let watch = args.contains(&"--watch");
    let follow = value("--follow");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let build = Command::new(&cargo)
        .current_dir(&project.root)
        .args(["build", "-p"])
        .arg(if project.is_engine {
            "trino-app-pc"
        } else {
            &project.pc_package
        })
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("cargo build: {e}"))?;
    if !build.success() {
        return Err("the PC app didn't build".into());
    }
    let exe = if project.is_engine {
        project.root.join("target/debug/trino-pc")
    } else {
        project.root.join("target/debug").join(&project.pc_package)
    };
    let out = project.root.join("target/net-local");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let spawn = |role: &str, net: String| -> Result<std::process::Child, String> {
        let mut c = Command::new(&exe);
        c.current_dir(&project.root)
            .env("TRINO_NET", net)
            .env("TRINO_NET_SIM", sim)
            .env("TRINO_WINDOWED", "1")
            .env("TRINO_NET_HASH_AT", hash_at)
            .env("TRINO_SCREENSHOT", out.join(format!("{role}.png")));
        if project.is_engine {
            c.env("TRINO_GAME", game);
        }
        if let Some(s) = script {
            c.env(
                "TRINO_NET_SCRIPT",
                Path::new(s)
                    .canonicalize()
                    .map_err(|e| format!("{s}: {e}"))?,
            );
        }
        if let Some(f) = frames {
            c.env("TRINO_SMOKE_FRAMES", f);
        }
        for (var, v) in [
            ("TRINO_NET_MODE", mode),
            ("TRINO_NET_ROLLBACK", rollback),
            ("TRINO_NET_DELAY", delay),
            ("TRINO_NET_ROUTE", route),
            ("TRINO_NET_FOLLOW", follow),
        ] {
            if let Some(v) = v {
                c.env(var, v);
            }
        }
        c.stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{}: {e}", exe.display()))
    };
    // Who meets where: a host's own room, or a lobby room (issue #117);
    // a spectator joins before the room fills.
    let (first, spectator, second) = if use_lobby {
        let (tx, rx) = mpsc::channel();
        lobby(0, 0, Some(tx))?;
        let (udp, _) = rx.recv().map_err(|e| e.to_string())?;
        println!("[net] {game}: two players meet in a lobby room (UDP {udp}); network {sim}");
        (
            format!("lobby:LOCAL@127.0.0.1:{udp}"),
            format!("watch:LOCAL@127.0.0.1:{udp}"),
            format!("lobby:LOCAL@127.0.0.1:{udp}"),
        )
    } else {
        println!("[net] {game}: host on UDP {port}, guest joins; network {sim}");
        (
            format!("host:{port}"),
            format!("watch:127.0.0.1:{port}"),
            format!("join:127.0.0.1:{port}"),
        )
    };
    let mut children = vec![("host", spawn("host", first)?)];
    std::thread::sleep(std::time::Duration::from_millis(500));
    if watch {
        children.push(("spectator", spawn("spectator", spectator)?));
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    children.push(("guest", spawn("guest", second)?));
    let mut hashes = Vec::new();
    let mut lines = Vec::new();
    for (role, child) in children {
        let o = child.wait_with_output().map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&o.stdout).to_string();
        let _ = std::fs::write(out.join(format!("{role}.log")), &text);
        for l in text.lines().filter(|l| l.starts_with("TRINO_NET")) {
            if !json_out {
                println!("[{role}] {l}");
            }
            lines.push(format!("{role}: {l}"));
        }
        hashes.push(
            text.lines()
                .find_map(|l| l.strip_prefix("TRINO_NET_HASH:"))
                .map(str::to_string),
        );
    }
    let same = script.is_none() || (hashes[0].is_some() && hashes.iter().all(|h| *h == hashes[0]));
    if json_out {
        println!(
            "{}",
            json!({ "ok": same, "hashes": hashes, "lines": lines, "screenshots": out })
        );
    } else if script.is_some() {
        println!(
            "[net] {}: {:?}",
            if same {
                "the same game on every window"
            } else {
                "the games differ"
            },
            hashes
        );
    }
    Ok(same)
}

/// `net config`: show or set `[players]` / `[net]`.
fn config(project: &Project, args: &[&str]) -> Result<bool, String> {
    if let Some(("set", pairs)) = args.split_first().map(|(a, b)| (*a, b)) {
        if !project.root.join("trino.toml").is_file() {
            return Err(
                "net config set: this project has no trino.toml (the engine's \
                 demos take TRINO_NET_* for a run)"
                    .into(),
            );
        }
        for pair in pairs.iter().filter(|p| !p.starts_with("--")) {
            let (k, v) = pair
                .split_once('=')
                .ok_or(format!("`{pair}`: write key=value (empty removes it)"))?;
            trino_asset_pipeline::net::set_in(&project.root, k.trim(), v.trim())?;
            println!("{} = {}", k.trim(), v.trim());
        }
        return Ok(true);
    }
    let n = trino_asset_pipeline::net::ProjectNet::load(&project.root)?;
    let (cfg, _) = trino_net::NetConfig::from_baked(Some(&n.bake()));
    if args.contains(&"--json") {
        println!(
            "{}",
            json!({
                "declared": n.declared,
                "players": { "max": n.max_players },
                "net": {
                    "mode": cfg.mode.key(),
                    "max_rollback": cfg.max_rollback,
                    "input_delay": cfg.input_delay,
                    "players": cfg.players,
                    "desync_every": cfg.desync_every,
                    "disconnect_ms": cfg.disconnect_ms,
                },
                "keys": trino_asset_pipeline::net::KEYS
                    .iter()
                    .map(|(k, w)| json!({ "key": k, "takes": w }))
                    .collect::<Vec<_>>(),
            })
        );
    } else {
        println!("[players] max = {}", n.max_players);
        print!("[net]\n{}", cfg.to_text());
        if !n.declared {
            println!("(trino.toml declares neither: these are the defaults)");
        }
    }
    Ok(true)
}

/// `net 3ds`: two Azahar instances over local wireless (issue #117).
fn n3ds(project: &Project, args: &[&str]) -> Result<bool, String> {
    let value = |flag: &str| {
        args.iter()
            .position(|a| *a == flag)
            .and_then(|i| args.get(i + 1).copied())
    };
    let hash_at: u32 = value("--hash-at")
        .and_then(|v| v.parse().ok())
        .unwrap_or(600);
    let seconds = value("--seconds").unwrap_or("120");
    let out = project.root.join("target/net-3ds");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let script_rec = project
        .engine
        .join("assets/shared/replays/castle64_green.trec");
    let apps = [
        ("host", out.join("host.3dsx")),
        ("join", out.join("join.3dsx")),
    ];
    if !args.contains(&"--no-build") {
        for (role, app) in &apps {
            // SAFETY: single-threaded here; the build reads them (the RomFS's
            // `/uds` and `/uds_script`).
            unsafe {
                std::env::set_var("TRINO_UDS", format!("{role} 2 delay=4 hash_at={hash_at}"));
                std::env::set_var("TRINO_UDS_SCRIPT", &script_rec);
            }
            let built = crate::n3ds::build(project, false);
            unsafe {
                std::env::remove_var("TRINO_UDS");
                std::env::remove_var("TRINO_UDS_SCRIPT");
            }
            if built != ExitCode::SUCCESS {
                return Err(format!("the {role} build failed"));
            }
            std::fs::copy(project.app_3dsx(), app).map_err(|e| e.to_string())?;
        }
    }
    let azahar = crate::n3ds::azahar_exe().ok_or("Azahar not found (TRINO_AZAHAR)")?;
    let script = project.engine.join("tools/uds-test/two-azahar.sh");
    let run = out.join("run");
    let _ = std::fs::remove_dir_all(&run);
    let status = match value("--docker") {
        Some(image) => {
            // The AppImage's folder (its wrapper and libraries) and the
            // apps, read-only; the run's output back here.
            let az_dir = azahar
                .canonicalize()
                .map_err(|e| e.to_string())?
                .parent()
                .ok_or("Azahar's folder")?
                .to_path_buf();
            let az_name = azahar
                .file_name()
                .ok_or("Azahar's name")?
                .to_string_lossy()
                .into_owned();
            Command::new("docker")
                .args(["run", "--rm", "--network", "host"])
                .arg("-v")
                .arg(format!("{}:/azahar:ro", az_dir.display()))
                .arg("-v")
                .arg(format!("{}:/w", out.display()))
                .arg("-v")
                .arg(format!("{}:/t/two-azahar.sh:ro", script.display()))
                .arg(image)
                .args(["sh", "-c"])
                .arg(format!(
                    "(command -v xdotool && command -v openbox && command -v import) >/dev/null || \
                     (apt-get update && apt-get install -y --no-install-recommends xdotool \
                     openbox imagemagick) >/dev/null 2>&1; sh /t/two-azahar.sh /azahar/{az_name} \
                     /w/host.3dsx /w/join.3dsx /w/run {seconds}"
                ))
                .status()
        }
        None => Command::new("sh")
            .arg(&script)
            .arg(&azahar)
            .args([&apps[0].1, &apps[1].1, &run])
            .arg(seconds)
            .status(),
    }
    .map_err(|e| format!("two-azahar.sh: {e}"))?;
    if !status.success() {
        return Err(format!("two-azahar.sh failed ({status})"));
    }
    let hash = |log: &str| {
        std::fs::read_to_string(run.join(log))
            .unwrap_or_default()
            .lines()
            .find_map(|l| {
                let (_, rest) = l.split_once("TRINO_NET_HASH:")?;
                let (f, h) = rest.trim().split_once(':')?;
                (f.parse::<u32>().ok()? == hash_at).then(|| h.to_string())
            })
    };
    let (a, b) = (hash("a.log"), hash("b.log"));
    let same = a.is_some() && a == b;
    println!(
        "[net] 3ds: {} at frame {hash_at}: host {a:?}, join {b:?} ({})",
        if same {
            "both consoles agree"
        } else {
            "the consoles differ"
        },
        run.display()
    );
    Ok(same)
}

/// `net web`: two browser pages online (WebRTC through the lobby).
fn web(project: &Project, args: &[&str]) -> Result<bool, String> {
    let web = project.root.join("target/web");
    if !args.contains(&"--no-build") {
        crate::web::build_inner(
            project,
            &crate::web::BuildOptions {
                release: true,
                out: Some(web.clone()),
            },
        )?;
    }
    // The recording each page plays its seat of.
    let rec = project
        .engine
        .join("examples/solid-eleven/assets/shared/replays/two_players.trec");
    std::fs::create_dir_all(web.join("replays")).map_err(|e| e.to_string())?;
    std::fs::copy(&rec, web.join("replays/two_players.trec"))
        .map_err(|e| format!("{}: {e}", rec.display()))?;
    let (tx, rx) = mpsc::channel();
    lobby(0, 0, Some(tx))?;
    let (_, ws) = rx.recv().map_err(|e| e.to_string())?;
    let harness = project.engine.join("tools/web-test");
    let browser = args
        .iter()
        .position(|a| *a == "--browser")
        .and_then(|i| args.get(i + 1).copied())
        .unwrap_or("chromium");
    crate::web::ensure_harness(&harness, &[browser.to_string()])?;
    let port = crate::web::serve(web, 0)?;
    let out = project.root.join("target/web-net");
    let _ = std::fs::remove_dir_all(&out);
    let ok = crate::web::harness_command(&project.root, &harness.join("net.mjs"))
        .arg("--url")
        .arg(format!("http://127.0.0.1:{port}/"))
        .arg("--lobby")
        .arg(format!("ws://127.0.0.1:{ws}"))
        .arg("--out")
        .arg(&out)
        .args(["--browser", browser])
        .args([
            "--query",
            "&net_script=replays/two_players.trec&net_hash_at=760&frames=900",
        ])
        // A spectator page too (issue #117).
        .args(args.contains(&"--watch").then_some("--watch"))
        .status()
        .is_ok_and(|s| s.success());
    println!(
        "[net] web: {} ({})",
        if ok {
            "both pages played the same game"
        } else {
            "FAILED"
        },
        out.join("results.json").display()
    );
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two UDP clients meet in a room of the real server.
    #[test]
    fn the_lobby_server_seats_two_udp_players() {
        let (tx, rx) = mpsc::channel();
        lobby(0, 0, Some(tx)).unwrap();
        let (udp, _) = rx.recv().unwrap();
        let server: SocketAddr = format!("127.0.0.1:{udp}").parse().unwrap();
        let join = move || {
            std::thread::spawn(move || {
                let mut s = trino_net::udp::UdpSocket::bind("127.0.0.1:0").unwrap();
                trino_net::lobby::rendezvous(
                    &mut s,
                    server,
                    "TEST",
                    2,
                    std::time::Duration::from_secs(10),
                )
                .unwrap()
            })
        };
        let (a, b) = (join(), join());
        let (a, b) = (a.join().unwrap(), b.join().unwrap());
        let mut seats = [a.0, b.0];
        seats.sort();
        assert_eq!(seats, [0, 1]);
        assert_eq!(a.1.len(), 1);
        assert_eq!(b.1.len(), 1);
    }
}
