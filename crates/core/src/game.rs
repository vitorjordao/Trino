//! The game loop contract. `apps/*` glue drives this: poll input, `update`,
//! then `render`, at the platform's frame rate.

use crate::input::InputState;
use crate::render::Renderer;

pub trait Game {
    /// Advance simulation by `dt` seconds using this frame's input.
    /// Must stay deterministic: same inputs + same state = same result
    /// (verified by tests from Fase 6 on).
    fn update(&mut self, input: &InputState, dt: f32);

    /// Issue draw calls. Must not mutate gameplay state.
    fn render(&mut self, renderer: &mut dyn Renderer);
}
