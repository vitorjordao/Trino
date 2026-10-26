//! Rollback sessions under latency and loss (issue #67): two peers of a
//! synthetic deterministic game over [`SimNet`], 10 000 frames for each
//! latency (0–200 ms, with jitter) and loss (0–10 %), state hashes compared
//! on every frame by ggrs's desync detection, and both peers' final state
//! held to a single-machine simulation of the same inputs. Then delay mode
//! (lockstep), a third peer, and a peer that leaves.

use trino_core::snapshot::{load_copy, save_copy, size_of_copy};
use trino_core::{
    Audio, Button, Game, HostedPlayers, InputState, MusicId, NetPad, Platform, PlayersInput,
    Renderer, SoundId, StateHasher, Vec2,
};
use trino_net::{Conditions, NetConfig, NetMode, NetSession, NetState, PeerAddr, SimNet};

/// A game whose state mixes every player's input every frame: any input a
/// peer got wrong shows in its hash.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Synth {
    pos: [[i32; 2]; 4],
    rng: u64,
    frame: u32,
    sounds: u32,
}

impl Synth {
    fn new() -> Synth {
        Synth {
            pos: [[0; 2]; 4],
            rng: 0x9e37_79b9_7f4a_7c15,
            frame: 0,
            sounds: 0,
        }
    }

    fn step(&mut self, players: &PlayersInput) {
        for (i, p) in players.players.iter().enumerate() {
            if !players.is_connected(i) {
                continue;
            }
            let s = p.stick;
            self.pos[i][0] = self.pos[i][0].wrapping_add((s.x * 100.0) as i32);
            self.pos[i][1] = self.pos[i][1].wrapping_add((s.y * 100.0) as i32);
            if p.is_down(Button::A) {
                self.pos[i][0] = self.pos[i][0].wrapping_mul(3) ^ self.frame as i32;
            }
            self.rng = self
                .rng
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add((self.pos[i][0] ^ self.pos[i][1]) as u64 | 1);
        }
        self.frame += 1;
    }

    fn hash(&self) -> u64 {
        let mut h = StateHasher::new();
        for p in &self.pos {
            h.i32(p[0]).i32(p[1]);
        }
        h.u64(self.rng).u32(self.frame);
        h.value()
    }
}

impl Game for Synth {
    fn update(&mut self, _: &InputState, _: &mut dyn Audio, _: f32) {}
    fn update_with(
        &mut self,
        platform: &mut dyn Platform,
        _: &InputState,
        audio: &mut dyn Audio,
        _: f32,
    ) {
        let players = *platform.players();
        self.step(&players);
        if self.frame.is_multiple_of(30) {
            audio.play_sound(SoundId(1));
        }
    }
    fn render(&mut self, _: &mut dyn Renderer) {}
    fn state_hash(&self) -> Option<u64> {
        Some(self.hash())
    }
    fn state_size(&self) -> usize {
        size_of_copy::<Synth>()
    }
    fn save_state(&self, out: &mut [u8]) -> Option<usize> {
        save_copy(self, out)
    }
    fn load_state(&mut self, bytes: &[u8]) -> bool {
        load_copy(self, bytes)
    }
}

struct Host {
    players: HostedPlayers,
}

impl Platform for Host {
    fn now_micros(&self) -> u64 {
        0
    }
    fn log(&self, _: &str) {}
    fn hosted_players(&self) -> Option<&HostedPlayers> {
        Some(&self.players)
    }
    fn hosted_players_mut(&mut self) -> Option<&mut HostedPlayers> {
        Some(&mut self.players)
    }
}

/// Counts the sounds that reach the speaker (re-simulated frames must not).
struct Speaker(u32);

