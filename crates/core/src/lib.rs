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

pub mod access;
pub mod anim;
pub mod ao;
pub mod asset;
pub mod audio;
pub mod bundle;
pub mod caps;
pub mod collide;
pub mod determinism;
pub mod entity;
pub mod game;
pub mod group;
pub mod i18n;
pub mod impostor;
pub mod input;
pub mod light;
pub mod lz4;
pub mod math;
pub mod math3d;
pub mod mem;
pub mod memo;
pub mod netpad;
pub mod options;
pub mod pad;
pub mod particles;
pub mod perf;
pub mod physics;
pub mod platform;
pub mod players;
pub mod post;
pub mod precompile;
pub mod quality;
pub mod render;
pub mod render3d;
pub mod replay;
pub mod save;
pub mod scene;
pub mod services;
pub mod shadow;
pub mod snapshot;
pub mod stereo;
pub mod text;
pub mod threads;
pub mod tilemap;
pub mod trace;
pub mod ui;
pub mod version;
pub mod vis;
pub mod water;

pub use access::{
    AccessBlob, AccessData, AccessSettings, CameraFx, CaptionMode, CaptionPlace, CaptionStyle,
    Captions, CvdMode, FlashMonitor, HoldToggle, HostedAccess, Outline, ScreenFlash,
};
pub use anim::{AnimEvent, AnimGraph, AnimGraphInstance, EventBuf, EventId, GraphBlob, RootDelta};
pub use ao::{AmbientOcclusion, AoQuality, AoTier};
pub use asset::asset_id;

pub use audio::{
    Ambience, Audio, AudioStats, Bus, BusMixer, BusSet, BusSettings, DuckRule, Listener,
    MAX_DUCK_RULES, MixerConfig, MusicId, SoundBlob, SoundCodec, SoundId, SoundParams, SoundSource,
    VoiceId, spatialize,
};
pub use caps::{Caps, CapsError, Family, HwTier, TexelFormat};
pub use collide::{MoveResult, move_and_collide};
pub use entity::{Entity, EntityId, EntityStore};
pub use game::Game;
pub use group::{GroupId, GroupTable, LoadState, MemoryReport};
pub use i18n::{Arg, LangId, LangTag, Localizer, StrKey};
pub use impostor::Impostor;
pub use input::{Button, Input, InputState, Key, KeySet, PointerButton};
pub use light::{
    GroundPool, LightKind, LightList, LightTier, LocalLight, MAX_LOCAL_LIGHTS, MAX_VERTEX_LIGHTS,
    ground_pool,
};
pub use math::{Color, Rect, Vec2, Vec3};
pub use math3d::{Mat34, Quat};
pub use netpad::{NET_PAD_LEN, NetPad};
pub use options::{GraphicsOptions, OptionsEvent, OptionsRow, OptionsStyle};
pub use pad::{GlyphShape, PadFamily, PadGlyph, PadSlot};
pub use particles::{Effect, Emitter, ParticleSystem};
pub use physics::{
    Aabb, CastShape, Collider, LAYER_ALL, Shape, ShapeHit, Solid, StaticWorld, TriggerEvent,
    TriggerPhase, TriggerTracker, World,
};
pub use platform::{
    BindDevice, BlobId, DisplayInfo, DisplaySettings, GamepadStatus, InputInfo, Platform,
    SaveError, SaveFault, SaveStatus,
};
pub use players::{
    DeviceId, DeviceKind, HostedPlayers, InputDevice, Join, JoinEvent, MAX_DEVICES, MAX_PLAYERS,
    PlayerSetup, PlayersInput, PlayersRequest, Seats,
};
pub use post::{
    Bloom, Glow, GlowMode, Lut, LutError, LutId, PostSettings, PostTier, Tonemap, draw_glow,
};
pub use precompile::{PrecompileMode, PrecompileRequest, PrecompileStatus};
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
pub use replay::{Recorder, Replay, ReplayError, StateHasher};
pub use scene::{BakedScene, LodLevel, LodPick, Lods, SceneNode, Tags};
pub use services::{
    AchievementId, AchievementInfo, AchievementList, AchievementState, AchievementTable,
    AchievementToast, CrashConsent, CrashReports, HostedServices, OverlayStyle, Presence,
    ServiceBackend, ServicesInfo, StatId,
};
pub use shadow::{
    MAX_SHADOW_PLANES, ShadowCast, ShadowFrame, ShadowPlane, ShadowSettings, ShadowStats,
    ShadowTier,
};
pub use stereo::{
    Eye, LayerDepth, StereoFrame, StereoGovernor, StereoMono, StereoPolicy, StereoSettings,
    StereoStats,
};
pub use text::{BitmapFont, GlyphIndex, TextBuf};
pub use threads::ThreadSetting;
pub use tilemap::{TILE_SIZE, Tilemap, TilemapError};
pub use ui::{Insets, Ui, UiEnv, UiEvent, UiEvents, UiScreen, UiStats};
pub use version::BuildInfo;
pub use vis::{CellGroups, Cells, MAX_CELLS, NO_CELL, Portal, VisVolume};
pub use water::{MAX_RIPPLES, Ripple, RippleBuffer, Water, WaterState, WaterTier};
