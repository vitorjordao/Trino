//! Trino's showcase platformer: run, jump, collect the coins, reach the
//! flag. One codebase, three consoles — this crate is the Fase 6 proof that
//! the engine's contracts carry a real game.
//!
//! Everything platform-specific is behind `trino_core` traits; the level is
//! plain ASCII (`level1.txt`) parsed by `trino_core::tilemap` with zero
//! allocation. Physics goes through `trino_core::collide` — pure `f32`
//! arithmetic, deterministic across PC/N64/3DS (verified by the
//! `update_is_deterministic` test and the console self-tests).

#![cfg_attr(any(target_os = "none", target_os = "horizon"), no_std)]

use trino_core::tilemap::tile;
use trino_core::{
    Audio, Button, Camera3, Color, Game, InputState, Material, ModelId, ModelParams, MusicId,
    Renderer, SoundId, SpriteId, SpriteParams, TILE_SIZE, Tilemap, Transform3, Vec2, Vec3,
    move_and_collide,
};

pub const LEVEL: &str = include_str!("level1.txt");

pub const HERO: SpriteId = SpriteId::from_path("sprites/hero");
pub const GROUND: SpriteId = SpriteId::from_path("sprites/ground");
pub const BRICK: SpriteId = SpriteId::from_path("sprites/brick");
pub const COIN: SpriteId = SpriteId::from_path("sprites/coin");
pub const FLAG: SpriteId = SpriteId::from_path("sprites/flag");
pub const SND_JUMP: SoundId = SoundId::from_path("sounds/jump");
pub const SND_COIN: SoundId = SoundId::from_path("sounds/coin");
pub const SND_WIN: SoundId = SoundId::from_path("sounds/win");
pub const MUSIC: MusicId = MusicId::from_path("music/theme");
pub const CUBE: ModelId = ModelId::from_path("models/cube");

/// Collision box (the hero sprite is 16x16; the box is slightly narrower
/// so the player does not snag on tile seams).
pub const HERO_SIZE: Vec2 = Vec2::new(12.0, 15.0);
const HERO_SPRITE_SIZE: f32 = 16.0;

const MOVE_SPEED: f32 = 100.0;
const GRAVITY: f32 = 600.0;
const JUMP_VELOCITY: f32 = -250.0;
const MAX_FALL: f32 = 300.0;
const MAX_COINS: usize = 64;

const SKY: Color = Color::rgb(92, 148, 252);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameState {
    Playing,
    Won,
}

pub struct PlatformerGame {
    map: Tilemap<'static>,
    pub pos: Vec2,
    pub vel: Vec2,
    pub on_ground: bool,
    pub state: GameState,
    pub collected: u32,
    coins: [(Vec2, bool); MAX_COINS],
    coin_count: usize,
    spawn: Vec2,
    flag: Vec2,
    screen: Vec2,
    camera: Vec2,
    face_left: bool,
    time: f32,
    music_started: bool,
    prev: InputState,
}

impl PlatformerGame {
    /// `screen` is the internal resolution of the current platform/profile.
    pub fn new(screen: Vec2) -> Self {
        let map = Tilemap::parse(LEVEL).expect("level1.txt must be rectangular");
        let mut coins = [(Vec2::ZERO, false); MAX_COINS];
        let mut coin_count = 0;
        let mut spawn = Vec2::ZERO;
        let mut flag = Vec2::ZERO;
        for (tx, ty, t) in map.cells() {
            match t {
                tile::COIN => {
                    if coin_count < MAX_COINS {
                        coins[coin_count] = (map.cell_pos(tx, ty), true);
                        coin_count += 1;
                    }
                }
                tile::SPAWN => spawn = map.cell_pos(tx, ty),
                tile::FLAG => flag = map.cell_pos(tx, ty),
                _ => {}
            }
        }
        // Feet on the spawn cell's floor.
        spawn.y += TILE_SIZE - HERO_SIZE.y;
        PlatformerGame {
            map,
            pos: spawn,
            vel: Vec2::ZERO,
            on_ground: false,
            state: GameState::Playing,
            collected: 0,
            coins,
            coin_count,
            spawn,
            flag,
            screen,
            camera: Vec2::ZERO,
            face_left: false,
            time: 0.0,
            music_started: false,
            prev: InputState::default(),
        }
    }

    pub fn total_coins(&self) -> u32 {
        self.coin_count as u32
    }

    fn reset(&mut self) {
        self.pos = self.spawn;
        self.vel = Vec2::ZERO;
        self.on_ground = false;
    }

    fn restart(&mut self) {
        self.reset();
        self.state = GameState::Playing;
        self.collected = 0;
        for coin in self.coins[..self.coin_count].iter_mut() {
            coin.1 = true;
        }
    }

    fn hero_rect_overlaps(&self, center: Vec2, half: f32) -> bool {
        let hero_center = self.pos + HERO_SIZE * 0.5;
        (hero_center.x - center.x).abs() < half + HERO_SIZE.x * 0.5
            && (hero_center.y - center.y).abs() < half + HERO_SIZE.y * 0.5
    }

