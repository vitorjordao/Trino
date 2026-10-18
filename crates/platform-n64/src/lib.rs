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
mod host;
#[cfg(target_os = "none")]
mod input;
#[cfg(any(target_os = "none", test))]
mod pad;
#[cfg(target_os = "none")]
mod renderer;
#[cfg(target_os = "none")]
mod rsp;
#[cfg(target_os = "none")]
pub mod runtime;
#[cfg(target_os = "none")]
mod system;

#[cfg(target_os = "none")]
pub use assets::N64Assets;
#[cfg(target_os = "none")]
pub use audio::{AudioCost, N64Audio};
#[cfg(target_os = "none")]
pub use host::{FrameReport, N64Host};
#[cfg(target_os = "none")]
pub use input::N64Input;
#[cfg(target_os = "none")]
pub use renderer::N64Renderer;
#[cfg(target_os = "none")]
pub use system::N64Platform;

/// Generate `trino_rust_main` (the symbol the C shim's `main` calls) for a
/// game: `n64_main!(MyGame::new)` or `n64_main!(MyGame::new, self_test)`,
/// where `MyGame::new: fn(Vec2) -> MyGame` and
/// `self_test: fn(&mut N64Host, &mut MyGame) -> Result<(), String>`; an
/// optional third `setup: fn(&mut N64Host)` runs after boot (renderer
/// options such as `renderer.set_rsp_stream(true)`).
/// Expands to nothing on other targets, so the crate builds everywhere.
#[macro_export]
macro_rules! n64_main {
    ($make:expr) => {
        $crate::n64_main!($make, |_: &mut $crate::N64Host, _: &mut _| Ok(()));
    };
    ($make:expr, $self_test:expr) => {
        $crate::n64_main!($make, $self_test, |_: &mut $crate::N64Host| {});
    };
    ($make:expr, $self_test:expr, $setup:expr) => {
        #[cfg(target_os = "none")]
        #[unsafe(no_mangle)]
        pub extern "C" fn trino_rust_main() {
            $crate::N64Host::main($make, $self_test, $setup)
        }
    };
}
