//! castle64 — um "Mario 64 de bolso" para estressar o pipeline 3D do Trino.
//!
//! Hub (pátio do castelo) + 3 fases de plataforma + sala do trono, tudo em
//! geometria de cubos vertex-lit (`draw_model`), com física 3D própria
//! (`physics.rs`), câmera orbital, moedas, estrelas, inimigos com stomp,
//! lava, plataformas móveis e HUD de sprites — zero alocação, `no_std`,
//! determinístico nas três plataformas.
//!
//! Controles (PC entre parênteses): stick/d-pad move relativo à câmera,
//! A (Z) pula, L/R (Q/E) giram a câmera, B (X) volta ao hub, Start (Enter)
//! reinicia a fase.

#![cfg_attr(any(target_os = "none", target_os = "horizon"), no_std)]

pub mod bot;
mod levels;
mod physics;

use levels::{BlockKind, DoorColor, LEVELS, Level, TOTAL_STARS};
use physics::{Aabb, move_aabb};
use trino_core::math3d::{atan2, cos, sin};
use trino_core::{
    Audio, Button, Camera3, Color, Game, InputState, Material, ModelId, ModelParams, MusicId,
    Renderer, SoundId, SpriteId, SpriteParams, Transform3, Vec2, Vec3,
};

// ------------------------------------------------------------ handles ----

const M_GRASS: ModelId = ModelId::from_path("models/c64_block_grass");
const M_STONE: ModelId = ModelId::from_path("models/c64_block_stone");
const M_BRICK: ModelId = ModelId::from_path("models/c64_block_brick");
const M_CASTLE: ModelId = ModelId::from_path("models/c64_block_castle");
const M_ROOF: ModelId = ModelId::from_path("models/c64_block_roof");
const M_LAVA: ModelId = ModelId::from_path("models/c64_block_lava");
const M_COIN: ModelId = ModelId::from_path("models/c64_coin");
const M_STAR: ModelId = ModelId::from_path("models/c64_star");
const M_TORSO: ModelId = ModelId::from_path("models/c64_player_torso");
const M_ARM: ModelId = ModelId::from_path("models/c64_player_arm");
const M_LEG: ModelId = ModelId::from_path("models/c64_player_leg");
const M_BOAR: ModelId = ModelId::from_path("models/c64_boar");
const M_SHADOW: ModelId = ModelId::from_path("models/c64_shadow");
/// Porta neutra gerada, tingida por destino (verde/vermelha/azul/cinza).
const M_DOOR_FRAME: ModelId = ModelId::from_path("models/c64_door_frame");
/// Entrada do castelo: doorway low-poly real (KayKit Dungeon Remastered,
/// CC0) — textura de paleta amostrada para vertex colors no bake.
const M_DOOR_KAYKIT: ModelId = ModelId::from_path("models/c64_door");

const S_COIN: SpriteId = SpriteId::from_path("sprites/coin");
const S_STAR: SpriteId = SpriteId::from_path("sprites/c64_star");
const S_DIGITS: [SpriteId; 10] = [
    SpriteId::from_path("sprites/c64_digit0"),
    SpriteId::from_path("sprites/c64_digit1"),
    SpriteId::from_path("sprites/c64_digit2"),
    SpriteId::from_path("sprites/c64_digit3"),
    SpriteId::from_path("sprites/c64_digit4"),
    SpriteId::from_path("sprites/c64_digit5"),
    SpriteId::from_path("sprites/c64_digit6"),
    SpriteId::from_path("sprites/c64_digit7"),
    SpriteId::from_path("sprites/c64_digit8"),
    SpriteId::from_path("sprites/c64_digit9"),
];

const SND_JUMP: SoundId = SoundId::from_path("sounds/jump");
const SND_COIN: SoundId = SoundId::from_path("sounds/coin");
const SND_WIN: SoundId = SoundId::from_path("sounds/win");
const SND_HURT: SoundId = SoundId::from_path("sounds/beep");
const MUSIC: MusicId = MusicId::from_path("music/theme");

// ------------------------------------------------------------ tuning ----

/// Meio-tamanho do AABB do player (modelo tem ~1.1 de altura).
const PLAYER_HALF: Vec3 = Vec3::new(0.3, 0.55, 0.3);
const MOVE_SPEED: f32 = 5.5;
/// Aceleração no chão / no ar (u/s²) — movimento com inércia, não binário.
const ACCEL_GROUND: f32 = 30.0;
const ACCEL_AIR: f32 = 14.0;
/// Velocidade angular do corpo virando para a direção do movimento (rad/s).
const TURN_SPEED: f32 = 12.0;
const GRAVITY: f32 = 25.0;
const JUMP_VELOCITY: f32 = 9.0;
/// Soltar A cedo corta a subida até aqui (pulo de altura variável).
const JUMP_CUT: f32 = 3.5;
const MAX_FALL: f32 = 20.0;
const CAM_DIST: f32 = 7.0;
const CAM_HEIGHT: f32 = 3.2;
const CAM_SPEED: f32 = 2.2;
const STOMP_BOUNCE: f32 = 6.0;
const STAR_GET_SECS: f32 = 2.5;

/// Distância mínima da câmera quando o raycast a encosta numa parede.
const CAM_MIN_DIST: f32 = 2.0;
const MAX_SOLIDS: usize = 40;
const MAX_ENEMIES: usize = 4;
const CULL_FAR: f32 = 60.0;

const ENEMY_HALF: Vec3 = Vec3::new(0.3, 0.26, 0.34);

