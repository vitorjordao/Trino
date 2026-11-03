//! Delay-based lockstep for consoles (issue #117): every peer simulates a
//! frame only once it holds every player's input for it, and each local
//! input plays `delay` frames after it was read, which hides that much
//! latency. No snapshots, no rollback: what a console without the CPU to
//! simulate again needs (the 3DS's measured rollback budget for Solid
//! Eleven is 12.1 ms for an 8-frame window, ADR 0036 §4).
//!
//! [`Lockstep`] is transport-free and allocation-free (`no_std`): the host
//! hands it this machine's input ([`add_local`](Lockstep::add_local)), asks
//! for a packet per peer every frame ([`packet`](Lockstep::packet)), feeds
//! it what arrives ([`receive`](Lockstep::receive)) and plays a frame when
//! [`advance`](Lockstep::advance) returns one. The 3DS's local wireless
//! (`uds`) carries the packets on hardware and in Azahar; the tests carry
//! them over a lossy in-memory network.
//!
//! **Packets.** Each one repeats every input the peer hasn't acknowledged
//! yet (up to [`MAX_INPUTS_PER_PACKET`]), so a lost packet costs nothing but
//! the next one; it acknowledges the receiver's inputs and carries the
//! sender's newest state hash, which the receiver compares with its own for
//! that frame: the first that differs is a desync ([`LockState::Desynced`]).
//!
//! | Bytes | Field |
//! |---|---|
//! | 1 | `L` |
//! | 1 | version (1) |
//! | 1 | sender's seat |
//! | 1 | players |
//! | 4 | inputs of the receiver held (frames `0..ack`) |
//! | 4 | the first input's frame |
//! | 1 | inputs that follow (`n`) |
//! | 4 + 8 | a hash's frame and value (frame `u32::MAX`: none) |
//! | 9 × n | the inputs ([`NetPad`]) |
//!
//! A peer that stays silent for [`LockConfig::timeout_ms`] leaves: its seat
//! reads disconnected and idle, and the others play on.

use crate::input::InputState;
use crate::netpad::{NET_PAD_LEN, NetPad};
use crate::players::{MAX_PLAYERS, PlayersInput};

/// Frames of inputs kept per seat (a peer can't run further ahead).
pub const RING: usize = 64;
/// Inputs one packet repeats at most.
pub const MAX_INPUTS_PER_PACKET: usize = 24;
/// Bytes before a packet's inputs.
pub const PACKET_HEADER: usize = 25;
/// The largest packet [`Lockstep::packet`] writes.
pub const MAX_PACKET: usize = PACKET_HEADER + MAX_INPUTS_PER_PACKET * NET_PAD_LEN;
/// Hashes kept to compare with the peers'.
const HASHES: usize = 16;
const TAG: u8 = b'L';
const VERSION: u8 = 1;
const NO_HASH: u32 = u32::MAX;

/// A session's settings (every peer uses the same).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LockConfig {
    /// Players, 2..=4, one per machine.
    pub players: u8,
    /// This machine's seat.
    pub me: u8,
    /// Frames each local input waits before it plays (1..=16).
    pub delay: u8,
    /// Compare state hashes every this many frames (0: never).
    pub hash_every: u16,
    /// A peer silent this long leaves.
    pub timeout_ms: u32,
}

impl LockConfig {
    pub const fn new(players: u8, me: u8) -> LockConfig {
        LockConfig {
            players,
            me,
            delay: 4,
            hash_every: 30,
            timeout_ms: 5000,
        }
    }
}

/// Where a session is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LockState {
    /// No packet from every peer yet.
    Waiting,
    Running,
    /// The peers' hashes differ at `frame` (the state after that many
    /// frames): the sims went apart.
    Desynced {
        frame: u32,
        local: u64,
        remote: u64,
    },
}

/// What a session did so far.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LockStats {
    /// Frames played, host frames that waited for a late input.
    pub frames: u32,
    pub stalls: u32,
    /// Packets taken and refused (damaged, another session's).
    pub received: u32,
    pub refused: u32,
    /// Hashes compared with a peer.
    pub checked: u32,
}

/// Why a packet was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PacketError {
    Short,
    NotLockstep,
    /// Another session's (players), or a seat that isn't a peer.
    WrongSession,
}

