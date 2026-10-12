//! showcase — the editor's demo scene (`scenes/showcase.scene.ron`) as a
//! portable game crate: run it on PC with `TRINO_GAME=showcase cargo run -p
//! trino-app-pc`; it type-checks for N64 and 3DS (`test-all` skill), and
//! `xtask/tests/showcase_render.rs` proves it renders exactly like the editor.
//!
//! The scene is authored in the editor (or as text), then exported to
//! `no_std` Rust data:
//!
//! ```text
//! cargo xtask scene export scenes/showcase.scene.ron examples/showcase/src/showcase_gen.rs
//! ```
//!
//! `xtask/tests/scene_codegen.rs` fails CI when the generated file drifts
//! from the scene, so the console build always matches what the editor
//! shows. Game logic finds things by **name and tag** (`spawn`, `coin`,
//! `npc`) instead of hardcoding positions.
//!
//! Controls: stick/d-pad orbits the camera, L/R zoom.

#![cfg_attr(any(target_os = "none", target_os = "horizon"), no_std)]

use trino_core::math3d::{cos, sin};
use trino_core::scene::{SceneNode, draw_nodes, find, with_tag};
use trino_core::{Audio, Button, Camera3, Game, InputState, Mat34, Material, Renderer, Vec3};

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

pub struct ShowcaseGame {
    time: f32,
    yaw: f32,
    pitch: f32,
    dist: f32,
    target: Vec3,
}

impl ShowcaseGame {
    pub fn new() -> Self {
        // Start from the scene's own camera, expressed as an orbit.
        let c = scene::CAMERA;
        let d = c.eye - c.target;
        let dist = d.dot(d).max(1e-6);
        let dist = trino_core::math3d::sqrt(dist);
        ShowcaseGame {
            time: 0.0,
            yaw: trino_core::math3d::atan2(d.x, -d.z),
            pitch: (d.y / dist).clamp(-1.0, 1.0),
            dist,
            target: find(NODES, "spawn").map_or(c.target, |n| n.position()),
        }
    }

    pub fn camera(&self) -> Camera3 {
        let cp = cos(self.pitch);
        Camera3 {
            eye: self.target
                + Vec3::new(cp * sin(self.yaw), sin(self.pitch), -cp * cos(self.yaw)) * self.dist,
            target: self.target,
            fov_y: scene::CAMERA.fov_y,
        }
    }

    /// Nodes tagged `coin` (for game logic / tests).
    pub fn coins(&self) -> usize {
        with_tag(NODES, "coin").count()
    }
}

impl Default for ShowcaseGame {
    fn default() -> Self {
        Self::new()
    }
}

impl Game for ShowcaseGame {
    fn update(&mut self, input: &InputState, _audio: &mut dyn Audio, dt: f32) {
        self.time += dt;
        let s = input.stick;
        let mut dx = s.x;
        let mut dy = s.y;
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
        self.yaw += dx * 1.6 * dt;
        self.pitch = (self.pitch + dy * 1.0 * dt).clamp(0.05, 1.4);
        if input.is_down(Button::L) {
            self.dist = (self.dist + 8.0 * dt).min(60.0);
        }
        if input.is_down(Button::R) {
            self.dist = (self.dist - 8.0 * dt).max(3.0);
        }
    }

    fn render(&mut self, r: &mut dyn Renderer) {
        r.begin_frame(scene::CLEAR);
        r.set_camera(&self.camera());
        r.set_light(&scene::LIGHT);
        // Everything the editor placed, animated clips included...
        let static_nodes = |n: &&SceneNode| !n.has_tag("coin");
        let mut i = 0;
        while i < NODES.len() {
            // Draw runs of non-coin nodes in one call (batched depth sort).
            let start = i;
            while i < NODES.len() && static_nodes(&&NODES[i]) {
                i += 1;
            }
            draw_nodes(r, &NODES[start..i], &Mat34::IDENTITY, self.time);
            if i < NODES.len() {
                // ...plus game logic driven by tags: coins spin and bob.
                let n = &NODES[i];
                if let Some(model) = n.model {
                    let spin = Mat34::from_rotation_scale_translation(
                        Vec3::new(0.0, self.time * 2.0, 0.0),
                        Vec3::ONE,
                        Vec3::new(0.0, sin(self.time * 3.0 + i as f32) * 0.12, 0.0),
                    );
                    let p = n.position();
                    let to = Mat34::from_rotation_scale_translation(Vec3::ZERO, Vec3::ONE, p);
                    let back = Mat34::from_rotation_scale_translation(Vec3::ZERO, Vec3::ONE, -p);
                    let m = to.mul(&spin).mul(&back).mul(&n.matrix);
                    r.draw_model_matrix(model, &m, Material::VertexLit, &n.params);
                }
                i += 1;
            }
        }
        r.end_frame();
    }
}

#[cfg(feature = "hot-reload")]
trino_game_api::export_game!(ShowcaseGame);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_data_has_what_the_game_needs() {
        let g = ShowcaseGame::new();
        assert!(find(NODES, "spawn").is_some(), "spawn marker");
        assert_eq!(g.coins(), 5);
        assert!(with_tag(NODES, "npc").count() >= 2);
        // Animated models carry their clip from the editor.
        assert!(NODES.iter().any(|n| n.anim.is_some()));
        // Prefab parts are flattened in (tree instances → trunk + leaves).
        assert!(NODES.iter().any(|n| n.name.ends_with("tree-1/tree/leaves")));
    }

    #[test]
    fn camera_orbits_around_the_spawn() {
        let mut g = ShowcaseGame::new();
        let before = g.camera().eye;
        let mut input = InputState::default();
        input.stick = trino_core::Vec2::new(1.0, 0.0);
        g.update(&input, &mut NullAudio, 0.5);
        let after = g.camera().eye;
        assert!((after - before).dot(after - before) > 0.01);
    }

    struct NullAudio;
    impl Audio for NullAudio {
        fn play_sound(&mut self, _: trino_core::SoundId) {}
        fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
    }
}
