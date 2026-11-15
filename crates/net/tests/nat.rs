//! NAT traversal (issue #117): peers behind every pair of simulated NATs
//! meet through the lobby, punch a direct path where the NATs allow it and
//! fall back to the lobby's relay where they don't, and play the same game
//! either way. Also: two machines behind one NAT without hairpinning (their
//! LAN addresses), four players and a spectator across mixed NATs, a forced
//! relay, and a peer whose settings differ refused at the door.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use trino_core::snapshot::{load_copy, save_copy, size_of_copy};
use trino_core::{
    Audio, Button, Game, HostedPlayers, InputState, MusicId, Platform, Renderer, SoundId,
    StateHasher, Vec2,
};
use trino_net::natsim::{Internet, NatKind, SimDatagrams};
use trino_net::udp::{Datagrams, LinkReport, MeetAt, Route, RoutePolicy, UdpSocket, meet};
use trino_net::{NetConfig, NetSession, NetState};

#[derive(Clone, Copy)]
struct Synth {
    acc: [i64; 4],
    frame: u32,
    viewer: u8,
}

impl Game for Synth {
    fn update(&mut self, _: &InputState, _: &mut dyn Audio, _: f32) {}
    fn update_with(&mut self, p: &mut dyn Platform, _: &InputState, _: &mut dyn Audio, _: f32) {
        let players = *p.players();
        self.viewer = p.local_seats();
        for i in 0..4 {
            let s = &players.players[i];
            self.acc[i] = self.acc[i]
                .wrapping_mul(31)
                .wrapping_add((s.stick.x * 127.0) as i64 + s.button_bits() as i64 + i as i64);
        }
        self.frame += 1;
    }
    fn render(&mut self, _: &mut dyn Renderer) {}
    fn state_hash(&self) -> Option<u64> {
        let mut h = StateHasher::new();
        for a in self.acc {
            h.u64(a as u64);
        }
        Some(h.u32(self.frame).value())
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

const FRAMES: u32 = 240;

fn input(seat: usize, f: i32) -> InputState {
    let mut s = InputState::IDLE;
    if !(0..FRAMES as i32 - 40).contains(&f) {
        return s;
    }
    let k = (f as u32 / (4 + seat as u32)).wrapping_mul(2_654_435_761);
    s.stick = Vec2::new(((k % 200) as f32 - 100.0) / 100.0, 0.0);
    s.set(Button::B, k.is_multiple_of(3));
    s
}

/// What one machine ended with.
#[derive(Debug, Clone)]
struct Outcome {
    seat: usize,
    hash: u64,
    viewer: u8,
    links: Vec<LinkReport>,
}

/// Signals once the socket delivers its first datagram (a lobby `JOIN` or
/// `WATCH`), so `session` spawns peers in deterministic seat order.
struct JoinOrder {
    io: SimDatagrams,
    joined: Option<mpsc::Sender<()>>,
}

impl Datagrams for JoinOrder {
    fn send_to(&mut self, datagram: &[u8], to: std::net::SocketAddr) -> std::io::Result<()> {
        let r = self.io.send_to(datagram, to);
        if let Some(tx) = self.joined.take() {
            let _ = tx.send(());
        }
        r
    }

    fn recv_from(&mut self, buf: &mut [u8]) -> Option<(usize, std::net::SocketAddr)> {
        self.io.recv_from(buf)
    }

    fn local_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        self.io.local_addr()
    }

    fn lan_addr(&self, toward: std::net::SocketAddr) -> Option<std::net::SocketAddr> {
        self.io.lan_addr(toward)
    }
}

/// Meet at the lobby and play `FRAMES` frames (a spectator watches them).
fn play(
    io: impl Datagrams + 'static,
    lobby: std::net::SocketAddr,
    cfg: NetConfig,
    watch: bool,
    policy: RoutePolicy,
) -> Result<Outcome, String> {
    let mut sock = UdpSocket::over(io);
    let at = MeetAt::Lobby {
        code: "NAT".into(),
        server: lobby,
    };
    let m = meet(&mut sock, &at, &cfg, watch, policy, Duration::from_secs(20))?;
    let routes = m.routes.clone();
    let mut s = match m.watching {
        Some(host) => NetSession::spectate(&cfg, host, sock)?,
        None => NetSession::with_spectators(&cfg, m.seat, &m.peers, &m.spectators, sock)?,
    };
    let mut g = Synth {
        acc: [0; 4],
        frame: 0,
        viewer: 0,
    };
    let mut host = Host(HostedPlayers::new());
    let mut at_end = None;
    let t0 = Instant::now();
    loop {
        let f = s.current_frame() + cfg.input_delay as i32;
        let local = if watch {
            InputState::IDLE
        } else {
            input(m.seat, f)
        };
        s.frame(&mut g, &mut host, &mut Mute, &local);
        if g.frame == FRAMES && at_end.is_none() {
            at_end = g.state_hash();
        }
        if let NetState::Desynced { frame, .. } = s.state() {
            return Err(format!("seat {}: desync at {frame}", m.seat));
        }
        if g.frame >= FRAMES && s.confirmed_frame() >= FRAMES as i32 - 1 {
            // Keep answering a while: the others may still need us.
            let until = Instant::now() + Duration::from_millis(600);
            while Instant::now() < until {
                let f = s.current_frame() + cfg.input_delay as i32;
                s.frame(&mut g, &mut host, &mut Mute, &input(m.seat, f));
                std::thread::sleep(Duration::from_millis(2));
            }
            break;
        }
        if t0.elapsed() > Duration::from_secs(60) {
            return Err(format!("seat {} stuck at frame {}", m.seat, g.frame));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(Outcome {
        seat: m.seat,
        hash: at_end.unwrap_or(0),
        viewer: g.viewer,
        links: routes.get(),
    })
}

/// Run `machines` (datagrams, watch) at once; their outcomes by seat.
fn session(
    lobby: std::net::SocketAddr,
    cfg: NetConfig,
    machines: Vec<(SimDatagrams, bool)>,
    policy: RoutePolicy,
) -> Vec<Outcome> {
    // Spectators first (a room takes them until it fills), then the players in
    // order (seats follow it); wait for each machine's first lobby datagram
    // before spawning the next so scheduler jitter cannot swap seats.
    let (watchers, players): (Vec<_>, Vec<_>) = machines.into_iter().partition(|(_, watch)| *watch);
    let (tx, rx) = mpsc::channel();
    let mut threads = Vec::new();
    for (io, watch) in watchers.into_iter().chain(players) {
        let tx = tx.clone();
        let (jtx, jrx) = mpsc::channel();
        let io = JoinOrder {
            io,
            joined: Some(jtx),
        };
        threads.push(std::thread::spawn(move || {
            tx.send(play(io, lobby, cfg, watch, policy)).unwrap();
        }));
        let _ = jrx.recv();
    }
    for t in threads {
        t.join().unwrap();
    }
    let mut out: Vec<Outcome> = rx
        .try_iter()
        .map(|r| r.unwrap_or_else(|e| panic!("{e}")))
        .collect();
    out.sort_by_key(|o| o.seat);
    out
}

fn direct(l: &LinkReport) -> bool {
    matches!(l.route, Route::Direct(_))
}

/// Every pair of NAT kinds: direct where hole punching works, relayed
/// where it can't, the same game on both machines either way.
#[test]
fn every_nat_pair_plays_direct_or_through_the_relay() {
    let cfg = NetConfig {
        desync_every: 1,
        ..NetConfig::DEFAULT
    };
    let mut table = Vec::new();
    for (i, a) in NatKind::ALL.iter().enumerate() {
        for b in &NatKind::ALL[i..] {
            let net = Internet::new();
            let lobby = net.lobby();
            let out = session(
                lobby,
                cfg,
                vec![(net.behind(*a), false), (net.behind(*b), false)],
                RoutePolicy::BOTH,
            );
            assert_eq!(out.len(), 2);
            assert_eq!(out[0].hash, out[1].hash, "{a:?} vs {b:?}: the games differ");
            assert_eq!((out[0].viewer, out[1].viewer), (1, 2), "local seats");
            let want = a.punches_with(*b);
            for o in &out {
                let l = &o.links[0];
                assert_eq!(direct(l), want, "{a:?} vs {b:?}, seat {}: {l:?}", o.seat);
                if !want {
                    assert!(l.relay_rx > 0 && l.relay_tx > 0, "{a:?} vs {b:?}: {l:?}");
                }
            }
            let s = net.stats();
            table.push(format!(
                "{:>15} vs {:<15} {:>6}  relayed {:>5} dropped {:>4}",
                a.name(),
                b.name(),
                if want { "direct" } else { "relay" },
                s.relayed,
                s.dropped
            ));
        }
    }
    println!("{}", table.join("\n"));
}

/// Two machines behind one NAT that doesn't hairpin reach each other by
/// their LAN addresses (the lobby only saw the NAT's).
#[test]
fn two_machines_behind_one_nat_meet_on_their_lan() {
    let net = Internet::new();
    let lobby = net.lobby();
    let nat = net.nat(NatKind::PortRestricted, false);
    let cfg = NetConfig::DEFAULT;
    let out = session(
        lobby,
        cfg,
        vec![
            (net.machine(Some(nat), 7000), false),
            (net.machine(Some(nat), 7001), false),
        ],
        RoutePolicy::BOTH,
    );
    assert_eq!(out[0].hash, out[1].hash);
    for o in &out {
        let Route::Direct(a) = o.links[0].route else {
            panic!("seat {}: {:?}", o.seat, o.links[0]);
        };
        assert!(a.ip().to_string().starts_with("10.0."), "{a}");
    }
}

/// Four players behind four kinds of NAT and a spectator behind a fifth:
/// each pair takes the route its NATs allow, every player sees its own
/// seat, the spectator follows seat 1 and everyone ends on one hash.
#[test]
fn four_players_and_a_spectator_across_mixed_nats() {
    let net = Internet::new();
    let lobby = net.lobby();
    let cfg = NetConfig {
        players: 4,
        desync_every: 1,
        ..NetConfig::DEFAULT
    };
    let kinds = [
        NatKind::Open,
        NatKind::Restricted,
        NatKind::PortRestricted,
        NatKind::Symmetric,
    ];
    let mut machines: Vec<(SimDatagrams, bool)> =
        kinds.iter().map(|k| (net.behind(*k), false)).collect();
    machines.push((net.behind(NatKind::Symmetric), true));
    let out = session(lobby, cfg, machines, RoutePolicy::BOTH);
    assert_eq!(out.len(), 5);
    for o in &out {
        assert_eq!(o.hash, out[0].hash, "seat {}", o.seat);
    }
    assert_eq!(
        out.iter().map(|o| o.viewer).collect::<Vec<_>>(),
        [1, 2, 4, 8, 1]
    );
    // Seat 2 (port-restricted) and seat 3 (symmetric) can only relay.
    let link = |seat: usize, to: u8| {
        out[seat]
            .links
            .iter()
            .find(|l| l.seat == to)
            .copied()
            .unwrap()
    };
    assert!(!direct(&link(2, 3)) && !direct(&link(3, 2)));
    assert!(direct(&link(0, 3)) && direct(&link(1, 3)) && direct(&link(1, 2)));
    // The spectator (symmetric) and seat 0 (open) reach each other.
    assert!(direct(&link(0, 4)) && direct(&link(4, 0)));
}

/// `TRINO_NET_ROUTE=relay`: everything through the lobby, same game.
#[test]
fn a_forced_relay_plays_the_same_game() {
    let net = Internet::new();
    let lobby = net.lobby();
    let cfg = NetConfig {
        desync_every: 1,
        ..NetConfig::DEFAULT
    };
    let relay = RoutePolicy::parse("relay").unwrap();
    let out = session(
        lobby,
        cfg,
        vec![
            (net.behind(NatKind::FullCone), false),
            (net.behind(NatKind::Open), false),
        ],
        relay,
    );
    assert_eq!(out[0].hash, out[1].hash);
    for o in &out {
        let l = o.links[0];
        assert_eq!((l.route, l.direct_tx, l.direct_rx), (Route::Relay, 0, 0));
    }
    assert!(net.stats().relayed > 100);
}

/// A peer whose `[net]` settings differ is refused by the lobby with both
/// fingerprints, instead of desyncing later.
#[test]
fn a_peer_with_other_settings_is_refused() {
    let net = Internet::new();
    let lobby = net.lobby();
    let a = NetConfig::DEFAULT;
    let b = NetConfig {
        input_delay: 4,
        ..a
    };
    let first = net.behind(NatKind::FullCone);
    let mut first = UdpSocket::over(first);
    let at = MeetAt::Lobby {
        code: "FP".into(),
        server: lobby,
    };
    // The first joiner waits (the room isn't full): give up quickly.
    let r = meet(
        &mut first,
        &at,
        &a,
        false,
        RoutePolicy::BOTH,
        Duration::from_millis(300),
    );
    assert!(r.err().expect("nobody else came").contains("didn't fill"));
    let mut second = UdpSocket::over(net.behind(NatKind::FullCone));
    let err = meet(
        &mut second,
        &at,
        &b,
        false,
        RoutePolicy::BOTH,
        Duration::from_secs(5),
    )
    .err()
    .expect("refused");
    assert!(
        err.contains(&a.fingerprint()) && err.contains(&b.fingerprint()),
        "{err}"
    );
}
