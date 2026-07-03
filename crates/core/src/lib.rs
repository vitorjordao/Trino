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

pub mod audio;
pub mod caps;
pub mod game;
pub mod input;
pub mod math;
pub mod platform;
pub mod render;

pub use audio::{Audio, MusicId, SoundId};
pub use caps::{Caps, CapsError};
pub use game::Game;
pub use input::{Button, Input, InputState};
pub use math::{Color, Rect, Vec2, Vec3};
pub use platform::Platform;
pub use render::{Material, MaterialId, ModelId, Renderer, SpriteId, SpriteParams, Transform3};
