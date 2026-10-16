//! showcase — the editor's demo scene (`scenes/showcase.scene.ron`) as a
//! small portable game: walk the hero around the plaza, bump into crates,
//! pillars, rocks and trees, collect the five coins and reach the goal.
//! Run it on PC with `TRINO_GAME=showcase cargo run -p trino-app-pc`; the
//! N64/3DS self-tests play it with a scripted run (`run_demo`), and
//! `xtask/tests/showcase_render.rs` proves the exported data renders
//! exactly like the editor.
//!
//! Everything the game needs comes from the scene, authored in the editor
//! (or as text) and exported to `no_std` Rust data:
//!
//! ```text
//! cargo xtask scene export scenes/showcase.scene.ron examples/showcase/src/showcase_gen.rs
//! ```
//!
//! - **Collision from the scene**: entities carry `collider` components
//!   (boxes, spheres, `Auto` = the model's box); solid ones block the hero
//!   through `trino_core::physics::move_aabb`.
//! - **Runtime entities**: the nodes become an `EntityStore`; coins are
//!   trigger colliders, despawned when touched; the hero is the entity
//!   tagged `player`, moved every frame.
//! - Game logic finds things by **name and tag** (`spawn`, `coin`,
//!   `player`, `goal`), never by hardcoded positions.
//!
//! `xtask/tests/scene_codegen.rs` fails CI when the generated file drifts
//! from the scene.
//!
//! Controls: stick/d-pad walk (camera-relative), A jump, L/R orbit; the
//! look toggles: X camera, Y light, B fog, Select texture filter, Start
//! animation (PC: C, V, X, Right Shift, Enter).
//!
//! Two screens (3DS, and the PC's 3DS profile, `TRINO_SIM=3ds`): the top
//! shows the scene, the bottom a touch panel ([`panel`]) with live
//! renderer metrics (frame time, `Renderer::stats`) and buttons for the
//! same toggles. Single-screen targets skip it (`Renderer::on_screen`
//! says no) and draw exactly what they drew before.

#![cfg_attr(any(target_os = "none", target_os = "horizon"), no_std)]

use core::fmt::Write as _;
use trino_core::math3d::{atan2, cos, sin, sqrt};
use trino_core::physics::{Aabb, move_aabb};
use trino_core::scene::{BakedScene, SceneNode};
use trino_core::{
    Ambience, Audio, BlobId, Button, Camera3, Color, Emitter, EntityId, EntityStore, FontId, Game,
    InputState, Light, Listener, Mat34, Material, MusicId, ParticleSystem, Platform, Renderer,
    Screen, SoundId, SpriteId, TextAlign, TextBuf, TextParams, Vec2, Vec3, spatialize,
};

pub mod panel;

#[allow(clippy::all)]
#[rustfmt::skip]
mod scene {
    include!("showcase_gen.rs");
}

pub use scene::NODES;

/// The exported scene's camera, light and clear color.
pub mod scene_consts {
    pub use super::scene::{CAMERA, CLEAR, LIGHT};
}

/// Live entities: the whole exported scene fits with room to spare.
pub const CAPACITY: usize = 96;
const FONT: FontId = FontId::from_path("fonts/trino5x7");
/// The same scene baked as data (`[scenes.showcase]` in the manifest).
pub const BAKED: BlobId = BlobId::from_path("scenes/showcase");
const HERO_HALF: Vec3 = Vec3::new(0.35, 0.6, 0.35);
const GRAVITY: f32 = 22.0;
const JUMP: f32 = 8.0;
const SPEED: f32 = 4.5;
const COIN_SOUND: SoundId = SoundId::from_path("sounds/coin");
/// All coins collected: a fanfare over the music (stinger).
const FANFARE: MusicId = MusicId::from_path("music/fanfare");

/// Coin pickup sparkles: a short, bright burst that rises and fades.
const SPARKS: Emitter = Emitter {
    sprite: SpriteId::from_path("sprites/fx_spark"),
    rate: 0.0,
    life: (0.35, 0.6),
    speed: (1.5, 3.0),
    direction: Vec3::new(0.0, 1.0, 0.0),
    spread: 1.1,
    gravity: Vec3::new(0.0, -6.0, 0.0),
    size: (0.35, 0.05),
    color: (Color::rgb(255, 240, 120), Color::rgba(255, 160, 40, 255)),
    ..Emitter::DEFAULT
};

/// Physics substep: small steps keep fast falls from tunneling.
const STEP: f32 = 1.0 / 120.0;

