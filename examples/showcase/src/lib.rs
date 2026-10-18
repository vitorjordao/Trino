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
//!   `player`, `goal`, `water`, `patrol`, `pushable`), never by hardcoded
//!   positions.
//! - **The river** (issue #49, [`river`]): the hero and a patrolling robot
//!   wade through it, the hero can shove the loose crate off the bank and
//!   watch it float away. Whatever enters the water starts a ripple
//!   (`trino_core::RippleBuffer`, handed to `Renderer::set_water`), a burst
//!   of droplets and a positional splash; wading keeps rippling. The water
//!   surface itself is a material (`materials/water`): swell, flow, Fresnel
//!   and glint, drawn by each target's tier (ADR 0015).
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
    Ambience, AmbientOcclusion, Audio, BlobId, Button, Camera3, Color, Emitter, EntityId,
    EntityStore, FontId, Game, InputState, Light, Listener, Mat34, Material, MusicId,
    ParticleSystem, Platform, Renderer, RippleBuffer, Screen, SoundId, SpriteId, TextAlign,
    TextBuf, TextParams, Vec2, Vec3, spatialize,
};

pub mod panel;
pub mod river;

#[allow(clippy::all)]
#[rustfmt::skip]
mod scene {
    include!("showcase_gen.rs");
}

pub use scene::NODES;

/// The exported scene's camera, light, ambient occlusion and clear color.
pub mod scene_consts {
    pub use super::scene::{AO, CAMERA, CLEAR, LIGHT};
}

/// Live entities: the whole exported scene fits with room to spare.
pub const CAPACITY: usize = 128;
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

/// Splash droplets where something enters the river: a quick crown of
/// drops that falls back.
const SPLASH: Emitter = Emitter {
    sprite: SpriteId::from_path("sprites/fx_drop"),
    rate: 0.0,
    life: (0.35, 0.7),
    speed: (2.0, 3.6),
    direction: Vec3::new(0.0, 1.0, 0.0),
    spread: 0.85,
    gravity: Vec3::new(0.0, -11.0, 0.0),
    size: (0.18, 0.06),
    color: (
        Color::rgba(235, 248, 255, 235),
        Color::rgba(170, 215, 255, 60),
    ),
    ..Emitter::DEFAULT
};
const SPLASH_SOUND: SoundId = SoundId::from_path("sounds/water_splash");
/// Crest height of the ripple a body makes entering the water (scaled up
/// by how fast it falls) and while it wades, every `WADE_EVERY` seconds.
const RIPPLE_ENTER: f32 = 0.07;
const RIPPLE_WADE: f32 = 0.03;
const WADE_EVERY: f32 = 0.32;
/// Wading slows a walker down.
const WADE_SPEED: f32 = 0.6;
/// The patrolling robot's pace and the loose crate's drift.
const PATROL_SPEED: f32 = 1.1;
const DRIFT_SPEED: f32 = 0.8;
const CRATE_HALF: f32 = 0.5;

/// Physics substep: small steps keep fast falls from tunneling.
const STEP: f32 = 1.0 / 120.0;

/// A body that can enter the water: wet last frame, time to its next
/// wading ripple.
#[derive(Clone, Copy, Default)]
struct Wader {
    wet: bool,
    timer: f32,
}

/// What happened to a body and the water this frame.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Contact {
    /// Entered the water at this point, this hard (0..1).
    Splash(Vec3, f32),
    /// Wading: a small ripple here.
    Wade(Vec3),
}

impl Wader {
    /// Track a body standing (or falling) at `feet`, moving horizontally
    /// at `speed` with vertical velocity `vy`.
    fn step(&mut self, feet: Vec3, speed: f32, vy: f32, dt: f32) -> Option<Contact> {
        let wet = river::wading_depth(feet) > 0.02;
        let was = core::mem::replace(&mut self.wet, wet);
        if wet && !was {
            self.timer = WADE_EVERY;
            let hard = (0.4 + -vy * 0.12).clamp(0.4, 1.0);
            let at = Vec3::new(feet.x, river::WATER_Y, feet.z);
            return Some(Contact::Splash(at, hard));
        }
        if wet && speed > 0.3 {
            self.timer -= dt;
            if self.timer <= 0.0 {
                self.timer += WADE_EVERY;
                return Some(Contact::Wade(Vec3::new(feet.x, river::WATER_Y, feet.z)));
            }
        }
        None
    }
}

