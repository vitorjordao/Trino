//! {{name}} — a Trino game.
//!
//! This crate must stay platform-agnostic: depend only on `trino_core`
//! (+ `trino_game_api`), never on std APIs — that is what makes it run on
//! PC, N64 and 3DS unchanged. See AGENTS.md in this directory.
//!
//! The menus are UI screens (`ui/title.ui.ron`, `pause`, `options`,
//! `controls`): the asset bake turns each into a `ui/<name>` blob,
//! [`ScreenBlob`] keeps it parsed (and follows live edits on the PC), [`Ui`]
//! lays it out, moves the focus with the pad, the mouse or touch, and
//! reports clicks. Options sets the music and effects volumes and, where
//! the host has them, opens the engine's Graphics page (quality presets)
//! and the Controls screen (key rebinding).

#![cfg_attr(any(target_os = "none", target_os = "horizon"), no_std)]

use trino_core::ui::{ScreenBlob, StringsBlob, UiEvent};
use trino_core::{
    Audio, BindDevice, Bus, Button, Color, FontId, Game, GraphicsOptions, InputState,
    OptionsStyle, Platform, Renderer, SoundId, SpriteId, SpriteParams, Ui, UiEnv, Vec2,
};

pub const PLAYER: SpriteId = SpriteId::from_path("sprites/hero");
pub const BEEP: SoundId = SoundId::from_path("sounds/beep");
const SPEED: f32 = 120.0; // pixels per second
/// The font the screens and the Graphics page use.
const UI_FONT: FontId = FontId::from_path("fonts/trino5x7");

/// The Controls screen's rows: its action, the button it rebinds and the
/// value that shows the key.
const CONTROLS: [(&str, Button, &str); 3] = [
    ("rebind:a", Button::A, "key.a"),
    ("rebind:b", Button::B, "key.b"),
    ("rebind:start", Button::Start, "key.start"),
];

/// What the game shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Title,
    Play,
    Paused,
    /// Options, opened from the title or the pause menu.
    Options { from_pause: bool },
    /// Key rebinding, opened from Options (the PC).
    Controls { from_pause: bool },
}

/// What a menu asks of the platform; `update_with` (which has it) applies it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ask {
    Graphics,
    Rebind(Button),
    ResetBindings,
}

/// The platform request a menu action makes, if any.
pub fn ask_for(action: &str) -> Option<Ask> {
    match action {
        "graphics" => Some(Ask::Graphics),
        "reset" => Some(Ask::ResetBindings),
        _ => CONTROLS
            .iter()
            .find(|(a, ..)| *a == action)
            .map(|&(_, b, _)| Ask::Rebind(b)),
    }
}

pub struct {{name_camel}}Game {
    pub pos: Vec2,
    pub mode: Mode,
    bounds: Vec2,
    prev: InputState,
    title: ScreenBlob,
    pause: ScreenBlob,
    options: ScreenBlob,
    controls: ScreenBlob,
    strings: StringsBlob,
    /// The engine's Graphics page (quality presets on the PC and the Web).
    graphics: GraphicsOptions,
    ask: Option<Ask>,
    /// The host waits for the key of a rebind; then input waits for every
    /// button to be released, so the key does not also click.
    waiting: bool,
    hold: bool,
    /// The volume sliders start from the host's saved bus volumes.
    volumes_read: bool,
    /// The player's interface scale (the PC's display settings; 1 elsewhere).
    interface_scale: f32,
    /// The menus' state: the focus, and the values they show and edit.
    pub ui: Ui<32>,
}