pub struct ShowcaseGame {
    pub world: EntityStore<CAPACITY>,
    hero: Option<EntityId>,
    /// Hero center (the model's origin sits at its feet).
    pub pos: Vec3,
    vel: Vec3,
    pub on_ground: bool,
    pub coins: u32,
    pub total_coins: u32,
    pub reached_goal: bool,
    /// Hero heading (radians about Y); kept while standing still.
    facing: f32,
    time: f32,
    yaw: f32,
    pitch: f32,
    dist: f32,
    prev: InputState,
    screen: Vec2,
    spawn: Vec3,
    camera_fov: f32,
    light: Light,
    clear: Color,
    /// Coin pickup particles (billboards).
    sparks: ParticleSystem<32>,
    /// Scene sound sources (the pond) playing near the camera.
    ambience: Ambience<4>,
    /// Coins collected this frame: count and where (positional sounds).
    picked: (u32, Vec3),
    /// The look the panel (or the pad) switched: camera, light, fog,
    /// texture filter, animation clock.
    pub look: panel::Look,
    /// Smoothed frame time (seconds, from `update`'s `dt`).
    frame_dt: f32,
    /// The scene's own camera (the `SCENE` camera preset) and its yaw.
    scene_camera: Camera3,
    scene_yaw: f32,
}

impl ShowcaseGame {
    pub fn new() -> Self {
        Self::with_screen(Vec2::new(320.0, 240.0))
    }

    /// The level from the exported code (`showcase_gen.rs`).
    pub fn with_screen(screen: Vec2) -> Self {
        Self::from_scene(
            NODES.iter().copied(),
            scene::CAMERA,
            scene::LIGHT,
            scene::CLEAR,
            screen,
        )
    }

    /// The level from the baked `TSCN` blob — the same scene loaded at
    /// runtime (data-driven levels; no rebuild to change the layout).
    pub fn from_baked(baked: &BakedScene, screen: Vec2) -> Self {
        Self::from_scene(
            baked.nodes(),
            baked.camera(),
            baked.light(),
            baked.clear(),
            screen,
        )
    }

    pub fn from_scene(
        nodes: impl IntoIterator<Item = SceneNode>,
        camera: Camera3,
        light: Light,
        clear: Color,
        screen: Vec2,
    ) -> Self {
        // Start from the scene's own camera, expressed as an orbit.
        let d = camera.eye - camera.target;
        let dist = sqrt(d.dot(d).max(1e-6));
        // Build the store inside the game value (no temporary copy): the
        // store is the bulk of the state, and console stacks are small (the
        // N64 main stack is 64 KB).
        let mut game = ShowcaseGame {
            world: EntityStore::new(),
            hero: None,
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            on_ground: false,
            coins: 0,
            total_coins: 0,
            reached_goal: false,
            facing: 0.0,
            time: 0.0,
            yaw: atan2(d.x, -d.z),
            pitch: (d.y / dist).clamp(0.15, 1.2),
            dist: dist * 0.7,
            prev: InputState::default(),
            screen,
            spawn: Vec3::ZERO,
            camera_fov: camera.fov_y,
            light,
            clear,
            sparks: ParticleSystem::new(SPARKS, 0x5EED),
            ambience: Ambience::new(),
            picked: (0, Vec3::ZERO),
            look: panel::Look::DEFAULT,
            frame_dt: 1.0 / 60.0,
            scene_camera: camera,
            scene_yaw: atan2(d.x, -d.z),
        };
        game.world.spawn_iter(nodes);
        game.hero = game.world.with_tag("player").next();
        game.spawn = game
            .world
            .find("spawn")
            .and_then(|id| game.world.get(id))
            .map_or(Vec3::ZERO, |e| e.position());
        game.pos = game.spawn + Vec3::new(0.0, HERO_HALF.y, 0.0);
        game.total_coins = game.world.with_tag("coin").count() as u32;
        game
    }

    /// The camera of the current preset ([`panel::Look::camera`]): the
    /// orbit behind the hero, a steep one, or the scene's own camera.
    pub fn camera(&self) -> Camera3 {
        let (pitch, dist) = match self.look.camera {
            panel::CAMERA_SCENE => return self.scene_camera,
            panel::CAMERA_HIGH => (1.1, self.dist * 1.5),
            _ => (self.pitch, self.dist),
        };
        let cp = cos(pitch);
        let target = self.pos + Vec3::new(0.0, 0.6, 0.0);
        Camera3 {
            eye: target + Vec3::new(cp * sin(self.yaw), sin(pitch), -cp * cos(self.yaw)) * dist,
            target,
            fov_y: self.camera_fov,
        }
    }

    /// The yaw walking is relative to: the orbit's, or the scene camera's.
    fn walk_yaw(&self) -> f32 {
        if self.look.camera == panel::CAMERA_SCENE {
            self.scene_yaw
        } else {
            self.yaw
        }
    }

