//! One peer of a network session: ggrs's P2P session driving a
//! [`Game`](trino_core::Game) through the engine's own contract.
//!
//! Each host frame, [`NetSession::frame`] polls the network, hands ggrs
//! this machine's input (quantized: [`NetPad`]) and runs what ggrs asks:
//! save the game's state (with its [`state_hash`](trino_core::Game::state_hash)
//! as the checksum the peers compare), load an older one (a rollback), and
//! advance with every player's input — published as the platform's
//! [`PlayersInput`] before each update, with a fixed `dt`. Simulated-again
//! frames play no sound: only the newest frame of a batch hears the
//! game's audio.

use core::net::SocketAddr;
use std::time::Duration;

use ggrs::{
    Config, DesyncDetection, GgrsError, GgrsEvent, GgrsRequest, InputStatus, NonBlockingSocket,
    P2PSession, PlayerType, PredictRepeatLast, SessionBuilder, SessionState,
};
use serde::{Deserialize, Serialize};
use trino_core::{Audio, Game, InputState, NET_PAD_LEN, NetPad, Platform, PlayersInput};

use crate::config::NetConfig;

/// A peer on any transport.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum PeerAddr {
    /// A [`crate::SimNet`] endpoint.
    Sim(u8),
    /// A UDP address (the PC).
    Udp(SocketAddr),
    /// A WebRTC data channel, by the lobby's peer number (the Web).
    Rtc(u16),
}

/// What crosses the network per player and frame: a [`NetPad`]'s bytes.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, Serialize, Deserialize)]
pub struct Wire(pub [u8; NET_PAD_LEN]);

/// The engine's ggrs configuration.
#[derive(Debug)]
pub struct TrinoGgrs;

impl Config for TrinoGgrs {
    type Input = Wire;
    type InputPredictor = PredictRepeatLast;
    type State = Vec<u8>;
    type Address = PeerAddr;
}

/// Where a session is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NetState {
    /// Handshaking with the peers (`count` of `total` round trips).
    Synchronizing { count: u32, total: u32 },
    /// Playing.
    Running,
    /// A peer left; its seat reads disconnected and the game goes on.
    PeerLeft,
    /// The peers' state hashes differ at `frame`: the sims went apart.
    Desynced { frame: i32, local: u64, remote: u64 },
}

/// What a session did so far.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct NetStats {
    /// Frames the game advanced (newest frames, not re-simulations).
    pub frames: u64,
    /// Rollbacks (state loads) and the frames simulated again after them.
    pub rollbacks: u64,
    pub resimulated: u64,
    /// Host frames that advanced nothing: waiting for a late peer (the
    /// prediction window full, lockstep's wait, a recommended skip).
    pub stalls: u64,
    /// State hashes compared with a peer, and the frames they disagreed on.
    pub checksums: u64,
    pub desyncs: u64,
    /// Round trip to the first remote peer (ms) and our frames ahead of it.
    pub ping_ms: u32,
    pub frames_ahead: i32,
}

/// Audio that plays nothing (a re-simulated frame).
struct Mute;

impl Audio for Mute {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// One peer's session.
pub struct NetSession {
    p2p: P2PSession<TrinoGgrs>,
    local: usize,
    input_delay: u8,
    players: usize,
    dt: f32,
    state: NetState,
    stats: NetStats,
    skip: u32,
    /// The newest frame's players (what the game saw last).
    last: PlayersInput,
    remote: Option<PeerAddr>,
}

impl NetSession {
    /// Start a session: this machine plays seat `local`; `peers` are the
    /// other seats' addresses (seat, address). Every peer must use the same
    /// `cfg` and seats.
    pub fn new(
        cfg: &NetConfig,
        local: usize,
        peers: &[(usize, PeerAddr)],
        socket: impl NonBlockingSocket<PeerAddr> + 'static,
    ) -> Result<NetSession, String> {
        let players = cfg.players.clamp(2, trino_core::MAX_PLAYERS as u8) as usize;
        let e = |e: GgrsError| e.to_string();
        let mut b = SessionBuilder::<TrinoGgrs>::new()
            .with_num_players(players)
            .map_err(e)?
            .with_max_prediction_window(cfg.prediction())
            .with_input_delay(cfg.input_delay as usize)
            .with_fps(cfg.fps as usize)
            .map_err(e)?
            .with_disconnect_timeout(Duration::from_millis(cfg.disconnect_ms as u64))
            .with_desync_detection_mode(if cfg.desync_every == 0 {
                DesyncDetection::Off
            } else {
                DesyncDetection::On {
                    interval: cfg.desync_every as u32,
                }
            })
            .add_player(PlayerType::Local, local)
            .map_err(e)?;
        for (seat, addr) in peers {
            b = b.add_player(PlayerType::Remote(*addr), *seat).map_err(e)?;
        }
        let p2p = b.start_p2p_session(socket).map_err(e)?;
        Ok(NetSession {
            p2p,
            local,
            input_delay: cfg.input_delay,
            players,
            dt: cfg.dt(),
            state: NetState::Synchronizing { count: 0, total: 0 },
            stats: NetStats::default(),
            skip: 0,
            last: PlayersInput::NONE,
            remote: peers.first().map(|p| p.1),
        })
    }