// ------------------------------------------------------------- state ----

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    Playing,
    /// Pegou uma estrela: pausa festiva, depois volta ao hub.
    StarGet(f32),
}

#[derive(Clone, Copy, Default)]
struct EnemyState {
    t: f32,
    pos: Vec3,
    yaw: f32,
    alive: bool,
}

/// Um sólido do frame + o que ele significa para o gameplay.
#[derive(Clone, Copy)]
struct Solid {
    aabb: Aabb,
    lava: bool,
    /// Índice do mover que gerou este sólido, se houver.
    mover: Option<usize>,
}

const EMPTY_SOLID: Solid = Solid {
    aabb: Aabb::new(Vec3::ZERO, Vec3::ZERO),
    lava: false,
    mover: None,
};

pub struct Castle64Game {
    /// Centro do AABB do player.
    pub pos: Vec3,
    pub vel: Vec3,
    pub on_ground: bool,
    pub level: usize,
    /// Bitmask das estrelas coletadas (bit por nível).
    pub stars: u8,
    yaw: f32,
    cam_yaw: f32,
    state: State,
    coins_taken: [u32; LEVELS.len()],
    enemies: [EnemyState; MAX_ENEMIES],
    standing_mover: Option<usize>,
    /// Relógio do nível (anima movers/moedas; zera ao trocar de nível).
    level_time: f32,
    /// Fase do ciclo de caminhada (braços/pernas articulados).
    walk_phase: f32,
    screen: Vec2,
    music_started: bool,
    prev: InputState,
}

/// Onda triangular 0..1 com período 1.
fn tri_wave(t: f32) -> f32 {
    let p = t - (t as i32) as f32;
    let p = if p < 0.0 { p + 1.0 } else { p };
    if p < 0.5 { p * 2.0 } else { 2.0 - p * 2.0 }
}

fn lerp3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a + (b - a) * t
}

fn mover_min(m: &levels::Mover, time: f32) -> Vec3 {
    lerp3(m.a, m.b, tri_wave(time / m.period))
}

impl Castle64Game {
    pub fn new(screen: Vec2) -> Self {
        let mut game = Castle64Game {
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            on_ground: false,
            level: 0,
            stars: 0,
            yaw: 0.0,
            cam_yaw: 0.0,
            state: State::Playing,
            coins_taken: [0; LEVELS.len()],
            enemies: [EnemyState::default(); MAX_ENEMIES],
            standing_mover: None,
            level_time: 0.0,
            walk_phase: 0.0,
            screen,
            music_started: false,
            prev: InputState::default(),
        };
        game.goto_level(0);
        game
    }

    fn cur(&self) -> &'static Level {
        LEVELS[self.level]
    }

    /// Entra num nível (ou re-spawna no atual): reposiciona player, câmera
    /// e inimigos. Moedas/estrelas coletadas persistem na sessão.
    /// Público para harnesses de teste/screenshot.
    pub fn goto_level(&mut self, level: usize) {
        self.level = level;
        let l = self.cur();
        self.pos = l.spawn + Vec3::new(0.0, PLAYER_HALF.y + 0.01, 0.0);
        self.vel = Vec3::ZERO;
        self.on_ground = false;
        self.cam_yaw = l.spawn_yaw;
        self.yaw = atan2(-sin(l.spawn_yaw), -cos(l.spawn_yaw));
        self.standing_mover = None;
        self.level_time = 0.0;
        self.enemies = [EnemyState::default(); MAX_ENEMIES];
        for (i, def) in l.enemies.iter().take(MAX_ENEMIES).enumerate() {
            self.enemies[i] = EnemyState {
                t: 0.0,
                pos: def.a,
                yaw: 0.0,
                alive: true,
            };
        }
    }

    pub fn total_coins(&self) -> u32 {
        let mut n = 0;
        for taken in &self.coins_taken {
            n += taken.count_ones();
        }
        n
    }

    pub fn star_count(&self) -> u32 {
        (self.stars & ((1 << TOTAL_STARS) - 1)).count_ones()
    }

    /// Monta a lista de sólidos do frame. Ordem: blocos, movers, portas
    /// trancadas — os índices dos movers são `n_blocks..n_blocks+n_movers`.
    fn build_solids(&self, out: &mut [Solid; MAX_SOLIDS]) -> usize {
        let l = self.cur();
        let mut n = 0;
        for b in l.blocks {
            if n == MAX_SOLIDS {
                break;
            }
            out[n] = Solid {
                aabb: Aabb::new(b.min, b.min + b.size),
                lava: b.kind == BlockKind::Lava,
                mover: None,
            };
            n += 1;
        }
        for (i, m) in l.movers.iter().enumerate() {
            if n == MAX_SOLIDS {
                break;
            }
            let min = mover_min(m, self.level_time);
            out[n] = Solid {
                aabb: Aabb::new(min, min + m.size),
                lava: false,
                mover: Some(i),
            };
            n += 1;
        }
        for p in l.portals {
            if n == MAX_SOLIDS {
                break;
            }
            if self.star_count() < p.need as u32 {
                out[n] = Solid {
                    aabb: Aabb::from_center_half(
                        p.pos + Vec3::new(0.0, 1.0, 0.0),
                        Vec3::new(0.7, 1.0, 0.45),
                    ),
                    lava: false,
                    mover: None,
                };
                n += 1;
            }
        }
        n
    }

    fn respawn(&mut self, audio: &mut dyn Audio) {
        audio.play_sound(SND_HURT);
        self.goto_level(self.level);
    }
}