    /// Smoothed frame time in seconds (the panel's readout).
    pub fn frame_dt(&self) -> f32 {
        self.frame_dt
    }

    /// Last frame's input (the panel lights a held button).
    pub fn prev_input(&self) -> &InputState {
        &self.prev
    }

    /// Coins still in the world (tag `coin`).
    pub fn coins_left(&self) -> usize {
        self.world.with_tag("coin").count()
    }

    fn physics(&mut self, wish: Vec3, jump: bool, dt: f32) {
        let mut solids = [Aabb::EMPTY; CAPACITY];
        let n = self.world.solids(&mut solids, None);
        self.vel.x = wish.x * SPEED;
        self.vel.z = wish.z * SPEED;
        if jump && self.on_ground {
            self.vel.y = JUMP;
        }
        let mut left = dt;
        while left > 0.0 {
            let h = left.min(STEP);
            left -= h;
            self.vel.y -= GRAVITY * h;
            let out = move_aabb(self.pos, HERO_HALF, self.vel, h, &solids[..n]);
            self.pos = out.pos;
            self.vel = out.vel;
            self.on_ground = out.on_ground;
        }
        // Fell off the world: back to the spawn.
        if self.pos.y < -10.0 {
            self.pos = self.spawn + Vec3::new(0.0, HERO_HALF.y, 0.0);
            self.vel = Vec3::ZERO;
        }
    }

    fn pickups(&mut self) {
        let me = Aabb::from_center_half(self.pos, HERO_HALF);
        let mut touched = [None; 8];
        for (slot, id) in touched.iter_mut().zip(self.world.triggers_overlapping(&me)) {
            *slot = Some(id);
        }
        for id in touched.into_iter().flatten() {
            let Some(e) = self.world.get(id) else {
                continue;
            };
            if e.has_tag("coin") {
                self.picked = (self.picked.0 + 1, e.position());
                self.sparks.origin = e.position();
                self.sparks.burst(12);
                self.world.despawn(id);
                self.coins += 1;
            } else if e.has_tag("goal") && self.coins == self.total_coins {
                self.reached_goal = true;
            }
        }
    }
}

impl Default for ShowcaseGame {
    fn default() -> Self {
        Self::new()
    }
}

impl Game for ShowcaseGame {
    fn update(&mut self, input: &InputState, audio: &mut dyn Audio, dt: f32) {
        // Look toggles: a pad button, or a tap on the bottom-screen panel
        // (fresh presses only: held does not repeat).
        let tap = panel::tapped(input, &self.prev);
        for t in panel::TOGGLES {
            if input.just_pressed(&self.prev, t.button()) || tap == Some(t) {
                self.look.apply(t);
            }
        }
        self.frame_dt += (dt - self.frame_dt) * 0.125;
        if !self.look.paused {
            self.time += dt;
        }
        let mut dx = input.stick.x;
        let mut dy = input.stick.y;
        if input.is_down(Button::DpadLeft) {
            dx -= 1.0;
        }
        if input.is_down(Button::DpadRight) {
            dx += 1.0;
        }
        if input.is_down(Button::DpadUp) {
            dy += 1.0;
        }
        if input.is_down(Button::DpadDown) {
            dy -= 1.0;
        }
        if input.is_down(Button::L) {
            self.yaw -= 1.8 * dt;
        }
        if input.is_down(Button::R) {
            self.yaw += 1.8 * dt;
        }
        // Camera-relative walk: "up" moves away from the camera.
        let yaw = self.walk_yaw();
        let fwd = Vec3::new(-sin(yaw), 0.0, cos(yaw));
        let right = Vec3::new(cos(yaw), 0.0, sin(yaw));
        let mut wish = fwd * dy + right * dx;
        let len2 = wish.dot(wish);
        if len2 > 1.0 {
            wish = wish * (1.0 / sqrt(len2));
        }
        if !self.look.paused {
            self.sparks.update(dt);
        }
        let jump = input.just_pressed(&self.prev, Button::A);
        self.prev = *input;
        if !self.reached_goal {
            self.physics(wish, jump, dt);
            let before = self.coins;
            self.pickups();
            // Positional audio: the pickup is heard from where it happened,
            // the last coin plays the fanfare over the music.
            let (n, at) = core::mem::replace(&mut self.picked, (0, Vec3::ZERO));
            if n > 0 {
                let listener = Listener::from_camera(&self.camera());
                let params = spatialize(&listener, at, 25.0, 1.0);
                for _ in 0..n {
                    audio.play_sound_with(COIN_SOUND, &params);
                }
                if before < self.total_coins && self.coins == self.total_coins {
                    audio.play_stinger(FANFARE);
                }
            }
        }
        // Scene sound sources (looping voices steered by distance and side).
        let listener = Listener::from_camera(&self.camera());
        self.ambience
            .update(audio, &listener, self.world.sound_sources());
        if let Some(hero) = self.hero.and_then(|h| self.world.get_mut(h)) {
            let feet = self.pos - Vec3::new(0.0, HERO_HALF.y, 0.0);
            // Face the walk direction.
            if len2 > 0.01 {
                self.facing = atan2(wish.x, wish.z);
            }
            hero.matrix = Mat34::from_rotation_scale_translation(
                Vec3::new(0.0, self.facing, 0.0),
                Vec3::ONE,
                feet,
            );
        }
    }

