//! The game contract driven like an app drives it: an `Input` backend
//! polled each frame, `Game::update_with` with platform + audio, then
//! `Game::render` — plus the trait's provided `update_with` and `resize`.

mod common;

use common::*;
use trino_core::{
    Audio, BlobId, Button, Color, Game, Input, InputState, Key, Platform, PointerButton, Renderer,
    SoundId, SpriteId, SpriteParams, Vec2,
};

/// A game that only implements the required methods.
#[derive(Default)]
struct Counter {
    updates: u32,
    dt_total: f32,
    jumps: u32,
    prev: InputState,
}

impl Game for Counter {
    fn update(&mut self, input: &InputState, audio: &mut dyn Audio, dt: f32) {
        self.updates += 1;
        self.dt_total += dt;
        if input.just_pressed(&self.prev, Button::A) {
            self.jumps += 1;
            audio.play_sound(SoundId::from_path("sounds/jump"));
        }
        self.prev = *input;
    }
    fn render(&mut self, r: &mut dyn Renderer) {
        r.begin_frame(Color::BLACK);
        for i in 0..self.jumps {
            r.draw_sprite(
                SpriteId(1),
                Vec2::new(i as f32 * 8.0, 0.0),
                &SpriteParams::DEFAULT,
            );
        }
        r.end_frame();
    }
}

/// A scripted pad: one `InputState` per frame.
struct Script(Vec<InputState>, usize);

impl Input for Script {
    fn poll(&mut self) -> InputState {
        let s = self.0.get(self.1).copied().unwrap_or_default();
        self.1 += 1;
        s
    }
}

fn pressed(b: Button) -> InputState {
    let mut s = InputState::default();
    s.set(b, true);
    s
}

#[test]
fn default_update_with_forwards_to_update_and_resize_is_a_no_op() {
    let mut g = Counter::default();
    let mut platform = MemoryPlatform::new(64);
    let mut audio = MockAudio::new(4);
    let mut input = Script(
        vec![
            InputState::default(),
            pressed(Button::A),
            pressed(Button::A), // held: not a new press
            InputState::default(),
            pressed(Button::A),
        ],
        0,
    );
    let mut r = Recorder::new();
    for _ in 0..5 {
        let state = input.poll();
        g.update_with(&mut platform, &state, &mut audio, 1.0 / 60.0);
        g.render(&mut r);
    }
    assert_eq!(g.updates, 5);
    assert!((g.dt_total - 5.0 / 60.0).abs() < 1e-6);
    assert_eq!(g.jumps, 2);
    assert_eq!(audio.one_shots.len(), 2);
    assert_eq!(
        r.sprites().len(),
        2,
        "the last frame drew one sprite per jump"
    );
    g.resize(Vec2::new(640.0, 360.0));
    assert_eq!(g.updates, 5, "resize does not tick the game");
}

/// A game that needs the platform: loads its level from a blob and keeps a
/// high score in save data.
struct Saver {
    level_len: usize,
    best: u8,
    screen: Vec2,
}

impl Game for Saver {
    fn update(&mut self, _: &InputState, _: &mut dyn Audio, _: f32) {
        unreachable!("the apps call update_with");
    }
    fn update_with(&mut self, p: &mut dyn Platform, input: &InputState, _: &mut dyn Audio, _: f32) {
        if self.level_len == 0 {
            self.level_len = p
                .blob(BlobId::from_path("blobs/level"))
                .map_or(0, <[u8]>::len);
            let mut buf = [0u8; 1];
            if p.save_read(&mut buf) == Ok(1) {
                self.best = buf[0];
            }
        }
        if input.key(Key::Enter) {
            self.best += 1;
            p.save_write(&[self.best]).unwrap();
            p.log("saved");
        }
    }
    fn render(&mut self, _: &mut dyn Renderer) {}
    fn resize(&mut self, screen: Vec2) {
        self.screen = screen;
    }
}

#[test]
fn games_reach_blobs_and_saves_through_update_with() {
    let mut platform = MemoryPlatform::new(128);
    platform.insert_blob(BlobId::from_path("blobs/level"), vec![b'#'; 40]);
    let mut audio = MockAudio::new(1);
    let mut enter = InputState::default();
    enter.set_key(Key::Enter, true);
    let mut g = Saver {
        level_len: 0,
        best: 0,
        screen: Vec2::ZERO,
    };
    g.update_with(&mut platform, &enter, &mut audio, 0.016);
    g.update_with(&mut platform, &enter, &mut audio, 0.016);
    assert_eq!((g.level_len, g.best), (40, 2));
    assert_eq!(platform.log.borrow().len(), 2);
    g.resize(Vec2::new(400.0, 240.0));
    assert_eq!(g.screen, Vec2::new(400.0, 240.0));

    // A new session (same medium) restores the score.
    let mut again = Saver {
        level_len: 0,
        best: 0,
        screen: Vec2::ZERO,
    };
    again.update_with(&mut platform, &InputState::default(), &mut audio, 0.016);
    assert_eq!(again.best, 2);
}

#[test]
fn input_state_edges_for_buttons_keys_and_pointer() {
    let prev = InputState::default();
    let mut now = InputState::default();
    for b in [
        Button::A,
        Button::B,
        Button::X,
        Button::Y,
        Button::L,
        Button::R,
        Button::Start,
        Button::Select,
        Button::DpadUp,
        Button::DpadDown,
        Button::DpadLeft,
        Button::DpadRight,
    ] {
        assert!(!now.is_down(b));
        now.set(b, true);
        assert!(now.is_down(b) && now.just_pressed(&prev, b) && !now.just_released(&prev, b));
    }
    let mut later = now;
    later.set(Button::Start, false);
    assert!(later.just_released(&now, Button::Start));
    assert!(!later.just_pressed(&now, Button::A), "held");

    now.set_key(Key::Ctrl, true);
    now.set_key(Key::F8, true);
    assert!(now.key(Key::Ctrl) && now.key(Key::F8) && !now.key(Key::Escape));
    assert!(now.key_pressed(&prev, Key::F8));
    assert!(!now.key_pressed(&now, Key::F8));
    now.set_key(Key::F8, false);
    assert!(!now.key(Key::F8));

    now.pointer = Some(Vec2::new(12.0, 30.0));
    now.pointer_screen = 1;
    now.wheel = -1.0;
    now.set_pointer_button(PointerButton::Secondary, true);
    assert!(
        now.pointer_down(PointerButton::Secondary) && !now.pointer_down(PointerButton::Primary)
    );
    assert!(now.pointer_pressed(&prev, PointerButton::Secondary));
    let mut up = now;
    up.set_pointer_button(PointerButton::Secondary, false);
    assert!(up.pointer_released(&now, PointerButton::Secondary));
    assert!(!up.pointer_released(&prev, PointerButton::Middle));
    // Separate bit spaces never alias.
    let mut only_key = InputState::default();
    only_key.set_key(Key::Escape, true);
    assert!(!only_key.is_down(Button::A) && !only_key.pointer_down(PointerButton::Primary));
}
