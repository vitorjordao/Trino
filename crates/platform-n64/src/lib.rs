//! # trino-platform-n64
//!
//! Nintendo 64 backend. Implements `trino_core`'s traits over a thin C shim
//! (`shim/trino_shim.c`) compiled against **pinned libdragon** inside the
//! Docker toolchain image (`docker/n64/Dockerfile`).
//!
//! Build flow (`cargo xtask build n64`):
//! 1. Docker image compiles the shim and exports `libdragon.a` + `n64.ld`
//!    into `target/n64/`.
//! 2. Host `cargo +nightly -Zbuild-std=core,alloc` builds `trino-app-n64`
//!    for `platforms/n64/mips-nintendo64-none.json`; rust-lld links the
//!    shim + libdragon into the ELF.
//! 3. The image's `n64sym`/`n64elfcompress`/`n64tool` turn the ELF + DFS
//!    into a bootable `.z64`.
//!
//! Assets arrive through the DFS: `cargo xtask assets n64` bakes masters
//! with `mksprite`/`audioconv64` (names = `<handle-hex>.sprite/.wav64`) plus
//! an `index.tsv` mapping handles to files, which [`N64Assets::load_all`]
//! reads at boot.

// On any non-N64 target this crate compiles to an empty lib so that plain
// `cargo build --workspace` on a dev machine stays green; the real code is
// gated on `target_os = "none"` (the mips-nintendo64-none target).
#![no_std]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
pub mod ffi;

#[cfg(target_os = "none")]
mod assets;
#[cfg(target_os = "none")]
mod audio;
#[cfg(target_os = "none")]
mod input;
#[cfg(target_os = "none")]
mod renderer;
#[cfg(target_os = "none")]
pub mod runtime;

#[cfg(target_os = "none")]
pub use assets::N64Assets;
#[cfg(target_os = "none")]
pub use audio::N64Audio;
#[cfg(target_os = "none")]
pub use input::N64Input;
#[cfg(target_os = "none")]
pub use renderer::N64Renderer;
