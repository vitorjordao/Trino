//! Online play in the browser (issue #67): `?net=<room>&lobby=ws://<host>:7401`
//! meets the other players through the lobby (`cargo xtask net lobby`),
//! opens WebRTC data channels and runs the session under rollback, from
//! the game's first frame (the PC's `TRINO_NET`, in a page).
//!
//! Also `?players=2..4`, `?stun=stun:<host>:<port>`, `?net_delay=N`,
//! `?net_mode=rollback|delay`, and for the end-to-end test
//! `?net_script=<url.trec>` (this page plays its seat of a recording) with
//! `?net_hash_at=N` (logs `TRINO_NET_HASH:N:<hash>` on game frame N).
//! `__trino.info.net` shows the connection.

use trino_core::replay::{Neutral, Reader};
use trino_core::{Audio, Game, InputState, Platform, PlayersInput};
use trino_net::rtc::{MeetStatus, Meeting};
use trino_net::{NetConfig, NetSession};
use trino_platform_web::{Query, WebHost, js};

enum Phase {
    Meeting(Meeting),
    Playing(Box<NetSession>),
    Failed,
}

pub struct WebNet {
    phase: Option<Phase>,
    cfg: NetConfig,
    script: Option<Vec<PlayersInput>>,
    hash_at: Option<u64>,
    last_status: String,
    frames: u64,
}

impl WebNet {
    /// The page's online session (`None` without `?net=`).
    pub async fn from_query(query: &Query, host: &WebHost) -> Result<Option<WebNet>, String> {
        let Some(code) = query.get("net") else {
            return Ok(None);
        };
        let lobby = query
            .get("lobby")
            .ok_or("?net needs ?lobby=ws://<host>:<port> (cargo xtask net lobby)")?;
        let baked = host
            .platform
            .blob(trino_core::BlobId::from_path("net/config"));
        let (mut cfg, _) = NetConfig::from_baked(baked);
        let mut over = String::new();
        for (q, key) in [
            ("players", "players"),
            ("net_delay", "input_delay"),
            ("net_mode", "mode"),
            ("net_rollback", "max_rollback"),
        ] {
            if let Some(v) = query.get(q) {
                over.push_str(&format!("{key} = {v}\n"));
            }
        }
        for p in cfg.apply_text(&over) {
            js::log(&format!("TRINO_NET: {p}"));
        }
        let script = match query.get("net_script") {
            None => None,
            Some(url) => {
                let bytes = trino_platform_web::fetch(&url).await?;
                let mut r = Reader::new(&bytes).map_err(|e| e.message().to_string())?;
                let mut out = Vec::new();
                while let Some(f) = r.next_frame() {
                    out.push(f.map_err(|e| e.message().to_string())?.players);
                }
                Some(out)
            }
        };
        let meeting = trino_net::rtc::meet(
            &lobby,
            &code,
            cfg.players as usize,
            query.get("stun").as_deref(),
        )?;
        js::log(&format!("TRINO_NET: room `{code}` at {lobby}"));
        Ok(Some(WebNet {
            phase: Some(Phase::Meeting(meeting)),
            cfg,
            script,
            hash_at: query.get("net_hash_at").and_then(|v| v.parse().ok()),
            last_status: String::new(),
            frames: 0,
        }))
    }

    fn status(&mut self, s: String) {
        if s != self.last_status {
            js::log(&format!("TRINO_NET: {s}"));
            js::trino_info("net", &s);
            self.last_status = s;
        }
    }

    /// One page frame: meet, then play. True when the game moved to a new
    /// frame.
    pub fn frame(
        &mut self,
        game: &mut dyn Game,
        platform: &mut dyn Platform,
        audio: &mut dyn Audio,
        live: &InputState,
    ) -> bool {
        match self.phase.take() {
            Some(Phase::Meeting(m)) => {
                let st = m.status();
                self.status(format!("{st:?}"));
                self.phase = Some(match st {
                    MeetStatus::Failed(_) => Phase::Failed,
                    MeetStatus::Ready => match m.take() {
                        Ok((seat, _, peers, socket)) => {
                            match NetSession::new(&self.cfg, seat, &peers, socket) {
                                Ok(s) => {
                                    js::log(&format!(
                                        "TRINO_NET: player {} of {}, {} mode",
                                        seat + 1,
                                        self.cfg.players,
                                        self.cfg.mode.key()
                                    ));
                                    Phase::Playing(Box::new(s))
                                }
                                Err(e) => {
                                    self.status(format!("Failed({e})"));
                                    Phase::Failed
                                }
                            }
                        }
                        Err(m) => Phase::Meeting(m),
                    },
                    _ => Phase::Meeting(m),
                });
                false
            }
            Some(Phase::Playing(mut s)) => {
                let scripted;
                let local = match &self.script {
                    Some(frames) => {
                        let f = s.current_frame() + s.input_delay() as i32;
                        scripted = frames
                            .get(f.max(0) as usize)
                            .map_or(InputState::IDLE, |p| p.players[s.local_seat()]);
                        &scripted
                    }
                    None => live,
                };
                let advanced = s.frame(game, &mut Neutral(platform), audio, local);
                if advanced {
                    self.frames += 1;
                    if Some(s.stats().frames) == self.hash_at {
                        js::log(&format!(
                            "TRINO_NET_HASH:{}:{:#018x}",
                            s.stats().frames,
                            game.state_hash().unwrap_or(0)
                        ));
                    }
                }
                let st = s.stats();
                self.status(format!(
                    "{:?} seat={} rollbacks={} desyncs={}",
                    s.state(),
                    s.local_seat(),
                    st.rollbacks / 60 * 60,
                    st.desyncs
                ));
                self.phase = Some(Phase::Playing(s));
                advanced
            }
            other => {
                self.phase = other;
                false
            }
        }
    }

    /// The `TRINO_NET:` report (at the end of a smoke run).
    pub fn line(&self) -> String {
        match &self.phase {
            Some(Phase::Playing(s)) => {
                let st = s.stats();
                format!(
                    "TRINO_NET: state={:?} seat={} frames={} confirmed={} rollbacks={} stalls={} desyncs={}",
                    s.state(),
                    s.local_seat(),
                    st.frames,
                    s.confirmed_frame(),
                    st.rollbacks,
                    st.stalls,
                    st.desyncs
                )
            }
            _ => format!("TRINO_NET: never played ({})", self.last_status),
        }
    }

    /// Frames the game advanced online.
    pub fn frames(&self) -> u64 {
        self.frames
    }
}