impl Game for Castle64Game {
    fn update(&mut self, input: &InputState, audio: &mut dyn Audio, dt: f32) {
        if !self.music_started {
            self.music_started = true;
            audio.play_music(MUSIC, true);
        }

        if let State::StarGet(timer) = self.state {
            let left = timer - dt;
            if left <= 0.0 {
                self.state = State::Playing;
                self.goto_level(0);
            } else {
                self.state = State::StarGet(left);
            }
            self.prev = *input;
            return;
        }

        self.level_time += dt;
        let l = self.cur();

        // Câmera orbital: L/R giram em torno do player.
        if input.is_down(Button::L) {
            self.cam_yaw += CAM_SPEED * dt;
        }
        if input.is_down(Button::R) {
            self.cam_yaw -= CAM_SPEED * dt;
        }

        // Direção de movimento relativa à câmera (stick é Y-up).
        let mut dir_in = input.stick;
        if input.is_down(Button::DpadLeft) {
            dir_in.x = -1.0;
        }
        if input.is_down(Button::DpadRight) {
            dir_in.x = 1.0;
        }
        if input.is_down(Button::DpadUp) {
            dir_in.y = 1.0;
        }
        if input.is_down(Button::DpadDown) {
            dir_in.y = -1.0;
        }
        let fwd = Vec3::new(-sin(self.cam_yaw), 0.0, -cos(self.cam_yaw));
        let right = Vec3::new(0.0, 1.0, 0.0).cross(fwd);
        let mut move_dir = fwd * dir_in.y + right * dir_in.x;
        let move_len = trino_core::math3d::sqrt(move_dir.dot(move_dir));
        if move_len > 1.0 {
            move_dir = move_dir * (1.0 / move_len);
        }

        // Movimento com inércia: a velocidade PERSEGUE o alvo (aceleração no
        // chão, controle reduzido no ar) em vez de trocar instantaneamente.
        let target = move_dir * MOVE_SPEED;
        let accel = if self.on_ground {
            ACCEL_GROUND
        } else {
            ACCEL_AIR
        } * dt;
        self.vel.x += (target.x - self.vel.x).clamp(-accel, accel);
        self.vel.z += (target.z - self.vel.z).clamp(-accel, accel);

        // O corpo gira suavemente para onde anda (yaw com velocidade máxima).
        let speed = trino_core::math3d::sqrt(self.vel.x * self.vel.x + self.vel.z * self.vel.z);
        if speed > 0.4 {
            let want = atan2(self.vel.x, self.vel.z);
            let mut d = want - self.yaw;
            while d > core::f32::consts::PI {
                d -= core::f32::consts::TAU;
            }
            while d < -core::f32::consts::PI {
                d += core::f32::consts::TAU;
            }
            let max = TURN_SPEED * dt;
            self.yaw += d.clamp(-max, max);
        }

        // Ciclo de caminhada avança com a velocidade real no chão.
        if self.on_ground {
            self.walk_phase += speed * dt * 1.9;
        }

        if self.on_ground && input.just_pressed(&self.prev, Button::A) {
            self.vel.y = JUMP_VELOCITY;
            self.on_ground = false;
            audio.play_sound(SND_JUMP);
        }
        // Pulo variável: soltar A cedo corta a subida.
        if !self.on_ground && self.vel.y > JUMP_CUT && !input.is_down(Button::A) {
            self.vel.y = JUMP_CUT;
        }

        // Plataformas móveis: quem está em cima é carregado pelo delta.
        if let Some(mi) = self.standing_mover
            && let Some(m) = l.movers.get(mi)
        {
            let prev = mover_min(m, self.level_time - dt);
            let cur = mover_min(m, self.level_time);
            self.pos += cur - prev;
        }

        // Física com substeps fixos (≤ 1/120 s) — sem tunneling.
        let mut solids = [EMPTY_SOLID; MAX_SOLIDS];
        let n_solids = self.build_solids(&mut solids);
        let mut boxes = [Aabb::new(Vec3::ZERO, Vec3::ZERO); MAX_SOLIDS];
        for i in 0..n_solids {
            boxes[i] = solids[i].aabb;
        }
        let steps = ((dt * 120.0) as i32 + 1).clamp(1, 12);
        let h = dt / steps as f32;
        for _ in 0..steps {
            self.vel.y = (self.vel.y - GRAVITY * h).max(-MAX_FALL);
            let out = move_aabb(self.pos, PLAYER_HALF, self.vel, h, &boxes[..n_solids]);
            self.pos = out.pos;
            self.vel = out.vel;
            self.on_ground = out.on_ground;
            self.standing_mover = out.standing_on.and_then(|i| solids[i].mover);
        }

        // Lava e queda no vazio.
        let feet = Aabb::from_center_half(self.pos, PLAYER_HALF);
        let touched_lava = solids[..n_solids].iter().any(|s| {
            s.lava
                && feet.overlaps(&Aabb::new(
                    s.aabb.min,
                    s.aabb.max + Vec3::new(0.0, 0.12, 0.0),
                ))
        });
        if touched_lava || self.pos.y < l.kill_y {
            self.respawn(audio);
            self.prev = *input;
            return;
        }

        // Moedas.
        for (i, c) in l.coins.iter().enumerate() {
            let bit = 1u32 << i;
            if self.coins_taken[self.level] & bit == 0 {
                let d = self.pos - *c;
                if d.dot(d) < 0.85 * 0.85 {
                    self.coins_taken[self.level] |= bit;
                    audio.play_sound(SND_COIN);
                }
            }
        }

        // Estrela do nível.
        if let Some(star) = l.star
            && self.stars & l.star_bit == 0
        {
            let d = self.pos - star;
            if d.dot(d) < 0.9 * 0.9 {
                self.stars |= l.star_bit;
                audio.play_sound(SND_WIN);
                self.state = State::StarGet(STAR_GET_SECS);
                self.prev = *input;
                return;
            }
        }

        // Portas destrancadas teleportam.
        for p in l.portals {
            if self.star_count() >= p.need as u32 {
                let trigger = Aabb::from_center_half(
                    p.pos + Vec3::new(0.0, 1.0, 0.0),
                    Vec3::new(0.7, 1.0, 0.6),
                );
                if feet.overlaps(&trigger) {
                    audio.play_sound(SND_JUMP);
                    self.goto_level(p.dest);
                    self.prev = *input;
                    return;
                }
            }
        }

        // B volta ao hub; Start reinicia o nível.
        if self.level != 0 && input.just_pressed(&self.prev, Button::B) {
            self.goto_level(0);
            self.prev = *input;
            return;
        }
        if input.just_pressed(&self.prev, Button::Start) {
            self.goto_level(self.level);
            self.prev = *input;
            return;
        }

        // Inimigos: patrulha + stomp/dano.
        for (i, def) in l.enemies.iter().take(MAX_ENEMIES).enumerate() {
            let e = &mut self.enemies[i];
            if !e.alive {
                continue;
            }
            let path = def.b - def.a;
            let path_len = trino_core::math3d::sqrt(path.dot(path));
            let path_len = if path_len < 0.001 { 0.001 } else { path_len };
            e.t += def.speed / path_len * dt * 0.5;
            let phase = tri_wave(e.t);
            let prev_pos = e.pos;
            e.pos = lerp3(def.a, def.b, phase);
            let delta = e.pos - prev_pos;
            if delta.dot(delta) > 1e-8 {
                e.yaw = atan2(delta.x, delta.z);
            }
            let ebox =
                Aabb::from_center_half(e.pos + Vec3::new(0.0, ENEMY_HALF.y, 0.0), ENEMY_HALF);
            if feet.overlaps(&ebox) {
                // Stomp: caindo com os pés acima da base do inimigo (margem
                // generosa — a queda pode atravessar meio corpo num frame).
                let player_feet_y = self.pos.y - PLAYER_HALF.y;
                if self.vel.y < -1.0 && player_feet_y > e.pos.y + 0.1 {
                    e.alive = false;
                    self.vel.y = STOMP_BOUNCE;
                    audio.play_sound(SND_COIN);
                } else {
                    self.respawn(audio);
                    self.prev = *input;
                    return;
                }
            }
        }

        self.prev = *input;
    }