/// The loose crate's state (`pushable`): on the ground (pushed by the
/// hero), falling, or floating downstream until it respawns.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CrateState {
    Resting,
    Falling(f32),
    Floating,
}

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
    /// Screen-space ambient occlusion (PC native only; the exported
    /// scene's `AO`, none from a baked scene unless the host sets it).
    pub ao: Option<AmbientOcclusion>,
    /// Coin pickup particles (billboards).
    sparks: ParticleSystem<32>,
    /// Droplets where things enter the river.
    splash: ParticleSystem<24>,
    /// The river's ripples (`Renderer::set_water`), oldest first.
    pub ripples: RippleBuffer<{ trino_core::MAX_RIPPLES }>,
    /// Splashes so far (tests and self-tests read it).
    pub splashes: u32,
    hero_wade: Wader,
    /// The patrolling robot (tag `patrol`), its two ends and its leg.
    walker: Option<EntityId>,
    patrol: (Vec3, Vec3),
    patrol_t: f32,
    walker_wade: Wader,
    /// The loose crate (tag `pushable`), where it respawns, its state.
    pushable: Option<EntityId>,
    crate_home: Vec3,
    pub crate_state: CrateState,
    crate_wade: Wader,
    /// Splashes this frame, for the sounds (`update` has the audio).
    splash_sounds: (u32, Vec3),
    /// Scene sound sources (the pond) playing near the camera.
    ambience: Ambience<4>,
    /// Coins collected this frame: count and where (positional sounds).
    picked: (u32, Vec3),
    /// The look the panel (or the pad) switched: camera, light, fog,
    /// texture filter, animation clock.
    pub look: panel::Look,
    /// Smoothed frame time: the game time (`update`'s `dt`) between two
    /// `render`s, so a host that updates several times per drawn frame (a
    /// self-test drawing 1 of 32) reads as the frames it really shows.
    frame_dt: f32,
    /// Game time since the last `render`.
    dt_since_render: f32,
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
        let mut game = Self::from_scene(
            NODES.iter().copied(),
            scene::CAMERA,
            scene::LIGHT,
            scene::CLEAR,
            screen,
        );
        game.ao = scene::AO;
        game
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
        // Every field is set by `reset_from_scene`; the store is filled in
        // place (no temporary copy of the bulk of the state).
        let mut game = Self::blank();
        game.reset_from_scene(nodes, camera, light, clear, screen);
        game
    }

    /// An empty game (no scene): what [`Self::reset`] fills in. `const`, so
    /// consoles copy it from a `static` straight into a box and reset it
    /// there — the game (~25 KB) never sits on the N64's 64 KB stack.
    pub const fn blank() -> Self {
        ShowcaseGame {
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
            yaw: 0.0,
            pitch: 0.0,
            dist: 0.0,
            prev: InputState::IDLE,
            screen: Vec2::ZERO,
            spawn: Vec3::ZERO,
            camera_fov: 1.0,
            light: trino_core::DEFAULT_LIGHT,
            clear: Color::BLACK,
            ao: None,
            sparks: ParticleSystem::new(SPARKS, 0x5EED),
            splash: ParticleSystem::new(SPLASH, 0x5A1A),
            ripples: RippleBuffer::new(),
            splashes: 0,
            hero_wade: Wader {
                wet: false,
                timer: 0.0,
            },
            walker: None,
            patrol: (Vec3::ZERO, Vec3::ZERO),
            patrol_t: 0.0,
            walker_wade: Wader {
                wet: false,
                timer: 0.0,
            },
            pushable: None,
            crate_home: Vec3::ZERO,
            crate_state: CrateState::Resting,
            crate_wade: Wader {
                wet: false,
                timer: 0.0,
            },
            splash_sounds: (0, Vec3::ZERO),
            ambience: Ambience::new(),
            picked: (0, Vec3::ZERO),
            look: panel::Look::DEFAULT,
            frame_dt: 0.0,
            dt_since_render: 0.0,
            scene_camera: Camera3 {
                eye: Vec3::ZERO,
                target: Vec3::ZERO,
                fov_y: 1.0,
            },
            scene_yaw: 0.0,
        }
    }

    /// Start over in the exported scene, in place (consoles keep one game
    /// in a box and reset it: a second ~25 KB value on the N64's 64 KB stack
    /// overflowed it).
    pub fn reset(&mut self, screen: Vec2) {
        self.reset_from_scene(
            NODES.iter().copied(),
            scene::CAMERA,
            scene::LIGHT,
            scene::CLEAR,
            screen,
        );
        self.ao = scene::AO;
    }

    /// [`Self::from_scene`] into this game, in place: every field is set
    /// (the pattern below names them all), the entity store refilled.
    pub fn reset_from_scene(
        &mut self,
        nodes: impl IntoIterator<Item = SceneNode>,
        camera: Camera3,
        light: Light,
        clear: Color,
        screen: Vec2,
    ) {
        // Start from the scene's own camera, expressed as an orbit.
        let d = camera.eye - camera.target;
        let dist = sqrt(d.dot(d).max(1e-6));
        let ShowcaseGame {
            world,
            hero,
            pos,
            vel,
            on_ground,
            coins,
            total_coins,
            reached_goal,
            facing,
            time,
            yaw,
            pitch,
            dist: orbit,
            prev,
            screen: scr,
            spawn,
            camera_fov,
            light: lgt,
            clear: clr,
            ao,
            sparks,
            splash,
            ripples,
            splashes,
            hero_wade,
            walker,
            patrol,
            patrol_t,
            walker_wade,
            pushable,
            crate_home,
            crate_state,
            crate_wade,
            splash_sounds,
            ambience,
            picked,
            look,
            frame_dt,
            dt_since_render,
            scene_camera,
            scene_yaw,
        } = self;
        world.clear();
        world.spawn_iter(nodes);
        *hero = world.with_tag("player").next();
        *spawn = world
            .find("spawn")
            .and_then(|id| world.get(id))
            .map_or(Vec3::ZERO, |e| e.position());
        *pos = *spawn + Vec3::new(0.0, HERO_HALF.y, 0.0);
        *vel = Vec3::ZERO;
        *on_ground = false;
        *coins = 0;
        *total_coins = world.with_tag("coin").count() as u32;
        *reached_goal = false;
        *facing = 0.0;
        *time = 0.0;
        *yaw = atan2(d.x, -d.z);
        *pitch = (d.y / dist).clamp(0.15, 1.2);
        *orbit = dist * 0.7;
        *prev = InputState::default();
        *scr = screen;
        *camera_fov = camera.fov_y;
        *lgt = light;
        *clr = clear;
        *ao = None;
        *sparks = ParticleSystem::new(SPARKS, 0x5EED);
        *splash = ParticleSystem::new(SPLASH, 0x5A1A);
        ripples.clear();
        *splashes = 0;
        *hero_wade = Wader::default();
        *walker = world.with_tag("patrol").next();
        let end = world
            .with_tag("patrol-end")
            .next()
            .and_then(|id| world.get(id))
            .map(|e| e.position());
        match (walker.and_then(|id| world.get(id)), end) {
            (Some(w), Some(end)) => *patrol = (w.position(), end),
            _ => {
                *walker = None;
                *patrol = (Vec3::ZERO, Vec3::ZERO);
            }
        }
        *patrol_t = 0.0;
        *walker_wade = Wader::default();
        *pushable = world.with_tag("pushable").next();
        *crate_home = pushable
            .and_then(|id| world.get(id))
            .map_or(Vec3::ZERO, |e| e.position());
        *crate_state = CrateState::Resting;
        *crate_wade = Wader::default();
        *splash_sounds = (0, Vec3::ZERO);
        *ambience = Ambience::new();
        *picked = (0, Vec3::ZERO);
        *look = panel::Look::DEFAULT;
        *frame_dt = 1.0 / 60.0;
        *dt_since_render = 0.0;
        *scene_camera = camera;
        *scene_yaw = atan2(d.x, -d.z);
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

    /// Draw one entity seen from `eye` (its LOD, its clip at the game's
    /// clock; coins spin and bob).
    fn draw_entity(&self, r: &mut dyn Renderer, e: &trino_core::Entity, eye: Vec3) {
        // LOD from the scene: far trees swap to cheaper fake objects.
        let Some(model) = e.model_from(eye).filter(|_| e.visible) else {
            return;
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

    /// The yaw walking is relative to: the orbit's, or the scene camera's.
    fn walk_yaw(&self) -> f32 {
        if self.look.camera == panel::CAMERA_SCENE {
            self.scene_yaw
        } else {
            self.yaw
        }
    }

    /// Smoothed time between drawn frames in seconds (the panel's readout).
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
        let mut ids = [EntityId::NONE; CAPACITY];
        let n = self.world.solids(&mut solids, Some(&mut ids));
        let feet = self.pos - Vec3::new(0.0, HERO_HALF.y, 0.0);
        let speed = if river::wading_depth(feet) > 0.15 {
            SPEED * WADE_SPEED
        } else {
            SPEED
        };
        self.vel.x = wish.x * speed;
        self.vel.z = wish.z * speed;
        if jump && self.on_ground {
            self.vel.y = JUMP;
        }
        let mut left = dt;
        while left > 0.0 {
            let h = left.min(STEP);
            left -= h;
            self.vel.y -= GRAVITY * h;
            let want = self.vel;
            let out = move_aabb(self.pos, HERO_HALF, self.vel, h, &solids[..n]);
            if out.hit_wall {
                self.push_crate(&mut solids[..n], &ids[..n], want, h);
            }
            self.pos = out.pos;
            self.vel = out.vel;
            self.on_ground = out.on_ground;
            // The ground: the carved terrain, the stepping stones, the
            // bridge (`river::floor_height`; no collider models it).
            let floor = river::floor_height(self.pos.x, self.pos.z) + HERO_HALF.y;
            if self.pos.y <= floor {
                self.pos.y = floor;
                if self.vel.y < 0.0 {
                    self.vel.y = 0.0;
                }
                self.on_ground = true;
            }
        }
        // Fell off the world: back to the spawn.
        if self.pos.y < -10.0 {
            self.pos = self.spawn + Vec3::new(0.0, HERO_HALF.y, 0.0);
            self.vel = Vec3::ZERO;
        }
    }

    /// The hero walked into the loose crate: shove it along (it stops at
    /// other solids), at the hero's pace.
    fn push_crate(&mut self, solids: &mut [Aabb], ids: &[EntityId], vel: Vec3, h: f32) {
        let Some(id) = self
            .pushable
            .filter(|_| self.crate_state == CrateState::Resting)
        else {
            return;
        };
        let Some(i) = ids.iter().position(|&x| x == id) else {
            return;
        };
        // Touching it, in the direction of the push.
        let reach = Aabb::from_center_half(
            self.pos + Vec3::new(vel.x, 0.0, vel.z) * (h * 2.0),
            HERO_HALF + Vec3::new(0.05, -0.05, 0.05),
        );
        if !reach.overlaps(&solids[i]) {
            return;
        }
        let d = Vec3::new(vel.x, 0.0, vel.z) * h;
        let moved = Aabb::new(solids[i].min + d, solids[i].max + d);
        let blocked = solids
            .iter()
            .enumerate()
            .any(|(j, s)| j != i && moved.overlaps(s));
        if blocked {
            return;
        }
        solids[i] = moved;
        if let Some(e) = self.world.get_mut(id) {
            e.translate(d);
        }
    }

    /// Something entered the water or waded: a ripple, and for an entry
    /// droplets and a splash sound.
    fn contact(&mut self, c: Contact) {
        match c {
            Contact::Splash(at, hard) => {
                self.ripples
                    .spawn(Vec2::new(at.x, at.z), RIPPLE_ENTER * (0.6 + hard));
                self.splash.origin = at;
                self.splash.burst((6.0 + 10.0 * hard) as usize);
                self.splashes += 1;
                self.splash_sounds = (self.splash_sounds.0 + 1, at);
            }
            Contact::Wade(at) => self.ripples.spawn(Vec2::new(at.x, at.z), RIPPLE_WADE),
        }
    }

    /// The patrolling robot walks to the far bank and back, through the
    /// ford: it wades, and the river ripples around it.
    fn patrol(&mut self, dt: f32) {
        let Some(id) = self.walker else {
            return;
        };
        let (a, b) = self.patrol;
        let d = b - a;
        let len = sqrt(d.x * d.x + d.z * d.z).max(1e-3);
        // A ping-pong over 0..2 (out and back).
        self.patrol_t = (self.patrol_t + dt * PATROL_SPEED / len) % 2.0;
        let (k, dir) = if self.patrol_t < 1.0 {
            (self.patrol_t, 1.0)
        } else {
            (2.0 - self.patrol_t, -1.0)
        };
        let x = a.x + d.x * k;
        let z = a.z + d.z * k;
        let feet = Vec3::new(x, river::terrain_height(x, z), z);
        let yaw = atan2(d.x * dir, d.z * dir);
        if let Some(e) = self.world.get_mut(id) {
            e.matrix =
                Mat34::from_rotation_scale_translation(Vec3::new(0.0, yaw, 0.0), Vec3::ONE, feet);
        }
        if let Some(c) = self.walker_wade.step(feet, PATROL_SPEED, 0.0, dt) {
            self.contact(c);
        }
    }

    /// The loose crate: falls when shoved off the bank, floats down the
    /// river (no longer solid), and comes back home past the delta.
    fn crate_physics(&mut self, dt: f32) {
        let Some(id) = self.pushable else {
            return;
        };
        let Some(p) = self.world.get(id).map(|e| e.position()) else {
            return;
        };
        let floor = river::terrain_height(p.x, p.z);
        let mut p = p;
        // Deep enough under the surface to float off its bank.
        let floats = |p: Vec3| river::over_water(p.x, p.z) && p.y < river::WATER_Y - 0.2;
        match self.crate_state {
            CrateState::Resting if floats(p) => {
                self.crate_state = CrateState::Falling(0.0);
            }
            CrateState::Resting => {
                if p.y > floor + 0.02 {
                    self.crate_state = CrateState::Falling(0.0);
                }
            }
            CrateState::Falling(vy) => {
                let vy = vy - GRAVITY * dt;
                p.y += vy * dt;
                let float_at = river::WATER_Y - CRATE_HALF * 0.6;
                if floats(p) || (river::over_water(p.x, p.z) && p.y <= float_at) {
                    p.y = float_at;
                    self.crate_state = CrateState::Floating;
                    // No longer an obstacle: it drifts in the water.
                    if let Some(e) = self.world.get_mut(id) {
                        e.collider = None;
                    }
                } else if p.y <= floor {
                    p.y = floor;
                    self.crate_state = CrateState::Resting;
                } else {
                    self.crate_state = CrateState::Falling(vy);
                }
                let feet = Vec3::new(p.x, p.y, p.z);
                if let Some(c) = self.crate_wade.step(feet, 0.0, vy, dt) {
                    self.contact(c);
                }
            }
            CrateState::Floating => match river::locate(p.x, p.z) {
                Some(c) if p.x < 19.0 => {
                    p.x += c.flow.0 * DRIFT_SPEED * dt;
                    p.z += c.flow.1 * DRIFT_SPEED * dt;
                    p.y = river::WATER_Y - CRATE_HALF * 0.6
                        + 0.03 * trino_core::water::sin_turns(self.time * 0.6);
                    if let Some(c) = self.crate_wade.step(p, DRIFT_SPEED, 0.0, dt) {
                        self.contact(c);
                    }
                }
                _ => {
                    // Washed out of the world: back on the bank.
                    p = self.crate_home;
                    self.crate_state = CrateState::Resting;
                    self.crate_wade = Wader::default();
                    if let Some(node) = trino_core::scene::find(NODES, "loose-crate")
                        && let Some(e) = self.world.get_mut(id)
                    {
                        e.collider = node.collider;
                    }
                }
            },
        }
        if let Some(e) = self.world.get_mut(id) {
            e.set_position(p);
        }
    }

    /// Something in the river this frame: where the hero is.
    fn hero_water(&mut self, dt: f32) {
        let feet = self.pos - Vec3::new(0.0, HERO_HALF.y, 0.0);
        let speed = sqrt(self.vel.x * self.vel.x + self.vel.z * self.vel.z);
        if let Some(c) = self.hero_wade.step(feet, speed, self.vel.y, dt) {
            self.contact(c);
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
        self.dt_since_render += dt;
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
            self.splash.update(dt);
            self.ripples.update(dt);
            self.patrol(dt);
            self.crate_physics(dt);
        }
        let jump = input.just_pressed(&self.prev, Button::A);
        self.prev = *input;
        if !self.reached_goal {
            self.physics(wish, jump, dt);
            self.hero_water(dt);
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
        // Splashes, heard from where they happened.
        let (n, at) = core::mem::replace(&mut self.splash_sounds, (0, Vec3::ZERO));
        if n > 0 {
            let listener = Listener::from_camera(&self.camera());
            audio.play_sound_with(SPLASH_SOUND, &spatialize(&listener, at, 22.0, 0.9));
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
        if self.dt_since_render > 0.0 {
            self.frame_dt += (self.dt_since_render - self.frame_dt) * 0.125;
            self.dt_since_render = 0.0;
        }
        let camera = self.camera();
        r.begin_frame(self.clear);
        r.set_camera(&camera);
        // The scene's light as authored unless the panel switched it; the
        // filter every frame (it persists in the renderer: `Default` hands
        // the next game the target's own default).
        r.set_light(&self.look.light(&self.light));
        r.set_texture_filter(self.look.filter);
        // The scene's screen-space AO (PC native; cleared every frame).
        r.set_ambient_occlusion(self.ao.as_ref());
        // The river's clock and ripples (cleared every frame, like AO).
        r.set_water(self.time, self.ripples.as_slice());
        // Everything the editor placed (animated clips included), coins
        // spinning and bobbing on top — game logic driven by tags. Water
        // last: it is translucent, so what lies under it must be drawn.
        for water in [false, true] {
            for (_, e) in self.world.iter() {
                if e.has_tag("water") == water {
                    self.draw_entity(r, e, camera.eye);
                }
            }
        }
        self.sparks.draw(r);
        self.splash.draw(r);
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

/// The river's performance scene (`showcase-river`): the same game state
/// on every target, measured like castle64's scenes (`castle64::perf`) —
/// its T&L work pinned on the PC (`xtask/tests/perf_budget.rs`), its time
/// in ares (`cargo xtask test n64`).
pub mod perf {
    use super::{ShowcaseGame, river};
    use trino_core::{Audio, Game, InputState, Renderer, Vec2, Vec3};

    pub const NAME: &str = "showcase-river";
    /// Frames measured (each one idle update + render).
    pub const MEASURE_FRAMES: u32 = 4;
    pub const DT: f32 = 1.0 / 60.0;

    /// Reset `game` into the scene: the scene's own camera, the hero walked
    /// into the middle of the river (a splash, wading ripples), the robot
    /// on its patrol. Callers then run `MEASURE_FRAMES` idle frames.
    pub fn river_scene(game: &mut ShowcaseGame, screen: Vec2, audio: &mut dyn Audio) {
        game.reset(screen);
        game.look.camera = super::panel::CAMERA_SCENE;
        let c = river::COURSE[8];
        game.pos = Vec3::new(c.x, 0.6, c.z - 2.4);
        let mut walk = InputState::default();
        // Scene camera: "up" walks along its view, toward the river.
        walk.stick = Vec2::new(0.0, 1.0);
        for _ in 0..40 {
            game.update(&walk, audio, DT);
        }
    }

    /// The river alone (`micro-river`, issue #69): the scene camera, the
    /// game's light and water material, two ripples, nothing else drawn —
    /// so the river's own cost is read directly instead of subtracted from
    /// [`NAME`]. Gated like the castle64 scenes (both baselines).
    pub const MICRO_RIVER: &str = "micro-river";

    /// Reset `game` for [`MICRO_RIVER`]: the scene camera, a fixed clock,
    /// one fresh ripple near the ford and one older ring downstream.
    pub fn micro_river_scene(game: &mut ShowcaseGame, screen: Vec2) {
        game.reset(screen);
        game.look.camera = super::panel::CAMERA_SCENE;
        game.time = 2.0;
        game.ripples.clear();
        let (a, b) = (river::COURSE[8], river::COURSE[6]);
        game.ripples.spawn(Vec2::new(b.x, b.z), super::RIPPLE_ENTER);
        game.ripples.update(0.9);
        game.ripples
            .spawn(Vec2::new(a.x, a.z + 0.4), super::RIPPLE_ENTER);
    }

    /// One [`MICRO_RIVER`] frame: the water's clock and ripples advance by
    /// [`DT`] (the "update"), and only the `water` entities are drawn.
    pub fn micro_river_update(game: &mut ShowcaseGame) {
        game.time += DT;
        game.ripples.update(DT);
    }

    /// Draw [`MICRO_RIVER`]'s frame: what [`ShowcaseGame::render`] sets up
    /// (camera, light, filter, water state), then the water entities only.
    pub fn micro_river_render(game: &ShowcaseGame, r: &mut dyn Renderer) {
        let camera = game.camera();
        r.begin_frame(game.clear);
        r.set_camera(&camera);
        r.set_light(&game.look.light(&game.light));
        r.set_texture_filter(game.look.filter);
        r.set_water(game.time, game.ripples.as_slice());
        for (_, e) in game.world.iter() {
            if e.has_tag("water") {
                game.draw_entity(r, e, camera.eye);
            }
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
        // Frames without progress: hop when stuck (crates, trees, a bank).
        let mut stuck = 0;
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
            if stuck >= 20 {
                input.set(Button::A, true);
                stuck = 0;
            }
            let before = game.pos;
            frame(game, &input);
            frames += 1;
            let (dx, dz) = (game.pos.x - before.x, game.pos.z - before.z);
            if dx * dx + dz * dz < 0.02 * 0.02 {
                stuck += 1;
            } else {
                stuck = 0;
            }
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
    frame: impl FnMut(&mut ShowcaseGame, &InputState),
) -> Result<(), &'static str> {
    check_baked_in(platform, &mut ShowcaseGame::new(), frame)
}

/// [`check_baked`] in `game`, which it resets from the blob in place (no
/// second game on the stack: consoles pass the one they keep boxed).
pub fn check_baked_in(
    platform: &dyn Platform,
    game: &mut ShowcaseGame,
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
    game.reset_from_scene(
        baked.nodes(),
        baked.camera(),
        baked.light(),
        baked.clear(),
        Vec2::new(320.0, 240.0),
    );
    run_demo(game, &mut frame).map(|_| ())
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
        assert_eq!(g.world.sound_sources().count(), 2, "the river's murmurs");
        assert!(g.walker.is_some(), "a robot patrols the ford");
        assert!(g.pushable.is_some(), "a crate can be shoved in");
        assert!(
            g.world
                .with_tag("water")
                .filter_map(|id| g.world.get(id))
                .all(|e| e.params.water.is_some()),
            "the river is a water surface"
        );
    }

    /// Records what the game asks of the audio backend.
    #[derive(Default)]
    struct AudioLog {
        splash_sounds: u32,
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
            if s == SPLASH_SOUND {
                self.splash_sounds += 1;
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

    /// The probed fills are told apart by read-back (tolerance 24 per
    /// channel, RGB565 on the 3DS), from each other and from the 3DS
    /// shim's bottom-screen clear (an undrawn panel).
    #[test]
    fn panel_colors_are_far_apart_for_the_probes() {
        let colors = [
            panel::BG,
            panel::BTN_DEFAULT,
            panel::BTN_CHANGED,
            panel::BTN_HELD,
            Color::rgb(16, 18, 28),
        ];
        for (i, a) in colors.iter().enumerate() {
            for b in &colors[i + 1..] {
                let d = |x: u8, y: u8| x.abs_diff(y);
                let far = d(a.r, b.r).max(d(a.g, b.g)).max(d(a.b, b.b));
                assert!(far > 24 + 8, "{a:?} vs {b:?}: {far}");
            }
        }
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

    /// The panel's frame rate counts drawn frames: a host that updates 32
    /// times per `render` (a self-test) reads ~2 fps, not the 60 of `dt`;
    /// one update per render reads 60.
    #[test]
    fn the_frame_rate_counts_drawn_frames() {
        let mut r = PerScreen::default();
        let idle = InputState::default();
        let mut g = ShowcaseGame::new();
        for _ in 0..40 {
            g.update(&idle, &mut NullAudio, 1.0 / 60.0);
            g.render(&mut r);
        }
        assert!((g.frame_dt() - 1.0 / 60.0).abs() < 1e-4, "{}", g.frame_dt());
        for _ in 0..60 {
            for _ in 0..32 {
                g.update(&idle, &mut NullAudio, 1.0 / 60.0);
            }
            g.render(&mut r);
        }
        let fps = 1.0 / g.frame_dt();
        assert!((1.8..2.0).contains(&fps), "{fps} fps");
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
        big.tnl.gpu_draws = 4_000;
        let base = lens(1.0 / 60.0, &trino_core::FrameStats::default());
        assert_eq!(lens(1.0 / 60.0, &big), base);
        assert_eq!(lens(0.5, &big), base);
        assert_eq!(lens(0.0, &big), base);
        let lines = panel::metric_lines(1.0 / 60.0, &trino_core::FrameStats::default());
        assert_eq!(lines[0].as_str(), "FRAME 16.7 MS 060 FPS");
        let mut hw = trino_core::FrameStats::default();
        (hw.tnl.gpu_draws, hw.tnl.gpu_tris) = (12, 1733);
        assert_eq!(
            panel::metric_lines(1.0 / 60.0, &hw)[4].as_str(),
            "HW DRAWS 012 TRIS 01733"
        );
        for l in &lines {
            assert!(l.as_str().len() * 12 <= 308, "{} fits", l.as_str());
        }
    }

    #[test]
    fn wading_into_the_river_splashes_and_ripples() {
        let mut g = ShowcaseGame::new();
        // Stand on the south bank, facing the river (+Z), then walk in.
        let c = river::COURSE[6];
        g.pos = Vec3::new(c.x, HERO_HALF.y, c.z - 3.0);
        g.yaw = 0.0;
        let mut audio = AudioLog::default();
        for _ in 0..10 {
            g.update(&InputState::default(), &mut audio, 1.0 / 60.0);
        }
        assert_eq!(g.splashes, 0, "dry on the bank");
        let mut input = InputState::default();
        input.stick = Vec2::new(0.0, 1.0);
        let mut deepest = f32::MAX;
        let mut most = 0;
        for _ in 0..90 {
            g.update(&input, &mut audio, 1.0 / 60.0);
            deepest = deepest.min(g.pos.y - HERO_HALF.y);
            most = most.max(g.ripples.len());
        }
        assert!(
            deepest < river::WATER_Y - 0.2,
            "walked down into the bed: {deepest}"
        );
        assert!(g.splashes >= 1, "entering the water splashes");
        assert!(most >= 2, "wading keeps rippling ({most})");
        assert!(audio.splash_sounds >= 1, "and is heard");
        // Out on the far bank: the ripples fade.
        for _ in 0..240 {
            g.update(&input, &mut audio, 1.0 / 60.0);
        }
        assert!(
            g.pos.y - HERO_HALF.y > -0.01,
            "climbed the far bank: {}",
            g.pos.y
        );
        assert!(!g.hero_wade.wet, "dry again");
    }

    #[test]
    fn the_robot_patrols_through_the_ford() {
        let mut g = ShowcaseGame::new();
        let id = g.walker.unwrap();
        let start = g.world.get(id).unwrap().position();
        let mut lowest = 0.0f32;
        for _ in 0..60 * 8 {
            g.update(&InputState::default(), &mut NullAudio, 1.0 / 60.0);
            lowest = lowest.min(g.world.get(id).unwrap().position().y);
        }
        assert!(lowest <= river::BED_Y + 1e-3, "it waded through: {lowest}");
        assert!(g.splashes >= 1, "and splashed");
        assert_ne!(g.world.get(id).unwrap().position(), start);
    }

    #[test]
    fn a_shoved_crate_falls_in_and_floats_away() {
        let mut g = ShowcaseGame::new();
        let id = g.pushable.unwrap();
        let home = g.world.get(id).unwrap().position();
        // Behind the crate (south), pushing north toward the river.
        g.pos = Vec3::new(home.x, HERO_HALF.y, home.z - 1.2);
        g.yaw = 0.0;
        let mut input = InputState::default();
        input.stick = Vec2::new(0.0, 1.0);
        let mut floated = false;
        for _ in 0..240 {
            g.update(&input, &mut NullAudio, 1.0 / 60.0);
            floated |= g.crate_state == CrateState::Floating;
        }
        assert!(floated, "shoved off the bank: {:?}", g.crate_state);
        assert!(g.splashes >= 1);
        assert!(
            g.world.get(id).unwrap().collider.is_none(),
            "a floating crate is no obstacle"
        );
        // It drifts downstream (east) and comes home.
        let x0 = g.world.get(id).unwrap().position().x;
        let idle = InputState::default();
        for _ in 0..60 {
            g.update(&idle, &mut NullAudio, 1.0 / 60.0);
        }
        assert!(g.world.get(id).unwrap().position().x > x0 + 0.5);
        for _ in 0..60 * 60 {
            g.update(&idle, &mut NullAudio, 1.0 / 60.0);
        }
        assert_eq!(g.crate_state, CrateState::Resting, "back home");
        assert_eq!(g.world.get(id).unwrap().position(), home);
        assert!(g.world.get(id).unwrap().collider.is_some());
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
        // The route wades across the river and shoves the loose crate in.
        assert!(g.splashes >= 2, "{} splashes", g.splashes);
        assert_ne!(g.crate_state, CrateState::Resting);
        assert!(frames < 60 * 90, "took {frames} frames");
    }
}
