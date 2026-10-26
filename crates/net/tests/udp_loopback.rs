//! Two peers over real UDP on this machine (issue #67): the host serves its
//! own room (`TRINO_NET=host:<port>`), the guest joins it
//! (`TRINO_NET=join:<addr>`), both play a synthetic game under rollback
//! with latency and loss added on the wall clock ([`Conditioned`]), and
//! they end on the same state.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use trino_core::snapshot::{load_copy, save_copy, size_of_copy};
use trino_core::{
    Audio, Button, Game, HostedPlayers, InputState, MusicId, Platform, Renderer, SoundId,
    StateHasher, Vec2,
};
use trino_net::udp::{Conditioned, UdpSocket};
use trino_net::{Conditions, NetConfig, NetSession, NetState, PeerAddr};

#[derive(Clone, Copy)]
struct Synth {
    acc: [i64; 2],
    frame: u32,
}

impl Game for Synth {
    fn update(&mut self, _: &InputState, _: &mut dyn Audio, _: f32) {}
    fn update_with(&mut self, p: &mut dyn Platform, _: &InputState, _: &mut dyn Audio, _: f32) {
        let players = *p.players();
        for i in 0..2 {
            let s = &players.players[i];
            self.acc[i] = self.acc[i]
                .wrapping_mul(31)
                .wrapping_add((s.stick.x * 127.0) as i64 + s.button_bits() as i64 + i as i64);
        }
        self.frame += 1;
    }
    fn render(&mut self, _: &mut dyn Renderer) {}
    fn state_hash(&self) -> Option<u64> {
        Some(
            StateHasher::new()
                .u64(self.acc[0] as u64)
                .u64(self.acc[1] as u64)
                .u32(self.frame)
                .value(),
        )
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

struct Host(HostedPlayers);
impl Platform for Host {
    fn now_micros(&self) -> u64 {
        0
    }
    fn log(&self, _: &str) {}
    fn hosted_players(&self) -> Option<&HostedPlayers> {
        Some(&self.0)
    }
    fn hosted_players_mut(&mut self) -> Option<&mut HostedPlayers> {
        Some(&mut self.0)
    }
}

struct Mute;
impl Audio for Mute {
    fn play_sound(&mut self, _: SoundId) {}
    fn play_music(&mut self, _: MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

fn input(seat: usize, f: i32) -> InputState {
    let mut s = InputState::IDLE;
    if !(0..500).contains(&f) {
        return s;
    }
    let k = (f as u32 / (4 + seat as u32)).wrapping_mul(2_654_435_761);
    s.stick = Vec2::new(((k % 200) as f32 - 100.0) / 100.0, 0.0);
    s.set(Button::B, k.is_multiple_of(3));
    s
}

/// Play until frame 560 is confirmed; returns (the hash at frame 560,
/// frames, rollbacks).
fn play(socket: Conditioned<UdpSocket>, seat: usize, peer: (usize, PeerAddr)) -> (u64, u32, u64) {
    let cfg = NetConfig {
        desync_every: 1,
        ..NetConfig::DEFAULT
    };
    let mut s = NetSession::new(&cfg, seat, &[peer], socket).unwrap();
    let mut g = Synth {
        acc: [0; 2],
        frame: 0,
    };
    let mut host = Host(HostedPlayers::new());
    let t0 = Instant::now();
    let mut at_560 = 0;
    loop {
        let f = s.current_frame() + cfg.input_delay as i32;
        s.frame(&mut g, &mut host, &mut Mute, &input(seat, f));
        if g.frame == 560 {
            // Inputs are idle from 500 on: frame 560 is right the first
            // time it is simulated.
            at_560 = g.state_hash().unwrap();
        }
        if let NetState::Desynced { frame, .. } = s.state() {
            panic!("seat {seat}: desync at {frame}");
        }
        if g.frame >= 560 && s.confirmed_frame() >= 559 {
            // Keep answering a few frames: the other peer may still need
            // our inputs to confirm its own.
            let until = Instant::now() + Duration::from_millis(400);
            while Instant::now() < until {
                let f = s.current_frame() + cfg.input_delay as i32;
                s.frame(&mut g, &mut host, &mut Mute, &input(seat, f));
                std::thread::sleep(Duration::from_millis(4));
            }
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(60), "seat {seat} stuck");
        std::thread::sleep(Duration::from_millis(4));
    }
    (at_560, g.frame, s.stats().rollbacks)
}

#[test]
fn a_host_and_a_guest_play_over_udp_with_latency_and_loss() {
    let cond = Conditions {
        latency_ms: 30,
        jitter_ms: 5,
        loss: 0.05,
    };
    let mut host_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let host_addr = host_sock.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    let tx2 = tx.clone();
    let host = std::thread::spawn(move || {
        let guests = host_sock
            .host_room("direct", 2, Duration::from_secs(20))
            .unwrap();
        assert_eq!(guests.len(), 1);
        let (seat, addr) = guests[0];
        assert_eq!(seat, 1);
        let r = play(
            Conditioned::new(host_sock, cond, 1),
            0,
            (seat, PeerAddr::Udp(addr)),
        );
        tx.send((0, r)).unwrap();
    });
    let guest = std::thread::spawn(move || {
        let mut sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let (seat, peers) = trino_net::lobby::rendezvous(
            &mut sock,
            host_addr,
            "direct",
            2,
            Duration::from_secs(20),
        )
        .unwrap();
        assert_eq!((seat, peers.clone()), (1, vec![(0, host_addr)]));
        let r = play(
            Conditioned::new(sock, cond, 2),
            1,
            (0, PeerAddr::Udp(host_addr)),
        );
        tx2.send((1, r)).unwrap();
    });
    host.join().unwrap();
    guest.join().unwrap();
    let mut results: Vec<_> = rx.try_iter().collect();
    results.sort_by_key(|r| r.0);
    let ((_, a), (_, b)) = (results[0], results[1]);
    println!("host {a:?}, guest {b:?}");
    // Both ran past frame 560, every confirmed frame's hash compared by
    // ggrs on the way (desync_every = 1).
    assert!(a.1 >= 560 && b.1 >= 560);
    assert_eq!(a.0, b.0, "the peers' frame 560 differ");
    assert!(a.2 + b.2 > 0, "5 % loss and 30 ms never rolled back");
}
