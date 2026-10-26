//! # trino-platform-web
//!
//! The Web backend (issue #47): Trino games in a browser tab, from one
//! `wasm32-unknown-unknown` build.
//!
//! - **Rendering:** the PC's own renderer (`trino-render-wgpu`, the same
//!   code, shaders and T&L paths) on a `<canvas>`: WebGPU where the browser
//!   has it, WebGL2 elsewhere ([`trino_render_wgpu::WebBackend`]). The
//!   canvas follows its element's size through a `ResizeObserver` at
//!   `devicePixelRatio`, so Retina screens get whole device pixels; the
//!   picture is the native profile's (the output over an integer pixel
//!   scale, `sim::native_internal`).
//! - **Platform:** `performance.now()` clock, the console (styled tags) and
//!   the page's log for `log`, blobs from the asset pack, saves in
//!   `localStorage` (base64, capped at the profile's `Caps::save_bytes`).
//! - **Input:** the keyboard by `KeyboardEvent.code` (layout-independent,
//!   the PC's bindings; game keys don't scroll the page), mouse with
//!   Pointer Lock, touch as the pointer, gamepads through the Gamepad API.
//! - **Audio:** Web Audio, every voice its own source → gain → panner →
//!   its bus (music, effects, voice, ambience, UI) → master, with the
//!   core's bus volumes, mutes and duck rules (#53). Sounds come Ogg
//!   Vorbis (or PCM16) and are decoded by lewton in the wasm module
//!   ([`sound`]); music streams in chunks scheduled a second ahead. The
//!   player's bus volumes persist in `localStorage`. Browsers start audio
//!   suspended: the first click, key or touch resumes it, and a banner says
//!   so until then.
//! - **Quality (#55):** the PC's presets on the Web: `?quality=` forces
//!   one, else the player's choice (`localStorage`), else a phone's or a
//!   tablet's browser gets the project's Low ([`device`]) and a desktop's
//!   the project's default — `auto`, a calibration of at most 2 s drawn
//!   over the first animation frames. The render scale, dynamic
//!   resolution (on the frame intervals), MSAA, AO and the frame cap (an
//!   animation frame skipped) apply as on the PC.
//! - **Assets:** one pack per game ([`pack`]), fetched at boot; KTX2
//!   textures are transcoded by Binomial's transcoder to BC, ETC2, ASTC or
//!   RGBA8, whichever the GPU samples. Asset groups besides `boot`
//!   (issue #56) are bundles beside the pack, downloaded when the game
//!   loads them and decoded a slice per frame ([`WebHost::pump_groups`]).
//!
//! Off wasm32 only the browser-free parts build ([`keys`], [`pack`],
//! [`b64`], [`sound`], [`device`]): the host tests run them, and `cargo xtask build
//! web` writes packs with [`pack::write`].

pub mod b64;
pub mod device;
pub mod keys;
pub mod language;
pub mod pack;
pub mod sound;

#[cfg(target_arch = "wasm32")]
mod assets;
#[cfg(target_arch = "wasm32")]
mod audio;
#[cfg(target_arch = "wasm32")]
mod groups;
#[cfg(target_arch = "wasm32")]
mod input;
#[cfg(target_arch = "wasm32")]
pub mod js;
#[cfg(target_arch = "wasm32")]
mod platform;
#[cfg(target_arch = "wasm32")]
mod quality;
#[cfg(target_arch = "wasm32")]
pub mod replay;
#[cfg(target_arch = "wasm32")]
mod runtime;

#[cfg(target_arch = "wasm32")]
pub use assets::{LoadReport, TextureTarget};
#[cfg(target_arch = "wasm32")]
pub use audio::WebAudio;
#[cfg(target_arch = "wasm32")]
pub use input::WebInput;
#[cfg(target_arch = "wasm32")]
pub use platform::WebPlatform;
#[cfg(target_arch = "wasm32")]
pub use quality::{BENCH_BUDGET, BENCH_TARGET_US};
#[cfg(target_arch = "wasm32")]
pub use runtime::{
    Flow, Query, WebConfig, WebHost, fail, fetch, install_panic_hook, run, run_game,
};
#[cfg(target_arch = "wasm32")]
pub use trino_render_wgpu::{PcRenderer, SimProfile, WebBackend};