    pub fn state(&self) -> NetState {
        self.state
    }

    pub fn stats(&self) -> NetStats {
        self.stats
    }

    /// The seat this machine plays.
    pub fn local_seat(&self) -> usize {
        self.local
    }

    /// Frames a local input waits before it plays.
    pub fn input_delay(&self) -> u8 {
        self.input_delay
    }

    /// The fixed `dt` every peer simulates with.
    pub fn dt(&self) -> f32 {
        self.dt
    }

    /// The newest frame ggrs has confirmed on every peer.
    pub fn confirmed_frame(&self) -> i32 {
        self.p2p.confirmed_frame()
    }

    /// The newest frame this peer simulated.
    pub fn current_frame(&self) -> i32 {
        self.p2p.current_frame()
    }

    /// The players the game saw on the newest frame.
    pub fn players(&self) -> &PlayersInput {
        &self.last
    }

    fn events(&mut self) {
        let events: Vec<_> = self.p2p.events().collect();
        for ev in events {
            match ev {
                GgrsEvent::Synchronizing { count, total, .. } => {
                    if !matches!(self.state, NetState::Running) {
                        self.state = NetState::Synchronizing { count, total };
                    }
                }
                GgrsEvent::Synchronized { .. } => self.state = NetState::Running,
                GgrsEvent::Disconnected { .. } => self.state = NetState::PeerLeft,
                GgrsEvent::WaitRecommendation { skip_frames } => self.skip += skip_frames,
                GgrsEvent::DesyncDetected {
                    frame,
                    local_checksum,
                    remote_checksum,
                    ..
                } => {
                    self.stats.desyncs += 1;
                    if !matches!(self.state, NetState::Desynced { .. }) {
                        self.state = NetState::Desynced {
                            frame,
                            local: local_checksum as u64,
                            remote: remote_checksum as u64,
                        };
                    }
                }
                GgrsEvent::NetworkInterrupted { .. } | GgrsEvent::NetworkResumed { .. } => {}
            }
        }
        if self.p2p.current_state() == SessionState::Running
            && matches!(self.state, NetState::Synchronizing { .. })
        {
            self.state = NetState::Running;
        }
        if let Some(addr) = self.remote {
            let handles = self.p2p.handles_by_address(addr);
            if let Some(h) = handles.first()
                && let Ok(s) = self.p2p.network_stats(*h)
            {
                self.stats.ping_ms = s.ping as u32;
            }
        }
        self.stats.frames_ahead = self.p2p.frames_ahead();
    }

    /// One host frame: `local` is this machine's input. Returns true when
    /// the game advanced to a new frame (then render it); false while the
    /// peers synchronize or this peer waits for a late one.
    pub fn frame(
        &mut self,
        game: &mut dyn Game,
        platform: &mut dyn Platform,
        audio: &mut dyn Audio,
        local: &InputState,
    ) -> bool {
        self.p2p.poll_remote_clients();
        self.events();
        if self.p2p.current_state() != SessionState::Running {
            return false;
        }
        if self.skip > 0 {
            self.skip -= 1;
            self.stats.stalls += 1;
            return false;
        }
        let wire = Wire(NetPad::from_input(local).to_bytes());
        if self.p2p.add_local_input(self.local, wire).is_err() {
            return false;
        }
        let requests = match self.p2p.advance_frame() {
            Ok(r) => r,
            Err(GgrsError::PredictionThreshold | GgrsError::NotSynchronized) => {
                self.stats.stalls += 1;
                return false;
            }
            Err(_) => return false,
        };
        let advances = requests
            .iter()
            .filter(|r| matches!(r, GgrsRequest::AdvanceFrame { .. }))
            .count();
        if advances == 0 {
            // Lockstep waiting for a remote input.
            self.stats.stalls += 1;
        }
        if let Some(p) = run_requests(
            requests,
            game,
            platform,
            audio,
            self.players,
            self.dt,
            &mut self.stats,
        ) {
            self.last = p;
        }
        if self.p2p.desync_detection() != DesyncDetection::Off {
            self.stats.checksums = self.stats.frames / self.desync_interval().max(1) as u64;
        }
        advances > 0
    }

