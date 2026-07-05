//! # trino-platform-3ds
//!
//! Nintendo 3DS backend. Implements `trino_core`'s traits over a thin C shim
//! (`shim/trino_shim_3ds.c`) compiled against the locally installed
//! devkitPro (libctru + citro2d/citro3d).
//!
//! Build flow (`cargo xtask build 3ds`):
//! 1. devkitARM's `arm-none-eabi-gcc` compiles the shim to `target/3ds/shim.o`.
//! 2. Host `cargo +nightly -Zbuild-std=core,alloc` builds `trino-app-3ds`
//!    for the built-in `armv6k-nintendo-3ds` target; the target's own
//!    linker recipe (arm-none-eabi-gcc + 3dsx.specs) links the shim +
//!    libcitro2d/libcitro3d/libctru into the ELF.
//! 3. `3dsxtool` turns the ELF + RomFS into a `.3dsx` for Azahar/hardware.
//!
//! Assets arrive through the RomFS: `cargo xtask assets 3ds` bakes masters
//! with `tex3ds` (sprites, names = `<handle-hex>.t3x`) and a raw-PCM16
//! conversion (sounds, `<handle-hex>.pcm16`) plus an `index.tsv` mapping
//! handles to files, which [`N3dsAssets::load_all`] reads at boot.

// On any non-3DS target this crate compiles to an empty lib so that plain
// `cargo build --workspace` on a dev machine stays green; the real code is
// gated on `target_os = "horizon"` (the armv6k-nintendo-3ds target).
#![no_std]

#[cfg(target_os = "horizon")]
extern crate alloc;

#[cfg(target_os = "horizon")]
pub mod ffi;

#[cfg(target_os = "horizon")]
mod assets;
#[cfg(target_os = "horizon")]
mod audio;
#[cfg(target_os = "horizon")]
mod input;
#[cfg(target_os = "horizon")]
mod renderer;
#[cfg(target_os = "horizon")]
pub mod runtime;

#[cfg(target_os = "horizon")]
pub use assets::N3dsAssets;
#[cfg(target_os = "horizon")]
pub use audio::N3dsAudio;
#[cfg(target_os = "horizon")]
pub use input::N3dsInput;
#[cfg(target_os = "horizon")]
pub use renderer::N3dsRenderer;
