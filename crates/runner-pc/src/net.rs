//! Online play in the PC runner (issue #67): `TRINO_NET` puts the whole
//! session under a rollback [`NetSession`], from the first frame.
//!
//! - `TRINO_NET=host:<port>` — serve a room on UDP `<port>` and wait for the
//!   other players (this machine is player 1);
//! - `TRINO_NET=join:<ip:port>` — join a host's room;
//! - `TRINO_NET=lobby:<code>@<ip:port>` — meet in room `<code>` of a lobby
//!   server (`cargo xtask net lobby`);
//! - `TRINO_NET=watch:<code>@<ip:port>` or `watch:<ip:port>` — spectate a
//!   lobby room or a host's game (issue #117; join before it starts), the
//!   view on `TRINO_NET_FOLLOW=1..4` (player 1 by default);
//! - `TRINO_NET_ROUTE=auto|direct|relay` — how lobby peers reach each
//!   other: punch through the NATs and fall back to the lobby's relay
//!   (auto), or only one of them;
//! - `TRINO_NET_KEYS=1`, `TRINO_NET_POINTER=1` — share the keyboard / the
//!   pointer with the other players (the project's `[net] keys`/`pointer`);
//! - `TRINO_NET_PLAYERS=2..4` (default `[net] players`, 2);
//! - `TRINO_NET_SIM=latency=80,jitter=10,loss=5` — a worse network on the
//!   way in (two local instances under the editor's Net panel);
//! - `TRINO_NET_MODE`, `TRINO_NET_ROLLBACK`, `TRINO_NET_DELAY` — the
//!   project's `[net]` (`net/config`) for one run;
//! - `TRINO_NET_SCRIPT=<file.trec>` — this machine plays its seat's input
//!   from a recording instead of its devices, and `TRINO_NET_HASH_AT=<n>`
//!   prints `TRINO_NET_HASH:<n>:<hash>` on game frame `n` (the two-instance
//!   end-to-end test: both machines must print the same hash).
//!
//! Every machine runs the same game from the same start, so nothing the
//! machine keeps may change the sim: the game sees a
//! [`Neutral`](trino_core::replay::Neutral) platform (no save, default
//! settings) and every seat's input arrives quantized ([`trino_core::NetPad`]).
//! The window's title shows the connection; `TRINO_NET:` lines report it
//! every few seconds and at exit, with each peer's route (`routes=2:direct`).
//! The game's `Platform::local_seats` is this machine's seat (a spectator's
//! followed seat), so its camera and HUD follow its own player.

use std::time::{Duration, Instant};

use trino_core::{Audio, Game, InputState, Platform};
use trino_net::udp::{Conditioned, MeetAt, RoutePolicy, Routes, UdpSocket};
use trino_net::{Conditions, NetConfig, NetSession, NetState};

/// How long a host waits for its room to fill, and a guest for its host.
const MEET_TIMEOUT: Duration = Duration::from_secs(300);

/// One machine's online session.
pub struct NetHost {
    pub session: NetSession,
    last_report: Instant,
    pub label: String,
    /// `TRINO_NET_SCRIPT`: every frame's players of a recording.
    script: Option<Vec<trino_core::PlayersInput>>,
    /// `TRINO_NET_HASH_AT`.
    hash_at: Option<u64>,
    /// The room links (lobby and host meetings).
    routes: Routes,
}

/// Where to meet, from `TRINO_NET`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Meet {
    Host(u16),
    Join(std::net::SocketAddr),
    Lobby {
        code: String,
        server: std::net::SocketAddr,
    },
    /// Spectate (issue #117): a lobby room, or a host's game (`code` None).
    Watch {
        code: Option<String>,
        server: std::net::SocketAddr,
    },
}

impl Meet {
    pub fn parse(v: &str) -> Result<Meet, String> {
        let bad = || {
            format!(
                "TRINO_NET={v}: host:<port>, join:<ip:port>, lobby:<code>@<ip:port> or \
                 watch:[<code>@]<ip:port>"
            )
        };
        let resolve = |a: &str| {
            use std::net::ToSocketAddrs;
            a.to_socket_addrs()
                .ok()
                .and_then(|mut i| i.next())
                .ok_or_else(bad)
        };
        if let Some(p) = v.strip_prefix("host:") {
            return p.parse().map(Meet::Host).map_err(|_| bad());
        }
        if let Some(a) = v.strip_prefix("join:") {
            return Ok(Meet::Join(resolve(a)?));
        }
        if let Some(rest) = v.strip_prefix("lobby:") {
            let (code, server) = rest.split_once('@').ok_or_else(bad)?;
            return Ok(Meet::Lobby {
                code: code.to_string(),
                server: resolve(server)?,
            });
        }
        if let Some(rest) = v.strip_prefix("watch:") {
            return Ok(match rest.split_once('@') {
                Some((code, server)) => Meet::Watch {
                    code: Some(code.to_string()),
                    server: resolve(server)?,
                },
                None => Meet::Watch {
                    code: None,
                    server: resolve(rest)?,
                },
            });
        }
        Err(bad())
    }
}