    fn render(&mut self, renderer: &mut dyn Renderer) {
        let l = self.cur();
        renderer.begin_frame(l.sky);

        // Sólidos do frame: câmera (raycast) e sombra usam a mesma lista.
        let mut solids = [EMPTY_SOLID; MAX_SOLIDS];
        let n_solids = self.build_solids(&mut solids);
        let mut boxes = [Aabb::new(Vec3::ZERO, Vec3::ZERO); MAX_SOLIDS];
        for i in 0..n_solids {
            boxes[i] = solids[i].aabb;
        }

        // Câmera orbital com colisão: encurta a distância até o primeiro
        // sólido entre o alvo e a posição desejada (não atravessa paredes).
        let target = self.pos + Vec3::new(0.0, 0.9, 0.0);
        let back = Vec3::new(
            sin(self.cam_yaw) * CAM_DIST,
            CAM_HEIGHT,
            cos(self.cam_yaw) * CAM_DIST,
        );
        let want = trino_core::math3d::sqrt(back.dot(back));
        let dir = back * (1.0 / want);
        let hit = physics::raycast_aabbs(target, dir, want, &boxes[..n_solids]);
        let cam_dist = (hit - 0.4).clamp(CAM_MIN_DIST, want);
        let camera = Camera3 {
            eye: target + dir * cam_dist,
            target,
            ..Camera3::default()
        };
        renderer.set_camera(&camera);
        let eye = camera.eye;
        // A engine ordena os triângulos do batch entre meshes; aqui só
        // cortamos o que está longe demais.
        let visible = |p: Vec3| {
            let d = p - eye;
            d.dot(d) < CULL_FAR * CULL_FAR
        };
        let plain = ModelParams::default();

        let block_model = |k: BlockKind| match k {
            BlockKind::Grass => M_GRASS,
            BlockKind::Stone => M_STONE,
            BlockKind::Brick => M_BRICK,
            BlockKind::Castle => M_CASTLE,
            BlockKind::Roof => M_ROOF,
            BlockKind::Lava => M_LAVA,
        };

        for b in l.blocks {
            let center = b.min + b.size * 0.5;
            if !visible(center) {
                continue;
            }
            // Lava pulsa via tint por draw (ModelParams).
            let params = if b.kind == BlockKind::Lava {
                let glow = 190 + (65.0 * tri_wave(self.level_time * 0.6)) as u8;
                ModelParams {
                    tint: Color::rgb(255, glow, glow),
                }
            } else {
                plain
            };
            renderer.draw_model(
                block_model(b.kind),
                &Transform3 {
                    position: center,
                    rotation: Vec3::ZERO,
                    scale: b.size,
                },
                Material::VertexLit,
                &params,
            );
        }
        for m in l.movers {
            let min = mover_min(m, self.level_time);
            let center = min + m.size * 0.5;
            if visible(center) {
                renderer.draw_model(
                    block_model(m.kind),
                    &Transform3 {
                        position: center,
                        rotation: Vec3::ZERO,
                        scale: m.size,
                    },
                    Material::VertexLit,
                    &plain,
                );
            }
        }

        // Portas: a dourada do castelo é a doorway REAL (KayKit, CC0, baked
        // de glTF texturizado); as de fase são a porta neutra tingida.
        for p in l.portals {
            if !visible(p.pos) {
                continue;
            }
            let locked = self.star_count() < p.need as u32;
            let (model, scale, open_tint) = match p.color {
                DoorColor::Green => (M_DOOR_FRAME, Vec3::ONE, Color::rgb(95, 205, 95)),
                DoorColor::Red => (M_DOOR_FRAME, Vec3::ONE, Color::rgb(230, 95, 80)),
                DoorColor::Blue => (M_DOOR_FRAME, Vec3::ONE, Color::rgb(95, 130, 235)),
                DoorColor::Gold => (
                    M_DOOR_KAYKIT,
                    // Doorway de 4x4 do pack escalada para a fachada.
                    Vec3::new(0.62, 0.62, 0.62),
                    Color::rgb(255, 225, 140),
                ),
            };
            let params = if locked {
                ModelParams {
                    tint: Color::rgb(115, 115, 120),
                }
            } else {
                ModelParams { tint: open_tint }
            };
            renderer.draw_model(
                model,
                &Transform3 {
                    position: p.pos,
                    rotation: Vec3::new(0.0, p.yaw, 0.0),
                    scale,
                },
                Material::VertexLit,
                &params,
            );
        }

        // Moedas girando + bob.
        let spin = self.level_time * 3.0;
        for (i, c) in l.coins.iter().enumerate() {
            if self.coins_taken[self.level] & (1 << i) == 0 && visible(*c) {
                renderer.draw_model(
                    M_COIN,
                    &Transform3 {
                        position: *c + Vec3::new(0.0, tri_wave(self.level_time * 0.5) * 0.2, 0.0),
                        rotation: Vec3::new(0.0, spin, 0.0),
                        scale: Vec3::ONE,
                    },
                    Material::VertexLit,
                    &plain,
                );
            }
        }

        // Estrela do nível (se ainda não coletada).
        if let Some(star) = l.star
            && self.stars & l.star_bit == 0
        {
            let pulse = 1.0 + tri_wave(self.level_time * 0.8) * 0.15;
            renderer.draw_model(
                M_STAR,
                &Transform3 {
                    position: star,
                    rotation: Vec3::new(0.0, self.level_time * 1.5, 0.0),
                    scale: Vec3::new(pulse, pulse, pulse),
                },
                Material::VertexLit,
                &plain,
            );
        }

        // Sala do trono: estrela gigante com brilho pulsante (tint).
        if self.level == 4 {
            let warm = 200 + (55.0 * tri_wave(self.level_time * 0.5)) as u8;
            renderer.draw_model(
                M_STAR,
                &Transform3 {
                    position: Vec3::new(0.0, 2.5 + tri_wave(self.level_time * 0.3) * 0.5, 0.0),
                    rotation: Vec3::new(0.0, self.level_time, 0.0),
                    scale: Vec3::new(4.0, 4.0, 4.0),
                },
                Material::VertexLit,
                &ModelParams {
                    tint: Color::rgb(255, 255, warm),
                },
            );
        }

        // Javalis de patrulha.
        for i in 0..l.enemies.len().min(MAX_ENEMIES) {
            let e = self.enemies[i];
            if e.alive && visible(e.pos) {
                renderer.draw_model(
                    M_BOAR,
                    &Transform3 {
                        position: e.pos,
                        rotation: Vec3::new(0.0, e.yaw, 0.0),
                        scale: Vec3::ONE,
                    },
                    Material::VertexLit,
                    &plain,
                );
            }
        }

        // Sombra "blob" no chão sob o player (leitura de aterrissagem).
        let feet_y = self.pos.y - PLAYER_HALF.y;
        let mut shadow_top = f32::MIN;
        for s in &solids[..n_solids] {
            let a = &s.aabb;
            if self.pos.x > a.min.x - 0.05
                && self.pos.x < a.max.x + 0.05
                && self.pos.z > a.min.z - 0.05
                && self.pos.z < a.max.z + 0.05
                && a.max.y <= feet_y + 0.05
                && a.max.y > shadow_top
            {
                shadow_top = a.max.y;
            }
        }
        if shadow_top > f32::MIN && feet_y - shadow_top < 10.0 {
            renderer.draw_model(
                M_SHADOW,
                &Transform3 {
                    position: Vec3::new(self.pos.x, shadow_top + 0.03, self.pos.z),
                    rotation: Vec3::ZERO,
                    scale: Vec3::new(0.8, 1.0, 0.8),
                },
                Material::VertexLit,
                &plain,
            );
        }

        // Player articulado: tronco + braços/pernas com pivô no ombro e no
        // quadril — ciclo de caminhada real (o z-buffer resolve as juntas).
        {
            let feet = self.pos - Vec3::new(0.0, PLAYER_HALF.y, 0.0);
            let speed = trino_core::math3d::sqrt(self.vel.x * self.vel.x + self.vel.z * self.vel.z);
            let stride = (speed / MOVE_SPEED).min(1.0);
            let swing = sin(self.walk_phase) * 0.85 * stride;
            // No ar: braços para cima, pernas encolhidas.
            let airborne = !self.on_ground;
            let (arm_l, arm_r, leg_l, leg_r) = if airborne {
                (-2.4, -2.4, 0.55, 0.9)
            } else {
                (swing, -swing, -swing, swing)
            };
            let (sy, cy) = (sin(self.yaw), cos(self.yaw));
            // Offset local (x, y, z) girado pelo yaw do corpo.
            let joint = |ox: f32, oy: f32, oz: f32| {
                feet + Vec3::new(ox * cy + oz * sy, oy, -ox * sy + oz * cy)
            };
            let part = |renderer: &mut dyn Renderer, model: ModelId, at: Vec3, pitch: f32| {
                renderer.draw_model(
                    model,
                    &Transform3 {
                        position: at,
                        rotation: Vec3::new(pitch, self.yaw, 0.0),
                        scale: Vec3::ONE,
                    },
                    Material::VertexLit,
                    &plain,
                );
            };
            part(renderer, M_TORSO, feet, 0.0);
            part(renderer, M_ARM, joint(-0.32, 0.66, 0.0), arm_l);
            part(renderer, M_ARM, joint(0.32, 0.66, 0.0), arm_r);
            part(renderer, M_LEG, joint(-0.13, 0.36, 0.0), leg_l);
            part(renderer, M_LEG, joint(0.13, 0.36, 0.0), leg_r);
        }

        // Estrela comemorativa sobre o player durante o StarGet.
        if let State::StarGet(t) = self.state {
            renderer.draw_model(
                M_STAR,
                &Transform3 {
                    position: self.pos + Vec3::new(0.0, 1.3, 0.0),
                    rotation: Vec3::new(0.0, t * 8.0, 0.0),
                    scale: Vec3::new(0.8, 0.8, 0.8),
                },
                Material::VertexLit,
                &plain,
            );
        }

        // ---- HUD (sprites por cima do 3D) ----
        let digits = |renderer: &mut dyn Renderer, value: u32, x: f32, y: f32| {
            let v = value.min(99);
            let scale = SpriteParams {
                scale: Vec2::new(2.0, 2.0),
                ..Default::default()
            };
            renderer.draw_sprite(S_DIGITS[(v / 10) as usize], Vec2::new(x, y), &scale);
            renderer.draw_sprite(S_DIGITS[(v % 10) as usize], Vec2::new(x + 18.0, y), &scale);
        };
        renderer.draw_sprite(S_COIN, Vec2::new(6.0, 6.0), &SpriteParams::default());
        digits(renderer, self.total_coins(), 26.0, 6.0);
        renderer.draw_sprite(
            S_STAR,
            Vec2::new(self.screen.x - 60.0, 6.0),
            &SpriteParams {
                scale: Vec2::new(2.0, 2.0),
                ..Default::default()
            },
        );
        digits(renderer, self.star_count(), self.screen.x - 40.0, 6.0);

        // Star get: estrela grande no centro da tela.
        if matches!(self.state, State::StarGet(_)) {
            renderer.draw_sprite(
                S_STAR,
                self.screen * 0.5 - Vec2::new(32.0, 48.0),
                &SpriteParams {
                    scale: Vec2::new(8.0, 8.0),
                    ..Default::default()
                },
            );
        }

        renderer.end_frame();
    }
}

