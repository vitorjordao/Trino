//! Deterministic performance scenes: the same game states measured on every
//! target. A scene is a level plus a fixed number of idle frames (gravity
//! settles the player, enemies and movers reach a known pose), so its draw
//! list — and the engine's T&L work — is identical on PC, in ares and in
//! Azahar.
//!
//! - **PC** (`xtask/tests/perf_budget.rs`, in `cargo test --workspace`)
//!   pins the deterministic T&L work of every scene in
//!   `tests/perf/castle64_work.toml`.
//! - **N64** times the scenes on its own clock (`TRINO_PERF_SCENE:` /
//!   `TRINO_PERF_MODEL:` lines, see `trino_core::perf`); `cargo xtask test
//!   n64` requires the same work counters as PC (cross-target determinism)
//!   and the times under the ceilings in `tests/perf/castle64_n64.toml`.
//!
//! Each scene starts from a fresh game: progress (coins, stars) changes
//! what a level draws, and the self-tests measure after the bot played.

use crate::Castle64Game;
use trino_core::{Audio, Game, InputState, Vec2};

/// One measured game state.
pub struct PerfScene {
    /// Report name (no spaces).
    pub name: &'static str,
    pub level: usize,
    /// Idle frames simulated before measuring.
    pub settle: u32,
}

/// Every castle64 level at its spawn: the hub (castle doorway, a boar,
/// many blocks), the three stages (boars, movers, lava) and the throne.
pub const SCENES: &[PerfScene] = &[
    PerfScene {
        name: "hub",
        level: 0,
        settle: 60,
    },
    PerfScene {
        name: "green-hills",
        level: 1,
        settle: 60,
    },
    PerfScene {
        name: "lava-land",
        level: 2,
        settle: 60,
    },
    PerfScene {
        name: "sky-tower",
        level: 3,
        settle: 60,
    },
    PerfScene {
        name: "throne",
        level: 4,
        settle: 60,
    },
];

/// Frames measured per scene (each one update + render).
pub const MEASURE_FRAMES: u32 = 4;

/// Fixed timestep of the scenes.
pub const DT: f32 = 1.0 / 60.0;

/// A fresh game in the scene's state: enter the level, then `settle` idle
/// updates (no render — callers render the measured frames only, each one
/// an idle update + render).
pub fn scene_game(scene: &PerfScene, screen: Vec2, audio: &mut dyn Audio) -> Castle64Game {
    let mut game = Castle64Game::new(screen);
    game.goto_level(scene.level);
    let idle = InputState::default();
    for _ in 0..scene.settle {
        game.update(&idle, audio, DT);
    }
    game
}
