//! Solid Eleven under ggrs's sync test (issue #67): every frame of the
//! two-player recording's script — the menus, then a P1 v P2 match — is
//! rolled back 7 frames and simulated again, and every re-simulated state
//! hash must equal the first run's. A snapshot that missed part of what
//! `update` changes (the visuals, the controls, an accumulator) fails here
//! before two machines desync.

use solid_eleven::SolidElevenGame;
use solid_eleven::selftest::{TWO_PLAYER_FRAMES, two_player_input};
use trino_core::{Audio, Game, HostedPlayers, MusicId, Platform, SoundId, Vec2};
use trino_net::SyncTest;

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

#[test]
fn solid_eleven_rolls_back_seven_frames_every_frame_without_a_desync() {
    let mut game = Box::new(SolidElevenGame::new(Vec2::new(320.0, 240.0)));
    assert!(game.state_size() > 0, "Solid Eleven takes snapshots");
    let mut host = Host {
        players: HostedPlayers::new(),
    };
    // The script runs at 30 fps (two sim ticks a frame), as recorded.
    let mut sync = SyncTest::new(2, 7, 30).unwrap();
    for f in 0..TWO_PLAYER_FRAMES {
        sync.frame(&mut *game, &mut host, &mut Mute, &two_player_input(f))
            .unwrap_or_else(|e| panic!("frame {f}: {e}"));
    }
    let s = game.session.as_ref().expect("the script reached its match");
    assert_eq!(s.second, Some(1), "a two-player match");
    assert!(sync.stats.rollbacks > 500, "{:?}", sync.stats);
    println!(
        "{:?}; snapshot {} bytes; score {:?}",
        sync.stats,
        game.state_size(),
        s.m.score
    );
}
