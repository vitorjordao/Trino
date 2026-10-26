//! The console self-test's game half (core-only, so the PC tests run it
//! too): a scripted match the N64 and 3DS hosts time and check.
//!
//! The hosts (`templates/football/{n64,n3ds}`) call [`begin`], drive
//! [`FRAMES`] frames of [`input`] at [`DT`] through `host.measure` (the
//! N64's emulated frame time is gated by `trino.toml`'s `n64_max_frame_us`),
//! then [`check`]: the match state's hash after the script must equal
//! [`EXPECTED_HASH`], which `cargo test` pins on the PC — the same match on
//! every target, tick for tick.

use crate::SolidElevenGame;
use crate::render::Sky;
use trino_core::{Button, InputState, Vec2};

/// Frames the hosts measure.
pub const FRAMES: u32 = 300;
/// Seconds per frame: 30 fps, two sim ticks a frame.
pub const DT: f32 = 1.0 / 30.0;
/// The seed and clubs of the scripted match (FC Rouge v Azure United).
pub const SEED: u32 = 0x5E11_7E57;

/// Put `game` in the scripted match: a human (the script) on side 0
/// against the AI, from kick-off.
pub fn begin(game: &mut SolidElevenGame) {
    game.start_exhibition(0, 1, SEED, [true, false], Sky::Day, 180);
}

/// The match's sim hash (printed as `TRINO_SE_HASH`).
pub fn hash(game: &SolidElevenGame) -> u32 {
    game.match_hash()
}

/// The script's input on `frame`: runs, sprints, passes and shots (a
/// human's buttons, so the human control path runs on the console too).
pub fn input(frame: u32) -> InputState {
    let mut s = InputState::default();
    let t = frame % 150;
    // Run right (toward the opponents' goal in the first half), weaving.
    let weave = if (frame / 40).is_multiple_of(2) {
        0.4
    } else {
        -0.4
    };
    s.stick = Vec2::new(1.0, weave);
    if t > 60 {
        s.set(Button::R, true);
    }
    // Kick off at once (a short pass released on frame 20, once the players are set), then a pass
    // released at 95 and a held shot released at 140 in every cycle.
    if (14..20).contains(&frame) || (85..95).contains(&t) {
        s.set(Button::A, true);
    }
    if (115..140).contains(&t) {
        s.set(Button::B, true);
    }
    // Switch player now and then.
    if t == 20 {
        s.set(Button::L, true);
    }
    s
}

/// The state hash after [`FRAMES`] of the script, pinned on the PC by
/// `cargo test` (`tests::the_script_is_deterministic`).
pub const EXPECTED_HASH: u32 = 0xb6f6_4c76;

/// After the script: the sim hash matches the PC's, and the match is sane.
pub fn check(game: &SolidElevenGame) -> Result<(), &'static str> {
    let Some(session) = &game.session else {
        return Err("no match running");
    };
    let m = &session.m;
    if m.tick == 0 {
        return Err("the match never ran");
    }
    for p in &m.players {
        if !p.off
            && (p.pos.x.abs()
                > crate::sim::pitch::HALF_LEN + crate::sim::pitch::RUNOFF + crate::sim::fx::Fx::ONE)
        {
            return Err("a player left the stadium");
        }
    }
    if EXPECTED_HASH != 0 && m.hash() != EXPECTED_HASH {
        return Err("the match diverged from the PC's (sim hash)");
    }
    Ok(())
}

// ---- issue #67: a two-player session replayed on every target ----

/// The committed recording (`assets/shared/replays/two_players.trec`): the
/// menus to a two-player quick match (P1 home, P2 away), then 20 seconds of
/// both players' pads, recorded on the PC through [`trino_core::replay::Neutral`]
/// (`two_players_recording`, re-made by `cargo test -p solid-eleven
/// write_two_player_recording -- --ignored`).
pub const TWO_PLAYERS: trino_core::BlobId =
    trino_core::BlobId::from_path("blobs/replay_two_players");

/// Frames in the two-player recording.
pub const TWO_PLAYER_FRAMES: u32 = 640;