// Hot-reload exports (trino_game_update / trino_game_render / version).
trino_game_api::export_game!(Castle64Game);

// ------------------------------------------------------------- tests ----

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

    fn game() -> Castle64Game {
        Castle64Game::new(Vec2::new(320.0, 240.0))
    }

    fn held(button: Button) -> InputState {
        let mut s = InputState::default();
        s.set(button, true);
        s
    }

    const DT: f32 = 1.0 / 60.0;

    fn settle(g: &mut Castle64Game, a: &mut CountAudio) {
        for _ in 0..60 {
            g.update(&InputState::default(), a, DT);
        }
    }

    #[test]
    fn spawns_and_settles_on_the_ground() {
        let mut g = game();
        let mut a = audio();
        settle(&mut g, &mut a);
        assert!(g.on_ground);
        // Chão do hub tem topo em y=0; centro do player a meia altura.
        assert!((g.pos.y - PLAYER_HALF.y).abs() < 1e-3, "y = {}", g.pos.y);
        assert_eq!(g.vel.y, 0.0);
    }

    #[test]
    fn dpad_up_walks_toward_the_castle() {
        let mut g = game();
        let mut a = audio();
        settle(&mut g, &mut a);
        let z0 = g.pos.z;
        for _ in 0..60 {
            g.update(&held(Button::DpadUp), &mut a, DT);
        }
        // Câmera inicial ao sul: "para frente" = +Z (rumo ao castelo).
        assert!(g.pos.z > z0 + 3.0, "andou só {}", g.pos.z - z0);
    }

    #[test]
    fn jumps_and_lands_back() {
        let mut g = game();
        let mut a = audio();
        settle(&mut g, &mut a);
        let ground_y = g.pos.y;
        // Segura A na subida (pulo variável: soltar cedo corta a altura).
        for _ in 0..14 {
            g.update(&held(Button::A), &mut a, DT);
        }
        assert!(g.pos.y > ground_y);
        let mut peak = ground_y;
        let mut landed = false;
        for _ in 0..180 {
            g.update(&InputState::default(), &mut a, DT);
            peak = peak.max(g.pos.y);
            if g.on_ground {
                landed = true;
                break;
            }
        }
        assert!(peak > ground_y + 1.0, "subiu só até {peak}");
        assert!(landed);
        assert!((g.pos.y - ground_y).abs() < 1e-3);
    }

    #[test]
    fn collects_a_coin_on_the_path() {
        let mut g = game();
        let mut a = audio();
        settle(&mut g, &mut a);
        // Moeda em (0, 0.6, -4): direto no caminho do spawn ao castelo.
        for _ in 0..240 {
            g.update(&held(Button::DpadUp), &mut a, DT);
            if g.total_coins() > 0 {
                break;
            }
        }
        assert_eq!(g.total_coins(), 1);
    }

    #[test]
    fn green_door_teleports_to_green_hills() {
        let mut g = game();
        let mut a = audio();
        settle(&mut g, &mut a);
        // Porta verde em (-12.5, 0, 0): mirar do spawn direto na porta.
        for _ in 0..900 {
            let mut s = InputState::default();
            s.stick = Vec2::new(-0.78, 0.62);
            g.update(&s, &mut a, DT);
            if g.level == 1 {
                break;
            }
        }
        assert_eq!(g.level, 1, "não entrou na porta verde (pos {:?})", g.pos);
    }

    #[test]
    fn locked_door_blocks_until_stars() {
        let mut g = game();
        let mut a = audio();
        settle(&mut g, &mut a);
        // Porta vermelha precisa de 1 estrela: mirar nela não entra.
        for _ in 0..900 {
            let mut s = InputState::default();
            s.stick = Vec2::new(0.78, 0.62);
            g.update(&s, &mut a, DT);
        }
        assert_eq!(g.level, 0, "porta trancada deixou passar");
        // Com uma estrela, abre.
        g.goto_level(0);
        g.stars = 1 << 1;
        let mut a = audio();
        settle(&mut g, &mut a);
        for _ in 0..900 {
            let mut s = InputState::default();
            s.stick = Vec2::new(0.78, 0.62);
            g.update(&s, &mut a, DT);
            if g.level == 2 {
                break;
            }
        }
        assert_eq!(g.level, 2, "porta destrancada não abriu (pos {:?})", g.pos);
    }

    #[test]
    fn lava_respawns_the_player() {
        let mut g = game();
        let mut a = audio();
        g.goto_level(2);
        settle(&mut g, &mut a);
        // Anda para trás da plataforma inicial até cair na lava.
        let mut respawned = false;
        for i in 0..900 {
            let mut s = InputState::default();
            s.stick = Vec2::new(0.0, -1.0);
            g.update(&s, &mut a, DT);
            // goto_level zera level_time: detecta o respawn após alguns frames.
            if i > 30 && g.level_time < 2.0 * DT {
                respawned = true;
                break;
            }
        }
        assert!(respawned, "nunca respawnou na lava (pos {:?})", g.pos);
        assert_eq!(g.level, 2);
    }

    #[test]
    fn stomping_kills_the_enemy_and_bounces() {
        let mut g = game();
        let mut a = audio();
        settle(&mut g, &mut a);
        // Queda livre sobre o inimigo, rastreando só o x/z da patrulha.
        g.pos = g.enemies[0].pos + Vec3::new(0.0, 1.5, 0.0);
        g.vel = Vec3::ZERO;
        let mut stomped = false;
        for _ in 0..60 {
            g.pos.x = g.enemies[0].pos.x;
            g.pos.z = g.enemies[0].pos.z;
            g.update(&InputState::default(), &mut a, DT);
            if !g.enemies[0].alive {
                stomped = true;
                break;
            }
        }
        assert!(stomped, "stomp não matou o inimigo");
        assert!(g.vel.y > 0.0, "sem bounce após o stomp");
    }

    #[test]
    fn touching_the_enemy_from_the_side_respawns() {
        let mut g = game();
        let mut a = audio();
        settle(&mut g, &mut a);
        let spawn_z = g.cur().spawn.z;
        // Encosta de lado (mesma altura do inimigo).
        g.pos = g.enemies[0].pos + Vec3::new(0.35, PLAYER_HALF.y - 0.05, 0.0);
        g.vel = Vec3::ZERO;
        for _ in 0..30 {
            g.update(&InputState::default(), &mut a, DT);
            if (g.pos.z - spawn_z).abs() < 0.2 {
                return; // respawnou no spawn do hub
            }
            // Continua colado no inimigo enquanto não respawna.
            g.pos.x = g.enemies[0].pos.x + 0.3;
            g.pos.z = g.enemies[0].pos.z;
            g.pos.y = g.enemies[0].pos.y + PLAYER_HALF.y - 0.05;
        }
        panic!("dano lateral não respawnou o player");
    }

    #[test]
    fn star_returns_to_hub_and_counts() {
        let mut g = game();
        let mut a = audio();
        g.goto_level(1);
        settle(&mut g, &mut a);
        // Teleporta ao lado da estrela de green-hills e encosta.
        let star = LEVELS[1].star.unwrap();
        g.pos = star + Vec3::new(0.0, 0.2, 0.0);
        g.vel = Vec3::ZERO;
        g.update(&InputState::default(), &mut a, DT);
        assert_eq!(g.star_count(), 1);
        assert!(matches!(g.state, State::StarGet(_)));
        // Espera o retorno festivo ao hub.
        for _ in 0..240 {
            g.update(&InputState::default(), &mut a, DT);
        }
        assert_eq!(g.level, 0);
        assert!(matches!(g.state, State::Playing));
    }

    /// E2E da progressão: coleta as 4 estrelas (hub + 3 fases), volta ao hub
    /// a cada StarGet e confirma que a porta dourada do castelo abre o trono.
    #[test]
    fn full_progression_opens_the_gold_door() {
        let mut g = game();
        let mut a = audio();
        for level in [0usize, 1, 2, 3] {
            g.goto_level(level);
            let star = LEVELS[level].star.unwrap();
            g.pos = star;
            g.vel = Vec3::ZERO;
            g.update(&InputState::default(), &mut a, DT);
            assert!(
                matches!(g.state, State::StarGet(_)),
                "estrela do nível {level} não coletou"
            );
            for _ in 0..240 {
                g.update(&InputState::default(), &mut a, DT);
            }
            assert_eq!(g.level, 0, "não voltou ao hub após a estrela {level}");
        }
        assert_eq!(g.star_count(), 4);
        // Porta dourada em (0, 0, 10.6): andar reto do spawn ao castelo.
        for _ in 0..1200 {
            let mut s = InputState::default();
            s.stick = Vec2::new(0.0, 1.0);
            g.update(&s, &mut a, DT);
            if g.level == 4 {
                break;
            }
        }
        assert_eq!(g.level, 4, "porta dourada não abriu (pos {:?})", g.pos);
    }

    #[test]
    fn music_starts_once() {
        let mut g = game();
        let mut a = audio();
        settle(&mut g, &mut a);
        assert_eq!(a.music, 1);
    }

    #[test]
    fn update_is_deterministic() {
        let run = || {
            let mut g = game();
            let mut a = audio();
            for i in 0..900u32 {
                let mut s = InputState::default();
                match (i / 40) % 5 {
                    0 => s.stick = Vec2::new(0.0, 1.0),
                    1 => {
                        s.stick = Vec2::new(0.7, 0.7);
                        s.set(Button::A, true);
                    }
                    2 => s.set(Button::L, true),
                    3 => s.stick = Vec2::new(-1.0, 0.2),
                    _ => {}
                }
                g.update(&s, &mut a, DT);
            }
            (g.pos, g.vel, g.total_coins(), g.on_ground, g.level)
        };
        assert_eq!(run(), run());
    }

    /// Orçamento do N64: nenhum frame pode passar de 4000 triângulos
    /// (Caps::N64.max_tris_per_frame).
    #[test]
    fn every_level_fits_the_n64_tri_budget() {
        struct TriCounter {
            caps: trino_core::Caps,
            tris: u32,
        }
        impl Renderer for TriCounter {
            fn caps(&self) -> &trino_core::Caps {
                &self.caps
            }
            fn begin_frame(&mut self, _: Color) {}
            fn draw_sprite(&mut self, _: SpriteId, _: Vec2, _: &SpriteParams) {}
            fn draw_model(&mut self, model: ModelId, _: &Transform3, _: Material, _: &ModelParams) {
                // Contagem por modelo (worst case: tudo visível).
                self.tris += match model {
                    m if m == M_COIN => 8,
                    m if m == M_STAR => 10,
                    m if m == M_TORSO => 60,
                    m if m == M_ARM || m == M_LEG => 24,
                    m if m == M_BOAR => 120,
                    m if m == M_DOOR_FRAME => 60,
                    m if m == M_DOOR_KAYKIT => 1068,
                    _ => 12,
                };
            }
            fn end_frame(&mut self) {}
        }

        for level in 0..LEVELS.len() {
            let mut g = game();
            g.goto_level(level);
            let mut counter = TriCounter {
                caps: trino_core::Caps::N64,
                tris: 0,
            };
            g.render(&mut counter);
            assert!(
                counter.tris <= trino_core::Caps::N64.max_tris_per_frame,
                "nível {level}: {} tris (> 4000)",
                counter.tris
            );
        }
    }

    /// O bot JOGA o jogo inteiro com inputs reais: 4 estrelas navegando
    /// cada fase (pulos, movers, inimigos) e entra na porta dourada.
    #[test]
    fn bot_plays_the_whole_game() {
        let mut g = game();
        let mut a = audio();
        let mut b = bot::Bot::new(bot::FULL_RUN);
        let mut frames = 0u32;
        while !b.done() {
            let input = b.drive(&g);
            g.update(&input, &mut a, DT);
            frames += 1;
            assert!(
                b.frames_in_step() < 1800,
                "bot travou no passo {} (pos {:?}, nível {}, estrelas {})",
                b.step_index(),
                g.pos,
                g.level,
                g.star_count()
            );
            assert!(frames < 60 * 60 * 8, "playthrough longo demais");
        }
        assert_eq!(g.star_count(), 4, "não coletou as 4 estrelas");
        assert_eq!(g.level, 4, "não chegou à sala do trono");
    }

    /// Roteiro reduzido dos consoles: hub → green hills → estrela → hub.
    #[test]
    fn bot_clears_green_hills() {
        let mut g = game();
        let mut a = audio();
        let mut b = bot::Bot::new(bot::GREEN_RUN);
        let mut frames = 0u32;
        while !b.done() {
            let input = b.drive(&g);
            g.update(&input, &mut a, DT);
            frames += 1;
            assert!(
                b.frames_in_step() < 1800,
                "bot travou no passo {} (pos {:?})",
                b.step_index(),
                g.pos
            );
            assert!(frames < 60 * 120, "green hills demorou demais");
        }
        assert_eq!(g.star_count(), 1);
        assert_eq!(g.level, 0);
    }

    #[test]
    fn level_data_fits_the_engine_limits() {
        for l in LEVELS {
            assert!(l.coins.len() <= 32, "{}: moedas demais", l.name);
            assert!(
                l.enemies.len() <= MAX_ENEMIES,
                "{}: inimigos demais",
                l.name
            );
            let solids = l.blocks.len() + l.movers.len() + l.portals.len();
            assert!(solids <= MAX_SOLIDS, "{}: sólidos demais", l.name);
        }
    }
}