    /// Triangle wave in [0, 1] with period 1s — the no-libm animation clock.
    fn wave(&self) -> f32 {
        let t = self.time % 1.0;
        if t < 0.5 { t * 2.0 } else { 2.0 - t * 2.0 }
    }
}

impl Game for PlatformerGame {
    fn update(&mut self, input: &InputState, audio: &mut dyn Audio, dt: f32) {
        if !self.music_started {
            self.music_started = true;
            audio.play_music(MUSIC, true);
        }
        self.time += dt;

        if self.state == GameState::Won {
            if input.just_pressed(&self.prev, Button::Start) {
                self.restart();
            }
            self.prev = *input;
            return;
        }

        // Horizontal intent: d-pad wins over stick.
        let mut dir = input.stick.x;
        if input.is_down(Button::DpadLeft) {
            dir = -1.0;
        }
        if input.is_down(Button::DpadRight) {
            dir = 1.0;
        }
        self.vel.x = dir * MOVE_SPEED;
        if dir < -0.01 {
            self.face_left = true;
        } else if dir > 0.01 {
            self.face_left = false;
        }

        if self.on_ground && input.just_pressed(&self.prev, Button::A) {
            self.vel.y = JUMP_VELOCITY;
            audio.play_sound(SND_JUMP);
        }
        self.vel.y = (self.vel.y + GRAVITY * dt).min(MAX_FALL);

        let moved = move_and_collide(&self.map, self.pos, HERO_SIZE, self.vel * dt);
        self.pos = moved.pos;
        self.on_ground = moved.on_ground;
        if moved.on_ground || moved.hit_ceiling {
            self.vel.y = 0.0;
        }

        // Fell into a pit: back to the spawn (coins stay collected).
        if self.pos.y > self.map.pixel_size().y + 2.0 * TILE_SIZE {
            self.reset();
        }

        // Coins.
        for i in 0..self.coin_count {
            let (pos, alive) = self.coins[i];
            if alive && self.hero_rect_overlaps(pos + Vec2::new(8.0, 8.0), 5.0) {
                self.coins[i].1 = false;
                self.collected += 1;
                audio.play_sound(SND_COIN);
            }
        }

        // Goal.
        if self.hero_rect_overlaps(self.flag + Vec2::new(8.0, 8.0), 7.0) {
            self.state = GameState::Won;
            audio.play_sound(SND_WIN);
            audio.stop_music();
        }

        // Camera follows, clamped to the level.
        let target = self.pos + HERO_SIZE * 0.5 - self.screen * 0.5;
        let max = self.map.pixel_size() - self.screen;
        self.camera.x = target.x.clamp(0.0, max.x.max(0.0));
        self.camera.y = target.y.clamp(0.0, max.y.max(0.0));

        self.prev = *input;
    }

    fn render(&mut self, renderer: &mut dyn Renderer) {
        renderer.begin_frame(SKY);
        let cam = self.camera;

        // Visible tile columns only.
        let first_col = (cam.x / TILE_SIZE) as i32 - 1;
        let last_col = ((cam.x + self.screen.x) / TILE_SIZE) as i32 + 1;
        for ty in 0..self.map.height as i32 {
            for tx in first_col.max(0)..=last_col.min(self.map.width as i32 - 1) {
                let sprite = match self.map.tile(tx, ty) {
                    tile::GROUND => GROUND,
                    tile::BRICK => BRICK,
                    _ => continue,
                };
                let pos = Vec2::new(tx as f32 * TILE_SIZE, ty as f32 * TILE_SIZE) - cam;
                renderer.draw_sprite(sprite, pos, &SpriteParams::default());
            }
        }

        // Coins bob on the animation clock.
        let bob = self.wave() * 3.0;
        for &(pos, alive) in &self.coins[..self.coin_count] {
            if alive {
                renderer.draw_sprite(
                    COIN,
                    pos - cam + Vec2::new(0.0, bob - 1.5),
                    &SpriteParams::default(),
                );
            }
        }

        // Flag flashes after winning.
        let flag_tint = if self.state == GameState::Won && self.wave() > 0.5 {
            Color::rgb(255, 255, 160)
        } else {
            Color::WHITE
        };
        renderer.draw_sprite(
            FLAG,
            self.flag - cam,
            &SpriteParams {
                tint: flag_tint,
                ..Default::default()
            },
        );

        // Hero: center the 16px sprite on the 12px collision box.
        let sprite_pos = self.pos - Vec2::new((HERO_SPRITE_SIZE - HERO_SIZE.x) * 0.5, 1.0);
        renderer.draw_sprite(
            HERO,
            sprite_pos - cam,
            &SpriteParams {
                flip_x: self.face_left,
                ..Default::default()
            },
        );

        // Showcase 3D (Fase 7): a spinning vertex-lit cube, fixed camera,
        // drawn over the scene and under the HUD.
        renderer.set_camera(&Camera3::default());
        renderer.draw_model(
            CUBE,
            &Transform3 {
                position: Vec3::new(2.4, 1.5, 0.0),
                rotation: Vec3::new(self.time * 0.7, self.time, 0.0),
                scale: Vec3::new(0.7, 0.7, 0.7),
            },
            Material::VertexLit,
            &ModelParams::default(),
        );

        // HUD: one coin icon per collected coin (no font needed).
        for i in 0..self.collected {
            renderer.draw_sprite(
                COIN,
                Vec2::new(4.0 + i as f32 * 10.0, 4.0),
                &SpriteParams {
                    scale: Vec2::new(0.75, 0.75),
                    ..Default::default()
                },
            );
        }

        renderer.end_frame();
    }
}

