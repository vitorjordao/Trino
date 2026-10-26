//! # trino-game-api
//!
//! The boundary for live code reload (PC dev builds). The game crate
//! compiles as both `rlib` (static link: consoles, release) and `dylib`
//! (hot reload); the host owns the game state and calls exported functions
//! that take `&mut State`, so the state survives library swaps.
//!
//! ## Boundary rules (violating these is undefined behavior)
//!
//! - State layout must not change between reloads — layout changes require
//!   a restart. Bump [`GAME_API_VERSION`] on any signature change; the host
//!   checks `trino_game_api_version()` after every reload and refuses
//!   mismatches. A new trait method *with a default* (`Renderer`,
//!   `Platform`) does not bump it: the host and the dylib always build
//!   against the same `trino-core` (a core change needs a restart anyway,
//!   `cargo xtask watch pc` only watches `examples/`), and no existing
//!   export's signature changes. `second_screen`/`set_screen` and the typed
//!   screen methods followed this rule.
//! - No statics in the game dylib (they reset on reload) and no `TypeId`
//!   across the boundary (it changes per compilation).
//! - Exports take references only; nothing owned crosses the boundary.
//!   State construction is host-side via the statically-linked crate.
//!
//! Use [`export_game!`] in the game crate to generate the exports from a
//! `trino_core::Game` implementation.

#![no_std]

/// Version handshake between host and game dylib. Bump on ANY change to the
/// exported function signatures or to types crossing the boundary.
/// 3: `InputState` gained the pointer (`pointer`, pointer buttons, `wheel`).
/// 4: `InputState` gained the full keyboard (`keys` is a 128-bit `KeySet`),
/// `right_stick`, `triggers` and the L2/R2/L3/R3 buttons (a `u32` mask).
/// 5: the state exports for recordings and rollback (issue #67):
/// `trino_game_state_hash`, `trino_game_state_size`, `trino_game_save_state`,
/// `trino_game_load_state`.
pub const GAME_API_VERSION: u32 = 5;

/// Generates the hot-reload exports for a game state type implementing
/// `trino_core::Game`:
///
/// ```ignore
/// pub struct MyGame { /* ... */ }
/// impl trino_core::Game for MyGame { /* ... */ }
/// trino_game_api::export_game!(MyGame);
/// ```
///
/// Exports: `trino_game_api_version`, `trino_game_update`,
/// `trino_game_render`, and the state a recording checks and a rollback
/// session snapshots (`trino_game_state_hash`, `trino_game_state_size`,
/// `trino_game_save_state`, `trino_game_load_state`).
#[macro_export]
macro_rules! export_game {
    ($state:ty) => {
        #[unsafe(no_mangle)]
        pub fn trino_game_api_version() -> u32 {
            $crate::GAME_API_VERSION
        }

        #[unsafe(no_mangle)]
        pub fn trino_game_update(
            state: &mut $state,
            platform: &mut dyn ::trino_core::Platform,
            input: &::trino_core::InputState,
            audio: &mut dyn ::trino_core::Audio,
            dt: f32,
        ) {
            ::trino_core::Game::update_with(state, platform, input, audio, dt)
        }

        #[unsafe(no_mangle)]
        pub fn trino_game_render(state: &mut $state, renderer: &mut dyn ::trino_core::Renderer) {
            ::trino_core::Game::render(state, renderer)
        }

        #[unsafe(no_mangle)]
        pub fn trino_game_state_hash(state: &$state) -> Option<u64> {
            ::trino_core::Game::state_hash(state)
        }

        #[unsafe(no_mangle)]
        pub fn trino_game_state_size(state: &$state) -> usize {
            ::trino_core::Game::state_size(state)
        }

        #[unsafe(no_mangle)]
        pub fn trino_game_save_state(state: &$state, out: &mut [u8]) -> Option<usize> {
            ::trino_core::Game::save_state(state, out)
        }

        #[unsafe(no_mangle)]
        pub fn trino_game_load_state(state: &mut $state, bytes: &[u8]) -> bool {
            ::trino_core::Game::load_state(state, bytes)
        }
    };
}
