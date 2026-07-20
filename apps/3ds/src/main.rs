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
    use trino_core::{Button, Game, Input, InputState, Vec2};
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
        let mut game = castle64::Castle64Game::new(Vec2::new(400.0, 240.0));

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

    /// Deterministic self-check exercising 3D physics + render + assets,
    /// reporting through the debug channel: settle on the ground, walk
    /// toward the castle (+Z), then jump and land back.
    fn run_self_test(
        game: &mut castle64::Castle64Game,
        renderer: &mut N3dsRenderer,
        audio: &mut N3dsAudio,
    ) {
        const DT: f32 = 1.0 / 60.0;
        // Render 1:4 — a lógica roda todo frame (determinismo intacto); o
        // Azahar emula a cena 3D cheia abaixo do tempo real.
        let mut frame_no = 0u32;
        let mut frame = |game: &mut castle64::Castle64Game,
                         renderer: &mut N3dsRenderer,
                         audio: &mut N3dsAudio,
                         input: &InputState| {
            game.update(input, audio, DT);
            if frame_no % 4 == 0 {
                game.render(renderer);
            }
            frame_no += 1;
            audio.poll();
        };
        let idle = InputState::default();
        let mut fwd = InputState::default();
        fwd.set(Button::DpadUp, true);
        let mut jump = InputState::default();
        jump.set(Button::A, true);

        let fail = |msg: alloc::string::String| {
            runtime::log(&format!("TRINO_TEST_FAIL:{msg}\n"));
        };

        // 1) Gravity settles the player on the hub floor.
        for _ in 0..60 {
            frame(game, renderer, audio, &idle);
        }
        if !game.on_ground || game.vel.y != 0.0 {
            fail(format!("did not settle: on_ground={}", game.on_ground));
        } else {
            let ground_y = game.pos.y;
            let z0 = game.pos.z;
            // 2) Walking moves toward the castle (+Z, world units).
            for _ in 0..30 {
                frame(game, renderer, audio, &fwd);
            }
            let walked = game.pos.z - z0;
            // 3) Jump rises (Y-up) and lands back at the same height.
            // Hold A through the ascent: releasing early cuts the jump
            // (variable-height jumps).
            for _ in 0..14 {
                frame(game, renderer, audio, &jump);
            }
            let mut peak = ground_y;
            let mut landed = false;
            for _ in 0..150 {
                frame(game, renderer, audio, &idle);
                if game.pos.y > peak {
                    peak = game.pos.y;
                }
                if game.on_ground {
                    landed = true;
                    break;
                }
            }
            if walked < 1.5 {
                fail(format!("walked only {walked}"));
            } else if peak < ground_y + 1.0 {
                fail(format!("jump peak {peak} vs ground {ground_y}"));
            } else if !landed || (game.pos.y - ground_y).abs() > 0.01 {
                fail(format!("landing y {} vs {ground_y}", game.pos.y));
            } else {
                // 4) E2E: o bot joga hub → green hills inteira → estrela.
                let mut bot = castle64::bot::Bot::new(castle64::bot::GREEN_RUN);
                let mut frames = 0u32;
                let mut bot_ok = true;
                while !bot.done() {
                    let input = bot.drive(game);
                    frame(game, renderer, audio, &input);
                    frames += 1;
                    if bot.frames_in_step() > 1800 || frames > 60 * 100 {
                        fail(format!("bot stuck at step {}", bot.step_index()));
                        bot_ok = false;
                        break;
                    }
                }
                if bot_ok && (game.star_count() != 1 || game.level != 0) {
                    fail(format!(
                        "bot run ended with {} stars, level {}",
                        game.star_count(),
                        game.level
                    ));
                } else if bot_ok {
                    runtime::log("TRINO_TEST_PASS\n");
                }
            }
        }
        while runtime::running() {
            frame(game, renderer, audio, &idle);
        }
    }
}