// Hot-reload exports (trino_game_update / trino_game_render / version).
trino_game_api::export_game!(PlatformerGame);

#[cfg(test)]
mod tests {
    use super::*;

    struct CountAudio {
        sounds: usize,
        music: usize,
    }
    impl Audio for CountAudio {
        fn play_sound(&mut self, _: SoundId) {
            self.sounds += 1;
        }
        fn play_music(&mut self, _: MusicId, _: bool) {
            self.music += 1;
        }
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
    }

    fn audio() -> CountAudio {
        CountAudio {
            sounds: 0,
            music: 0,
        }
    }

    fn game() -> PlatformerGame {
        PlatformerGame::new(Vec2::new(320.0, 240.0))
    }

    fn held(button: Button) -> InputState {
        let mut s = InputState::default();
        s.set(button, true);
        s
    }

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn spawns_on_the_ground_after_settling() {
        let mut g = game();
        let mut a = audio();
        for _ in 0..30 {
            g.update(&InputState::default(), &mut a, DT);
        }
        assert!(g.on_ground);
        // Spawn cell row 12; floor top at row 13 -> feet flush at 13*16.
        assert_eq!(g.pos.y, 13.0 * 16.0 - HERO_SIZE.y);
        assert_eq!(g.vel.y, 0.0);
    }

    #[test]
    fn walks_right_and_left() {
        let mut g = game();
        let mut a = audio();
        for _ in 0..30 {
            g.update(&InputState::default(), &mut a, DT);
        }
        let x0 = g.pos.x;
        for _ in 0..30 {
            g.update(&held(Button::DpadRight), &mut a, DT);
        }
        assert!(g.pos.x > x0 + 40.0, "moved only {}", g.pos.x - x0);
        let x1 = g.pos.x;
        for _ in 0..15 {
            g.update(&held(Button::DpadLeft), &mut a, DT);
        }
        assert!(g.pos.x < x1);
    }

    #[test]
    fn jumps_and_lands_back() {
        let mut g = game();
        let mut a = audio();
        for _ in 0..30 {
            g.update(&InputState::default(), &mut a, DT);
        }
        let ground_y = g.pos.y;
        let sounds_before = a.sounds;
        g.update(&held(Button::A), &mut a, DT);
        assert_eq!(a.sounds, sounds_before + 1, "jump sound on press edge");
        let mut peak = ground_y;
        let mut landed = false;
        for _ in 0..120 {
            g.update(&InputState::default(), &mut a, DT);
            peak = peak.min(g.pos.y);
            if g.on_ground {
                landed = true;
                break;
            }
        }
        assert!(
            peak < ground_y - 30.0,
            "rose only to {peak} from {ground_y}"
        );
        assert!(landed);
        assert!((g.pos.y - ground_y).abs() < 0.001);
    }

    #[test]
    fn collects_the_coin_on_the_path() {
        // Coin at column 15, row 12: on the walking path right of spawn,
        // before the first pit.
        let mut g = game();
        let mut a = audio();
        for _ in 0..30 {
            g.update(&InputState::default(), &mut a, DT);
        }
        let total = g.total_coins();
        for _ in 0..240 {
            g.update(&held(Button::DpadRight), &mut a, DT);
            if g.collected > 0 {
                break;
            }
        }
        assert_eq!(g.collected, 1, "should collect the first ground coin");
        assert_eq!(g.total_coins(), total);
    }

    #[test]
    fn music_starts_once() {
        let mut g = game();
        let mut a = audio();
        for _ in 0..10 {
            g.update(&InputState::default(), &mut a, DT);
        }
        assert_eq!(a.music, 1);
    }

    #[test]
    fn update_is_deterministic() {
        let run = || {
            let mut g = game();
            let mut a = audio();
            for i in 0..600u32 {
                let input = match (i / 30) % 4 {
                    0 => held(Button::DpadRight),
                    1 => {
                        let mut s = held(Button::DpadRight);
                        s.set(Button::A, true);
                        s
                    }
                    2 => held(Button::DpadLeft),
                    _ => InputState::default(),
                };
                g.update(&input, &mut a, DT);
            }
            (g.pos, g.vel, g.collected, g.on_ground)
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn level_has_the_expected_pickups() {
        let g = game();
        assert_eq!(g.total_coins(), 15);
        assert!(g.flag.x > 0.0 && g.spawn.x > 0.0);
    }
}
