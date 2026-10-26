//! Online play in the PC runner (issue #67): `TRINO_NET` puts the whole
//! session under a rollback [`NetSession`], from the first frame.
//!
//! - `TRINO_NET=host:<port>` — serve a room on UDP `<port>` and wait for the
//!   other players (this machine is player 1);
//! - `TRINO_NET=join:<ip:port>` — join a host's room;
//! - `TRINO_NET=lobby:<code>@<ip:port>` — meet in room `<code>` of a lobby
//!   server (`cargo xtask net lobby`);
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
//! every few seconds and at exit.

use std::time::{Duration, Instant};

use trino_core::{Audio, Game, InputState, Platform};
use trino_net::udp::{Conditioned, UdpSocket};
use trino_net::{Conditions, NetConfig, NetSession, NetState, PeerAddr};

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
}

impl Meet {
    pub fn parse(v: &str) -> Result<Meet, String> {
        let bad =
            || format!("TRINO_NET={v}: host:<port>, join:<ip:port> or lobby:<code>@<ip:port>");
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
        let cond = std::env::var("TRINO_NET_SIM")
            .ok()
            .map(|v| {
                Conditions::parse(&v).ok_or(format!("TRINO_NET_SIM={v}: latency=…,jitter=…,loss=…"))
            })
            .transpose()?
            .unwrap_or(Conditions::NONE);
        let (socket, seat, peers) = match meet {
            Meet::Host(port) => {
                let mut s =
                    UdpSocket::bind(("0.0.0.0", *port)).map_err(|e| format!("UDP {port}: {e}"))?;
                println!(
                    "TRINO_NET: hosting on UDP {port}, waiting for {} player(s)",
                    players - 1
                );
                let guests = s.host_room("direct", players, MEET_TIMEOUT)?;
                (s, 0, guests)
            }
            Meet::Join(addr) => {
                let mut s = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("UDP: {e}"))?;
                println!("TRINO_NET: joining {addr}");
                let (seat, peers) =
                    trino_net::lobby::rendezvous(&mut s, *addr, "direct", players, MEET_TIMEOUT)?;
                (s, seat, peers)
            }
            Meet::Lobby { code, server } => {
                let mut s = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("UDP: {e}"))?;
                println!("TRINO_NET: room `{code}` at the lobby {server}");
                let (seat, peers) =
                    trino_net::lobby::rendezvous(&mut s, *server, code, players, MEET_TIMEOUT)?;
                (s, seat, peers)
            }
        };
        let peers: Vec<(usize, PeerAddr)> = peers
            .into_iter()
            .map(|(s, a)| (s, PeerAddr::Udp(a)))
            .collect();
        println!(
            "TRINO_NET: player {} of {players}, {} mode, rollback {} input delay {}{}",
            seat + 1,
            cfg.mode.key(),
            cfg.max_rollback,
            cfg.input_delay,
            if cond == Conditions::NONE {
                String::new()
            } else {
                format!(", simulated {cond:?}")
            }
        );
        let seed = std::process::id() as u64;
        let session = NetSession::new(cfg, seat, &peers, Conditioned::new(socket, cond, seed))?;
        let script = match std::env::var("TRINO_NET_SCRIPT")
            .ok()
            .filter(|v| !v.is_empty())
        {
            None => None,
            Some(path) => Some(read_script(&path)?),
        };
        let hash_at = std::env::var("TRINO_NET_HASH_AT")
            .ok()
            .and_then(|v| v.parse().ok());
        Ok(NetHost {
            session,
            last_report: Instant::now(),
            label: String::new(),
            script,
            hash_at,
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
    ) -> bool {
        let mut neutral = trino_core::replay::Neutral(platform);
        let scripted;
        let local = match &self.script {
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
            NetState::Running => format!(
                "online P{}: ping {} ms, {} rollbacks",
                self.session.local_seat() + 1,
                s.ping_ms,
                s.rollbacks
            ),
            NetState::PeerLeft => "online: the other player left".into(),
            NetState::Desynced { frame, .. } => format!("online: DESYNC at frame {frame}"),
        }
    }

    /// The `TRINO_NET:` report line.
    pub fn line(&self) -> String {
        let s = self.session.stats();
        format!(
            "TRINO_NET: state={:?} seat={} frames={} confirmed={} rollbacks={} resimulated={} \
             stalls={} ping_ms={} desyncs={}",
            self.session.state(),
            self.session.local_seat(),
            s.frames,
            self.session.confirmed_frame(),
            s.rollbacks,
            s.resimulated,
            s.stalls,
            s.ping_ms,
            s.desyncs
        )
    }
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
        assert!(Meet::parse("nope").is_err());
        let cfg = config(Some(b"max_players = 4\nmode = delay\ninput_delay = 4\n"));
        assert_eq!((cfg.mode, cfg.input_delay), (trino_net::NetMode::Delay, 4));
    }
}