    fn render(&mut self, r: &mut dyn Renderer) {
        let camera = self.camera();
        r.begin_frame(self.clear);
        r.set_camera(&camera);
        // The scene's light as authored unless the panel switched it; the
        // filter every frame (it persists in the renderer: `Default` hands
        // the next game the target's own default).
        r.set_light(&self.look.light(&self.light));
        r.set_texture_filter(self.look.filter);
        // Everything the editor placed (animated clips included), coins
        // spinning and bobbing on top — game logic driven by tags.
        for (_, e) in self.world.iter() {
            // LOD from the scene: far trees swap to cheaper fake objects.
            let Some(model) = e.model_from(camera.eye).filter(|_| e.visible) else {
                continue;
            };
            let mut params = e.params;
            if let Some((clip, speed)) = e.anim {
                params.anim = Some(trino_core::AnimRef::new(clip, self.time * speed));
            }
            let m = if e.has_tag("coin") {
                let p = e.position();
                let spin = Mat34::from_rotation_scale_translation(
                    Vec3::new(0.0, self.time * 2.0, 0.0),
                    Vec3::ONE,
                    p + Vec3::new(0.0, sin(self.time * 3.0 + p.x) * 0.12, 0.0),
                );
                let back = Mat34::from_rotation_scale_translation(Vec3::ZERO, Vec3::ONE, -p);
                spin.mul(&back).mul(&e.matrix)
            } else {
                e.matrix
            };
            r.draw_model_matrix(model, &m, Material::VertexLit, &params);
        }
        self.sparks.draw(r);
        let mut hud = TextBuf::<24>::new();
        let _ = write!(hud, "COINS {}/{}", self.coins, self.total_coins);
        r.draw_text(
            FONT,
            Vec2::new(8.0, 8.0),
            hud.as_str(),
            &TextParams::DEFAULT,
        );
        if self.reached_goal {
            r.draw_text(
                FONT,
                Vec2::new(self.screen.x * 0.5, self.screen.y * 0.5 - 10.0),
                "YOU WIN!",
                &TextParams {
                    scale: 2.0,
                    tint: Color::rgb(255, 230, 90),
                    align: TextAlign::Center,
                },
            );
        } else if self.coins == self.total_coins {
            r.draw_text(
                FONT,
                Vec2::new(self.screen.x * 0.5, 8.0),
                "GO TO THE GOAL",
                &TextParams {
                    align: TextAlign::Center,
                    ..TextParams::DEFAULT
                },
            );
        }
        // ---- Bottom screen (3DS touch screen / PC 3DS profile) ----
        // Single-screen targets refuse it: nothing more is drawn there.
        if r.on_screen(Screen::Bottom) {
            panel::draw(self, r);
        }
        r.end_frame();
    }
}

/// A scripted playthrough for console self-tests and PC tests: walks the
/// hero from the spawn through every coin to the goal, driving the real
/// `update` with steering input. Returns the frames it took, or `Err`
/// with the step that never completed.
pub fn run_demo(
    game: &mut ShowcaseGame,
    mut frame: impl FnMut(&mut ShowcaseGame, &InputState),
) -> Result<u32, &'static str> {
    let mut frames = 0u32;
    // Coins in scene order, then the goal.
    let mut targets = [Vec3::ZERO; 8];
    let mut n = 0;
    for (_, e) in game.world.iter().filter(|(_, e)| e.has_tag("coin")) {
        if n + 1 == targets.len() {
            return Err("too many coins for the demo route");
        }
        targets[n] = e.position();
        n += 1;
    }
    let Some(goal) = game.world.find("goal").and_then(|id| game.world.get(id)) else {
        return Err("no goal node");
    };
    targets[n] = goal.position();
    n += 1;
    // Settle on the ground first.
    for _ in 0..30 {
        frame(game, &InputState::default());
        frames += 1;
    }
    if !game.on_ground {
        return Err("hero did not land on the ground collider");
    }
    for (i, target) in targets[..n].iter().enumerate() {
        let is_goal = i == n - 1;
        let mut steps = 0;
        loop {
            let done = if is_goal {
                game.reached_goal
            } else {
                game.coins as usize > i
            };
            if done {
                break;
            }
            steps += 1;
            if steps > 60 * 20 {
                return Err(if is_goal {
                    "never reached the goal"
                } else {
                    "a coin was never collected"
                });
            }
            // Steer straight at the target in camera space.
            let to = *target - game.pos;
            let (s, c) = (sin(game.yaw), cos(game.yaw));
            // Inverse of the walk basis in `update`.
            let fwd = -to.x * s + to.z * c;
            let right = to.x * c + to.z * s;
            let len = sqrt(fwd * fwd + right * right).max(1e-3);
            let mut input = InputState::default();
            input.stick = Vec2::new(right / len, fwd / len);
            // Hop when stuck against something (crates, trees).
            if steps % 90 == 45 {
                input.set(Button::A, true);
            }
            frame(game, &input);
            frames += 1;
        }
    }
    Ok(frames)
}

