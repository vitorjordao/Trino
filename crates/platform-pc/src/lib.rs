//! # trino-platform-pc
//!
//! The PC backend: implements `trino_core`'s `Renderer` (wgpu), `Audio`
//! (cpal) and `Input` (keyboard via winit keycodes).
//!
//! Rendering always goes through an internal offscreen framebuffer at the
//! resolution of the active [`SimProfile`] (320x240 for N64, 400x240 for
//! 3DS), then blits to the window with nearest-neighbor integer upscaling —
//! the foundation of the console-simulation modes and of golden-image tests
//! (which read the offscreen target back instead of presenting it).

pub mod audio;
pub mod input;
pub mod renderer;
pub mod sim;

pub use audio::PcAudio;
pub use input::PcInput;
pub use renderer::{PcRenderer, RendererError};
pub use sim::SimProfile;
