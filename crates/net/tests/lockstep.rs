//! Solid Eleven over the consoles' lockstep (issue #117,
//! `trino_core::lockstep`): two machines play the two-player recording,
//! each its own seat, over a lossy and late network, and reach frame 760
//! on the same hash the PC's rollback session (ggrs) reaches there,
//! `0xaeadc42f3bbe07f9` (`cargo xtask net local`, ADR 0036): the delay
//! lockstep a 3DS plays over local wireless and the rollback the PC and
//! the Web play online feed the game the very same frames.

use solid_eleven::SolidElevenGame;
use trino_core::lockstep::{LockConfig, LockState, Lockstep, MAX_PACKET};
use trino_core::replay::{Neutral, Reader};
use trino_core::{Audio, Game, HostedPlayers, MusicId, Platform, PlayersInput, SoundId, Vec2};

/// The rollback session's hash at frame 760 (ADR 0036 › Consequences).
const GGRS_760: u64 = 0xaead_c42f_3bbe_07f9;

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

struct Mute;
impl Audio for Mute {
    fn play_sound(&mut self, _: SoundId) {}
    fn play_music(&mut self, _: MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

fn script() -> Vec<PlayersInput> {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/solid-eleven/assets/shared/replays/two_players.trec"
    ))
    .unwrap();
    let mut r = Reader::new(&bytes).unwrap();
    let mut out = Vec::new();
    while let Some(f) = r.next_frame() {
        out.push(f.unwrap().players);
    }
    out
}

struct Machine {
    game: Box<SolidElevenGame>,
    host: Host,
    lock: Lockstep,
    at_760: Option<u64>,
    viewer: u8,
}

// Each machine sends to the other by index: a range loop reads clearer.
#[allow(clippy::needless_range_loop)]
#[test]
fn solid_eleven_in_lockstep_matches_the_rollback_session() {
    let script = script();
    let delay = 2;
    let mut machines: Vec<Machine> = (0..2u8)
        .map(|me| Machine {
            game: Box::new(SolidElevenGame::new(Vec2::new(320.0, 240.0))),
            host: Host {
                players: HostedPlayers::new(),
            },
            lock: Lockstep::new(LockConfig {
                delay,
                hash_every: 30,
                ..LockConfig::new(2, me)
            }),
            at_760: None,
            viewer: 0,
        })
        .collect();
    // 15 % loss, 3–6 ticks of latency, deterministic.
    let mut rng = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 33) as u32
    };
    let mut queue: Vec<(u32, usize, Vec<u8>)> = Vec::new();
    let mut buf = [0u8; MAX_PACKET];
    let dt = 1.0 / 60.0;
    for tick in 0..20_000u32 {
        if machines.iter().all(|m| m.at_760.is_some()) {
            break;
        }
        let (due, later): (Vec<_>, Vec<_>) = queue.drain(..).partition(|q| q.0 <= tick);
        queue = later;
        for (_, to, bytes) in due {
            machines[to].lock.receive(&bytes, tick as u64 * 16).unwrap();
        }
        for me in 0..2 {
            let m = &mut machines[me];
            // This seat's input for the frame it will play on.
            let f = (m.lock.frame() + delay as u32) as usize;
            let local = script
                .get(f)
                .map_or(trino_core::InputState::IDLE, |p| p.players[me]);
            m.lock.add_local(&local);
            if let Some(p) = m.lock.advance() {
                m.host.players.publish_net(&p, m.lock.local_seats());
                let mut neutral = Neutral(&mut m.host);
                m.game
                    .update_with(&mut neutral, &p.players[0], &mut Mute, dt);
                m.viewer = trino_core::first_seat(neutral.local_seats()) as u8;
                let h = m.game.state_hash().unwrap();
                m.lock.note_hash(h);
                if m.lock.frame() == 760 {
                    m.at_760 = Some(h);
                }
            }
            let n = machines[me].lock.packet(1 - me as u8, &mut buf);
            if next() % 100 >= 15 {
                queue.push((tick + 3 + next() % 4, 1 - me, buf[..n].to_vec()));
            }
        }
    }
    for (i, m) in machines.iter().enumerate() {
        assert_eq!(m.lock.state(), LockState::Running, "seat {i}");
        assert_eq!(
            m.at_760,
            Some(GGRS_760),
            "seat {i}: lockstep and rollback disagree at frame 760 ({:?})",
            m.lock.stats()
        );
        assert_eq!(
            m.viewer as usize, i,
            "seat {i}'s HUD follows its own player"
        );
        println!("seat {i}: {:?}", m.lock.stats());
    }
}
