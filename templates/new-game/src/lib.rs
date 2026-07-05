//! {{name}} — a Trino game.
//!
//! This crate must stay platform-agnostic: depend only on `trino_core`
//! (+ `trino_game_api`), never on std APIs — that is what makes it run on
//! PC, N64 and 3DS unchanged. See AGENTS.md in this directory.

#![cfg_attr(any(target_os = "none", target_os = "horizon"), no_std)]

use trino_core::{
    Audio, Button, Color, Game, InputState, Renderer, SoundId, SpriteId, SpriteParams, Vec2,
};

pub const PLAYER: SpriteId = SpriteId::from_path("sprites/hero");
pub const BEEP: SoundId = SoundId::from_path("sounds/beep");
const SPEED: f32 = 120.0; // pixels per second

pub struct {{name_camel}}Game {
    pub pos: Vec2,
    bounds: Vec2,
    prev: InputState,
}

impl {{name_camel}}Game {
    /// `screen` is the internal resolution of the current platform/profile.
    pub fn new(screen: Vec2) -> Self {
        {{name_camel}}Game {
            pos: (screen - Vec2::new(16.0, 16.0)) * 0.5,
            bounds: screen,
            prev: InputState::default(),
        }
    }
}

impl Game for {{name_camel}}Game {
    fn update(&mut self, input: &InputState, audio: &mut dyn Audio, dt: f32) {
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
        // Stick is Y-up; screen is Y-down.
        self.pos.x = (self.pos.x + dir.x * SPEED * dt).clamp(0.0, self.bounds.x - 16.0);
        self.pos.y = (self.pos.y - dir.y * SPEED * dt).clamp(0.0, self.bounds.y - 16.0);

        if input.just_pressed(&self.prev, Button::A) {
            audio.play_sound(BEEP);
        }
        self.prev = *input;
    }

    fn render(&mut self, renderer: &mut dyn Renderer) {
        renderer.begin_frame(Color::rgb(24, 26, 40));
        renderer.draw_sprite(PLAYER, self.pos, &SpriteParams::default());
        renderer.end_frame();
    }
}

// Hot-reload exports (trino_game_update / trino_game_render / version).
trino_game_api::export_game!({{name_camel}}Game);

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::{MusicId, SoundId};

    struct NullAudio;
    impl Audio for NullAudio {
        fn play_sound(&mut self, _: SoundId) {}
        fn play_music(&mut self, _: MusicId, _: bool) {}
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
    }

    #[test]
    fn moves_with_dpad() {
        let mut game = {{name_camel}}Game::new(Vec2::new(320.0, 240.0));
        let mut input = InputState::default();
        input.set(Button::DpadRight, true);
        let x0 = game.pos.x;
        game.update(&input, &mut NullAudio, 0.5);
        assert!(game.pos.x > x0);
    }
}