/// The session settings: the baked `net/config` under `TRINO_NET_*`.
pub fn config(baked: Option<&[u8]>) -> NetConfig {
    let (mut cfg, problems) = NetConfig::from_baked(baked);
    for p in problems {
        eprintln!("[trino] net/config: {p}");
    }
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let mut over = String::new();
    for (var, key) in [
        ("TRINO_NET_MODE", "mode"),
        ("TRINO_NET_ROLLBACK", "max_rollback"),
        ("TRINO_NET_DELAY", "input_delay"),
        ("TRINO_NET_PLAYERS", "players"),
        ("TRINO_NET_KEYS", "keys"),
        ("TRINO_NET_POINTER", "pointer"),
    ] {
        if let Some(v) = env(var) {
            over.push_str(&format!("{key} = {v}\n"));
        }
    }
    for p in cfg.apply_text(&over) {
        eprintln!("[trino] TRINO_NET_*: {p}");
    }
    cfg
}

impl NetHost {
    /// `TRINO_NET`'s session, or `None` without the variable. Blocks until
    /// the players have met (printing where it waits).
    pub fn from_env(baked_config: Option<&[u8]>) -> Option<Result<NetHost, String>> {
        let v = std::env::var("TRINO_NET").ok().filter(|v| !v.is_empty())?;
        Some(Meet::parse(&v).and_then(|m| NetHost::start(&m, &config(baked_config))))
    }

    /// Meet as `meet` says and start the session.
    pub fn start(meet: &Meet, cfg: &NetConfig) -> Result<NetHost, String> {
        let players = cfg.players as usize;
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let cond = env("TRINO_NET_SIM")
            .map(|v| {
                Conditions::parse(&v).ok_or(format!("TRINO_NET_SIM={v}: latency=…,jitter=…,loss=…"))
            })
            .transpose()?
            .unwrap_or(Conditions::NONE);
        let policy = match env("TRINO_NET_ROUTE") {
            None => RoutePolicy::BOTH,
            Some(v) => RoutePolicy::parse(&v)
                .ok_or(format!("TRINO_NET_ROUTE={v}: auto, direct or relay"))?,
        };
        let (mut socket, at, watch) = match meet {
            Meet::Host(port) => {
                let s =
                    UdpSocket::bind(("0.0.0.0", *port)).map_err(|e| format!("UDP {port}: {e}"))?;
                println!(
                    "TRINO_NET: hosting on UDP {port}, waiting for {} player(s)",
                    players - 1
                );
                (s, MeetAt::Host, false)
            }
            Meet::Join(addr) => {
                println!("TRINO_NET: joining {addr}");
                (bind_any()?, MeetAt::Join(*addr), false)
            }
            Meet::Lobby { code, server } => {
                println!("TRINO_NET: room `{code}` at the lobby {server}");
                let at = MeetAt::Lobby {
                    code: code.clone(),
                    server: *server,
                };
                (bind_any()?, at, false)
            }
            Meet::Watch { code, server } => {
                let at = match code {
                    Some(code) => {
                        println!("TRINO_NET: watching room `{code}` at the lobby {server}");
                        MeetAt::Lobby {
                            code: code.clone(),
                            server: *server,
                        }
                    }
                    None => {
                        println!("TRINO_NET: watching the game hosted at {server}");
                        MeetAt::Join(*server)
                    }
                };
                (bind_any()?, at, true)
            }
        };
        let m = trino_net::udp::meet(&mut socket, &at, cfg, watch, policy, MEET_TIMEOUT)?;
        let routes = m.routes.clone();
        let sim = if cond == Conditions::NONE {
            String::new()
        } else {
            format!(", simulated {cond:?}")
        };
        let seed = std::process::id() as u64;
        let socket = Conditioned::new(socket, cond, seed);
        let session = match m.watching {
            Some(host) => {
                let mut s = NetSession::spectate(cfg, host, socket)?;
                let follow = env("TRINO_NET_FOLLOW")
                    .and_then(|v| v.parse::<u8>().ok())
                    .filter(|p| (1..=4).contains(p))
                    .unwrap_or(1);
                s.follow(1 << (follow - 1));
                println!(
                    "TRINO_NET: spectator of {players} players, following player {follow}{sim}"
                );
                s
            }
            None => {
                println!(
                    "TRINO_NET: player {} of {players}, {} mode, rollback {} input delay {}, \
                     {} spectator(s){sim}",
                    m.seat + 1,
                    cfg.mode.key(),
                    cfg.max_rollback,
                    cfg.input_delay,
                    m.spectators.len()
                );
                NetSession::with_spectators(cfg, m.seat, &m.peers, &m.spectators, socket)?
            }
        };
        let script = match env("TRINO_NET_SCRIPT") {
            None => None,
            Some(path) => Some(read_script(&path)?),
        };
        let hash_at = env("TRINO_NET_HASH_AT").and_then(|v| v.parse().ok());
        Ok(NetHost {
            session,
            last_report: Instant::now(),
            label: String::new(),
            script,
            hash_at,
            routes,
        })
    }

