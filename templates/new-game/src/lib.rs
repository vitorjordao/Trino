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
//! and the Controls screen (key rebinding). The Accessibility screen sets
//! the player's settings (`Platform::set_access`): every screen draws with
//! their text size, high contrast and color-vision icons.
//!
//! Achievements (`assets/achievements.ron`, docs/SHIPPING.md): moving
//! unlocks `FIRST_STEPS`, the `STEPS` stat unlocks `WANDERER` at 100, a
//! screen corner the hidden `CORNER`; the engine's popup shows each and
//! the title's and pause menu's Achievements button opens the engine's
//! list. The state also goes into the save (`save_begin`), which is where
//! the N64 and the 3DS keep it; the PC and the Web keep their own copy too
//! (Steam's, on a Steam build). About shows the stamped version.

#![cfg_attr(any(target_os = "none", target_os = "horizon"), no_std)]

use trino_core::ui::{ScreenBlob, StringsBlob, UiEvent, UiStrings};
use trino_core::{
    AchievementId, AchievementList, AchievementState, AchievementTable, AchievementToast,
    BuildInfo, OverlayStyle, StatId, StrKey,
};
use trino_core::{
    AccessSettings, Audio, BindDevice, CaptionMode, CvdMode, Bus, Button, Color, FontId, Game, GraphicsOptions, InputState,
    OptionsStyle, Platform, Renderer, SoundId, SpriteId, SpriteParams, Ui, UiEnv, Vec2,
};

pub const PLAYER: SpriteId = SpriteId::from_path("sprites/hero");
pub const BEEP: SoundId = SoundId::from_path("sounds/beep");
const SPEED: f32 = 120.0; // pixels per second
/// The font the screens and the Graphics page use.
const UI_FONT: FontId = FontId::from_path("fonts/trino5x7");
/// This build: the About screen shows it (`cargo trino package` stamps it).
pub const BUILD: BuildInfo = trino_core::build_info!();
/// The achievements (`assets/achievements.ron`).
pub const FIRST_STEPS: AchievementId = AchievementId::new("FIRST_STEPS");
pub const CORNER: AchievementId = AchievementId::new("CORNER");
pub const STEPS: StatId = StatId::new("STEPS");
/// A step: this many pixels walked.
const STEP_PX: f32 = 16.0;

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
    /// The player's accessibility settings, opened from Options.
    Access { from_pause: bool },
    /// The engine's achievements list, from the title or the pause menu.
    Achievements { from_pause: bool },
    /// The game's version and the engine credit, from the title.
    About,
    /// "Send the crash report?", once, before the title (PC packages with
    /// an endpoint).
    CrashAsk,
}

/// What a menu asks of the platform; `update_with` (which has it) applies it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ask {
    Graphics,
    /// The crash question's answer.
    Crash(trino_core::CrashConsent),
    Rebind(Button),
    ResetBindings,
}