/// The baked scene (from the platform) must match the exported code node
/// for node, and must play: build the game from the blob and run the demo.
/// Console self-tests call this — it proves `[scenes.*]` baking, blob
/// loading and `BakedScene` parsing on the real target.
pub fn check_baked(
    platform: &dyn Platform,
    mut frame: impl FnMut(&mut ShowcaseGame, &InputState),
) -> Result<(), &'static str> {
    let bytes = platform.blob(BAKED).ok_or("baked scene blob missing")?;
    let baked = BakedScene::parse(bytes).map_err(|_| "baked scene does not parse")?;
    if baked.node_count() != NODES.len() {
        return Err("baked scene node count differs from the exported code");
    }
    for (a, b) in baked.nodes().zip(NODES) {
        if a.name != b.name
            || a.model != b.model
            || a.matrix != b.matrix
            || a.collider != b.collider
            || a.anim != b.anim
            || a.params != b.params
            || !a.tags.iter().eq(b.tags.iter())
            || !a.lod.iter().eq(b.lod.iter())
            || a.sound != b.sound
            || a.cell != b.cell
            || a.vis != b.vis
        {
            return Err("baked node differs from the exported code");
        }
    }
    let mut game = ShowcaseGame::from_baked(&baked, Vec2::new(320.0, 240.0));
    run_demo(&mut game, &mut frame).map(|_| ())
}

