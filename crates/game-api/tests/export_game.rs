//! `export_game!` generates the hot-reload exports from a `Game`. Called
//! statically here (the dylib swap itself is `xtask/tests/hot_reload.rs`):
//! the version handshake, and `update`/`render` forwarding to the game
//! with the host's platform, input, audio and renderer, and the state
//! hooks replays and netplay call (`GAME_API_VERSION` 5).

use trino_core::{
    Audio, Button, Caps, Color, Game, InputState, MusicId, Platform, Renderer, SoundId, SpriteId,
    SpriteParams, Vec2,
};
use trino_game_api::GAME_API_VERSION;

/// A game that records what the host handed it.
#[derive(Default)]
pub struct Probe {
    updates: u32,
    dt_sum: f32,
    saw_a: bool,
    clock: u64,
    blob_len: Option<usize>,
    frames: u32,
}

impl Game for Probe {
    fn update(&mut self, _: &InputState, _: &mut dyn Audio, _: f32) {
        unreachable!("the exports call update_with");
    }

    fn update_with(
        &mut self,
        platform: &mut dyn Platform,
        input: &InputState,
        audio: &mut dyn Audio,
        dt: f32,
    ) {
        self.updates += 1;
        self.dt_sum += dt;
        self.saw_a |= input.is_down(Button::A);
        self.clock = platform.now_micros();
        self.blob_len = platform.blob(trino_core::BlobId(7)).map(<[u8]>::len);
        if input.is_down(Button::A) {
            audio.play_sound(SoundId(3));
        }
    }

    fn render(&mut self, renderer: &mut dyn Renderer) {
        self.frames += 1;
        renderer.begin_frame(Color::BLACK);
        renderer.draw_sprite(SpriteId(9), Vec2::new(1.0, 2.0), &SpriteParams::DEFAULT);
        renderer.end_frame();
    }

    fn state_hash(&self) -> Option<u64> {
        Some(u64::from(self.updates))
    }

    fn state_size(&self) -> usize {
        4
    }

    fn save_state(&self, out: &mut [u8]) -> Option<usize> {
        out.get_mut(..4)?
            .copy_from_slice(&self.updates.to_le_bytes());
        Some(4)
    }

    fn load_state(&mut self, bytes: &[u8]) -> bool {
        match bytes.try_into() {
            Ok(b) => {
                self.updates = u32::from_le_bytes(b);
                true
            }
            Err(_) => false,
        }
    }
}

trino_game_api::export_game!(Probe);

struct Host {
    caps: Caps,
    sounds: Vec<u32>,
    sprites: Vec<(u32, Vec2)>,
    frames: u32,
}

impl Platform for Host {
    fn now_micros(&self) -> u64 {
        42
    }
    fn log(&self, _: &str) {}
    fn blob(&self, id: trino_core::BlobId) -> Option<&'static [u8]> {
        (id.0 == 7).then_some(&b"abcd"[..])
    }
}

impl Audio for Host {
    fn play_sound(&mut self, sound: SoundId) {
        self.sounds.push(sound.0);
    }
    fn play_music(&mut self, _: MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

impl Renderer for Host {
    fn caps(&self) -> &Caps {
        &self.caps
    }
    fn begin_frame(&mut self, _: Color) {}
    fn draw_sprite(&mut self, sprite: SpriteId, pos: Vec2, _: &SpriteParams) {
        self.sprites.push((sprite.0, pos));
    }
    fn draw_model(
        &mut self,
        _: trino_core::ModelId,
        _: &trino_core::Transform3,
        _: trino_core::Material,
        _: &trino_core::ModelParams,
    ) {
    }
    fn end_frame(&mut self) {
        self.frames += 1;
    }
}

#[test]
fn exports_handshake_and_forward_to_the_game() {
    assert_eq!(trino_game_api_version(), GAME_API_VERSION);
    assert_eq!(GAME_API_VERSION, 5, "bump on purpose, with the host");

    let mut game = Probe::default();
    let mut host = Host {
        caps: Caps::N64,
        sounds: Vec::new(),
        sprites: Vec::new(),
        frames: 0,
    };
    let idle = InputState::default();
    let mut pressed = InputState::default();
    pressed.set(Button::A, true);

    // The exports take the host's services as separate trait objects.
    let mut audio = Host {
        caps: Caps::N64,
        sounds: Vec::new(),
        sprites: Vec::new(),
        frames: 0,
    };
    trino_game_update(&mut game, &mut host, &idle, &mut audio, 0.25);
    trino_game_update(&mut game, &mut host, &pressed, &mut audio, 0.5);
    assert_eq!(game.updates, 2);
    assert_eq!(game.dt_sum, 0.75);
    assert!(game.saw_a, "input reaches the game");
    assert_eq!(game.clock, 42, "platform reaches the game");
    assert_eq!(game.blob_len, Some(4));
    assert_eq!(audio.sounds, [3], "audio reaches the game");

    trino_game_render(&mut game, &mut host);
    assert_eq!(game.frames, 1);
    assert_eq!(host.frames, 1);
    assert_eq!(host.sprites, [(9, Vec2::new(1.0, 2.0))]);

    // The state hooks (replays, rollback) reach the game too.
    assert_eq!(trino_game_state_hash(&game), Some(2));
    assert_eq!(trino_game_state_size(&game), 4);
    let mut snap = [0u8; 4];
    assert_eq!(trino_game_save_state(&game, &mut snap), Some(4));
    trino_game_update(&mut game, &mut host, &idle, &mut audio, 0.25);
    assert_eq!(trino_game_state_hash(&game), Some(3));
    assert!(trino_game_load_state(&mut game, &snap));
    assert_eq!(trino_game_state_hash(&game), Some(2), "a load rolls back");
    assert!(!trino_game_load_state(&mut game, &[1, 2]));
}
