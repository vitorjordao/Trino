//! **Play in editor**: the open scene runs inside the Game view, in the
//! editor process — no build, no window switch, unsaved edits included.
//!
//! The scene (as currently edited, prefabs and materials resolved) is baked
//! to a `TSCN` blob in memory and handed to the scene runtime that ships
//! with the engine examples (`showcase::ShowcaseGame::from_baked`): rules by
//! **tag** — a `player`-tagged entity walks with physics against the
//! scene's colliders, `coin` triggers are collected, a `goal` trigger wins;
//! without a player the camera simply frames the scene. Real game crates
//! still run in their own process (Play ▸ Play the game) so a crash in game
//! code never takes the editor down.

use std::time::Instant;

use eframe::egui;
use trino_core::{BakedScene, Button, InputState, Vec2};

/// The editor plays silently for now (no audio device from the editor).
pub struct NullAudio;

impl trino_core::Audio for NullAudio {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// One running in-editor play session.
pub struct PlaySession {
    pub game: Box<showcase::ShowcaseGame>,
    pub last: Instant,
    pub frames: u64,
}

impl PlaySession {
    /// Bake `scene` against `lib` for `target` (the Game view's platform:
    /// its variants) and start the scene runtime at the Game view's
    /// internal resolution. The blob is leaked (a few KB per Play
    /// press — the runtime keeps `'static` names and tags, like consoles).
    pub fn start(
        scene: &trino_scene::Scene,
        lib: &trino_scene::Library,
        target: trino_scene::Target,
        screen: (u32, u32),
    ) -> Result<Self, String> {
        let (blob, warnings) = trino_scene::bake::to_tscn_for(scene, lib, target);
        if let Some(w) = warnings.first() {
            return Err(format!("cannot play: {w}"));
        }
        let blob: &'static [u8] = Box::leak(blob.into_boxed_slice());
        let baked = BakedScene::parse(blob).map_err(|e| format!("baked scene: {e:?}"))?;
        let mut game = Box::new(showcase::ShowcaseGame::from_baked(
            &baked,
            Vec2::new(screen.0 as f32, screen.1 as f32),
        ));
        // TSCN carries no AO (consoles bake theirs): the scene's own.
        game.ao = scene.environment.core_ao();
        game.stereo = scene.environment.core_stereo();
        Ok(PlaySession {
            game,
            last: Instant::now(),
            frames: 0,
        })
    }

    /// Seconds since the previous step, clamped like the PC app does.
    pub fn tick(&mut self) -> f32 {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;
        self.frames += 1;
        dt
    }
}

/// The PC keyboard mapping (`trino_platform_pc::PcInput`), from egui keys:
/// A/B = Z/X, X/Y = C/V, L/R = Q/E, Start = Enter, D-pad = arrows,
/// stick = WASD. Space doubles as A (jumping without hunting for Z).
pub fn input_from_keys(down: impl Fn(egui::Key) -> bool) -> InputState {
    use egui::Key;
    let mut s = InputState::default();
    for (key, button) in [
        (Key::Z, Button::A),
        (Key::Space, Button::A),
        (Key::X, Button::B),
        (Key::C, Button::X),
        (Key::V, Button::Y),
        (Key::Q, Button::L),
        (Key::E, Button::R),
        (Key::Enter, Button::Start),
        (Key::ArrowUp, Button::DpadUp),
        (Key::ArrowDown, Button::DpadDown),
        (Key::ArrowLeft, Button::DpadLeft),
        (Key::ArrowRight, Button::DpadRight),
    ] {
        if down(key) {
            s.set(button, true);
        }
    }
    let axis = |neg: Key, pos: Key| (down(pos) as i32 - down(neg) as i32) as f32;
    // Stick is Y-up like real hardware.
    s.stick = Vec2::new(axis(Key::A, Key::D), axis(Key::S, Key::W));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_like_the_pc_app() {
        let held = [egui::Key::Space, egui::Key::W, egui::Key::D, egui::Key::Q];
        let s = input_from_keys(|k| held.contains(&k));
        assert!(s.is_down(Button::A));
        assert!(s.is_down(Button::L));
        assert!(!s.is_down(Button::B));
        assert_eq!(s.stick, Vec2::new(1.0, 1.0));
    }

    #[test]
    fn plays_the_showcase_scene_in_memory() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scene = trino_scene::Scene::load(&root.join("scenes/showcase.scene.ron")).unwrap();
        let mut lib = trino_scene::Library::load(&root);
        lib.model_bounds = trino_asset_pipeline::manifest_model_bounds(&root.join("assets"));
        let mut s = PlaySession::start(&scene, &lib, trino_scene::Target::Pc, (320, 240)).unwrap();
        assert_eq!(s.game.total_coins, 5);
        let r = showcase::run_demo(&mut s.game, |g, input| {
            trino_core::Game::update(g, input, &mut NullAudio, 1.0 / 60.0)
        });
        assert!(r.is_ok(), "{r:?}");
    }
}