/// One machine's lockstep session.
#[derive(Clone, Debug)]
pub struct Lockstep {
    cfg: LockConfig,
    /// The next frame to play.
    frame: u32,
    /// Every seat's inputs by frame (`frame % RING`).
    inputs: [[NetPad; RING]; MAX_PLAYERS],
    /// Frames `0..have[s]` of seat `s` are held.
    have: [u32; MAX_PLAYERS],
    /// Frames `0..acked[s]` of our inputs peer `s` holds.
    acked: [u32; MAX_PLAYERS],
    /// When each peer was last heard (ms), and whether it has been.
    heard: [Option<u64>; MAX_PLAYERS],
    /// Seats that left (they read disconnected and idle).
    left: u8,
    /// Our hashes (frame, hash), newest last.
    hashes: [(u32, u64); HASHES],
    newest_hash: Option<(u32, u64)>,
    state: LockState,
    stats: LockStats,
    now_ms: u64,
}

impl Lockstep {
    pub fn new(cfg: LockConfig) -> Lockstep {
        let players = cfg.players.clamp(2, MAX_PLAYERS as u8);
        let cfg = LockConfig {
            players,
            me: cfg.me.min(players - 1),
            delay: cfg.delay.clamp(1, 16),
            ..cfg
        };
        // The first `delay` frames of every seat are idle on every peer.
        let d = cfg.delay as u32;
        Lockstep {
            cfg,
            frame: 0,
            inputs: [[NetPad::IDLE; RING]; MAX_PLAYERS],
            have: [d; MAX_PLAYERS],
            acked: [d; MAX_PLAYERS],
            heard: [None; MAX_PLAYERS],
            left: 0,
            hashes: [(NO_HASH, 0); HASHES],
            newest_hash: None,
            state: LockState::Waiting,
            stats: LockStats::default(),
            now_ms: 0,
        }
    }

    pub fn config(&self) -> &LockConfig {
        &self.cfg
    }

    pub fn state(&self) -> LockState {
        self.state
    }

    pub fn stats(&self) -> LockStats {
        self.stats
    }

    /// The next frame to play (frames played so far).
    pub fn frame(&self) -> u32 {
        self.frame
    }

    /// Seats that left, a bit per seat.
    pub fn left(&self) -> u8 {
        self.left
    }

    /// The seats this machine plays (`Platform::local_seats`).
    pub fn local_seats(&self) -> u8 {
        1 << self.cfg.me
    }

