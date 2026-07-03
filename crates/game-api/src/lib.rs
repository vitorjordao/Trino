//! # trino-game-api
//!
//! The ABI boundary for live code reload (Fase 2). On PC (dev builds only),
//! the game crate compiles as a `dylib` and the host swaps it at runtime via
//! `hot-lib-reloader`. This crate owns every type that crosses that boundary.
//!
//! ## Boundary rules (violating these is undefined behavior)
//!
//! - Everything crossing the boundary is `#[repr(C)]` or an opaque pointer.
//! - No generics in exported function signatures.
//! - Game state is **owned by the host** and passed in as `&mut`; the dylib
//!   never keeps state in statics (statics reset on every reload).
//! - `TypeId` and thread-locals do not survive reloads — identify types by
//!   name/hash, never by `TypeId`.
//! - On layout changes the state is serialized before reload and migrated
//!   after (`on_before_reload` / `on_after_reload`).
//!
//! Console and release builds link the game statically; this crate is then
//! just a thin, zero-cost pass-through.
//!
//! The concrete function table (`init` / `update` / `render` / reload hooks)
//! lands in Fase 2 together with the host side. Bump [`GAME_API_VERSION`] on
//! every breaking change to the boundary; host and dylib refuse to link on
//! mismatch.

#![no_std]

/// Version handshake between host and game dylib.
pub const GAME_API_VERSION: u32 = 0;
