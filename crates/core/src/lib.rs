//! # trino-core
//!
//! Platform-agnostic contracts for the Trino engine.
//!
//! Everything a game or a platform backend needs to talk to each other lives
//! here: math primitives, asset handles, the [`render::Renderer`],
//! [`audio::Audio`], [`input`] and [`platform::Platform`] traits, and the
//! per-console capability tables in [`caps`].
//!
//! ## Rules
//!
//! - `no_std`, zero dependencies. This crate compiles for N64 (MIPS bare
//!   metal), 3DS (armv6k) and PC without change.
//! - Nothing from libdragon, ctru, wgpu or any other backend may leak into
//!   these types. Backends implement the traits; games consume them.
//! - The N64 is the design ceiling: if a feature cannot be expressed on N64
//!   hardware, it does not get a trait method here.

#![cfg_attr(not(test), no_std)]

pub mod ao;
pub mod asset;
pub mod audio;
pub mod bundle;
pub mod caps;
pub mod collide;
pub mod entity;
pub mod game;
pub mod group;
pub mod impostor;
pub mod input;
pub mod light;
pub mod lz4;
pub mod math;
pub mod math3d;
pub mod mem;
pub mod memo;
pub mod options;
pub mod particles;
pub mod perf;
pub mod physics;
pub mod platform;
pub mod quality;
pub mod render;
pub mod render3d;
pub mod save;
pub mod scene;
pub mod shadow;
pub mod text;
pub mod tilemap;
pub mod vis;
pub mod water;

pub use ao::{AmbientOcclusion, AoQuality, AoTier};
pub use asset::asset_id;

pub use audio::{
    Ambience, Audio, AudioStats, Bus, BusMixer, BusSet, BusSettings, DuckRule, Listener,
    MAX_DUCK_RULES, MixerConfig, MusicId, SoundBlob, SoundCodec, SoundId, SoundParams, SoundSource,
    VoiceId, spatialize,
};
pub use caps::{Caps, CapsError, HwTier};
pub use collide::{MoveResult, move_and_collide};
pub use entity::{Entity, EntityId, EntityStore};
pub use game::Game;
pub use group::{GroupId, GroupTable, LoadState, MemoryReport};
pub use impostor::Impostor;
pub use input::{Button, Input, InputState, Key, KeySet, PointerButton};
pub use light::{
    GroundPool, LightKind, LightList, LightTier, LocalLight, MAX_LOCAL_LIGHTS, MAX_VERTEX_LIGHTS,
    ground_pool,
};
pub use math::{Color, Rect, Vec2, Vec3};
pub use math3d::{Mat34, Quat};
pub use options::{GraphicsOptions, OptionsEvent, OptionsRow, OptionsStyle};
pub use particles::{Effect, Emitter, ParticleSystem};
pub use physics::{Aabb, Collider, Shape};
pub use platform::{
    BindDevice, BlobId, DisplayInfo, DisplaySettings, GamepadStatus, InputInfo, Platform, SaveError,
};
pub use quality::{
    AoSetting, DynRes, FrameLimit, GpuClass, PostSetting, PresetTable, QualityInfo, QualityPreset,
    QualitySettings, QualitySource, ShadowSetting,
};
pub use render::{
    AnimBlend, AnimRef, BillboardParams, ClipId, DetailTexture, FontId, FrameStats, Instance,
    Material, MaterialId, ModelId, ModelParams, Palette, Renderer, Screen, Shading, SpriteId,
    SpriteParams, TexFilter, TextAlign, TextParams, Transform3,
};
pub use render3d::{
    Camera3, ClipInfo, DEFAULT_LIGHT, Fog, Light, MAX_JOINTS, Mesh, MeshError, MeshLayout,
    PcGpuReject, RspReject, ScreenTri, SurfaceScratch, SurfaceVert, TnlContext, TnlScratch,
    TnlStats, tessellate, tessellate_surface, tessellate_with,
};
pub use scene::{BakedScene, LodLevel, LodPick, Lods, SceneNode, Tags};
pub use shadow::{
    MAX_SHADOW_PLANES, ShadowCast, ShadowFrame, ShadowPlane, ShadowSettings, ShadowStats,
    ShadowTier,
};
pub use text::{BitmapFont, GlyphIndex, TextBuf};
pub use tilemap::{TILE_SIZE, Tilemap, TilemapError};
pub use vis::{CellGroups, Cells, MAX_CELLS, NO_CELL, Portal, VisVolume};
pub use water::{MAX_RIPPLES, Ripple, RippleBuffer, Water, WaterState, WaterTier};
