//! The game loop contract. `apps/*` glue drives this: poll input, `update`,
//! then `render`, at the platform's frame rate.

use crate::audio::Audio;
use crate::input::InputState;
use crate::platform::Platform;
use crate::render::Renderer;

pub trait Game {
    /// Advance simulation by `dt` seconds using this frame's input.
    /// Must stay deterministic: same inputs + same state = same result
    /// (verified by tests from Fase 6 on). Audio calls are fire-and-forget
    /// and do not affect determinism.
    fn update(&mut self, input: &InputState, audio: &mut dyn Audio, dt: f32);

    /// What the apps actually call each frame: [`update`](Self::update)
    /// plus the platform services (data blobs, save data, clock, log).
    /// Override this instead when the game needs them; the default ignores
    /// the platform. Same determinism rules as `update`.
    fn update_with(
        &mut self,
        platform: &mut dyn Platform,
        input: &InputState,
        audio: &mut dyn Audio,
        dt: f32,
    ) {
        let _ = platform;
        self.update(input, audio, dt);
    }

    /// Issue draw calls, bracketed by `begin_frame`/`end_frame`.
    /// Must not mutate gameplay state.
    fn render(&mut self, renderer: &mut dyn Renderer);

    /// The picture changed size (PC: window resized, fullscreen toggled,
    /// pixel scale changed). `screen` is the new internal resolution, the
    /// same space as `InputState::pointer`. Consoles never call it.
    fn resize(&mut self, screen: crate::Vec2) {
        let _ = screen;
    }
}