/// The platform request a menu action makes, if any.
pub fn ask_for(action: &str) -> Option<Ask> {
    match action {
        "graphics" => Some(Ask::Graphics),
        "reset" => Some(Ask::ResetBindings),
        "yes" => Some(Ask::Crash(trino_core::CrashConsent::Allow)),
        "no" => Some(Ask::Crash(trino_core::CrashConsent::Deny)),
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
    access: ScreenBlob,
    about: ScreenBlob,
    crash: ScreenBlob,
    strings: StringsBlob,
    /// The player's accessibility settings: read from the platform, changed
    /// by the Accessibility screen, sent back (`dirty`) after the frame.
    pub settings: AccessSettings,
    dirty: bool,
    /// The Accessibility screen's values are set (from `settings`).
    shown: bool,
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
    /// The unlock popup and the list screen (the engine's overlay), with the
    /// table and state as of the last update.
    pub toast: AchievementToast,
    pub list: AchievementList,
    table: AchievementTable<'static>,
    achievements: AchievementState,
    /// Pixels walked toward the next step; the steps reported.
    walked: f32,
    steps: i32,
    /// The save was read (once, at the first update).
    loaded: bool,
    /// A save is being written (`save_step` each frame).
    saving: bool,
    /// What presence says (title or playing).
    presence: Option<bool>,
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
            access: ScreenBlob::new("accessibility"),
            about: ScreenBlob::new("about"),
            crash: ScreenBlob::new("crash"),
            strings: StringsBlob::new("en-US"),
            settings: AccessSettings::DEFAULT,
            dirty: false,
            shown: false,
            graphics: GraphicsOptions::new(),
            ask: None,
            waiting: false,
            hold: false,
            volumes_read: false,
            interface_scale: 1.0,
            ui: Ui::new(),
            toast: AchievementToast::new(),
            list: AchievementList::new(),
            table: AchievementTable::EMPTY,
            achievements: AchievementState::EMPTY,
            walked: 0.0,
            steps: 0,
            loaded: false,
            saving: false,
            presence: None,
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
            (Mode::Title | Mode::Paused, "achievements") => Some(Mode::Achievements {
                from_pause: self.mode == Mode::Paused,
            }),
            (Mode::Title, "about") => Some(Mode::About),
            (Mode::About, "back") => Some(Mode::Title),
            (Mode::CrashAsk, "yes" | "no") => Some(Mode::Title),
            (Mode::Options { from_pause }, "controls") => Some(Mode::Controls { from_pause }),
            (Mode::Options { from_pause }, "accessibility") => Some(Mode::Access { from_pause }),
            (Mode::Controls { from_pause } | Mode::Access { from_pause }, "back") => {
                Some(Mode::Options { from_pause })
            }
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
            Mode::Access { .. } => &self.access,
            Mode::About => &self.about,
            Mode::CrashAsk => &self.crash,
            Mode::Play | Mode::Achievements { .. } => return,
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
                    self.change_access(action, 0);
                    self.action(action)
                }
                UiEvent::Changed { bind, value, .. } => {
                    self.change_access(bind, value);
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
                UiEvent::Back { action } => {
                    // Backing out of the crash question answers "no".
                    if self.mode == Mode::CrashAsk {
                        self.ask = self.ask.or(ask_for(action));
                    }
                    self.action(action)
                }
                UiEvent::Moved { .. } => None,
            };
            next = next.or(to);
        }
        if let Some(mode) = next {
            self.go(mode);
        }
    }
}

impl {{name_camel}}Game {
    /// An Accessibility row changed (`cvd` and `captions` cycle on a click,
    /// the rest carry their value).
    fn change_access(&mut self, what: &str, value: i32) {
        let s = &mut self.settings;
        match what {
            "cvd" => s.cvd = s.cvd.next(),
            "captions" => s.captions = s.captions.next(),
            "text" => s.text_scale = value.clamp(100, 200) as u8,
            "contrast" => s.high_contrast = value != 0,
            "motion" => s.reduce_motion = value != 0,
            "flash" => s.flash_guard = value != 0,
            "hold" => s.hold_to_toggle = value != 0,
            _ => return,
        }
        self.dirty = true;
        self.show_access();
    }

    /// The Accessibility screen's values from [`settings`](Self::settings).
    fn show_access(&mut self) {
        let s = self.settings;
        let cvd = match s.cvd {
            CvdMode::Off => "Off",
            CvdMode::Protanopia => "Protan",
            CvdMode::Deuteranopia => "Deutan",
            CvdMode::Tritanopia => "Tritan",
            CvdMode::Grayscale => "Gray",
        };
        self.ui.set_text("cvd", cvd);
        self.ui.set_text("captions", s.captions.label());
        self.ui.set("text", s.text_scale as i32);
        self.ui.set_bool("contrast", s.high_contrast);
        self.ui.set_bool("motion", s.reduce_motion);
        self.ui.set_bool("flash", s.flash_guard);
        self.ui.set_bool("hold", s.hold_to_toggle);
    }
}

