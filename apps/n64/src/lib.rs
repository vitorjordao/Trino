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

    mod micro;

    use alloc::format;
    use trino_core::{Button, Game, Input, InputState, Vec2};
    use trino_platform_n64::{N64Assets, N64Audio, N64Input, N64Platform, N64Renderer, runtime};

    #[unsafe(no_mangle)]
    pub extern "C" fn trino_rust_main() {
        runtime::init();
        runtime::log("TRINO_BOOT\n");

        let mut renderer = N64Renderer::new();
        let mut audio = N64Audio::new();
        let mut input = N64Input;
        let mut platform = N64Platform::new();
        N64Assets::load_all(&mut renderer, &mut audio, &mut platform);

        let mut game = castle64::Castle64Game::new(Vec2::new(320.0, 240.0));

        if N64Assets::exists("/test_mode") {
            run_self_test(&mut game, &mut renderer, &mut audio, &mut platform);
        }

        let mut last = runtime::now_us();
        loop {
            let now = runtime::now_us();
            let dt = (now.wrapping_sub(last) as f32 / 1_000_000.0).min(0.1);
            last = now;

            let state = input.poll();
            game.update_with(&mut platform, &state, &mut audio, dt);
            game.render(&mut renderer);
            audio.poll();
        }
    }

    // Steps 0b–0d each run out of line: a ShowcaseGame is ~22 KB, built by
    // value (constructor frame + the caller's slot), and libdragon's main
    // stack is 64 KB. Inline in `run_self_test`, the baked-scene step stacked
    // main + run_self_test + check_baked + from_baked past the limit — the
    // overflow corrupted memory and hung the ROM at random points. Each
    // step's frame is gone before the next; `check_stack` guards the limit.

    #[inline(never)]
    fn showcase_demo(renderer: &mut N64Renderer, audio: &mut N64Audio, platform: &mut N64Platform) {
        let _ = platform;
        runtime::log("TRINO_STEP:showcase demo\n");
        // 0b) Collision from scenes + runtime entities: the showcase demo
        // (editor scene exported to code) plays through every coin to the goal.
        {
            let mut demo = showcase::ShowcaseGame::with_screen(Vec2::new(320.0, 240.0));
            let mut n = 0u32;
            let result = showcase::run_demo(&mut demo, |g, input| {
                g.update(input, audio, 1.0 / 60.0);
                // Physics every frame; render 1:32 — the emulators run the
                // full-scene software T&L well below real time.
                if n % 32 == 0 {
                    g.render(renderer);
                }
                n += 1;
            });
            if let Err(e) = result {
                runtime::log(&format!("TRINO_TEST_FAIL:showcase: {e}\n"));
                loop {
                    trino_core::Renderer::begin_frame(renderer, trino_core::Color::BLACK);
                    trino_core::Renderer::end_frame(renderer);
                }
            }
        }
    }

    #[inline(never)]
    fn showcase_baked(
        renderer: &mut N64Renderer,
        audio: &mut N64Audio,
        platform: &mut N64Platform,
    ) {
        runtime::log("TRINO_STEP:baked scene\n");
        // 0c) The same level baked as data ([scenes.showcase] -> TSCN blob),
        // parsed at runtime, must equal the exported code and play through.
        {
            let mut n = 0u32;
            let result = showcase::check_baked(platform, |g, input| {
                g.update(input, audio, 1.0 / 60.0);
                // Physics every frame; render 1:32 — the emulators run the
                // full-scene software T&L well below real time.
                if n % 32 == 0 {
                    g.render(renderer);
                }
                n += 1;
            });
            if let Err(e) = result {
                runtime::log(&format!("TRINO_TEST_FAIL:baked scene: {e}\n"));
                loop {
                    trino_core::Renderer::begin_frame(renderer, trino_core::Color::BLACK);
                    trino_core::Renderer::end_frame(renderer);
                }
            }
        }
    }

    #[inline(never)]
    fn showcase_golden(
        renderer: &mut N64Renderer,
        audio: &mut N64Audio,
        platform: &mut N64Platform,
    ) {
        let _ = (audio, platform);
        // 0d) Emulator golden image: the showcase's first frame (fixed state,
        // deterministic T&L), dumped over ISViewer; `cargo xtask test n64`
        // compares it with tests/golden/n64/showcase.png (--bless writes it).
        runtime::log("TRINO_STEP:golden frame\n");
        {
            let mut g = showcase::ShowcaseGame::with_screen(Vec2::new(320.0, 240.0));
            g.render(renderer);
            renderer.dump_framebuffer();
        }
    }

    /// Libdragon's main stack is 64 KB and nothing stops an overflow: it
    /// silently corrupts memory (interrupts push onto it too). Report the
    /// high-water mark and fail the self-test well before the limit.
    fn check_stack(after: &str) -> bool {
        const LIMIT: u32 = 60 * 1024;
        let used = runtime::stack_used();
        runtime::log(&format!("TRINO_STACK:{used} bytes after {after}\n"));
        if used > LIMIT {
            runtime::log(&format!(
                "TRINO_TEST_FAIL:stack: {used} of 65536 bytes used after {after} — \
                 a large value on the stack (boxed or out-of-line it)\n"
            ));
            return false;
        }
        true
    }

    /// Deterministic self-check exercising 3D physics + render + assets,
    /// reporting through the ISViewer channel: settle on the ground, walk
    /// toward the castle (+Z), then jump and land back.
    fn run_self_test(
        game: &mut castle64::Castle64Game,
        renderer: &mut N64Renderer,
        audio: &mut N64Audio,
        platform: &mut N64Platform,
    ) -> ! {
        const DT: f32 = 1.0 / 60.0;
        // 0) Engine services through the portable traits (text, blobs, save).
        if let Err(e) = castle64::selftest::engine_checks(renderer, platform) {
            runtime::log(&format!("TRINO_TEST_FAIL:engine: {e}\n"));
            loop {
                trino_core::Renderer::begin_frame(renderer, trino_core::Color::BLACK);
                trino_core::Renderer::end_frame(renderer);
            }
        }
        runtime::log(&format!(
            "TRINO_STACK:{} bytes t={}ms\n",
            runtime::stack_used(),
            runtime::now_us() / 1000
        ));
        runtime::log("TRINO_STEP:audio\n");

        if let Err(e) = castle64::selftest::audio_checks(audio) {
            runtime::log(&format!("TRINO_TEST_FAIL:audio: {e}\n"));
            loop {
                trino_core::Renderer::begin_frame(renderer, trino_core::Color::BLACK);
                trino_core::Renderer::end_frame(renderer);
            }
        }
        // Its own frame: the showcase games (~22 KB each) must be gone from
        // the stack before castle64 renders on top of this function.
        showcase_demo(renderer, audio, platform);
        showcase_baked(renderer, audio, platform);
        showcase_golden(renderer, audio, platform);
        if !check_stack("the showcase steps") {
            loop {
                trino_core::Renderer::begin_frame(renderer, trino_core::Color::BLACK);
                trino_core::Renderer::end_frame(renderer);
            }
        }
        // Render 1:4 — a lógica roda todo frame (determinismo intacto); o
        // T&L em software da cena completa a cada frame estourava o timeout
        // do harness no emulador.
        let mut frame_no = 0u32;
        // Timing of the last frame (emulated clock) and a switch that
        // renders every frame while measuring (step 5).
        let force_render = core::cell::Cell::new(false);
        let last_update_us = core::cell::Cell::new(0u32);
        let last_render_us = core::cell::Cell::new(0u32);
        let mut frame = |game: &mut castle64::Castle64Game,
                         renderer: &mut N64Renderer,
                         audio: &mut N64Audio,
                         input: &InputState| {
            let t0 = runtime::now_us();
            game.update(input, audio, DT);
            let t1 = runtime::now_us();
            if force_render.get() || frame_no % 4 == 0 {
                game.render(renderer);
            }
            let t2 = runtime::now_us();
            last_update_us.set(t1.wrapping_sub(t0));
            last_render_us.set(t2.wrapping_sub(t1));
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
                    if bot.frames_in_step() > 1800 || frames > 60 * 120 {
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
                    // 5) Frame cost on the console's own clock (ares
                    // emulates the CPU count register, so this is emulated
                    // N64 time, not host time): every castle64 perf scene
                    // (`castle64::perf`), per stage and per model. xtask
                    // compares the lines with tests/perf/n64.toml.
                    force_render.set(true);
                    for scene in castle64::perf::SCENES {
                        let mut g = alloc::boxed::Box::new(castle64::perf::scene_game(
                            scene,
                            Vec2::new(320.0, 240.0),
                            audio,
                        ));
                        let mut times = trino_core::perf::FrameTimes::default();
                        let v0 = runtime::vblanks();
                        renderer.start_profile();
                        for _ in 0..castle64::perf::MEASURE_FRAMES {
                            frame(&mut *g, renderer, audio, &idle);
                            times.update += last_update_us.get();
                            times.render += last_render_us.get();
                        }
                        times.vblanks = runtime::vblanks().wrapping_sub(v0);
                        if let Some(p) = renderer.take_profile() {
                            let mut report = alloc::string::String::new();
                            let _ = p.write_report(scene.name, &times, &mut report);
                            runtime::log(&report);
                        }
                        if scene.name == "hub" {
                            let n = castle64::perf::MEASURE_FRAMES;
                            runtime::log(&format!(
                                "TRINO_PERF:castle64 hub frame update={}us render={}us \
                                 total={}us (emulated N64 time; 60 fps = 16667us, \
                                 30 fps = 33333us)\n",
                                times.update / n,
                                times.render / n,
                                (times.update + times.render) / n
                            ));
                        }
                    }
                    force_render.set(false);
                    // 6) Fixed costs of the T&L (synthetic draw sets,
                    // ungated): after the castle64 scenes, so their camera
                    // and light cannot leak into those.
                    micro::run(renderer, castle64::perf::MEASURE_FRAMES);
                    // High-water mark of the whole run (bot, perf scenes).
                    if check_stack("the bot and the perf scenes") {
                        runtime::log("TRINO_TEST_PASS\n");
                    }
                }
            }
        }
        loop {
            frame(game, renderer, audio, &idle);
        }
    }
}
