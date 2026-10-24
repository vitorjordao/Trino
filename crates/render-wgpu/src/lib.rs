//! # trino-render-wgpu
//!
//! The engine's wgpu renderer, shared by two backends: `trino-platform-pc`
//! (a winit window on Vulkan/Metal/DX12/GL) and `trino-platform-web` (a
//! canvas on WebGPU, or WebGL2 where the browser has no WebGPU).
//!
//! Rendering always goes through an internal offscreen framebuffer at the
//! resolution of the active [`SimProfile`] (320x240 for N64, 400x240 for
//! 3DS), then blits to the window or canvas with nearest-neighbor integer
//! upscaling — the foundation of the console-simulation modes and of
//! golden-image tests (which read the offscreen target back instead of
//! presenting it).

pub mod dynres;
pub mod frame_report;
pub mod pacing;
pub mod quality;
pub mod renderer;
pub mod sim;
pub mod trace_host;
#[cfg(all(feature = "tracy", not(target_arch = "wasm32")))]
pub mod tracy;

pub use frame_report::{FrameMeter, QualityNote};
pub use renderer::{
    AoView, Bench, Calibration, CompressedFormat, MSAA_SAMPLES, N64Estimate, PcFrameTimes,
    PcRenderer, PendingRead, RendererError, ShadowView, SpriteMemory, WebBackend,
};
pub use sim::SimProfile;