/// The two-player recording's input on `frame`: the menus (quick match,
/// "P1 V P2", go), then both sides' pads.
pub fn two_player_input(frame: u32) -> trino_core::PlayersInput {
    let mut p = trino_core::PlayersInput::single(InputState::default());
    p.connected = 0b11;
    let menu = |f: u32, b: Button| (f == frame).then_some(b);
    // Title: A on "quick match"; then down twice to the side row, left
    // once (P1 V P2), down five times to GO, A.
    let press = menu(10, Button::A)
        .or(menu(16, Button::DpadDown))
        .or(menu(20, Button::DpadDown))
        .or(menu(24, Button::DpadLeft))
        .or(menu(28, Button::DpadDown))
        .or(menu(32, Button::DpadDown))
        .or(menu(36, Button::DpadDown))
        .or(menu(40, Button::DpadDown))
        .or(menu(44, Button::DpadDown))
        .or(menu(50, Button::A));
    if let Some(b) = press {
        p.players[0].set(b, true);
    }
    if frame < 60 {
        return p;
    }
    // The match: player 1 is the script the self-test plays, player 2 runs
    // the other way and passes on its own rhythm.
    let t = frame - 60;
    p.players[0] = input(t);
    let s2 = &mut p.players[1];
    let u = t % 120;
    s2.stick = Vec2::new(
        -1.0,
        if (t / 30).is_multiple_of(2) {
            0.5
        } else {
            -0.5
        },
    );
    if u > 40 {
        s2.set(Button::R, true);
    }
    if (12..18).contains(&t) || (70..78).contains(&u) {
        s2.set(Button::A, true);
    }
    if (95..110).contains(&u) {
        s2.set(Button::B, true);
    }
    p
}

/// What [`replay_two_players`] played.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayReport {
    pub frames: u32,
    pub checked: u32,
    pub hash: u64,
}

struct Silent;

