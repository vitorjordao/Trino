//! Recordings through the public API (issue #67): a game recorded through
//! a `Session` replays through another hash for hash, a disturbed replay
//! names its frame, a recording survives the consoles' debug channel, and
//! `Neutral` hides what the machine keeps.

use trino_core::replay::{
    ChannelSession, HEADER_LEN, Header, LineSink, Neutral, Reader, Session, decode_line,
};
use trino_core::{
    AccessSettings, Audio, Button, Game, HostedPlayers, InputState, MusicId, Platform,
    PlayersInput, Renderer, SoundId, StateHasher, Vec2,
};

struct Walker {
    pos: Vec2,
    jumps: u32,
    prev: u32,
}

impl Game for Walker {
    fn update(&mut self, input: &InputState, _: &mut dyn Audio, dt: f32) {
        self.pos += input.stick * dt;
        let b = input.button_bits();
        if b & !self.prev & 1 != 0 {
            self.jumps += 1;
        }
        self.prev = b;
    }
    fn render(&mut self, _: &mut dyn Renderer) {}
    fn state_hash(&self) -> Option<u64> {
        Some(
            StateHasher::new()
                .vec2(self.pos)
                .u32(self.jumps)
                .u32(self.prev)
                .value(),
        )
    }
}

struct Mute;
impl Audio for Mute {
    fn play_sound(&mut self, _: SoundId) {}
    fn play_music(&mut self, _: MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

struct Host {
    players: HostedPlayers,
    access: AccessSettings,
}
impl Platform for Host {
    fn now_micros(&self) -> u64 {
        0
    }
    fn log(&self, _: &str) {}
    fn access(&self) -> AccessSettings {
        self.access
    }
    fn hosted_players(&self) -> Option<&HostedPlayers> {
        Some(&self.players)
    }
    fn hosted_players_mut(&mut self) -> Option<&mut HostedPlayers> {
        Some(&mut self.players)
    }
}

fn live(f: u32) -> PlayersInput {
    let mut s = InputState::IDLE;
    s.stick = Vec2::new(((f % 17) as f32 - 8.0) / 8.0, 0.5);
    s.set(Button::A, f % 13 < 2);
    PlayersInput::single(s)
}

fn record(frames: u32) -> Vec<u8> {
    let mut g = Walker {
        pos: Vec2::ZERO,
        jumps: 0,
        prev: 0,
    };
    let mut out = Vec::new();
    let mut s = Session::record(
        Header::new("walker", (320, 240), 1, 2),
        &mut |b: &[u8]| out.extend_from_slice(b),
    );
    if let Some(h) = s.start(&g) {
        out[..HEADER_LEN].copy_from_slice(&h);
    }
    for f in 0..frames {
        let dt = 1.0 / 60.0 + (f % 4) as f32 * 1e-4;
        let (p, dt) = s.input(&live(f), dt).unwrap();
        g.update(&p.players[0], &mut Mute, dt);
        s.after(&g, &p, dt, &mut |b: &[u8]| out.extend_from_slice(b));
    }
    let h = s.finish_header().unwrap();
    out[..HEADER_LEN].copy_from_slice(&h);
    out
}

#[test]
fn a_recorded_session_replays_hash_for_hash() {
    let bytes = record(200);
    assert_eq!(Header::parse(&bytes).unwrap().frames, 200);
    let mut g = Walker {
        pos: Vec2::ZERO,
        jumps: 0,
        prev: 0,
    };
    let mut s = Session::replay(&bytes, "walker", None).unwrap();
    s.start(&g);
    let mut host = Host {
        players: HostedPlayers::new(),
        access: AccessSettings::DEFAULT,
    };
    while let Some((p, dt)) = s.input(&PlayersInput::NONE, 0.0) {
        host.players.publish_players(&p);
        g.update_with(&mut host, &p.players[0], &mut Mute, dt);
        assert!(s.after(&g, &p, dt, &mut |_: &[u8]| {}).is_none());
    }
    assert_eq!((s.frames(), s.hashes()), (200, 100));
    let mut line = String::new();
    s.verdict("pc", &mut line).unwrap();
    assert!(
        line.starts_with("TRINO_REPLAY: ok frames=200 checked=100"),
        "{line}"
    );
    assert!(g.jumps > 10);
}

#[test]
fn a_disturbed_replay_names_the_frame_it_diverged_on() {
    let bytes = record(120);
    let mut g = Walker {
        pos: Vec2::ZERO,
        jumps: 0,
        prev: 0,
    };
    let mut s = Session::replay(&bytes, "walker", None).unwrap();
    let mut first = None;
    while let Some((p, dt)) = s.input(&PlayersInput::NONE, 0.0) {
        g.update(&p.players[0], &mut Mute, dt);
        if s.frames() == 51 {
            g.jumps += 1;
        }
        if let Some(d) = s.after(&g, &p, dt, &mut |_: &[u8]| {}) {
            first.get_or_insert(d);
        }
    }
    let d = first.expect("a divergence");
    assert_eq!(d.frame, 52, "the first hash after the disturbance");
    let mut line = String::new();
    s.verdict("n64", &mut line).unwrap();
    assert!(
        line.contains("diverged at frame 52") && line.ends_with("on n64"),
        "{line}"
    );
}

#[test]
fn a_console_session_streams_and_replays_through_its_debug_channel() {
    // Record on a "console": the lines it would log.
    let mut lines: Vec<String> = Vec::new();
    {
        let mut g = Walker {
            pos: Vec2::ZERO,
            jumps: 0,
            prev: 0,
        };
        let mut s = ChannelSession::record(&g, "walker", (320, 240), 1, "n64", |l: &str| {
            lines.push(l.to_string())
        });
        for f in 0..90 {
            let (p, dt) = s.input(&live(f), 1.0 / 60.0).unwrap();
            g.update(&p.players[0], &mut Mute, dt);
            s.after(&g, &p, dt);
        }
        s.finish();
    }
    assert_eq!(lines.last().map(String::as_str), Some("TRINO_REC_END\n"));
    let mut bytes = Vec::new();
    for l in &lines {
        decode_line(l, &mut |b: &[u8]| bytes.extend_from_slice(b));
    }
    let mut r = Reader::new(&bytes).unwrap();
    let mut n = 0;
    while let Some(f) = r.next_frame() {
        f.unwrap();
        n += 1;
    }
    assert_eq!(n, 90, "an unfinished recording reads to its end");
    // Replay it as a console would: the verdict line through the channel.
    let mut out = Vec::new();
    {
        let mut g = Walker {
            pos: Vec2::ZERO,
            jumps: 0,
            prev: 0,
        };
        let mut s =
            ChannelSession::replay(&bytes, None, "3ds", |l: &str| out.push(l.to_string())).unwrap();
        s.start(&g);
        while let Some((p, dt)) = s.input(&PlayersInput::NONE, 0.0) {
            g.update(&p.players[0], &mut Mute, dt);
            s.after(&g, &p, dt);
        }
        assert!(s.passed());
        s.finish();
    }
    assert!(out.iter().any(|l| l.starts_with("TRINO_REPLAY: ok frames=90 checked=90") && l.contains("on 3ds")), "{out:?}");
    // LineSink alone: the same bytes back.
    let mut lines2 = Vec::new();
    let mut sink = LineSink::new(|l: &str| lines2.push(l.to_string()));
    trino_core::replay::Sink::put(&mut sink, &bytes);
    sink.end();
    let mut back = Vec::new();
    for l in &lines2 {
        decode_line(l, &mut |b: &[u8]| back.extend_from_slice(b));
    }
    assert_eq!(back, bytes);
}

#[test]
fn neutral_hides_what_the_machine_keeps() {
    let mut host = Host {
        players: HostedPlayers::new(),
        access: AccessSettings {
            text_scale: 200,
            ..AccessSettings::DEFAULT
        },
    };
    host.players.publish_players(&live(3));
    assert_eq!(host.access().text_scale, 200);
    let n = Neutral(&mut host);
    assert_eq!(n.access(), AccessSettings::DEFAULT, "no player settings");
    assert_eq!(n.save_capacity(), 0, "no save");
    assert_eq!(*n.players(), live(3), "the players pass");
}