    /// One host frame: this machine's input goes out, the game advances as
    /// the session says (through a neutral platform). True when the game
    /// moved to a new frame.
    pub fn frame(
        &mut self,
        game: &mut dyn Game,
        platform: &mut dyn Platform,
        audio: &mut dyn Audio,
        local: &InputState,
        screen: (u16, u16),
    ) -> bool {
        self.session.set_local_screen(screen);
        let mut neutral = trino_core::replay::Neutral(platform);
        let scripted;
        let local = match &self.script {
            Some(_) if self.session.is_spectator() => local,
            Some(frames) => {
                // The frame this input plays on (after the input delay).
                let f = self.session.current_frame() + self.session.input_delay() as i32;
                scripted = frames
                    .get(f.max(0) as usize)
                    .map_or(InputState::IDLE, |p| p.players[self.session.local_seat()]);
                &scripted
            }
            None => local,
        };
        let advanced = self.session.frame(game, &mut neutral, audio, local);
        if advanced && Some(self.session.stats().frames) == self.hash_at {
            println!(
                "TRINO_NET_HASH:{}:{:#018x}",
                self.session.stats().frames,
                game.state_hash().unwrap_or(0)
            );
        }
        self.label = self.status();
        if self.last_report.elapsed() > Duration::from_secs(5) {
            self.last_report = Instant::now();
            println!("{}", self.line());
        }
        advanced
    }

    /// For the window's title.
    pub fn status(&self) -> String {
        let s = self.session.stats();
        match self.session.state() {
            NetState::Synchronizing { count, total } => {
                format!("online: connecting {count}/{total}")
            }
            NetState::Running if self.session.is_spectator() => format!(
                "watching P{}: {} frames behind",
                trino_core::first_seat(self.session.local_seats()) + 1,
                self.session.frames_behind()
            ),
            NetState::Running => format!(
                "online P{}: ping {} ms, {} rollbacks{}",
                self.session.local_seat() + 1,
                s.ping_ms,
                s.rollbacks,
                if self.routes.summary().contains("relay") {
                    " (relayed)"
                } else {
                    ""
                }
            ),
            NetState::PeerLeft => "online: the other player left".into(),
            NetState::Desynced { frame, .. } => format!("online: DESYNC at frame {frame}"),
        }
    }

    /// The `TRINO_NET:` report line.
    pub fn line(&self) -> String {
        let s = self.session.stats();
        let seat = if self.session.is_spectator() {
            "spectator".to_string()
        } else {
            self.session.local_seat().to_string()
        };
        format!(
            "TRINO_NET: state={:?} seat={seat} local_seats={:#06b} frames={} confirmed={} \
             rollbacks={} resimulated={} stalls={} ping_ms={} desyncs={} routes=[{}]",
            self.session.state(),
            self.session.local_seats(),
            s.frames,
            self.session.confirmed_frame(),
            s.rollbacks,
            s.resimulated,
            s.stalls,
            s.ping_ms,
            s.desyncs,
            self.routes.summary()
        )
    }
}

fn bind_any() -> Result<UdpSocket, String> {
    UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("UDP: {e}"))
}

/// A recording's players, frame by frame.
fn read_script(path: &str) -> Result<Vec<trino_core::PlayersInput>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("TRINO_NET_SCRIPT {path}: {e}"))?;
    let mut r = trino_core::replay::Reader::new(&bytes).map_err(|e| e.message().to_string())?;
    let mut out = Vec::new();
    while let Some(f) = r.next_frame() {
        out.push(f.map_err(|e| e.message().to_string())?.players);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meeting_places_parse() {
        assert_eq!(Meet::parse("host:7000"), Ok(Meet::Host(7000)));
        assert_eq!(
            Meet::parse("join:127.0.0.1:7000"),
            Ok(Meet::Join("127.0.0.1:7000".parse().unwrap()))
        );
        assert!(matches!(
            Meet::parse("lobby:ABCD@127.0.0.1:7100"),
            Ok(Meet::Lobby { ref code, .. }) if code == "ABCD"
        ));
        assert!(Meet::parse("join:").is_err());
        assert!(matches!(
            Meet::parse("watch:ABCD@127.0.0.1:7100"),
            Ok(Meet::Watch { code: Some(ref c), .. }) if c == "ABCD"
        ));
        assert!(matches!(
            Meet::parse("watch:127.0.0.1:7311"),
            Ok(Meet::Watch { code: None, .. })
        ));
        assert!(Meet::parse("nope").is_err());
        let cfg = config(Some(b"max_players = 4\nmode = delay\ninput_delay = 4\n"));
        assert_eq!((cfg.mode, cfg.input_delay), (trino_net::NetMode::Delay, 4));
    }
}