impl trino_core::Audio for Silent {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// Play the two-player recording on `game`, which must be fresh from
/// [`SolidElevenGame::new`] (the hosts run this first), through a
/// [`Neutral`](trino_core::replay::Neutral) platform: every recorded hash
/// must match, and player 2's side must have scored or kicked (its input
/// reached the match). Logs `TRINO_SE_REPLAY ok frames=… hash=…`.
pub fn replay_two_players(
    platform: &mut dyn trino_core::Platform,
    game: &mut SolidElevenGame,
) -> Result<ReplayReport, &'static str> {
    use trino_core::replay::{Check, Neutral, Replay};
    let bytes = platform
        .blob(TWO_PLAYERS)
        .ok_or("replay: blobs/replay_two_players is missing")?;
    let mut replay = Replay::new(bytes, "solid-eleven").map_err(|e| e.message())?;
    if let Check::Diverged { .. } = replay.start(game) {
        return Err("replay: the game isn't fresh (start hash)");
    }
    let mut neutral = Neutral(platform);
    let mut report = ReplayReport {
        frames: 0,
        checked: 0,
        hash: 0,
    };
    while let Some(step) = replay.step(game, &mut neutral, &mut Silent) {
        let (_, check) = step.map_err(|e| e.message())?;
        report.frames += 1;
        match check {
            Check::Match => report.checked += 1,
            Check::Diverged { .. } => {
                let d = replay.diverged.unwrap_or(trino_core::replay::Divergence {
                    frame: 0,
                    expected: 0,
                    got: 0,
                });
                let mut line = trino_core::TextBuf::<160>::new();
                let _ = core::fmt::Write::write_fmt(
                    &mut line,
                    format_args!(
                        "TRINO_REPLAY: diverged at frame {} (expected {:#018x}, got {:#018x})\n",
                        d.frame, d.expected, d.got
                    ),
                );
                neutral.0.log(line.as_str());
                return Err("replay: Solid Eleven diverged from the PC recording");
            }
            _ => {}
        }
    }
    report.hash = replay.last_hash.unwrap_or(0);
    let Some(s) = &game.session else {
        return Err("replay: the recording never reached its match");
    };
    if s.second != Some(1) {
        return Err("replay: the match isn't a two-player one");
    }
    if s.m.stats[1].passes + s.m.stats[1].shots == 0 {
        return Err("replay: player 2's side never kicked");
    }
    let mut line = trino_core::TextBuf::<128>::new();
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!(
            "TRINO_SE_REPLAY ok frames={} checked={} hash={:#018x}\n",
            report.frames, report.checked, report.hash
        ),
    );
    neutral.0.log(line.as_str());
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::{Audio, Game, MusicId, SoundId};

    struct Mute;
    impl Audio for Mute {
        fn play_sound(&mut self, _: SoundId) {}
        fn play_music(&mut self, _: MusicId, _: bool) {}
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
    }

    fn run() -> SolidElevenGame {
        let mut g = SolidElevenGame::new(Vec2::new(320.0, 240.0));
        begin(&mut g);
        for f in 0..FRAMES {
            g.update(&input(f), &mut Mute, DT);
        }
        g
    }

    #[test]
    fn the_script_kicks_off_at_once_and_plays() {
        // The human's kick-off is released on frame 20 (tick 40): the
        // taker lines up and kicks within a few ticks, so most of the
        // window is open play (a lost button would idle the AI's
        // eight-second timeout instead).
        let mut g = SolidElevenGame::new(Vec2::new(320.0, 240.0));
        begin(&mut g);
        let mut first_play = None;
        let mut play_ticks = 0;
        for f in 0..FRAMES {
            g.update(&input(f), &mut Mute, DT);
            let m = &g.session.as_ref().unwrap().m;
            if m.phase == crate::sim::engine::Phase::Play {
                first_play.get_or_insert(m.tick);
                play_ticks += 2;
            }
        }
        let first = first_play.expect("never kicked off");
        std::println!("kick-off at tick {first}, {play_ticks} ticks of play");
        assert!(first <= 60, "kicked off at tick {first}");
        assert!(play_ticks >= 300, "{play_ticks} ticks of play");
    }

    #[test]
    fn the_script_is_deterministic() {
        let a = run();
        let b = run();
        assert_eq!(
            a.session.as_ref().unwrap().m.hash(),
            b.session.as_ref().unwrap().m.hash()
        );
        // 300 frames at 30 fps = 600 ticks.
        assert_eq!(a.session.as_ref().unwrap().m.tick, 600);
        std::println!(
            "self-test hash: {:#010x}",
            a.session.as_ref().unwrap().m.hash()
        );
        if EXPECTED_HASH != 0 {
            assert_eq!(
                a.session.as_ref().unwrap().m.hash(),
                EXPECTED_HASH,
                "re-pin EXPECTED_HASH after a sim change"
            );
        }
        check(&a).unwrap();
    }

    /// A platform holding only players and blobs (the host of the tests).
    struct Host {
        players: trino_core::HostedPlayers,
        blob: Option<&'static [u8]>,
    }

    impl trino_core::Platform for Host {
        fn now_micros(&self) -> u64 {
            0
        }
        fn log(&self, _: &str) {}
        fn blob(&self, id: trino_core::BlobId) -> Option<&'static [u8]> {
            (id == TWO_PLAYERS).then_some(self.blob).flatten()
        }
        fn hosted_players(&self) -> Option<&trino_core::HostedPlayers> {
            Some(&self.players)
        }
        fn hosted_players_mut(&mut self) -> Option<&mut trino_core::HostedPlayers> {
            Some(&mut self.players)
        }
    }

    /// The two-player recording as the PC's recorder writes it: the script
    /// through a neutral platform, a hash every 4 frames.
    fn two_players_recording() -> std::vec::Vec<u8> {
        use trino_core::replay::{HEADER_LEN, Header, Neutral, Recorder};
        let screen = Vec2::new(320.0, 240.0);
        let mut g = SolidElevenGame::new(screen);
        let mut host = Host {
            players: trino_core::HostedPlayers::new(),
            blob: None,
        };
        let mut header = Header::new("solid-eleven", (320, 240), 2, 4);
        header.start_hash = g.state_hash().unwrap();
        let mut out = std::vec::Vec::new();
        let mut sink = |b: &[u8]| out.extend_from_slice(b);
        let mut rec = Recorder::begin(header, &mut sink);
        for f in 0..TWO_PLAYER_FRAMES {
            let p = two_player_input(f);
            host.players.publish_players(&p);
            g.update_with(&mut Neutral(&mut host), &p.players[0], &mut Mute, DT);
            let h = rec.hash_due().then(|| g.state_hash().unwrap());
            rec.frame(&mut sink, &p, DT, h);
        }
        let head = rec.finish_header();
        out[..HEADER_LEN].copy_from_slice(&head);
        out
    }

    fn fixture_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/shared/replays/two_players.trec")
    }

    /// Writes the committed recording (after a sim change: review the
    /// match it plays, then commit).
    #[test]
    #[ignore = "rewrites assets/shared/replays/two_players.trec"]
    fn write_two_player_recording() {
        let bytes = two_players_recording();
        std::fs::create_dir_all(fixture_path().parent().unwrap()).unwrap();
        std::fs::write(fixture_path(), &bytes).unwrap();
        std::println!("{} bytes", bytes.len());
    }

    /// The committed recording is what the script records today, and it
    /// replays through the step the consoles run: a two-player match whose
    /// second side plays.
    #[test]
    fn the_two_player_recording_is_fresh_and_replays() {
        let committed = std::fs::read(fixture_path()).expect("two_players.trec");
        assert!(
            committed == two_players_recording(),
            "assets/shared/replays/two_players.trec is stale: \
             cargo test -p solid-eleven write_two_player_recording -- --ignored"
        );
        let blob: &'static [u8] = std::boxed::Box::leak(committed.into_boxed_slice());
        let mut host = Host {
            players: trino_core::HostedPlayers::new(),
            blob: Some(blob),
        };
        let mut g = SolidElevenGame::new(Vec2::new(320.0, 240.0));
        let r = replay_two_players(&mut host, &mut g).unwrap();
        assert_eq!(r.frames, TWO_PLAYER_FRAMES);
        assert_eq!(r.checked, TWO_PLAYER_FRAMES / 4);
        let s = g.session.as_ref().unwrap();
        std::println!(
            "two players: score {:?}, passes {} / {}, shots {} / {}",
            s.m.score,
            s.m.stats[0].passes,
            s.m.stats[1].passes,
            s.m.stats[0].shots,
            s.m.stats[1].shots
        );
        // A recording replayed on another game diverges at its start.
        let mut used = SolidElevenGame::new(Vec2::new(320.0, 240.0));
        used.update(&input(0), &mut Mute, DT);
        assert!(replay_two_players(&mut host, &mut used).is_err());
    }

    extern crate std;
}