impl {{name_camel}}Game {
    /// `screen` is the internal resolution of the current platform/profile.
    pub fn new(screen: Vec2) -> Self {
        {{name_camel}}Game {
            pos: (screen - Vec2::new(16.0, 16.0)) * 0.5,
            mode: Mode::Title,
            bounds: screen,
            prev: InputState::default(),
            title: ScreenBlob::new("title"),
            pause: ScreenBlob::new("pause"),
            options: ScreenBlob::new("options"),
            controls: ScreenBlob::new("controls"),
            strings: StringsBlob::new("en-US"),
            graphics: GraphicsOptions::new(),
            ask: None,
            waiting: false,
            hold: false,
            volumes_read: false,
            interface_scale: 1.0,
            ui: Ui::new(),
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
            (Mode::Title | Mode::Paused, "options") => Some(Mode::Options {
                from_pause: self.mode == Mode::Paused,
            }),
            (Mode::Options { from_pause }, "controls") => Some(Mode::Controls { from_pause }),
            (Mode::Controls { from_pause }, "back") => Some(Mode::Options { from_pause }),
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
            Mode::Controls { .. } => &self.controls,
            Mode::Play => return,
        };
        if self.waiting {
            self.hold = true;
            return;
        }
        if self.hold {
            if input.any_button() {
                return;
            }
            self.hold = false;
        }
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
                    self.ask = self.ask.or(ask_for(action));
                    self.action(action)
                }
                UiEvent::Changed { bind, value, .. } => {
                    let bus = match bind {
                        "music" => Some(Bus::Music),
                        "sfx" => Some(Bus::Sfx),
                        _ => None,
                    };
                    if let Some(bus) = bus {
                        audio.set_bus_volume(bus, value as f32 / 10.0);
                    }
                    audio.play_sound(BEEP);
                    None
                }
                UiEvent::Back { action } => self.action(action),
                UiEvent::Moved { .. } => None,
            };
            next = next.or(to);
        }
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
        self.controls.get(platform);
        self.strings.get(platform);
        self.interface_scale = platform
            .display()
            .map_or(1.0, |d| d.settings.interface_scale());
        // The Graphics page runs itself (presets, "Detect again") while it
        // is open; the button that closes it must not also go back.
        if self.graphics.is_open() {
            self.graphics.update(platform, input, &self.prev);
            self.hold = !self.graphics.is_open();
            self.prev = *input;
            return;
        }
        // What the host offers, and the keys the Controls screen shows.
        self.ui.set_bool("graphics", platform.quality_info().is_some());
        let rebind = platform.input_info();
        self.ui.set_bool("rebind", rebind.is_some());
        self.waiting = rebind.is_some_and(|i| i.capturing.is_some());
        self.ui.set_bool("waiting", self.waiting);
        if let Mode::Controls { .. } = self.mode {
            for (_, button, key) in CONTROLS {
                let label = platform.binding_label(button, BindDevice::Keyboard);
                self.ui.set_text(key, label.unwrap_or("-"));
            }
        }
        self.update(input, audio, dt);
        match self.ask.take() {
            Some(Ask::Graphics) => self.graphics.open(),
            Some(Ask::Rebind(button)) => platform.rebind(button, BindDevice::Keyboard),
            Some(Ask::ResetBindings) => platform.reset_bindings(),
            None => {}
        }
    }

    fn update(&mut self, input: &InputState, audio: &mut dyn Audio, dt: f32) {
        if !self.volumes_read {
            self.volumes_read = true;
            for (key, bus) in [("music", Bus::Music), ("sfx", Bus::Sfx)] {
                self.ui.set(key, (audio.bus_volume(bus) * 10.0 + 0.5) as i32);
            }
        }
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
            Mode::Controls { .. } => Some(&self.controls),
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
        if self.graphics.is_open() {
            let style = OptionsStyle::DEFAULT;
            let at = (self.bounds - self.graphics.size(&style)) * 0.5;
            self.graphics.draw(renderer, UI_FONT, at, &style);
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
    use trino_core::ui::{NO_ITEM, text_value};
    use trino_core::{GamepadStatus, InputInfo, MusicId, SoundId};

    struct NullAudio;
    impl Audio for NullAudio {
        fn play_sound(&mut self, _: SoundId) {}
        fn play_music(&mut self, _: MusicId, _: bool) {}
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
    }

    /// A PC-like host: keys to show, a rebind and a reset to record.
    #[derive(Default)]
    struct Host {
        rebound: Option<Button>,
        reset: bool,
        capturing: bool,
    }
    impl Platform for Host {
        fn now_micros(&self) -> u64 {
            0
        }
        fn log(&self, _: &str) {}
        fn input_info(&self) -> Option<InputInfo> {
            Some(InputInfo {
                gamepad: GamepadStatus::Disconnected,
                capturing: self.capturing.then_some((Button::A, BindDevice::Keyboard)),
            })
        }
        fn binding_label(&self, button: Button, _: BindDevice) -> Option<&str> {
            (button == Button::A).then_some("Z")
        }
        fn rebind(&mut self, button: Button, _: BindDevice) {
            self.rebound = Some(button);
        }
        fn reset_bindings(&mut self) {
            self.reset = true;
        }
    }

    #[test]
    fn controls_show_the_keys_and_ask_the_host_to_rebind() {
        let mut game = {{name_camel}}Game::new(Vec2::new(320.0, 240.0));
        game.mode = Mode::Controls { from_pause: false };
        let mut host = Host::default();
        let idle = InputState::default();
        game.ask = Some(Ask::Rebind(Button::B));
        game.update_with(&mut host, &idle, &mut NullAudio, 0.016);
        assert_eq!(host.rebound, Some(Button::B));
        assert!(game.ui.get_bool("rebind") && !game.ui.get_bool("graphics"));
        let key = |game: &{{name_camel}}Game, k| game.ui.values().get(k, NO_ITEM);
        assert_eq!(key(&game, "key.a"), Some(text_value("Z")));
        assert_eq!(key(&game, "key.b"), Some(text_value("-")), "no key bound");
        // While the host waits for the key the screen says so.
        host.capturing = true;
        game.update_with(&mut host, &idle, &mut NullAudio, 0.016);
        assert!(game.waiting && game.ui.get_bool("waiting"));
        game.ask = Some(Ask::ResetBindings);
        host.capturing = false;
        game.update_with(&mut host, &idle, &mut NullAudio, 0.016);
        assert!(host.reset && !game.ui.get_bool("waiting"));
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
    fn options_lead_to_controls_and_back_where_they_came_from() {
        let mut game = {{name_camel}}Game::new(Vec2::new(320.0, 240.0));
        game.mode = Mode::Paused;
        let options = game.action("options").unwrap();
        assert_eq!(options, Mode::Options { from_pause: true });
        game.mode = options;
        let controls = game.action("controls").unwrap();
        assert_eq!(controls, Mode::Controls { from_pause: true });
        game.mode = controls;
        assert_eq!(game.action("back"), Some(options));
        game.mode = options;
        assert_eq!(game.action("back"), Some(Mode::Paused));
        // The rows that need the platform ask for it.
        assert_eq!(ask_for("rebind:start"), Some(Ask::Rebind(Button::Start)));
        assert_eq!(ask_for("graphics"), Some(Ask::Graphics));
        assert_eq!(ask_for("reset"), Some(Ask::ResetBindings));
        assert_eq!(ask_for("back"), None);
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
