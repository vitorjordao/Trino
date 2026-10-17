//! `TRINO_SCENE=scenes/x.scene.ron` — play an editor scene directly (the
//! editor's *Play scene*): resolves the RON scene + prefabs + materials at
//! runtime, orbits a camera around it, plays animation clips, and reloads
//! live whenever any of those files change on disk (edit in the editor, a
//! text editor or an AI agent — the running game follows).
//!
//! PC-only by design (it parses RON); console builds ship scenes through
//! `cargo xtask scene export` instead (see `examples/showcase`).

use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use trino_core::{
    Button, Camera3, Color, InputState, Material, Renderer, SpriteId, SpriteParams, Vec2, Vec3,
};
use trino_scene::{Library, Scene, Target, resolve_for};

pub struct ScenePlayer {
    path: PathBuf,
    scene: Scene,
    lib: Library,
    /// Whose platform variants to draw (the `TRINO_SIM` profile).
    platform: Target,
    time: f32,
    yaw: f32,
    pitch: f32,
    dist: f32,
    target: Vec3,
    fov: f32,
    stamp: u128,
    last_check: Instant,
}

/// Sum of modification times of everything a scene depends on.
fn fingerprint(scene: &Path) -> u128 {
    let mtime = |p: &Path| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos())
    };
    let mut sum = mtime(scene);
    for dir in ["prefabs", "assets/materials"] {
        let mut stack = vec![PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    sum = sum.wrapping_add(mtime(&p));
                }
            }
        }
    }
    sum
}

/// Materials + prefabs + baked model bounds (for `Auto` colliders).
fn load_library() -> Library {
    let mut lib = Library::load(Path::new(""));
    lib.model_bounds = trino_asset_pipeline::manifest_model_bounds(Path::new("assets"));
    lib
}

impl ScenePlayer {
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let scene = Scene::load(&path)?;
        let lib = load_library();
        for e in &lib.errors {
            eprintln!("library: {e}");
        }
        let c = scene.environment.camera;
        let d = Vec3::new(
            c.eye[0] - c.target[0],
            c.eye[1] - c.target[1],
            c.eye[2] - c.target[2],
        );
        let dist = d.dot(d).sqrt().max(1.0);
        let player = ScenePlayer {
            stamp: fingerprint(&path),
            path,
            yaw: d.x.atan2(-d.z),
            pitch: (d.y / dist).clamp(-1.0, 1.0).asin(),
            dist,
            target: Vec3::new(c.target[0], c.target[1], c.target[2]),
            fov: c.fov.to_radians(),
            scene,
            lib,
            // The simulated console's variants (`TRINO_SIM`).
            platform: match trino_runner_pc::profile_from_env() {
                trino_platform_pc::SimProfile::Native => Target::Pc,
                trino_platform_pc::SimProfile::N64 => Target::N64,
                trino_platform_pc::SimProfile::N3ds => Target::N3ds,
            },
            time: 0.0,
            last_check: Instant::now(),
        };
        player.report();
        Ok(player)
    }

    pub fn title(&self) -> String {
        format!("Trino — scene: {}", self.scene.name)
    }

    fn report(&self) {
        for w in resolve_for(&self.scene, &self.lib, 0.0, self.platform).warnings {
            eprintln!("scene: {w}");
        }
    }

    fn maybe_reload(&mut self) {
        if self.last_check.elapsed().as_secs_f32() < 0.4 {
            return;
        }
        self.last_check = Instant::now();
        let stamp = fingerprint(&self.path);
        if stamp == self.stamp {
            return;
        }
        self.stamp = stamp;
        match Scene::load(&self.path) {
            Ok(scene) => {
                self.scene = scene;
                self.lib = load_library();
                println!("scene reloaded ({})", self.path.display());
                self.report();
            }
            Err(e) => eprintln!("scene reload failed (keeping the old one): {e}"),
        }
    }

    pub fn update(&mut self, input: &InputState, dt: f32) {
        self.maybe_reload();
        self.time += dt;
        let mut d = input.stick;
        for (b, v) in [
            (Button::DpadLeft, Vec2::new(-1.0, 0.0)),
            (Button::DpadRight, Vec2::new(1.0, 0.0)),
            (Button::DpadUp, Vec2::new(0.0, 1.0)),
            (Button::DpadDown, Vec2::new(0.0, -1.0)),
        ] {
            if input.is_down(b) {
                d += v;
            }
        }
        self.yaw += d.x * 1.6 * dt;
        self.pitch = (self.pitch + d.y * dt).clamp(-0.2, 1.45);
        if input.is_down(Button::L) {
            self.dist = (self.dist * (1.0 + dt)).min(200.0);
        }
        if input.is_down(Button::R) {
            self.dist = (self.dist * (1.0 - dt)).max(1.5);
        }
    }

    pub fn render(&mut self, r: &mut dyn Renderer) {
        let env = &self.scene.environment;
        let c = env.clear;
        r.begin_frame(Color::rgb(c[0], c[1], c[2]));
        let cp = self.pitch.cos();
        let camera = Camera3 {
            eye: self.target
                + Vec3::new(cp * self.yaw.sin(), self.pitch.sin(), -cp * self.yaw.cos())
                    * self.dist,
            target: self.target,
            fov_y: self.fov,
        };
        r.set_camera(&camera);
        r.set_light(&env.core_light());
        r.set_ambient_occlusion(env.core_ao().as_ref());
        let resolved = resolve_for(&self.scene, &self.lib, self.time, self.platform);
        let caps = r.caps();
        let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
        let visible = resolved.visible_cells(&camera, screen);
        for item in &resolved.items {
            if !trino_core::vis::in_view(item.cell, visible) {
                continue;
            }
            if let Some(id) = item.model_from(camera.eye) {
                r.draw_model_matrix(id, &item.matrix, Material::VertexLit, &item.params);
            }
        }
        for item in &resolved.items {
            if let Some(s) = &item.sprite {
                let t = s.tint;
                r.draw_sprite(
                    SpriteId(s.id.0),
                    Vec2::new(s.pos[0], s.pos[1]),
                    &SpriteParams {
                        scale: Vec2::new(s.scale[0], s.scale[1]),
                        rotation: s.rotation,
                        tint: Color::rgba(t[0], t[1], t[2], t[3]),
                        flip_x: s.flip_x,
                        flip_y: s.flip_y,
                        ..SpriteParams::DEFAULT
                    },
                );
            }
        }
        r.end_frame();
    }
}