#[cfg(feature = "hot-reload")]
trino_game_api::export_game!(ShowcaseGame);

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::scene::find;

    struct NullAudio;
    impl Audio for NullAudio {
        fn play_sound(&mut self, _: trino_core::SoundId) {}
        fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
    }

    #[test]
    fn scene_data_has_what_the_game_needs() {
        let g = ShowcaseGame::new();
        assert!(find(NODES, "spawn").is_some(), "spawn marker");
        assert_eq!(g.coins_left(), 5);
        assert_eq!(g.total_coins, 5);
        assert!(trino_core::scene::with_tag(NODES, "npc").count() >= 2);
        assert!(NODES.iter().any(|n| n.anim.is_some()));
        // Prefab parts are flattened in (tree instances -> trunk + leaves),
        // and their colliders come along.
        let trunk = NODES
            .iter()
            .find(|n| n.name.ends_with("tree-1/tree/trunk"))
            .unwrap();
        assert!(trunk.collider.is_some());
        // Coins are triggers, crates are solid.
        assert!(find(NODES, "coin-1").unwrap().collider.unwrap().trigger);
        assert!(!find(NODES, "crate-a").unwrap().collider.unwrap().trigger);
        assert!(g.world.len() == NODES.len(), "every node is a live entity");
        assert_eq!(g.world.sound_sources().count(), 1, "the pond");
    }

    /// Records what the game asks of the audio backend.
    #[derive(Default)]
    struct AudioLog {
        coin_sounds: u32,
        stingers: u32,
        voices_started: u32,
        loudest_voice: f32,
        next: u32,
    }

    impl Audio for AudioLog {
        fn play_sound(&mut self, _: trino_core::SoundId) {}
        fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
        fn play_sound_with(&mut self, s: trino_core::SoundId, p: &trino_core::SoundParams) {
            if s == COIN_SOUND {
                assert!(p.volume > 0.0, "pickups happen next to the camera");
                self.coin_sounds += 1;
            }
        }
        fn play_voice(
            &mut self,
            _: trino_core::SoundId,
            p: &trino_core::SoundParams,
            looped: bool,
        ) -> Option<trino_core::VoiceId> {
            assert!(looped, "ambience loops");
            self.voices_started += 1;
            self.loudest_voice = self.loudest_voice.max(p.volume);
            self.next += 1;
            Some(trino_core::VoiceId(self.next))
        }
        fn set_voice(&mut self, _: trino_core::VoiceId, p: &trino_core::SoundParams) {
            self.loudest_voice = self.loudest_voice.max(p.volume);
        }
        fn play_stinger(&mut self, m: trino_core::MusicId) {
            assert_eq!(m, FANFARE);
            self.stingers += 1;
        }
    }

    #[test]
    fn audio_is_positional_and_the_last_coin_plays_the_fanfare() {
        let mut g = ShowcaseGame::new();
        let mut log = AudioLog::default();
        run_demo(&mut g, |g, input| g.update(input, &mut log, 1.0 / 60.0)).unwrap();
        assert_eq!(log.coin_sounds, 5);
        assert_eq!(log.stingers, 1, "fanfare once, on the last coin");
        assert!(log.voices_started >= 1, "the pond's water loop started");
        assert!(log.loudest_voice > 0.0);
    }

    #[test]
    fn far_trees_switch_to_cheaper_lods() {
        let leaves = find(NODES, "tree-1/tree/leaves").unwrap();
        let p = leaves.position();
        let near = p + Vec3::new(0.0, 0.0, 5.0);
        let mid = p + Vec3::new(0.0, 0.0, 20.0);
        let far = p + Vec3::new(0.0, 0.0, 50.0);
        let id = trino_core::ModelId::from_path;
        assert_eq!(leaves.model_from(near), Some(id("models/tree-top")));
        assert_eq!(leaves.model_from(mid), Some(id("models/tree-top-far")));
        assert_eq!(leaves.model_from(far), None, "culled past the fog");
    }

    #[test]
    fn hero_lands_and_crates_block_it() {
        let mut g = ShowcaseGame::new();
        for _ in 0..60 {
            g.update(&InputState::default(), &mut NullAudio, 1.0 / 60.0);
        }
        assert!(g.on_ground, "ground collider holds the hero");
        assert!((g.pos.y - HERO_HALF.y).abs() < 1e-3, "y = {}", g.pos.y);
        // Teleport next to crate-a and walk into it: the box stops us.
        let c = find(NODES, "crate-a").unwrap();
        let wall = c.world_aabb().unwrap();
        g.pos = Vec3::new(wall.min.x - 2.0, HERO_HALF.y, wall.center().z);
        g.yaw = 0.0; // forward = +Z, right = +X
        let mut input = InputState::default();
        input.stick = Vec2::new(1.0, 0.0);
        for _ in 0..120 {
            g.update(&input, &mut NullAudio, 1.0 / 60.0);
        }
        assert!(
            g.pos.x <= wall.min.x - HERO_HALF.x + 1e-3,
            "walked through the crate: {} vs {}",
            g.pos.x,
            wall.min.x
        );
    }

    /// A pointer pressed at `at` on `screen`.
    fn touch(screen: Screen, at: Vec2) -> InputState {
        let mut s = InputState::default();
        s.pointer = Some(at);
        s.pointer_screen = screen.index();
        s.set_pointer_button(trino_core::PointerButton::Primary, true);
        s
    }

    /// Hit-testing: each button's corners are inside it, a pixel past
    /// each edge is not (`Rect::contains` is half-open), and no two
    /// buttons overlap.
    #[test]
    fn panel_buttons_hit_inside_and_miss_just_outside() {
        for t in panel::TOGGLES {
            let b = t.rect();
            let (x0, y0) = (b.pos.x, b.pos.y);
            let (x1, y1) = (x0 + b.size.x, y0 + b.size.y);
            for p in [
                Vec2::new(x0, y0),
                Vec2::new(x1 - 0.5, y0),
                Vec2::new(x0, y1 - 0.5),
                Vec2::new(x1 - 0.5, y1 - 0.5),
            ] {
                assert_eq!(panel::hit(p), Some(t), "{t:?} at {p:?}");
            }
            for p in [
                Vec2::new(x0 - 1.0, y0 + 5.0),
                Vec2::new(x1, y0 + 5.0),
                Vec2::new(x0 + 5.0, y0 - 1.0),
                Vec2::new(x0 + 5.0, y1),
            ] {
                assert_eq!(panel::hit(p), None, "{t:?} edge at {p:?}");
            }
            assert!(x0 >= 0.0 && y0 >= 0.0 && x1 <= 320.0 && y1 <= 240.0);
            for u in panel::TOGGLES.into_iter().filter(|&u| u != t) {
                assert!(!b.intersects(&u.rect()), "{t:?} overlaps {u:?}");
            }
        }
        assert_eq!(panel::hit(Vec2::new(160.0, 60.0)), None, "the metrics");
    }

    /// Each toggle cycles through its values and back to the default.
    #[test]
    fn panel_toggles_cycle_back_to_the_default() {
        for (t, steps) in [
            (panel::Toggle::Camera, 3),
            (panel::Toggle::Light, 2),
            (panel::Toggle::Fog, 2),
            (panel::Toggle::Filter, 3),
            (panel::Toggle::Anim, 2),
        ] {
            let mut look = panel::Look::DEFAULT;
            for i in 1..=steps {
                look.apply(t);
                assert_eq!(look.changed(t), i < steps, "{t:?} after {i}");
                for u in panel::TOGGLES.into_iter().filter(|&u| u != t) {
                    assert!(!look.changed(u), "{t:?} changed {u:?}");
                }
            }
            assert_eq!(look, panel::Look::DEFAULT);
        }
    }

    /// Bottom-screen taps toggle on the press only (held does not
    /// repeat); the same spot clicked on the top screen does nothing; the
    /// pad buttons toggle the same options.
    #[test]
    fn panel_taps_and_pad_buttons_toggle_the_look() {
        let mut g = ShowcaseGame::new();
        let idle = InputState::default();
        let fog = panel::Toggle::Fog.rect();
        let at = fog.pos + fog.size * 0.5;
        g.update(&touch(Screen::Top, at), &mut NullAudio, 1.0 / 60.0);
        g.update(&idle, &mut NullAudio, 1.0 / 60.0);
        assert_eq!(g.look, panel::Look::DEFAULT, "top-screen clicks");
        g.update(&touch(Screen::Bottom, at), &mut NullAudio, 1.0 / 60.0);
        assert!(!g.look.fog, "a FOG tap");
        for _ in 0..5 {
            g.update(&touch(Screen::Bottom, at), &mut NullAudio, 1.0 / 60.0);
        }
        assert!(!g.look.fog, "held does not toggle again");
        g.update(&idle, &mut NullAudio, 1.0 / 60.0);
        g.update(&touch(Screen::Bottom, at), &mut NullAudio, 1.0 / 60.0);
        assert!(g.look.fog, "a second tap");
        g.update(&idle, &mut NullAudio, 1.0 / 60.0);
        // A tap between buttons does nothing.
        g.update(
            &touch(Screen::Bottom, Vec2::new(107.0, 130.0)),
            &mut NullAudio,
            1.0 / 60.0,
        );
        g.update(&idle, &mut NullAudio, 1.0 / 60.0);
        assert_eq!(g.look, panel::Look::DEFAULT, "the gap");
        for t in panel::TOGGLES {
            let mut press = InputState::default();
            press.set(t.button(), true);
            g.update(&press, &mut NullAudio, 1.0 / 60.0);
            assert!(g.look.changed(t), "{t:?} by {:?}", t.button());
            g.update(&idle, &mut NullAudio, 1.0 / 60.0);
        }
    }

    /// Pause freezes the animation clock; the SCENE preset is the scene's
    /// own camera; HIGH looks down more steeply than FOLLOW.
    #[test]
    fn panel_options_change_the_clock_and_the_camera() {
        let mut g = ShowcaseGame::new();
        let dt = 1.0 / 60.0;
        g.update(&InputState::default(), &mut NullAudio, dt);
        let t0 = g.time;
        g.look.paused = true;
        g.update(&InputState::default(), &mut NullAudio, dt);
        assert_eq!(g.time, t0, "paused");
        let follow = g.camera();
        g.look.camera = panel::CAMERA_HIGH;
        let high = g.camera();
        let rise =
            |c: Camera3| (c.eye.y - c.target.y) / sqrt((c.eye - c.target).dot(c.eye - c.target));
        assert!(rise(high) > rise(follow) + 0.1, "steeper");
        assert_eq!(high.target, follow.target, "still on the hero");
        g.look.camera = panel::CAMERA_SCENE;
        assert_eq!(g.camera(), scene::CAMERA);
    }

    /// Records what `render` asks of a renderer, per screen.
    #[derive(Default)]
    struct PerScreen {
        two: bool,
        screen: u8,
        rects: [u32; 2],
        models: [u32; 2],
        texts: [u32; 2],
        lights: Vec<Light>,
        filters: Vec<trino_core::TexFilter>,
    }

    impl Renderer for PerScreen {
        fn caps(&self) -> &trino_core::Caps {
            &trino_core::Caps::N3DS
        }
        fn begin_frame(&mut self, _: Color) {
            self.screen = 0;
        }
        fn draw_sprite(&mut self, _: SpriteId, _: Vec2, _: &trino_core::SpriteParams) {}
        fn fill_rect(&mut self, _: trino_core::Rect, _: Color) {
            self.rects[self.screen as usize] += 1;
        }
        fn draw_text(&mut self, _: FontId, _: Vec2, _: &str, _: &TextParams) {
            self.texts[self.screen as usize] += 1;
        }
        fn draw_model(
            &mut self,
            _: trino_core::ModelId,
            _: &trino_core::Transform3,
            _: Material,
            _: &trino_core::ModelParams,
        ) {
            self.models[self.screen as usize] += 1;
        }
        fn set_light(&mut self, light: &Light) {
            self.lights.push(*light);
        }
        fn set_texture_filter(&mut self, filter: trino_core::TexFilter) {
            self.filters.push(filter);
        }
        fn end_frame(&mut self) {}
        fn second_screen(&self) -> Option<Vec2> {
            self.two.then_some(Vec2::new(320.0, 240.0))
        }
        fn set_screen(&mut self, screen: u8) {
            if self.two && screen == 1 {
                self.screen = 1;
            }
        }
        fn screen(&self) -> Screen {
            Screen::from_index(self.screen).unwrap()
        }
    }

    /// The panel draws only where `on_screen(Bottom)` succeeds, after the
    /// top's draws: a single screen gets the same top and nothing else.
    #[test]
    fn panel_draws_only_with_a_second_screen() {
        let mut one = PerScreen::default();
        let mut two = PerScreen {
            two: true,
            ..PerScreen::default()
        };
        let mut g = ShowcaseGame::new();
        g.render(&mut one);
        g.render(&mut two);
        // The showcase's top screen has no rectangles: any here would be
        // the panel landing on the only screen.
        assert_eq!(one.rects, [0, 0], "no panel on a single screen");
        assert_eq!(one.texts[1] + one.models[1], 0);
        assert_eq!(two.models[0], one.models[0], "the same top");
        assert_eq!(two.texts[0], one.texts[0]);
        assert!(two.models[0] > 20, "the scene: {}", two.models[0]);
        assert_eq!(two.models[1], 0, "the panel is 2D");
        assert_eq!(two.rects[1], 2 + panel::TOGGLES.len() as u32);
        // Title, coins, five metric lines, three labels per button.
        assert_eq!(two.texts[1], 2 + 5 + 3 * panel::TOGGLES.len() as u32);
    }

    /// The default look renders the scene as authored (its light, the
    /// target's default filter); the toggles reach the renderer.
    #[test]
    fn panel_options_reach_the_renderer() {
        let mut r = PerScreen::default();
        let mut g = ShowcaseGame::new();
        g.render(&mut r);
        assert_eq!(r.lights, [scene::LIGHT], "the scene's light, exactly");
        assert_eq!(r.filters, [trino_core::TexFilter::Default]);
        g.look.apply(panel::Toggle::Fog);
        g.look.apply(panel::Toggle::Filter);
        g.render(&mut r);
        assert!(!r.lights[1].fog.enabled(), "fog off");
        assert_eq!(r.lights[1].color, scene::LIGHT.color, "same light");
        assert_eq!(r.filters[1], trino_core::TexFilter::Point);
        g.look.apply(panel::Toggle::Light);
        g.render(&mut r);
        assert_ne!(r.lights[2].color, scene::LIGHT.color, "dusk");
        assert!(r.lights[2].ambient < scene::LIGHT.ambient);
    }

    /// The readout keeps a fixed width whatever the numbers (the panel's
    /// own glyph count, which `stats` reports next frame, must not move).
    #[test]
    fn metric_lines_have_a_fixed_width() {
        let lens = |dt: f32, s: &trino_core::FrameStats| {
            panel::metric_lines(dt, s).map(|l| l.as_str().len())
        };
        let mut big = trino_core::FrameStats {
            tris: 1_000_000,
            sprites: 123_456,
            models: 5_000,
            ..Default::default()
        };
        big.tnl.verts = 1_000_000;
        big.tnl.rsp_draws = 4_000;
        let base = lens(1.0 / 60.0, &trino_core::FrameStats::default());
        assert_eq!(lens(1.0 / 60.0, &big), base);
        assert_eq!(lens(0.5, &big), base);
        assert_eq!(lens(0.0, &big), base);
        let lines = panel::metric_lines(1.0 / 60.0, &trino_core::FrameStats::default());
        assert_eq!(lines[0].as_str(), "FRAME 16.7 MS 060 FPS");
        for l in &lines {
            assert!(l.as_str().len() * 12 <= 308, "{} fits", l.as_str());
        }
    }

    #[test]
    fn scripted_run_collects_every_coin_and_wins() {
        let mut g = ShowcaseGame::new();
        let frames = run_demo(&mut g, |g: &mut ShowcaseGame, input: &InputState| {
            g.update(input, &mut NullAudio, 1.0 / 60.0);
        })
        .unwrap();
        assert_eq!(g.coins, 5);
        assert_eq!(g.coins_left(), 0, "collected coins are despawned");
        assert!(g.reached_goal);
        assert!(frames < 60 * 90, "took {frames} frames");
    }
}