impl {{name_camel}}Game {
    /// The save, once: the achievements stored in it go back to the
    /// platform (where the N64 and the 3DS keep them). Then a save being
    /// written goes on a step per frame, and the About screen's texts.
    fn services(&mut self, platform: &mut dyn Platform) {
        if !self.loaded {
            self.loaded = true;
            if platform.crash_reports().is_some_and(|c| c.should_ask()) && self.mode == Mode::Title {
                self.go(Mode::CrashAsk);
            }
            let mut buf = [0u8; 128];
            if let Ok(n) = platform.save_read(&mut buf)
                && let Ok((saved, _)) = AchievementState::unpack(&buf[..n])
            {
                platform.restore_achievements(&saved);
            }
            let mut version = trino_core::TextBuf::<32>::new();
            let _ = core::fmt::Write::write_str(&mut version, BUILD.version);
            self.ui.set_text("version", version.as_str());
            let mut build = trino_core::TextBuf::<48>::new();
            let _ = core::fmt::Write::write_str(&mut build, BUILD.commit);
            if !BUILD.date.is_empty() {
                let _ = core::fmt::Write::write_fmt(&mut build, format_args!(", {}", BUILD.date));
            }
            self.ui.set_text("build", build.as_str());
        }
        if self.saving && platform.save_step(2000).unwrap_or(true) {
            self.saving = false;
            platform.cloud_sync_hint();
        }
        if let Some(t) = AchievementTable::from_platform(platform) {
            self.table = t;
        }
        self.achievements = platform.achievements();
    }

