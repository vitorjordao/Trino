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
//!   mismatches.
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
pub const GAME_API_VERSION: u32 = 1;

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
/// `trino_game_render`.
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
            input: &::trino_core::InputState,
            audio: &mut dyn ::trino_core::Audio,
            dt: f32,
        ) {
            ::trino_core::Game::update(state, input, audio, dt)
        }

        #[unsafe(no_mangle)]
        pub fn trino_game_render(state: &mut $state, renderer: &mut dyn ::trino_core::Renderer) {
            ::trino_core::Game::render(state, renderer)
        }
    };
}
