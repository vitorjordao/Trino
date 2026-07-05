//! N64 glue crate. Builds as a **staticlib** for the mips-nintendo64-none
//! target; the final ELF is linked inside the Docker toolchain image against
//! the C shim + libdragon (see `xtask/src/n64.rs`). On host targets it
//! compiles to an empty staticlib so `--workspace` stays green.
//!
//! The C shim's `main()` (libdragon entry) calls `trino_rust_main`;
//! everything else is Rust.
//!
//! Test protocol: when the DFS contains `test_mode` (written by
//! `cargo xtask test n64`), the ROM runs a self-check and prints
//! `TRINO_TEST_PASS` / `TRINO_TEST_FAIL:<reason>` over ISViewer, then keeps
//! rendering forever (the harness kills the emulator on match or timeout).

#![cfg_attr(target_os = "none", no_std)]

#[cfg(target_os = "none")]
mod n64 {
    extern crate alloc;

    use alloc::format;
    use trino_core::{Game, Input, InputState, Vec2};
    use trino_platform_n64::{N64Assets, N64Audio, N64Input, N64Renderer, runtime};

    #[unsafe(no_mangle)]
    pub extern "C" fn trino_rust_main() {
        runtime::init();
        runtime::log("TRINO_BOOT\n");

        let mut renderer = N64Renderer::new();
        let mut audio = N64Audio::new();
        let mut input = N64Input;
        N64Assets::load_all(&mut renderer, &mut audio);

        let mut game = hello_sprite::HelloGame::new(Vec2::new(320.0, 240.0));

        if N64Assets::exists("/test_mode") {
            run_self_test(&mut game, &mut renderer, &mut audio);
        }

        let mut last = runtime::now_us();
        loop {
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
    /// reporting through the ISViewer channel.
    fn run_self_test(
        game: &mut hello_sprite::HelloGame,
        renderer: &mut N64Renderer,
        audio: &mut N64Audio,
    ) -> ! {
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
        // 120 px/s * 1 s = 120 px (clamp-free from the center of 320x240).
        if (moved - 120.0).abs() < 0.5 {
            runtime::log("TRINO_TEST_PASS\n");
        } else {
            runtime::log(&format!("TRINO_TEST_FAIL:moved {moved} expected 120\n"));
        }
        loop {
            game.render(renderer);
            audio.poll();
        }
    }
}