    pub(crate) fn desync_interval(&self) -> u32 {
        match self.p2p.desync_detection() {
            DesyncDetection::On { interval } => interval,
            DesyncDetection::Off => 0,
        }
    }
}

/// Carry out a batch of ggrs requests on `game`: snapshots with the state
/// hash as checksum, rollbacks, and advances with every player's input
/// published to `platform` — the newest frame with `audio`, re-simulated
/// ones silent. Returns the newest frame's players.
pub fn run_requests(
    requests: Vec<GgrsRequest<TrinoGgrs>>,
    game: &mut dyn Game,
    platform: &mut dyn Platform,
    audio: &mut dyn Audio,
    players: usize,
    dt: f32,
    stats: &mut NetStats,
) -> Option<PlayersInput> {
    let advances = requests
        .iter()
        .filter(|r| matches!(r, GgrsRequest::AdvanceFrame { .. }))
        .count();
    let mut seen = 0;
    let mut rolled_back = false;
    let mut newest = None;
    for r in requests {
        match r {
            GgrsRequest::SaveGameState { cell, frame } => {
                let mut buf = vec![0u8; game.state_size()];
                let n = game.save_state(&mut buf).unwrap_or(0);
                buf.truncate(n);
                let hash = game.state_hash().map(|h| h as u128);
                cell.save(frame, Some(buf), hash);
            }
            GgrsRequest::LoadGameState { cell, .. } => {
                if let Some(bytes) = cell.load() {
                    game.load_state(&bytes);
                }
                stats.rollbacks += 1;
                rolled_back = true;
            }
            GgrsRequest::AdvanceFrame { inputs } => {
                seen += 1;
                let mut p = PlayersInput::NONE;
                for (i, (w, status)) in inputs.iter().enumerate().take(players) {
                    if *status != InputStatus::Disconnected {
                        p.connected |= 1 << i;
                        p.players[i] = NetPad::from_bytes(w.0).to_input();
                    }
                }
                if let Some(h) = platform.hosted_players_mut() {
                    h.publish_players(&p);
                }
                if seen == advances {
                    game.update_with(platform, &p.players[0], audio, dt);
                    stats.frames += 1;
                    newest = Some(p);
                } else {
                    game.update_with(platform, &p.players[0], &mut Mute, dt);
                    if rolled_back {
                        stats.resimulated += 1;
                    }
                }
            }
        }
    }
    newest
}

/// A one-machine session that rolls back every frame and compares each
/// re-simulated frame's state hash with the first run's (ggrs's
/// `SyncTestSession`): a game whose snapshot misses something it updates
/// fails here before it desyncs two machines.
pub struct SyncTest {
    s: ggrs::SyncTestSession<TrinoGgrs>,
    players: usize,
    dt: f32,
    pub stats: NetStats,
}

impl SyncTest {
    /// `players` local players, rolling back `check_distance` frames.
    pub fn new(players: usize, check_distance: usize, fps: u8) -> Result<SyncTest, String> {
        let s = SessionBuilder::<TrinoGgrs>::new()
            .with_num_players(players)
            .map_err(|e| e.to_string())?
            .with_check_distance(check_distance)
            .with_max_prediction_window(check_distance + 1)
            .start_synctest_session()
            .map_err(|e| e.to_string())?;
        Ok(SyncTest {
            s,
            players,
            dt: 1.0 / fps.max(1) as f32,
            stats: NetStats::default(),
        })
    }

    /// One frame with every player's input; `Err` names the frames whose
    /// hashes differed after a rollback.
    pub fn frame(
        &mut self,
        game: &mut dyn Game,
        platform: &mut dyn Platform,
        audio: &mut dyn Audio,
        inputs: &PlayersInput,
    ) -> Result<(), String> {
        for i in 0..self.players {
            let w = Wire(NetPad::from_input(&inputs.players[i]).to_bytes());
            self.s.add_local_input(i, w).map_err(|e| e.to_string())?;
        }
        let requests = self.s.advance_frame().map_err(|e| e.to_string())?;
        run_requests(
            requests,
            game,
            platform,
            audio,
            self.players,
            self.dt,
            &mut self.stats,
        );
        Ok(())
    }
}