impl Audio for Speaker {
    fn play_sound(&mut self, _: SoundId) {
        self.0 += 1;
    }
    fn play_music(&mut self, _: MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// Seat `seat`'s input on game frame `f`: a new stick and A every few
/// frames (prediction misses), idle from `idle_from` on.
fn input(seat: usize, f: i32, idle_from: i32) -> InputState {
    let mut s = InputState::IDLE;
    if f >= idle_from || f < 0 {
        return s;
    }
    let k = (f as u32 / (5 + seat as u32 * 2)).wrapping_mul(2_654_435_761) ^ (seat as u32 * 77);
    s.stick = Vec2::new(
        ((k % 201) as f32 - 100.0) / 100.0,
        (((k >> 8) % 201) as f32 - 100.0) / 100.0,
    );
    s.set(Button::A, (k >> 16).is_multiple_of(3));
    s
}

struct Peer {
    session: NetSession,
    game: Synth,
    host: Host,
    speaker: Speaker,
}

/// The reference: one machine, every seat's real input (quantized as the
/// network does; nothing during the first `delay` frames, as the input
/// delay plays them), `frames` frames.
fn reference(players: usize, frames: i32, idle_from: i32, delay: i32) -> Vec<u64> {
    let mut g = Synth::new();
    let mut hashes = vec![g.hash()];
    for f in 0..frames {
        let mut p = PlayersInput::NONE;
        for s in 0..players {
            p.connected |= 1 << s;
            if f >= delay {
                p.players[s] = NetPad::quantize(&input(s, f, idle_from));
            }
        }
        g.step(&p);
        hashes.push(g.hash());
    }
    hashes
}

fn peers(cfg: &NetConfig, net: &SimNet, n: usize) -> Vec<Peer> {
    (0..n)
        .map(|seat| {
            let others: Vec<(usize, PeerAddr)> = (0..n)
                .filter(|s| *s != seat)
                .map(|s| (s, PeerAddr::Sim(s as u8)))
                .collect();
            Peer {
                session: NetSession::new(cfg, seat, &others, net.socket(PeerAddr::Sim(seat as u8)))
                    .unwrap(),
                game: Synth::new(),
                host: Host {
                    players: HostedPlayers::new(),
                },
                speaker: Speaker(0),
            }
        })
        .collect()
}

/// Run `n` peers until every one simulated `frames` frames and confirmed
/// them all; returns the peers.
fn soak(cfg: NetConfig, cond: Conditions, n: usize, frames: i32, seed: u64) -> Vec<Peer> {
    let net = SimNet::new(cond, seed);
    let mut ps = peers(&cfg, &net, n);
    let idle_from = frames - 60;
    // A hang is no progress for a while of *real* time: lockstep under loss
    // waits on ggrs's 200 ms wall-clock resends, so loop counts depend on
    // the machine (CI's runners spin faster than they progress).
    let start = std::time::Instant::now();
    let mut last_progress = (std::time::Instant::now(), 0u32);
    loop {
        let mut advanced = false;
        for p in ps.iter_mut() {
            // ggrs plays an input added on frame `c` on frame `c + delay`.
            let f = p.session.current_frame() + cfg.input_delay as i32;
            let local = input(p.session.local_seat(), f, idle_from);
            advanced |= p
                .session
                .frame(&mut p.game, &mut p.host, &mut p.speaker, &local);
            if let NetState::Desynced {
                frame,
                local,
                remote,
            } = p.session.state()
            {
                panic!(
                    "{cond:?} seat {}: desync at frame {frame} ({local:#x} vs {remote:#x})",
                    p.session.local_seat()
                );
            }
        }
        net.advance(1000 / cfg.fps as u64);
        // ggrs times its handshake retries on the wall clock (200 ms after
        // a lost sync packet): let real time pass until every peer is in,
        // then run as fast as the sims go (the session itself is driven by
        // frames and the simulated network's clock).
        if ps.iter().any(|p| p.session.state() != NetState::Running) {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(120),
                "{cond:?}: the peers never synchronized"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
            continue;
        }
        // Every peer waiting: ggrs re-sends unacknowledged inputs on its
        // wall-clock timer (200 ms), so let a little real time pass.
        if !advanced {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let done = ps
            .iter()
            .all(|p| p.game.frame as i32 >= frames && p.session.confirmed_frame() >= frames - 1);
        if done {
            break;
        }
        let least = ps.iter().map(|p| p.game.frame).min().unwrap_or(0);
        if least > last_progress.1 {
            last_progress = (std::time::Instant::now(), least);
        }
        assert!(
            last_progress.0.elapsed() < std::time::Duration::from_secs(20)
                && start.elapsed() < std::time::Duration::from_secs(120),
            "{cond:?}: stuck at frames {:?} (confirmed {:?})",
            ps.iter().map(|p| p.game.frame).collect::<Vec<_>>(),
            ps.iter()
                .map(|p| p.session.confirmed_frame())
                .collect::<Vec<_>>()
        );
    }
    ps
}

fn check(ps: &[Peer], cfg: &NetConfig, frames: i32, cond: Conditions, players: usize) {
    let refs = reference(players, frames + 400, frames - 60, cfg.input_delay as i32);
    for p in ps {
        let f = p.game.frame as usize;
        assert_eq!(
            p.game.hash(),
            refs[f],
            "{cond:?} seat {}: frame {f} differs from the single-machine run",
            p.session.local_seat()
        );
        let s = p.session.stats();
        assert_eq!(s.desyncs, 0);
        // Every new frame was heard once, re-simulations never.
        assert_eq!(p.speaker.0, p.game.frame / 30, "{cond:?}: sounds {s:?}");
    }
}

#[test]
fn rollback_holds_under_latency_and_loss_for_10_000_frames() {
    let cfg = NetConfig {
        desync_every: 1,
        ..NetConfig::DEFAULT
    };
    let mut table = Vec::new();
    for latency in [0, 50, 100, 200] {
        for loss in [0.0, 0.05, 0.10] {
            let cond = Conditions {
                latency_ms: latency,
                jitter_ms: if latency > 0 { 10 } else { 0 },
                loss,
            };
            let ps = soak(cfg, cond, 2, 10_000, 7 + latency as u64);
            check(&ps, &cfg, 10_000, cond, 2);
            let s = ps[0].session.stats();
            if latency >= 50 {
                assert!(s.rollbacks > 0, "{cond:?}: no rollback happened: {s:?}");
            }
            table.push(format!(
                "latency {latency:>3} ms loss {:>2}%: frames {} rollbacks {} resimulated {} stalls {}",
                (loss * 100.0) as u32,
                s.frames,
                s.rollbacks,
                s.resimulated,
                s.stalls
            ));
        }
    }
    println!("{}", table.join("\n"));
}

#[test]
fn delay_mode_is_lockstep_and_never_rolls_back() {
    let cfg = NetConfig {
        mode: NetMode::Delay,
        input_delay: 3,
        desync_every: 1,
        ..NetConfig::DEFAULT
    };
    let cond = Conditions {
        latency_ms: 80,
        jitter_ms: 10,
        loss: 0.05,
    };
    let ps = soak(cfg, cond, 2, 2_000, 99);
    check(&ps, &cfg, 2_000, cond, 2);
    for p in &ps {
        assert_eq!(p.session.stats().rollbacks, 0, "lockstep rolled back");
        assert!(
            p.session.stats().stalls > 0,
            "80 ms never made lockstep wait"
        );
    }
}

#[test]
fn three_players_stay_in_step() {
    let cfg = NetConfig {
        players: 3,
        desync_every: 1,
        ..NetConfig::DEFAULT
    };
    let cond = Conditions {
        latency_ms: 60,
        jitter_ms: 10,
        loss: 0.02,
    };
    let ps = soak(cfg, cond, 3, 2_000, 5);
    check(&ps, &cfg, 2_000, cond, 3);
}

#[test]
fn a_peer_that_leaves_reads_disconnected_and_the_game_goes_on() {
    let cfg = NetConfig {
        disconnect_ms: 500,
        ..NetConfig::DEFAULT
    };
    let net = SimNet::new(Conditions::NONE, 3);
    let mut ps = peers(&cfg, &net, 2);
    // Play a while, then peer 1 goes silent.
    for _ in 0..300 {
        for p in ps.iter_mut() {
            let f = p.session.current_frame() + 1;
            let local = input(p.session.local_seat(), f, i32::MAX);
            p.session
                .frame(&mut p.game, &mut p.host, &mut p.speaker, &local);
        }
        net.advance(16);
    }
    let before = ps[0].game.frame;
    assert!(before > 200, "only {before} frames before the cut");
    let t0 = std::time::Instant::now();
    let p = &mut ps[0];
    while p.session.state() != NetState::PeerLeft {
        let f = p.session.current_frame() + 1;
        p.session.frame(
            &mut p.game,
            &mut p.host,
            &mut p.speaker,
            &input(0, f, i32::MAX),
        );
        net.advance(16);
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(
            t0.elapsed().as_secs() < 10,
            "the silent peer was never dropped"
        );
    }
    // The game goes on alone: its seat 2 reads disconnected.
    for _ in 0..60 {
        let f = p.session.current_frame() + 1;
        p.session.frame(
            &mut p.game,
            &mut p.host,
            &mut p.speaker,
            &input(0, f, i32::MAX),
        );
        net.advance(16);
    }
    assert!(
        p.game.frame > before + 30,
        "the game stopped after the peer left"
    );
    assert!(!p.session.players().is_connected(1));
    assert!(p.session.players().is_connected(0));
}
