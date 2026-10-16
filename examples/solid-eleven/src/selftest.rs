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
    let screen = game.screen;
    *game = SolidElevenGame::exhibition(screen, 0, 1, SEED, [true, false], Sky::Day);
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
    // A pass released at frame 95, a held shot released at 140.
    if (85..95).contains(&t) {
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
pub const EXPECTED_HASH: u32 = 0;

/// After the script: the sim hash matches the PC's, and the match is sane.
pub fn check(game: &SolidElevenGame) -> Result<(), &'static str> {
    let m = &game.session.m;
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
    fn the_script_is_deterministic() {
        let a = run();
        let b = run();
        assert_eq!(a.session.m.hash(), b.session.m.hash());
        // 300 frames at 30 fps = 600 ticks.
        assert_eq!(a.session.m.tick, 600);
        std::println!("self-test hash: {:#010x}", a.session.m.hash());
        if EXPECTED_HASH != 0 {
            assert_eq!(
                a.session.m.hash(),
                EXPECTED_HASH,
                "re-pin EXPECTED_HASH after a sim change"
            );
        }
        check(&a).unwrap();
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
        let mut m: Match = g.session.m.clone();
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
