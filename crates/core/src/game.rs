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

    /// A hash of the simulation state (issue #67): everything `update`
    /// reads and changes that decides what happens next — positions,
    /// velocities, timers, scores, the RNG — and nothing render-only.
    /// Recordings store it every few frames and replays compare it
    /// ([`crate::replay`]); network sessions exchange it to find a desync.
    /// Hash with [`StateHasher`](crate::StateHasher), which is the same on
    /// every target. `None` (the default): the game can be recorded and
    /// replayed, but nothing is checked.
    fn state_hash(&self) -> Option<u64> {
        None
    }

    /// The bytes [`save_state`](Self::save_state) needs (0: the game takes
    /// no snapshots, the default).
    fn state_size(&self) -> usize {
        0
    }

    /// Write a snapshot of the simulation state into `out` for a rollback
    /// session (issue #67): everything `update` changes, so that
    /// [`load_state`](Self::load_state) of it followed by the same updates
    /// gives the same frames. Returns the bytes written; `None` when the
    /// game takes no snapshots (the default) or `out` is shorter than
    /// [`state_size`](Self::state_size). Snapshots stay on the machine that
    /// took them (sessions exchange inputs and hashes, never states), so a
    /// `Copy` struct may be copied byte for byte
    /// ([`snapshot::save_copy`](crate::snapshot::save_copy)).
    fn save_state(&self, out: &mut [u8]) -> Option<usize> {
        let _ = out;
        None
    }

    /// Restore a [`save_state`](Self::save_state) snapshot. `false` when the
    /// game takes no snapshots or `bytes` is not one of its own.
    fn load_state(&mut self, bytes: &[u8]) -> bool {
        let _ = bytes;
        false
    }

    /// Turn a save an older version of the game wrote into the current
    /// version, in place (issue #68): `data[..len]` holds version `from`'s
    /// bytes (0 = a save from before the game tagged its versions); return
    /// the current version's length (it may grow up to `data.len()`), or
    /// `None` to start from defaults (the default). Called by
    /// [`save::load_game`](crate::save::load_game), which reads the tag
    /// [`save::write_versioned`](crate::save::write_versioned) wrote.
    fn migrate_save(&mut self, from: u16, data: &mut [u8], len: usize) -> Option<usize> {
        let _ = (from, data, len);
        None
    }

    /// The picture changed size (PC: window resized, fullscreen toggled,
    /// pixel scale changed). `screen` is the new internal resolution, the
    /// same space as `InputState::pointer`. Consoles never call it.
    fn resize(&mut self, screen: crate::Vec2) {
        let _ = screen;
    }
}