#[cfg(test)]
mod bench {
    extern crate std;
    use super::*;
    use crate::sim::engine::{Match, Pad};
    use trino_core::{Audio, Game, MusicId, SoundId};

    struct Mute;
    impl Audio for Mute {
        fn play_sound(&mut self, _: SoundId) {}
        fn play_music(&mut self, _: MusicId, _: bool) {}
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
    }

    /// Host-side split of the frame's update: sim ticks vs. the visuals.
    /// `cargo test -p solid-eleven --release bench_update -- --ignored --nocapture`
    #[test]
    #[ignore = "timing aid"]
    fn bench_update() {
        let mut g = SolidElevenGame::new(Vec2::new(320.0, 240.0));
        begin(&mut g);
        let t0 = std::time::Instant::now();
        for f in 0..FRAMES {
            g.update(&input(f), &mut Mute, DT);
        }
        let whole = t0.elapsed();
        let mut m: Match = g.session.as_ref().unwrap().m.clone();
        let t1 = std::time::Instant::now();
        for _ in 0..FRAMES * 2 {
            m.step([Pad::default(); 2]);
        }
        let sim = t1.elapsed();
        std::println!(
            "per frame: update {:?}, of which 2 sim ticks ~{:?}",
            whole / FRAMES,
            sim / FRAMES
        );
    }
}

/// A renderer that draws nothing: the hosts time [`crate::SolidElevenGame`]'s
/// render code against it to split the game's own CPU work (posing,
/// culling, the HUD's layout) from the backend's.
pub struct NullRenderer {
    pub caps: trino_core::Caps,
}

impl trino_core::Renderer for NullRenderer {
    fn caps(&self) -> &trino_core::Caps {
        &self.caps
    }
    fn begin_frame(&mut self, _: trino_core::Color) {}
    fn draw_sprite(&mut self, _: trino_core::SpriteId, _: Vec2, _: &trino_core::SpriteParams) {}
    fn draw_model(
        &mut self,
        _: trino_core::ModelId,
        _: &trino_core::Transform3,
        _: trino_core::Material,
        _: &trino_core::ModelParams,
    ) {
    }
    fn end_frame(&mut self) {}
    fn measure_text(&self, _: trino_core::FontId, t: &str, scale: f32) -> Vec2 {
        Vec2::new(t.len() as f32 * 6.0 * scale, 11.0 * scale)
    }
}
