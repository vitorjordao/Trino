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
    use trino_core::{Button, Game, Input, InputState, Vec2};
    use trino_platform_n64::{N64Assets, N64Audio, N64Input, N64Renderer, runtime};

    #[unsafe(no_mangle)]
    pub extern "C" fn trino_rust_main() {
        runtime::init();
        runtime::log("TRINO_BOOT\n");

        let mut renderer = N64Renderer::new();
        let mut audio = N64Audio::new();
        let mut input = N64Input;
        N64Assets::load_all(&mut renderer, &mut audio);

        let mut game = platformer::PlatformerGame::new(Vec2::new(320.0, 240.0));

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

    /// Deterministic self-check exercising physics + render + assets,
    /// reporting through the ISViewer channel: settle on the ground, walk,
    /// then jump and land back.
    fn run_self_test(
        game: &mut platformer::PlatformerGame,
        renderer: &mut N64Renderer,
        audio: &mut N64Audio,
    ) -> ! {
        const DT: f32 = 1.0 / 60.0;
        let mut frame = |game: &mut platformer::PlatformerGame, input: &InputState| {
            game.update(input, audio, DT);
            game.render(renderer);
            audio.poll();
        };
        let idle = InputState::default();
        let mut right = InputState::default();
        right.set(Button::DpadRight, true);
        let mut jump = InputState::default();
        jump.set(Button::A, true);

        let fail = |msg: alloc::string::String| {
            runtime::log(&format!("TRINO_TEST_FAIL:{msg}\n"));
        };

        // 1) Gravity settles the player on the floor.
        for _ in 0..60 {
            frame(game, &idle);
        }
        if !game.on_ground || game.vel.y != 0.0 {
            fail(format!("did not settle: on_ground={}", game.on_ground));
        } else {
            let ground_y = game.pos.y;
            let x0 = game.pos.x;
            // 2) Walking moves right.
            for _ in 0..30 {
                frame(game, &right);
            }
            let walked = game.pos.x - x0;
            // 3) Jump rises and lands back at the same height.
            frame(game, &jump);
            let mut peak = ground_y;
            let mut landed = false;
            for _ in 0..150 {
                frame(game, &idle);
                if game.pos.y < peak {
                    peak = game.pos.y;
                }
                if game.on_ground {
                    landed = true;
                    break;
                }
            }
            if walked < 40.0 {
                fail(format!("walked only {walked}"));
            } else if peak > ground_y - 30.0 {
                fail(format!("jump peak {peak} vs ground {ground_y}"));
            } else if !landed || (game.pos.y - ground_y).abs() > 0.01 {
                fail(format!("landing y {} vs {ground_y}", game.pos.y));
            } else {
                runtime::log("TRINO_TEST_PASS\n");
            }
        }
        loop {
            frame(game, &idle);
        }
    }
}
