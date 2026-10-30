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
//! - `net config [--json]`, `net config set players.max=2 net.mode=delay ..`
//!   — `trino.toml`'s `[players]` and `[net]`, read and written in place
//!   (the editor's Net tab writes the same keys).
//! - `net web [--no-build] [--browser chromium|firefox]` — the same in a
//!   browser: the Web build served, a lobby, two pages of Solid Eleven
//!   meeting in a room and playing over WebRTC (`tools/web-test/net.mjs`),
//!   each its seat of the two-player recording; both pages must log the
//!   same hash on frame 760.

use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex, mpsc};

use serde_json::json;
use trino_net::lobby::{Rooms, UDP_PREFIX};

use crate::project::Project;

const USAGE: &str = "xtask: usage:\n  cargo xtask net lobby [--udp 7400] [--ws 7401]\n  \
     cargo xtask net local [--game solid-eleven] [--sim latency=80,jitter=10,loss=3] \
     [--mode rollback|delay] [--rollback N] [--delay N] [--port 7311] [--script file.trec] \
     [--hash-at N] [--frames N] [--json]\n  \
     cargo xtask net web [--no-build] [--browser chromium|firefox]\n  \
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
        let mut buf = [0u8; 2048];
        loop {
            let Ok((n, from)) = sock.recv_from(&mut buf) else {
                continue;
            };
            let Ok(line) = std::str::from_utf8(&buf[..n]) else {
                continue;
            };
            let out = udp_rooms
                .lock()
                .map(|mut r| r.handle(&from.to_string(), line, UDP_PREFIX))
                .unwrap_or_default();
            for (to, msg) in out {
                if let Ok(a) = to.parse::<SocketAddr>() {
                    let _ = sock.send_to(msg.as_bytes(), a);
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
        ] {
            if let Some(v) = v {
                c.env(var, v);
            }
        }
        c.stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{}: {e}", exe.display()))
    };
    println!("[net] {game}: host on UDP {port}, guest joins; network {sim}");
    let host = spawn("host", format!("host:{port}"))?;
    std::thread::sleep(std::time::Duration::from_millis(500));
    let guest = spawn("guest", format!("join:127.0.0.1:{port}"))?;
    let mut hashes = Vec::new();
    let mut lines = Vec::new();
    for (role, child) in [("host", host), ("guest", guest)] {
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
    let same = script.is_none() || (hashes[0].is_some() && hashes[0] == hashes[1]);
    if json_out {
        println!(
            "{}",
            json!({ "ok": same, "hashes": hashes, "lines": lines, "screenshots": out })
        );
    } else if script.is_some() {
        println!(
            "[net] {}: host {:?}, guest {:?}",
            if same {
                "same game on both"
            } else {
                "the games differ"
            },
            hashes[0],
            hashes[1]
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