    /// Tell the platform what this frame changed: steps (the `STEPS` stat
    /// unlocks `WANDERER`), the first move, a corner, presence. A new
    /// unlock saves.
    fn report(&mut self, platform: &mut dyn Platform, (steps, pos): (i32, Vec2)) {
        let playing = self.mode == Mode::Play;
        if self.presence != Some(playing) {
            self.presence = Some(playing);
            let key = if playing { "presence.playing" } else { "presence.title" };
            platform.set_presence(StrKey::new(key), &[]);
        }
        if self.steps == steps && self.pos == pos {
            return;
        }
        let unlocked = platform.achievements().unlocked_count();
        platform.unlock(FIRST_STEPS);
        if self.steps != steps {
            platform.set_stat(STEPS, self.steps);
        }
        let max = self.bounds - Vec2::new(16.0, 16.0);
        let edge = |v: f32, hi: f32| v <= 0.0 || v >= hi;
        if edge(self.pos.x, max.x) && edge(self.pos.y, max.y) {
            platform.unlock(CORNER);
        }
        if platform.achievements().unlocked_count() != unlocked
            && let Some(t) = AchievementTable::from_platform(platform)
        {
            let mut buf = [0u8; 128];
            let n = platform.achievements().pack_for(&t, &mut buf);
            self.saving = n > 0 && platform.save_begin(&buf[..n]).is_ok();
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
        self.access.get(platform);
        self.about.get(platform);
        self.crash.get(platform);
        self.services(platform);
        self.strings.get(platform);
        // The host's settings, unless the screen just changed them.
        if !self.dirty && (platform.access() != self.settings || !self.shown) {
            self.settings = platform.access();
            self.show_access();
            self.shown = true;
        }
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
        let before = (self.steps, self.pos);
        self.update(input, audio, dt);
        self.report(platform, before);
        self.toast.update(platform, dt);
        if core::mem::take(&mut self.dirty) {
            platform.set_access(self.settings);
        }
        match self.ask.take() {
            Some(Ask::Graphics) => self.graphics.open(),
            Some(Ask::Rebind(button)) => platform.rebind(button, BindDevice::Keyboard),
            Some(Ask::ResetBindings) => platform.reset_bindings(),
            Some(Ask::Crash(c)) => platform.set_crash_consent(c),
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
        if let Mode::Achievements { from_pause } = self.mode {
            // B or Escape goes back where it came from.
            if input.just_pressed(&self.prev, Button::B)
                || input.key_pressed(&self.prev, trino_core::Key::Escape)
            {
                self.go(if from_pause { Mode::Paused } else { Mode::Title });
            } else {
                self.list.update(input, &self.table);
            }
            self.prev = *input;
            return;
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
        let from = self.pos;
        self.pos.x = (self.pos.x + dir.x * SPEED * dt).clamp(0.0, self.bounds.x - 16.0);
        self.pos.y = (self.pos.y - dir.y * SPEED * dt).clamp(0.0, self.bounds.y - 16.0);
        let d = self.pos - from;
        self.walked += d.x.abs() + d.y.abs();
        while self.walked >= STEP_PX {
            self.walked -= STEP_PX;
            self.steps += 1;
        }

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
            Mode::Access { .. } => Some(&self.access),
            Mode::About => Some(&self.about),
            Mode::CrashAsk => Some(&self.crash),
            Mode::Achievements { .. } => None,
        };
        if self.mode != Mode::Title {
            renderer.draw_sprite(PLAYER, self.pos, &SpriteParams::default());
        }
        if let Some(screen) = blob.and_then(ScreenBlob::screen) {
            let env = UiEnv::of(renderer)
                .with_interface_scale(self.interface_scale)
                .with_access(&self.settings);
            match self.strings.table() {
                Some(t) => self.ui.draw(renderer, screen, &env.with_strings(t)),
                None => self.ui.draw(renderer, screen, &env),
            }
        }
        let strings: &dyn UiStrings = match self.strings.table() {
            Some(t) => t,
            None => &trino_core::ui::NoStrings,
        };
        let style = OverlayStyle {
            font: UI_FONT,
            ..OverlayStyle::DEFAULT.with_access(&self.settings)
        };
        if let Mode::Achievements { .. } = self.mode {
            self.list
                .draw(renderer, strings, &self.table, &self.achievements, &style);
        }
        self.toast.draw(renderer, strings, &self.table, &style);
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

    /// A console-like host that keeps the accessibility settings.
    #[derive(Default)]
    struct Keeper {
        access: AccessSettings,
    }
    impl Platform for Keeper {
        fn now_micros(&self) -> u64 {
            0
        }
        fn log(&self, _: &str) {}
        fn access(&self) -> AccessSettings {
            self.access
        }
        fn set_access(&mut self, s: AccessSettings) {
            self.access = s;
        }
    }

    #[test]
    fn the_accessibility_screen_sets_the_platforms_settings() {
        let mut game = {{name_camel}}Game::new(Vec2::new(320.0, 240.0));
        game.mode = Mode::Options { from_pause: false };
        assert_eq!(
            game.action("accessibility"),
            Some(Mode::Access { from_pause: false })
        );
        game.mode = Mode::Access { from_pause: false };
        let mut host = Keeper::default();
        let idle = InputState::default();
        game.update_with(&mut host, &idle, &mut NullAudio, 0.016);
        assert_eq!(game.ui.values().get("cvd", NO_ITEM), Some(text_value("Off")));
        game.change_access("cvd", 0);
        game.change_access("cvd", 0);
        game.change_access("text", 175);
        game.change_access("contrast", 1);
        game.change_access("captions", 0);
        game.update_with(&mut host, &idle, &mut NullAudio, 0.016);
        assert_eq!(host.access.cvd, CvdMode::Deuteranopia);
        assert_eq!(host.access.text_scale, 175);
        assert!(host.access.high_contrast);
        assert_eq!(host.access.captions, CaptionMode::Important);
        assert_eq!(game.ui.values().get("cvd", NO_ITEM), Some(text_value("Deutan")));
        // A change from the host (a saved file) shows on the screen.
        host.access.reduce_motion = true;
        game.update_with(&mut host, &idle, &mut NullAudio, 0.016);
        assert!(game.ui.get_bool("motion"));
        assert_eq!(game.action("back"), Some(Mode::Options { from_pause: false }));
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

    /// A console-like platform: the achievements table, the services every
    /// backend holds, and a save in memory.
    struct Console {
        table: &'static [u8],
        services: trino_core::HostedServices,
        save: Vec<u8>,
        crash: Option<trino_core::CrashReports>,
    }

    impl Platform for Console {
        fn now_micros(&self) -> u64 {
            0
        }
        fn log(&self, _: &str) {}
        fn blob(&self, id: trino_core::BlobId) -> Option<&'static [u8]> {
            (id == trino_core::services::table::BLOB).then_some(self.table)
        }
        fn services(&self) -> Option<&trino_core::HostedServices> {
            Some(&self.services)
        }
        fn services_mut(&mut self) -> Option<&mut trino_core::HostedServices> {
            Some(&mut self.services)
        }
        fn save_capacity(&self) -> usize {
            2032
        }
        fn save_read(&mut self, buf: &mut [u8]) -> Result<usize, trino_core::SaveError> {
            buf[..self.save.len()].copy_from_slice(&self.save);
            Ok(self.save.len())
        }
        fn save_write(&mut self, data: &[u8]) -> Result<(), trino_core::SaveError> {
            self.save = data.to_vec();
            Ok(())
        }
        fn crash_reports(&self) -> Option<trino_core::CrashReports> {
            self.crash
        }
        fn set_crash_consent(&mut self, c: trino_core::CrashConsent) {
            if let Some(r) = &mut self.crash {
                r.consent = c;
            }
        }
    }

    fn console(save: Vec<u8>) -> Console {
        use trino_core::services::table::{self, AchievementRecord, StatRecord};
        let a = |api, stat, hidden| AchievementRecord {
            api,
            name: "n",
            description: "d",
            icon: 0,
            hidden,
            stat,
        };
        let ach = [
            a("FIRST_STEPS", None, false),
            a("WANDERER", Some((0, 100)), false),
            a("CORNER", None, true),
        ];
        let stats = [StatRecord {
            api: "STEPS",
            min: 0,
            max: 1_000_000,
            increment_only: true,
        }];
        let mut blob = vec![0u8; table::encoded_len(&ach, &stats, &[])];
        table::encode(&ach, &stats, &[], &mut blob).unwrap();
        Console {
            table: Box::leak(blob.into_boxed_slice()),
            services: trino_core::HostedServices::new(trino_core::ServiceBackend::Session),
            save,
            crash: None,
        }
    }

    /// Moving unlocks FIRST_STEPS (a popup) and saves the achievements; a
    /// new boot on the same save restores them, as an N64 or a 3DS does.
    #[test]
    fn moving_unlocks_an_achievement_that_the_save_keeps() {
        let mut game = {{name_camel}}Game::new(Vec2::new(320.0, 240.0));
        let mut host = console(Vec::new());
        game.mode = Mode::Play;
        let mut input = InputState::default();
        input.set(Button::DpadRight, true);
        for _ in 0..4 {
            game.update_with(&mut host, &input, &mut NullAudio, 0.1);
        }
        assert!(host.is_unlocked(FIRST_STEPS));
        assert!(host.stat(STEPS) > 0);
        assert_eq!(game.toast.current(), Some(0));
        assert!(!host.save.is_empty(), "the unlock saved");
        let mut again = {{name_camel}}Game::new(Vec2::new(320.0, 240.0));
        let mut next = console(host.save.clone());
        again.update_with(&mut next, &InputState::default(), &mut NullAudio, 0.1);
        assert!(next.is_unlocked(FIRST_STEPS), "restored from the save");
        assert_eq!(again.toast.current(), None, "no popup for a restored one");
        // The list opens from the title and B goes back.
        again.mode = Mode::Title;
        assert_eq!(
            again.action("achievements"),
            Some(Mode::Achievements { from_pause: false })
        );
        again.go(Mode::Achievements { from_pause: false });
        let mut b = InputState::default();
        b.set(Button::B, true);
        again.update_with(&mut next, &b, &mut NullAudio, 0.1);
        assert_eq!(again.mode, Mode::Title);
        assert_eq!(again.action("about"), Some(Mode::About));
    }

    /// A report waiting with an endpoint and no answer: the question comes
    /// first; its buttons answer through the platform and lead to the title.
    #[test]
    fn the_crash_question_comes_first_and_answers() {
        use trino_core::{CrashConsent, CrashReports};
        let mut game = {{name_camel}}Game::new(Vec2::new(320.0, 240.0));
        let mut host = console(Vec::new());
        host.crash = Some(CrashReports {
            pending: 1,
            consent: CrashConsent::Ask,
            upload: true,
        });
        game.update_with(&mut host, &InputState::default(), &mut NullAudio, 0.1);
        assert_eq!(game.mode, Mode::CrashAsk);
        assert_eq!(ask_for("yes"), Some(Ask::Crash(CrashConsent::Allow)));
        assert_eq!(game.action("no"), Some(Mode::Title));
        game.ask = ask_for("yes");
        game.update_with(&mut host, &InputState::default(), &mut NullAudio, 0.1);
        assert_eq!(host.crash.unwrap().consent, CrashConsent::Allow);
        // Answered (or no endpoint): never asked again.
        let mut next = {{name_camel}}Game::new(Vec2::new(320.0, 240.0));
        next.update_with(&mut host, &InputState::default(), &mut NullAudio, 0.1);
        assert_eq!(next.mode, Mode::Title);
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
