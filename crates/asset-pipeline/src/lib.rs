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

pub mod access;
pub mod anim;
pub mod anim_compress;
pub mod anim_graph;
pub mod ao;
pub mod audio;
pub mod bake;
pub mod collision;
pub mod font;
pub mod groups;
pub mod i18n;
pub mod impostor;
pub mod loader;
pub mod lut;
pub mod lz4;
pub mod manifest;
pub mod memory;
pub mod model;
pub mod net;
pub mod par;
pub mod particles;
pub mod primitive;
pub mod quality;
pub mod resolve;
pub mod services;
pub mod ship;
pub mod texture;
pub mod tier;
pub mod ui;
#[cfg(feature = "watch")]
pub mod watch;

pub use ao::{AoBake, AoDecl, bake_vertex_ao};
pub use bake::{BakeError, BakeReport, bake_all, bake_all_with};
pub use bake::{decode_master, decode_png};
pub use font::{BakedFont, FontDecl, bake_font};
pub use groups::{BundleBuilder, Compression, GroupDecl};
pub use loader::{
    BundleLoad, CompressedSprite, LoadedAssets, LoadedBlob, LoadedFont, LoadedModel, LoadedSound,
    LoadedSprite, load_boot, load_dir,
};
pub use manifest::{Manifest, Platform};
pub use model::{
    MeshData, ModelOptions, bake_model, bake_model_tmdl, bake_scene, bake_scene_for, clip_names,
    compression_report, manifest_model_bounds,
};
pub use primitive::{Pivot, Primitive, Shape};
pub use resolve::resolve_source;
pub use texture::{BcFormat, N3dsFormat, N64Format};
pub use tier::{TierBlob, tier_twins};