    fn peers(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.cfg.players as usize).filter(move |s| *s != self.cfg.me as usize)
    }

    /// This machine's input for the next frame it hasn't given one for
    /// (`frame + delay`). Once per host frame; a call while waiting for a
    /// peer adds nothing (the input for that frame is already in).
    pub fn add_local(&mut self, input: &InputState) {
        let me = self.cfg.me as usize;
        let want = self.frame + self.cfg.delay as u32;
        if self.have[me] <= want && self.have[me] < self.frame + RING as u32 {
            let f = self.have[me];
            self.inputs[me][f as usize % RING] = NetPad::from_input(input);
            self.have[me] = f + 1;
        }
    }

    /// The packet for peer `to` this frame (into `out`, at least
    /// [`MAX_PACKET`] bytes); its length.
    pub fn packet(&self, to: u8, out: &mut [u8]) -> usize {
        let me = self.cfg.me as usize;
        let to = to as usize;
        if out.len() < MAX_PACKET || to >= self.cfg.players as usize || to == me {
            return 0;
        }
        let first = self.acked[to];
        let n = (self.have[me].saturating_sub(first) as usize).min(MAX_INPUTS_PER_PACKET);
        out[0] = TAG;
        out[1] = VERSION;
        out[2] = me as u8;
        out[3] = self.cfg.players;
        out[4..8].copy_from_slice(&self.have[to].to_le_bytes());
        out[8..12].copy_from_slice(&first.to_le_bytes());
        out[12] = n as u8;
        let (hf, hv) = self.newest_hash.unwrap_or((NO_HASH, 0));
        out[13..17].copy_from_slice(&hf.to_le_bytes());
        out[17..25].copy_from_slice(&hv.to_le_bytes());
        for i in 0..n {
            let f = first + i as u32;
            let at = PACKET_HEADER + i * NET_PAD_LEN;
            out[at..at + NET_PAD_LEN]
                .copy_from_slice(&self.inputs[me][f as usize % RING].to_bytes());
        }
        PACKET_HEADER + n * NET_PAD_LEN
    }

    /// A packet from a peer, at `now_ms` (any clock the host keeps).
    pub fn receive(&mut self, bytes: &[u8], now_ms: u64) -> Result<(), PacketError> {
        let r = self.take(bytes, now_ms);
        match r {
            Ok(()) => self.stats.received += 1,
            Err(_) => self.stats.refused += 1,
        }
        r
    }

    fn take(&mut self, b: &[u8], now_ms: u64) -> Result<(), PacketError> {
        if b.len() < PACKET_HEADER {
            return Err(PacketError::Short);
        }
        if b[0] != TAG || b[1] != VERSION {
            return Err(PacketError::NotLockstep);
        }
        let (from, players) = (b[2] as usize, b[3]);
        if players != self.cfg.players || from >= players as usize || from == self.cfg.me as usize {
            return Err(PacketError::WrongSession);
        }
        let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let (ack, first, n) = (u32_at(4), u32_at(8), b[12] as usize);
        if n > MAX_INPUTS_PER_PACKET || b.len() < PACKET_HEADER + n * NET_PAD_LEN {
            return Err(PacketError::Short);
        }
        self.heard[from] = Some(now_ms);
        self.now_ms = self.now_ms.max(now_ms);
        // What it holds of ours (never more than we sent).
        let me = self.cfg.me as usize;
        self.acked[from] = self.acked[from].max(ack.min(self.have[me]));
        // Its inputs, in order: only the next one we lack is taken (a gap
        // waits for the repeat).
        for i in 0..n {
            let f = first.wrapping_add(i as u32);
            if f == self.have[from] && f < self.frame + RING as u32 {
                let at = PACKET_HEADER + i * NET_PAD_LEN;
                let mut pad = [0u8; NET_PAD_LEN];
                pad.copy_from_slice(&b[at..at + NET_PAD_LEN]);
                self.inputs[from][f as usize % RING] = NetPad::from_bytes(pad);
                self.have[from] = f + 1;
            }
        }
        // Its hash against ours for the same frame.
        let hf = u32_at(13);
        if hf != NO_HASH {
            let mut hv = [0u8; 8];
            hv.copy_from_slice(&b[17..25]);
            let remote = u64::from_le_bytes(hv);
            if let Some(&(_, local)) = self.hashes.iter().find(|(f, _)| *f == hf) {
                self.stats.checked += 1;
                if local != remote && !matches!(self.state, LockState::Desynced { .. }) {
                    self.state = LockState::Desynced {
                        frame: hf,
                        local,
                        remote,
                    };
                }
            }
        }
        if self.state == LockState::Waiting && self.peers().all(|s| self.heard[s].is_some()) {
            self.state = LockState::Running;
        }
        Ok(())
    }

    /// Drop peers silent for longer than the timeout (`now_ms` on the
    /// clock [`receive`](Self::receive) gets). Returns the seats that left
    /// now.
    pub fn poll(&mut self, now_ms: u64) -> u8 {
        self.now_ms = self.now_ms.max(now_ms);
        let mut gone = 0;
        if self.state != LockState::Running {
            return 0;
        }
        for s in 0..self.cfg.players as usize {
            if s == self.cfg.me as usize || self.left & (1 << s) != 0 {
                continue;
            }
            if let Some(t) = self.heard[s]
                && now_ms.saturating_sub(t) > self.cfg.timeout_ms as u64
            {
                self.left |= 1 << s;
                gone |= 1 << s;
            }
        }
        gone
    }

    /// Every seat's input for the next frame, once every one is here: the
    /// frame to play (a seat that left reads disconnected and idle). `None`
    /// while waiting (a stall: render the last frame again).
    pub fn advance(&mut self) -> Option<PlayersInput> {
        let f = self.frame;
        let ready =
            (0..self.cfg.players as usize).all(|s| self.left & (1 << s) != 0 || self.have[s] > f);
        if !ready || self.state == LockState::Waiting {
            self.stats.stalls += 1;
            return None;
        }
        let mut p = PlayersInput::NONE;
        for s in 0..self.cfg.players as usize {
            if self.left & (1 << s) == 0 {
                p.connected |= 1 << s;
                p.players[s] = self.inputs[s][f as usize % RING].to_input();
            }
        }
        self.frame += 1;
        self.stats.frames += 1;
        Some(p)
    }

    /// The game's state hash after the frame [`advance`](Self::advance)
    /// returned (call it every frame; only every `hash_every`-th is kept
    /// and sent).
    pub fn note_hash(&mut self, hash: u64) {
        let n = self.cfg.hash_every as u32;
        if n == 0 || !self.frame.is_multiple_of(n) {
            return;
        }
        self.hashes.rotate_left(1);
        self.hashes[HASHES - 1] = (self.frame, hash);
        self.newest_hash = Some((self.frame, hash));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Button;
    use crate::math::Vec2;

    /// A tiny deterministic "game": every connected player's input mixed in.
    #[derive(Clone, Copy, PartialEq, Debug)]
    struct Mix(u64);

    impl Mix {
        fn step(&mut self, p: &PlayersInput) {
            for (i, s) in p.players.iter().enumerate() {
                if p.is_connected(i) {
                    self.0 = self.0.wrapping_mul(0x100_0000_01b3)
                        ^ (s.button_bits() as u64)
                            .wrapping_add((s.stick.x * 127.0) as i64 as u64)
                            .wrapping_add(i as u64);
                }
            }
        }
    }

    fn input(seat: usize, frame: u32) -> InputState {
        let mut s = InputState::IDLE;
        let k = (frame / (3 + seat as u32)).wrapping_mul(2_654_435_761) ^ seat as u32;
        s.set(Button::A, k.is_multiple_of(3));
        s.stick = Vec2::new(((k % 255) as f32 - 127.0) / 127.0, 0.0);
        s
    }

    /// A lossy, late network: packets wait `latency` ticks (± jitter), a
    /// share is lost; deterministic by seed.
    struct Net {
        rng: u64,
        loss: u32,
        latency: u32,
        jitter: u32,
        queue: std::vec::Vec<(u32, usize, std::vec::Vec<u8>)>,
    }

    impl Net {
        fn next(&mut self) -> u32 {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            (self.rng >> 32) as u32
        }

        fn send(&mut self, now: u32, to: usize, bytes: &[u8]) {
            if self.next() % 100 < self.loss {
                return;
            }
            let j = if self.jitter > 0 {
                self.next() % (self.jitter + 1)
            } else {
                0
            };
            self.queue
                .push((now + self.latency + j, to, bytes.to_vec()));
        }
    }

    /// Run `players` machines for `frames` frames; their games and
    /// sessions.
    #[allow(clippy::needless_range_loop)] // each machine sends by index
    fn run(
        players: u8,
        frames: u32,
        loss: u32,
        latency: u32,
        delay: u8,
    ) -> std::vec::Vec<(Mix, Lockstep)> {
        let mut net = Net {
            rng: 0x9e37_79b9_7f4a_7c15 ^ (loss as u64 * 31 + latency as u64),
            loss,
            latency,
            jitter: latency / 2,
            queue: std::vec::Vec::new(),
        };
        let mut peers: std::vec::Vec<(Mix, Lockstep)> = (0..players)
            .map(|me| {
                let cfg = LockConfig {
                    delay,
                    hash_every: 10,
                    ..LockConfig::new(players, me)
                };
                (Mix(1), Lockstep::new(cfg))
            })
            .collect();
        let mut buf = [0u8; MAX_PACKET];
        let mut tick = 0u32;
        while peers.iter().any(|(_, l)| l.frame() < frames) {
            tick += 1;
            assert!(
                tick < frames * 40,
                "stuck: {:?}",
                peers
                    .iter()
                    .map(|p| p.1.frame())
                    .collect::<std::vec::Vec<_>>()
            );
            // Deliver what is due.
            let (due, later): (std::vec::Vec<_>, std::vec::Vec<_>) =
                net.queue.drain(..).partition(|(at, _, _)| *at <= tick);
            net.queue = later;
            for (_, to, bytes) in due {
                peers[to].1.receive(&bytes, tick as u64 * 16).unwrap();
            }
            for me in 0..players as usize {
                let (game, lock) = &mut peers[me];
                let f = lock.frame() + lock.config().delay as u32;
                lock.add_local(&input(me, f));
                if lock.frame() < frames
                    && let Some(p) = lock.advance()
                {
                    game.step(&p);
                    lock.note_hash(game.0);
                }
                for to in 0..players {
                    if to as usize != me {
                        let n = peers[me].1.packet(to, &mut buf);
                        net.send(tick, to as usize, &buf[..n]);
                    }
                }
            }
        }
        peers
    }

    /// The same frames on one machine, every seat's input quantized as the
    /// network does (idle for the first `delay` frames).
    fn reference(players: u8, frames: u32, delay: u8) -> Mix {
        let mut g = Mix(1);
        for f in 0..frames {
            let mut p = PlayersInput::NONE;
            for s in 0..players as usize {
                p.connected |= 1 << s;
                if f >= delay as u32 {
                    p.players[s] = NetPad::quantize(&input(s, f));
                }
            }
            g.step(&p);
        }
        g
    }

    #[test]
    fn peers_stay_in_step_under_loss_and_latency() {
        for (players, loss, latency, delay) in [
            (2, 0, 0, 2),
            (2, 10, 3, 4),
            (2, 30, 8, 6),
            (3, 10, 4, 4),
            (4, 20, 6, 5),
        ] {
            let peers = run(players, 1500, loss, latency, delay);
            let want = reference(players, 1500, delay);
            for (i, (g, l)) in peers.iter().enumerate() {
                assert_eq!(
                    *g, want,
                    "{players} players, loss {loss}%, latency {latency}: seat {i} differs"
                );
                assert_eq!(l.state(), LockState::Running);
                assert!(l.stats().checked > 0, "no hash compared");
                assert_eq!(l.local_seats(), 1 << i);
            }
            if latency > delay as u32 {
                assert!(
                    peers[0].1.stats().stalls > 0,
                    "latency past the delay never waited"
                );
            }
        }
    }

    #[test]
    fn a_desync_is_found_by_the_hashes() {
        let mut a = Lockstep::new(LockConfig {
            hash_every: 1,
            ..LockConfig::new(2, 0)
        });
        let mut b = Lockstep::new(LockConfig {
            hash_every: 1,
            ..LockConfig::new(2, 1)
        });
        let mut buf = [0u8; MAX_PACKET];
        for f in 0..20u32 {
            a.add_local(&InputState::IDLE);
            b.add_local(&InputState::IDLE);
            let n = a.packet(1, &mut buf);
            b.receive(&buf[..n], f as u64).unwrap();
            let n = b.packet(0, &mut buf);
            a.receive(&buf[..n], f as u64).unwrap();
            if a.advance().is_some() {
                a.note_hash(a.frame() as u64);
            }
            if b.advance().is_some() {
                // Seat 1's sim drifts from frame 9 on.
                b.note_hash(b.frame() as u64 + (b.frame() >= 9) as u64);
            }
        }
        match a.state() {
            LockState::Desynced { frame, .. } => assert_eq!(frame, 9),
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn a_silent_peer_leaves_and_the_game_goes_on() {
        let mut a = Lockstep::new(LockConfig {
            timeout_ms: 500,
            ..LockConfig::new(2, 0)
        });
        let mut b = Lockstep::new(LockConfig::new(2, 1));
        let mut buf = [0u8; MAX_PACKET];
        for f in 0..30u64 {
            a.add_local(&InputState::IDLE);
            b.add_local(&InputState::IDLE);
            let n = b.packet(0, &mut buf);
            a.receive(&buf[..n], f * 16).unwrap();
            a.advance();
        }
        let played = a.frame();
        // b goes quiet: a waits, then drops it.
        for t in 0..10u64 {
            a.add_local(&InputState::IDLE);
            assert!(a.advance().is_none() || a.left() != 0);
            a.poll(30 * 16 + t * 100);
        }
        assert_eq!(a.left(), 0b10);
        a.add_local(&InputState::IDLE);
        let p = a.advance().expect("plays alone");
        assert!(p.is_connected(0) && !p.is_connected(1));
        assert!(a.frame() > played);
    }

    #[test]
    fn hostile_packets_are_refused_or_harmless() {
        let mut a = Lockstep::new(LockConfig::new(2, 0));
        assert_eq!(a.receive(&[], 0), Err(PacketError::Short));
        assert_eq!(a.receive(&[0u8; 40], 0), Err(PacketError::NotLockstep));
        let mut b = Lockstep::new(LockConfig::new(2, 1));
        let mut buf = [0u8; MAX_PACKET];
        b.add_local(&InputState::IDLE);
        let n = b.packet(0, &mut buf);
        let mut bad = buf;
        bad[3] = 3; // another session's player count
        assert_eq!(a.receive(&bad[..n], 0), Err(PacketError::WrongSession));
        let mut bad = buf;
        bad[2] = 0; // our own seat
        assert_eq!(a.receive(&bad[..n], 0), Err(PacketError::WrongSession));
        let mut bad = buf;
        bad[12] = 200; // more inputs than a packet has
        assert_eq!(a.receive(&bad[..n], 0), Err(PacketError::Short));
        // A far-future frame and an ack past what we sent change nothing.
        let mut bad = buf;
        bad[8..12].copy_from_slice(&1_000_000u32.to_le_bytes());
        bad[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        a.receive(&bad[..n], 0).unwrap();
        assert_eq!(a.have[1], a.cfg.delay as u32, "{:?}", &bad[..n]);
        assert!(a.acked[1] <= a.have[0]);
        assert_eq!(a.stats().refused, 5);
        // Every prefix and every byte value: no panic.
        for len in 0..n {
            let _ = a.receive(&buf[..len], 1);
        }
        for i in 0..n {
            let mut m = buf;
            m[i] ^= 0xff;
            let _ = a.receive(&m[..n], 2);
        }
    }
}
