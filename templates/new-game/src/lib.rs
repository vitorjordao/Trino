//! {{name}} — a Trino game.
//!
//! This crate must stay platform-agnostic: depend only on `trino_core`
//! (+ `trino_game_api`), never on std APIs — that is what makes it run on
//! PC, N64 and 3DS unchanged. See AGENTS.md in this directory.
//!
//! The menus are UI screens (`ui/title.ui.ron`, `pause`, `options`): the
//! asset bake turns each into a `ui/<name>` blob, [`ScreenBlob`] keeps it
//! parsed (and follows live edits on the PC), [`Ui`] lays it out, moves the
//! focus with the pad, the mouse or touch, and reports clicks.

#![cfg_attr(any(target_os = "none", target_os = "horizon"), no_std)]

use trino_core::ui::{ScreenBlob, StringsBlob, UiEvent};
use trino_core::{
    Audio, Button, Color, Game, InputState, Platform, Renderer, SoundId, SpriteId, SpriteParams,
    Ui, UiEnv, Vec2,
};

pub const PLAYER: SpriteId = SpriteId::from_path("sprites/hero");
pub const BEEP: SoundId = SoundId::from_path("sounds/beep");
const SPEED: f32 = 120.0; // pixels per second

/// What the game shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Title,
    Play,
    Paused,
    /// Options, opened from the title or the pause menu.
    Options { from_pause: bool },
}

pub struct {{name_camel}}Game {
    pub pos: Vec2,
    pub mode: Mode,
    bounds: Vec2,
    prev: InputState,
    title: ScreenBlob,
    pause: ScreenBlob,
    options: ScreenBlob,
    strings: StringsBlob,
    /// The player's interface scale (the PC's display settings; 1 elsewhere).
    interface_scale: f32,
    /// The menus' state: the focus, and the values they show and edit.
    pub ui: Ui<32>,
}

impl {{name_camel}}Game {
    /// `screen` is the internal resolution of the current platform/profile.
    pub fn new(screen: Vec2) -> Self {
        let mut ui = Ui::new();
        ui.set_bool("music", true);
        ui.set("volume", 8);
        {{name_camel}}Game {
            pos: (screen - Vec2::new(16.0, 16.0)) * 0.5,
            mode: Mode::Title,
            bounds: screen,
            prev: InputState::default(),
            title: ScreenBlob::new("title"),
            pause: ScreenBlob::new("pause"),
            options: ScreenBlob::new("options"),
            strings: StringsBlob::new("en-US"),
            interface_scale: 1.0,
            ui,
        }
    }

    fn go(&mut self, mode: Mode) {
        self.mode = mode;
        self.ui.reset();
    }

    /// Where a menu action leads.
    fn action(&self, action: &str) -> Option<Mode> {
        match (self.mode, action) {
            (Mode::Title, "play") | (Mode::Paused, "resume") => Some(Mode::Play),
            (Mode::Paused, "quit") => Some(Mode::Title),
            (from, "options") => Some(Mode::Options {
                from_pause: from == Mode::Paused,
            }),
            (Mode::Options { from_pause: true }, "back") => Some(Mode::Paused),
            (Mode::Options { from_pause: false }, "back") => Some(Mode::Title),
            _ => None,
        }
    }

    /// The current menu: focus moves, clicks, value changes.
    fn menu(&mut self, input: &InputState, audio: &mut dyn Audio, dt: f32) {
        let blob = match self.mode {
            Mode::Title => &self.title,
            Mode::Paused => &self.pause,
            Mode::Options { .. } => &self.options,
            Mode::Play => return,
        };
        let Some(screen) = blob.screen() else {
            // No baked screens (unit tests, a missing bake): Start plays.
            if input.just_pressed(&self.prev, Button::Start) {
                self.go(Mode::Play);
            }
            return;
        };
        let mut next = None;
        for e in self.ui.update(screen, input, &self.prev, dt).iter() {
            let to = match e {
                UiEvent::Click { action, .. } => {
                    audio.play_sound(BEEP);
                    self.action(action)
                }
                UiEvent::Changed { .. } => {
                    audio.play_sound(BEEP);
                    None
                }
                UiEvent::Back { action } => self.action(action),
                UiEvent::Moved { .. } => None,
            };
            next = next.or(to);
        }
        let volume = self.ui.get("volume").unwrap_or(8) as f32 / 10.0;
        audio.set_master_volume(if self.ui.get_bool("music") { volume } else { 0.0 });
        if let Some(mode) = next {
            self.go(mode);
        }
    }
}

impl Game for {{name_camel}}Game {
    fn update_with(
        &mut self,
        platform: &mut dyn Platform,
        input: &InputState,
        audio: &mut dyn Audio,
        dt: f32,
    ) {
        // Fetched where the platform is; parsed again when a live edit
        // replaces a blob.
        self.title.get(platform);
        self.pause.get(platform);
        self.options.get(platform);
        self.strings.get(platform);
        self.interface_scale = platform
            .display()
            .map_or(1.0, |d| d.settings.interface_scale());
        self.update(input, audio, dt);
    }

    fn update(&mut self, input: &InputState, audio: &mut dyn Audio, dt: f32) {
        if self.mode != Mode::Play {
            self.menu(input, audio, dt);
            self.prev = *input;
            return;
        }
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
        if input.just_pressed(&self.prev, Button::Start) {
            self.go(Mode::Paused);
        }
        self.prev = *input;
    }

    fn render(&mut self, renderer: &mut dyn Renderer) {
        renderer.begin_frame(Color::rgb(24, 26, 40));
        let blob = match self.mode {
            Mode::Title => Some(&self.title),
            Mode::Play => None,
            Mode::Paused => Some(&self.pause),
            Mode::Options { .. } => Some(&self.options),
        };
        if self.mode != Mode::Title {
            renderer.draw_sprite(PLAYER, self.pos, &SpriteParams::default());
        }
        if let Some(screen) = blob.and_then(ScreenBlob::screen) {
            let env = UiEnv::of(renderer).with_interface_scale(self.interface_scale);
            match self.strings.table() {
                Some(t) => self.ui.draw(renderer, screen, &env.with_strings(t)),
                None => self.ui.draw(renderer, screen, &env),
            }
        }
        renderer.end_frame();
    }

    fn resize(&mut self, screen: Vec2) {
        self.bounds = screen;
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
        game.mode = Mode::Play;
        let mut input = InputState::default();
        input.set(Button::DpadRight, true);
        let x0 = game.pos.x;
        game.update(&input, &mut NullAudio, 0.5);
        assert!(game.pos.x > x0);
    }

    #[test]
    fn start_plays_and_pauses() {
        let mut game = {{name_camel}}Game::new(Vec2::new(320.0, 240.0));
        assert_eq!(game.mode, Mode::Title);
        let mut start = InputState::default();
        start.set(Button::Start, true);
        game.update(&start, &mut NullAudio, 0.016);
        assert_eq!(game.mode, Mode::Play);
        game.update(&InputState::default(), &mut NullAudio, 0.016);
        game.update(&start, &mut NullAudio, 0.016);
        assert_eq!(game.mode, Mode::Paused);
    }
}
