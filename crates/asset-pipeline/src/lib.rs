//! # trino-asset-pipeline
//!
//! Bakes master assets into per-platform native data, shared by `xtask`,
//! the PC app (bake-on-start in dev) and the editor (Fase 3).
//!
//! ## Model
//!
//! - Masters live in `assets/shared/`; `assets/<platform>/` holds overrides.
//!   Resolution rule: override wins, else shared, else **error** — never a
//!   silent fallback.
//! - `assets/manifest.toml` declares every asset under a **logical path**
//!   (`sprites/player`). The handle is `trino_core::asset_id(logical_path)`,
//!   computed identically at compile time (games), bake time (here) and load
//!   time (platforms). The bake fails on hash collisions.
//! - Formats are per-platform; a format the target cannot represent is a
//!   bake error (e.g. RGBA8 texture on N64).
//!
//! Baked output goes to `<out_dir>/` as `<hash-hex>.sprite` / `.sound`
//! plus `index.toml` (the manifest of what was baked, used by loaders and
//! by snapshot tests).

pub mod bake;
pub mod loader;
pub mod manifest;
pub mod resolve;
#[cfg(feature = "watch")]
pub mod watch;

pub use bake::{BakeError, BakeReport, bake_all};
pub use loader::{LoadedAssets, LoadedSound, LoadedSprite, load_dir};
pub use manifest::{Manifest, Platform};
pub use resolve::resolve_source;
