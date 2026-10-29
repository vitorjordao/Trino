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
//! with `tex3ds` (sprites, names = `<handle-hex>.t3x`), sounds to `T3AU`
//! (DSP-ADPCM or PCM16, `<handle-hex>.t3au`; music streams from the RomFS)
//! and the project's mix (`<handle-hex>.mixer`), plus an `index.tsv` mapping
//! handles to files, which [`N3dsAssets::load_all`] reads at boot.

// On any non-3DS target this crate compiles to an empty lib so that plain
// `cargo build --workspace` on a dev machine stays green; the real code is
// gated on `target_os = "horizon"` (the armv6k-nintendo-3ds target).
#![no_std]

#[cfg(any(target_os = "horizon", test))]
extern crate alloc;

#[cfg(target_os = "horizon")]
pub mod ffi;

#[cfg(target_os = "horizon")]
mod assets;
#[cfg(target_os = "horizon")]
mod audio;
#[cfg(target_os = "horizon")]
mod host;
#[cfg(any(target_os = "horizon", test))]
pub mod hw;
#[cfg(target_os = "horizon")]
mod input;
#[cfg(any(target_os = "horizon", test))]
pub mod language;
#[cfg(any(target_os = "horizon", test))]
mod pad;
#[cfg(target_os = "horizon")]
mod renderer;
#[cfg(target_os = "horizon")]
pub mod runtime;
#[cfg(any(target_os = "horizon", test))]
pub mod savedir;
#[cfg(target_os = "horizon")]
mod stereo;
#[cfg(target_os = "horizon")]
mod system;
#[cfg(any(target_os = "horizon", test))]
pub mod worker;

#[cfg(target_os = "horizon")]
pub use assets::N3dsAssets;
#[cfg(target_os = "horizon")]
pub use audio::N3dsAudio;
#[cfg(target_os = "horizon")]
pub use host::{FrameReport, N3dsHost, off_gpu_list};
#[cfg(target_os = "horizon")]
pub use input::N3dsInput;
#[cfg(target_os = "horizon")]
pub use renderer::N3dsRenderer;
#[cfg(target_os = "horizon")]
pub use system::N3dsPlatform;

/// Generate `trino_rust_main` (the symbol the C shim's `main` calls) for a
/// game: `n3ds_main!(MyGame::new)` or `n3ds_main!(MyGame::new, self_test)`,
/// where `MyGame::new: fn(Vec2) -> MyGame` and
/// `self_test: fn(&mut N3dsHost, &mut MyGame) -> Result<(), String>`.
/// On other targets it expands to a `main` that explains how to build, so
/// the bin crate builds everywhere.
#[macro_export]
macro_rules! n3ds_main {
    ($make:expr) => {
        $crate::n3ds_main!($make, |_: &mut $crate::N3dsHost, _: &mut _| Ok(()));
    };
    ($make:expr, $self_test:expr) => {
        #[cfg(target_os = "horizon")]
        #[unsafe(no_mangle)]
        pub extern "C" fn trino_rust_main() {
            $crate::N3dsHost::main($make, $self_test)
        }

        #[cfg(not(target_os = "horizon"))]
        fn main() {
            eprintln!("this crate targets the 3DS — build it with `cargo xtask build 3ds`");
            std::process::exit(1);
        }
    };
}
