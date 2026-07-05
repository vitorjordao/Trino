//! 3DS glue binary. The C shim's `main()` (3dsx crt0 entry) calls
//! `trino_rust_main`; everything else is Rust. Unlike the N64's forever
//! loop, the 3DS loop exits when the applet manager asks (HOME/power) so
//! the shim can tear the OS services down.
//!
//! Only meaningful for the armv6k-nintendo-3ds target (`cargo xtask build
//! 3ds`); on host targets it compiles to a stub so `--workspace` stays green.
//!
//! Test protocol: when the RomFS contains `test_mode` (written by
//! `cargo xtask test 3ds`), the app runs a self-check and prints
//! `TRINO_TEST_PASS` / `TRINO_TEST_FAIL:<reason>` over svcOutputDebugString,
//! then keeps rendering until the harness kills the emulator.

#![cfg_attr(target_os = "horizon", no_std)]
#![cfg_attr(target_os = "horizon", no_main)]

#[cfg(not(target_os = "horizon"))]
fn main() {
    eprintln!("trino-app-3ds targets the 3DS — build it with `cargo xtask build 3ds`");
    std::process::exit(1);
}

#[cfg(target_os = "horizon")]
mod n3ds {
    extern crate alloc;

    use alloc::format;
    use trino_core::{Game, Input, InputState, Vec2};
    use trino_platform_3ds::{N3dsAssets, N3dsAudio, N3dsInput, N3dsRenderer, runtime};

    #[unsafe(no_mangle)]
    pub extern "C" fn trino_rust_main() {
        runtime::init();
        runtime::log("TRINO_BOOT\n");

        let mut renderer = N3dsRenderer::new();
        let mut audio = N3dsAudio::new();
        let mut input = N3dsInput;
        N3dsAssets::load_all(&mut renderer, &mut audio);

        // Top screen: 400x240.
        let mut game = hello_sprite::HelloGame::new(Vec2::new(400.0, 240.0));

        if N3dsAssets::exists("/test_mode") {
            run_self_test(&mut game, &mut renderer, &mut audio);
            return;
        }

        let mut last = runtime::now_us();
        while runtime::running() {
            let now = runtime::now_us();
            let dt = (now.wrapping_sub(last) as f32 / 1_000_000.0).min(0.1);
            last = now;

            let state = input.poll();
            game.update(&state, &mut audio, dt);
            game.render(&mut renderer);
            audio.poll();
        }
    }

    /// Deterministic self-check exercising update + render + assets,
    /// reporting through the debug channel.
    fn run_self_test(
        game: &mut hello_sprite::HelloGame,
        renderer: &mut N3dsRenderer,
        audio: &mut N3dsAudio,
    ) {
        let start = game.pos;

        // Simulate 60 frames of holding D-pad right at fixed dt.
        let mut held = InputState::default();
        held.set(trino_core::Button::DpadRight, true);
        for _ in 0..60 {
            game.update(&held, audio, 1.0 / 60.0);
            game.render(renderer);
            audio.poll();
        }

        let moved = game.pos.x - start.x;
        // 120 px/s * 1 s = 120 px (clamp-free from the center of 400x240).
        if (moved - 120.0).abs() < 0.5 {
            runtime::log("TRINO_TEST_PASS\n");
        } else {
            runtime::log(&format!("TRINO_TEST_FAIL:moved {moved} expected 120\n"));
        }
        while runtime::running() {
            game.render(renderer);
            audio.poll();
        }
    }
}
