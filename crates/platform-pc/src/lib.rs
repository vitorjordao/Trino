//! # trino-platform-pc
//!
//! The PC backend: implements `trino_core`'s `Renderer` (wgpu, through the
//! renderer `trino-render-wgpu` shares with the Web backend), `Audio`
//! (cpal), `Input` (keyboard via winit keycodes) and `Platform` (clock,
//! blobs, save file).
//!
//! Rendering always goes through an internal offscreen framebuffer at the
//! resolution of the active [`SimProfile`] (320x240 for N64, 400x240 for
//! 3DS), then blits to the window with nearest-neighbor integer upscaling —
//! the foundation of the console-simulation modes and of golden-image tests
//! (which read the offscreen target back instead of presenting it).

pub mod audio;
pub mod input;
pub mod language;
pub mod system;
pub mod tnl_oracle;

/// The wgpu renderer (`trino-render-wgpu`), at its historical paths:
/// `trino_platform_pc::renderer::…` and `trino_platform_pc::sim::…` (plus
/// the quality's `dynres`, `pacing` and `quality`, issue #55).
pub use trino_render_wgpu::{dynres, frame_report, pacing, quality, renderer, sim};

pub use audio::PcAudio;
pub use frame_report::FrameMeter;
pub use input::PcInput;
pub use renderer::{
    AoView, Calibration, CompressedFormat, MSAA_SAMPLES, N64Estimate, PcFrameTimes, PcRenderer,
    RendererError, ShadowView, SpriteMemory,
};
pub use sim::SimProfile;
pub use system::PcPlatform;
