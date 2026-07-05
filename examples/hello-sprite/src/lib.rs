//! Minimal example game: a sprite that moves with stick/d-pad and beeps on A.
//!
//! Dependency-free on purpose — this exact crate must compile for PC, N64
//! and 3DS. `no_std` is enforced on console targets (N64 `target_os = "none"`,
//! 3DS `target_os = "horizon"`); on PC the dylib build links std only for its
//! panic handler — game code itself must never use std APIs (console CI
//! builds catch violations).
//!
//! Handles come from logical asset paths (see `assets/manifest.toml`); the
//! platform app uploads the baked data behind them.

#![cfg_attr(any(target_os = "none", target_os = "horizon"), no_std)]

use trino_core::{
    Audio, Button, Color, Game, InputState, Renderer, SoundId, SpriteId, SpriteParams, Vec2,
};

pub const PLAYER_SPRITE: SpriteId = SpriteId::from_path("sprites/player");
pub const BEEP: SoundId = SoundId::from_path("sounds/beep");
pub const PLAYER_SIZE: u32 = 32;
const SPEED: f32 = 120.0; // pixels per second

pub struct HelloGame {
    pub pos: Vec2,
    bounds: Vec2,
    prev: InputState,
}

impl HelloGame {
    /// `screen` is the internal resolution of the current platform/profile.
    pub fn new(screen: Vec2) -> Self {
        HelloGame {
            pos: Vec2::new(
                (screen.x - PLAYER_SIZE as f32) * 0.5,
                (screen.y - PLAYER_SIZE as f32) * 0.5,
            ),
            bounds: screen,
            prev: InputState::default(),
        }
    }

    fn direction(input: &InputState) -> Vec2 {
        let mut dir = input.stick;
        if input.is_down(Button::DpadLeft) {
            dir.x = -1.0;
        }
        if input.is_down(Button::DpadRight) {
            dir.x = 1.0;
        }
        if input.is_down(Button::DpadUp) {
            dir.y = 1.0;
        }
        if input.is_down(Button::DpadDown) {
            dir.y = -1.0;
        }
        dir
    }
}

impl Game for HelloGame {
    fn update(&mut self, input: &InputState, audio: &mut dyn Audio, dt: f32) {
        let dir = Self::direction(input);
        // Stick is Y-up; screen is Y-down.
        self.pos.x += dir.x * SPEED * dt;
        self.pos.y -= dir.y * SPEED * dt;

        let max = self.bounds - Vec2::new(PLAYER_SIZE as f32, PLAYER_SIZE as f32);
        self.pos.x = self.pos.x.clamp(0.0, max.x.max(0.0));
        self.pos.y = self.pos.y.clamp(0.0, max.y.max(0.0));

        if input.just_pressed(&self.prev, Button::A) {
            audio.play_sound(BEEP);
        }
        self.prev = *input;
    }

    fn render(&mut self, renderer: &mut dyn Renderer) {
        renderer.begin_frame(Color::rgb(24, 26, 40));
        renderer.draw_sprite(PLAYER_SPRITE, self.pos, &SpriteParams::default());
        renderer.end_frame();
    }
}

// Hot-reload exports (trino_game_update / trino_game_render / version).
trino_game_api::export_game!(HelloGame);

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::{MusicId, SoundId};

    struct NullAudio {
        played: usize,
    }
    impl Audio for NullAudio {
        fn play_sound(&mut self, _: SoundId) {
            self.played += 1;
        }
        fn play_music(&mut self, _: MusicId, _: bool) {}
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
    }

    fn dpad(button: Button) -> InputState {
        let mut s = InputState::default();
        s.set(button, true);
        s
    }

    #[test]
    fn moves_with_dpad_in_screen_space() {
        let mut game = HelloGame::new(Vec2::new(320.0, 240.0));
        let mut audio = NullAudio { played: 0 };
        let start = game.pos;
        game.update(&dpad(Button::DpadRight), &mut audio, 0.5);
        assert_eq!(game.pos.x, start.x + 60.0);
        // DpadUp means up on screen = smaller y.
        game.update(&dpad(Button::DpadUp), &mut audio, 0.5);
        assert_eq!(game.pos.y, start.y - 60.0);
    }

    #[test]
    fn clamps_to_bounds() {
        let mut game = HelloGame::new(Vec2::new(320.0, 240.0));
        let mut audio = NullAudio { played: 0 };
        for _ in 0..100 {
            game.update(&dpad(Button::DpadLeft), &mut audio, 1.0);
        }
        assert_eq!(game.pos.x, 0.0);
        for _ in 0..100 {
            game.update(&dpad(Button::DpadRight), &mut audio, 1.0);
        }
        assert_eq!(game.pos.x, 320.0 - PLAYER_SIZE as f32);
    }

    #[test]
    fn beeps_only_on_a_press_edge() {
        let mut game = HelloGame::new(Vec2::new(320.0, 240.0));
        let mut audio = NullAudio { played: 0 };
        let mut held = InputState::default();
        held.set(Button::A, true);

        game.update(&held, &mut audio, 0.016);
        game.update(&held, &mut audio, 0.016); // still held: no retrigger
        assert_eq!(audio.played, 1);

        game.update(&InputState::default(), &mut audio, 0.016);
        game.update(&held, &mut audio, 0.016); // released and re-pressed
        assert_eq!(audio.played, 2);
    }

    #[test]
    fn update_is_deterministic() {
        let run = || {
            let mut game = HelloGame::new(Vec2::new(320.0, 240.0));
            let mut audio = NullAudio { played: 0 };
            for i in 0..60 {
                let input = if i % 2 == 0 {
                    dpad(Button::DpadRight)
                } else {
                    dpad(Button::DpadDown)
                };
                game.update(&input, &mut audio, 1.0 / 60.0);
            }
            (game.pos.x, game.pos.y)
        };
        assert_eq!(run(), run());
    }
}
