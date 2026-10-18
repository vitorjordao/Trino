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
    use trino_platform_3ds::{
        N3dsAssets, N3dsAudio, N3dsInput, N3dsPlatform, N3dsRenderer, runtime,
    };

    #[unsafe(no_mangle)]
    pub extern "C" fn trino_rust_main() {
        runtime::init();
        runtime::log("TRINO_BOOT\n");

        let mut renderer = N3dsRenderer::new();
        let mut audio = N3dsAudio::new();
        let mut input = N3dsInput;
        let mut platform = N3dsPlatform::new();
        N3dsAssets::load_all(&mut renderer, &mut audio, &mut platform);

        // Top screen: 400x240.
        let mut game = castle64::Castle64Game::new(Vec2::new(400.0, 240.0));

        if N3dsAssets::exists("/test_mode") {
            run_self_test(&mut game, &mut renderer, &mut audio, &mut platform);
            return;
        }

        let mut last = runtime::now_us();
        while runtime::running() {
            let now = runtime::now_us();
            let dt = (now.wrapping_sub(last) as f32 / 1_000_000.0).min(0.1);
            last = now;

            let state = input.poll();
            game.update_with(&mut platform, &state, &mut audio, dt);
            game.render(&mut renderer);
            audio.poll();
        }
    }

    /// The GPU T&L path against the CPU's software T&L on one frame (fixed
    /// state, drawn by `draw`): the same frame drawn both ways must match
    /// within the GPU's float rounding, and the GPU must have drawn some of
    /// it. A wrong cull winding, projection or fog shows here as whole faces
    /// of the wrong color.
    fn gpu_tnl_frame(
        renderer: &mut N3dsRenderer,
        scene: &str,
        draw: &mut dyn FnMut(&mut N3dsRenderer),
    ) -> Result<(), alloc::string::String> {
        let mut grab = |renderer: &mut N3dsRenderer, gpu: bool| {
            renderer.set_gpu_tnl(gpu);
            // The LCD is double-buffered: draw the probed frame a few times.
            for _ in 0..3 {
                draw(renderer);
            }
            let stats = trino_core::Renderer::stats(renderer);
            let off = renderer.gpu_rejects().map(u32::from);
            let mut px = alloc::vec::Vec::with_capacity(40 * 24);
            for y in 0..24 {
                for x in 0..40 {
                    let c = trino_core::Renderer::read_pixel(renderer, x * 10 + 5, y * 10 + 5)
                        .unwrap_or(trino_core::Color::BLACK);
                    px.push(c);
                }
            }
            (stats, off, px)
        };
        let (cpu_stats, _, cpu) = grab(renderer, false);
        let (gpu_stats, off_gpu, gpu) = grab(renderer, true);
        renderer.set_gpu_tnl(true);
        let diff = |a: trino_core::Color, b: trino_core::Color| {
            let d = |x: u8, y: u8| (x as i32 - y as i32).unsigned_abs();
            d(a.r, b.r).max(d(a.g, b.g)).max(d(a.b, b.b))
        };
        let off = cpu
            .iter()
            .zip(&gpu)
            .filter(|(a, b)| diff(**a, **b) > 24)
            .count();
        let total: u32 = cpu.iter().zip(&gpu).map(|(a, b)| diff(*a, *b)).sum();
        runtime::log(&format!(
            "TRINO_STEP:gpu tnl {scene} draws={} of {} gpu_tris={} off_gpu={} cpu_draws_reference={} differ={off}/{} mean={:.2}\n",
            gpu_stats.tnl.gpu_draws,
            gpu_stats.models,
            gpu_stats.tnl.gpu_tris,
            trino_platform_3ds::off_gpu_list(&off_gpu),
            cpu_stats.tnl.gpu_draws,
            cpu.len(),
            total as f32 / cpu.len() as f32,
        ));
        if cpu_stats.tnl.gpu_draws != 0 {
            return Err(format!(
                "{scene}: {} GPU draws with the GPU path off",
                cpu_stats.tnl.gpu_draws
            ));
        }
        if gpu_stats.tnl.gpu_draws == 0 {
            return Err(format!("{scene}: no draw took the GPU path"));
        }
        // A few samples on silhouettes may flip (float24 vs f32 edges).
        if off * 50 > cpu.len() {
            return Err(format!(
                "{scene}: {off} of {} samples differ by more than 24",
                cpu.len()
            ));
        }
        Ok(())
    }

    /// [`gpu_tnl_frame`] on the castle64 hub (fresh game, fixed camera:
    /// white light, no fog) and on the showcase's first frame (its
    /// authored look: colored light and distance fog, which the vertex
    /// shader applies per vertex).
    fn gpu_tnl_check(
        renderer: &mut N3dsRenderer,
        audio: &mut N3dsAudio,
    ) -> Result<(), alloc::string::String> {
        let hub = &castle64::perf::SCENES[0];
        let mut g = alloc::boxed::Box::new(castle64::perf::scene_game(
            hub,
            Vec2::new(400.0, 240.0),
            audio,
        ));
        gpu_tnl_frame(renderer, "hub", &mut |r| g.render(r))?;
        drop(g);
        let mut demo =
            alloc::boxed::Box::new(showcase::ShowcaseGame::with_screen(Vec2::new(400.0, 240.0)));
        gpu_tnl_frame(renderer, "showcase-fog", &mut |r| demo.render(r))?;
        drop(demo);
        // Water (ADR 0015): the sheet through water.v.pica against the CPU's
        // water T&L (swell, a ripple, fog-free sky, glint, both layers).
        let (camera, params, time, ripples) = castle64::selftest::gpu_water_scene();
        gpu_tnl_frame(renderer, "water", &mut |r| {
            castle64::selftest::water_scene(r, &camera, &params, time, &ripples)
        })
    }

    /// `TRINO_FRAME:<scene> avg_us= max_us= frames= fps= cpu_us=`: 30
    /// frames of a castle64 perf scene (render only). `avg_us` is the wall
    /// clock, GPU and vsync waits included; `cpu_us` the ARM11's own work,
    /// what the New 3DS's speedup changes.
    fn time_scene(
        scene: &castle64::perf::PerfScene,
        renderer: &mut N3dsRenderer,
        audio: &mut N3dsAudio,
    ) {
        const FRAMES: u32 = 30;
        let mut g = alloc::boxed::Box::new(castle64::perf::scene_game(
            scene,
            Vec2::new(400.0, 240.0),
            audio,
        ));
        g.render(renderer);
        let _ = renderer.take_wait_us();
        let (mut total, mut cpu, mut max) = (0u32, 0u32, 0u32);
        for _ in 0..FRAMES {
            let t0 = runtime::now_us();
            g.render(renderer);
            let us = runtime::now_us().wrapping_sub(t0);
            total += us;
            cpu += us.saturating_sub(renderer.take_wait_us());
            max = max.max(us);
        }
        let avg = total / FRAMES;
        runtime::log(&format!(
            "TRINO_FRAME:{} avg_us={avg} max_us={max} frames={FRAMES} fps={} cpu_us={}\n",
            scene.name,
            1_000_000 / avg.max(1),
            cpu / FRAMES
        ));
    }

    /// The worker core (issue #54 Phase 3): eight jobs on the second core
    /// compute what the main thread computes meanwhile, and match it. Logs
    /// `TRINO_STEP:worker core=2 queued=8 …` (core -1: inline, no worker).
    fn worker_check() -> Result<alloc::string::String, alloc::string::String> {
        use core::sync::atomic::{AtomicU32, Ordering};
        use trino_platform_3ds::worker;
        const JOBS: usize = 8;
        static OUT: [AtomicU32; JOBS] = [const { AtomicU32::new(0) }; JOBS];
        fn work(i: u32) -> u32 {
            let mut x = 0x9E37_79B9u32 ^ i.wrapping_mul(0x85EB_CA6B);
            for _ in 0..40_000 {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
            }
            x
        }
        extern "C" fn job(i: u32) {
            OUT[i as usize].store(work(i), Ordering::Release);
        }
        let core = worker::start();
        let t0 = runtime::now_us();
        let queued = (0..JOBS as u32).filter(|&i| worker::submit(job, i)).count();
        let want: [u32; JOBS] = core::array::from_fn(|i| work(i as u32));
        worker::wait();
        let us = runtime::now_us().wrapping_sub(t0);
        for (i, w) in want.iter().enumerate() {
            if OUT[i].load(Ordering::Acquire) != *w {
                return Err(format!("worker: job {i} computed the wrong value"));
            }
        }
        Ok(format!(
            "TRINO_STEP:worker core={} queued={queued} us={us}\n",
            core.map_or(-1, i32::from)
        ))
    }

    /// Deterministic self-check exercising 3D physics + render + assets,
    /// reporting through the debug channel: settle on the ground, walk
    /// toward the castle (+Z), then jump and land back.
    fn run_self_test(
        game: &mut castle64::Castle64Game,
        renderer: &mut N3dsRenderer,
        audio: &mut N3dsAudio,
        platform: &mut N3dsPlatform,
    ) {
        const DT: f32 = 1.0 / 60.0;
        // 0) Engine services through the portable traits (text, blobs, save).
        if let Err(e) = castle64::selftest::engine_checks(renderer, platform) {
            runtime::log(&format!("TRINO_TEST_FAIL:engine: {e}\n"));
            while runtime::running() {
                trino_core::Renderer::begin_frame(renderer, trino_core::Color::BLACK);
                trino_core::Renderer::end_frame(renderer);
            }
            return;
        }
        runtime::log("TRINO_STEP:worker\n");
        match worker_check() {
            Ok(line) => runtime::log(&line),
            Err(e) => {
                runtime::log(&format!("TRINO_TEST_FAIL:{e}\n"));
                while runtime::running() {
                    trino_core::Renderer::begin_frame(renderer, trino_core::Color::BLACK);
                    trino_core::Renderer::end_frame(renderer);
                }
                return;
            }
        }
        runtime::log("TRINO_STEP:audio\n");
        if let Err(e) = castle64::selftest::audio_checks(audio) {
            runtime::log(&format!("TRINO_TEST_FAIL:audio: {e}\n"));
            while runtime::running() {
                trino_core::Renderer::begin_frame(renderer, trino_core::Color::BLACK);
                trino_core::Renderer::end_frame(renderer);
            }
            return;
        }
        runtime::log("TRINO_STEP:input\n");
        if let Err(e) = castle64::selftest::idle_input_checks(&mut N3dsInput) {
            runtime::log(&format!("TRINO_TEST_FAIL:{e}\n"));
            while runtime::running() {
                trino_core::Renderer::begin_frame(renderer, trino_core::Color::BLACK);
                trino_core::Renderer::end_frame(renderer);
            }
            return;
        }
        runtime::log("TRINO_STEP:showcase demo\n");
        // 0b) Collision from scenes + runtime entities: the showcase demo
        // (editor scene exported to code) plays through every coin to the goal.
        {
            let mut demo = showcase::ShowcaseGame::with_screen(Vec2::new(400.0, 240.0));
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
                while runtime::running() {
                    trino_core::Renderer::begin_frame(renderer, trino_core::Color::BLACK);
                    trino_core::Renderer::end_frame(renderer);
                }
                return;
            }
            // The route wades across the river (ripples, splashes, water).
            if demo.splashes == 0 {
                runtime::log("TRINO_TEST_FAIL:showcase: the demo never entered the river\n");
            }
        }
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
                while runtime::running() {
                    trino_core::Renderer::begin_frame(renderer, trino_core::Color::BLACK);
                    trino_core::Renderer::end_frame(renderer);
                }
                return;
            }
        }
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
                    // 5) Emulator frame of a fixed castle64 state (the hub
                    // perf scene, fresh game): `cargo xtask test 3ds`
                    // compares it with tests/golden/3ds/castle64_hub.png
                    // within a tolerance, and xtask/tests/n3ds_parity.rs
                    // holds the PC's N3ds look to the same image. Then the
                    // touch screen of the same frame (castle64's map panel:
                    // 2D plus a 3D star), castle64_hub_bottom.png.
                    if let Err(e) = gpu_tnl_check(renderer, audio) {
                        fail(format!("gpu tnl: {e}"));
                        while runtime::running() {
                            frame(game, renderer, audio, &idle);
                        }
                        return;
                    }
                    runtime::log("TRINO_STEP:golden frame\n");
                    let hub = &castle64::perf::SCENES[0];
                    let mut g = alloc::boxed::Box::new(castle64::perf::scene_game(
                        hub,
                        Vec2::new(400.0, 240.0),
                        audio,
                    ));
                    for _ in 0..3 {
                        g.render(renderer);
                    }
                    renderer.dump_framebuffer();
                    renderer.dump_framebuffer_bottom();
                    drop(g);
                    // Every castle64 level timed on this hardware (reported
                    // only: the New 3DS's speedup against the old clock,
                    // issue #54; `--accurate-timing` for the GPU's share).
                    for scene in castle64::perf::SCENES {
                        time_scene(scene, renderer, audio);
                    }
                    // 6) The showcase's touch panel (metrics + look
                    // toggles) read back: button fills by state and edges,
                    // a top-screen click ignored, two FOG taps through
                    // `update`. Last, so the toggles leak into nothing;
                    // its final frame (the default panel) is the third
                    // golden, showcase_bottom.png.
                    runtime::log("TRINO_STEP:showcase panel\n");
                    match showcase::panel::check_panel(renderer, audio) {
                        Ok(true) => {
                            runtime::log("TRINO_STEP:showcase panel probed\n");
                            renderer.dump_framebuffer_bottom();
                            // 7) The showcase's river (ADR 0015) under its
                            // scene camera, the hero wading in: the fourth
                            // golden, showcase_river.png (top screen).
                            runtime::log("TRINO_STEP:river golden frame\n");
                            let mut g = alloc::boxed::Box::new(showcase::ShowcaseGame::blank());
                            showcase::perf::river_scene(&mut g, Vec2::new(400.0, 240.0), audio);
                            for _ in 0..3 {
                                g.render(renderer);
                            }
                            renderer.dump_framebuffer();
                            drop(g);
                            // 8) One card per 3DS texture format (#52), as
                            // sprites and textured 3D: the fifth golden,
                            // texture_formats.png (top screen).
                            runtime::log("TRINO_STEP:texture formats golden frame\n");
                            castle64::selftest::draw_texture_formats(renderer, true);
                            renderer.dump_framebuffer();
                            runtime::log("TRINO_TEST_PASS\n");
                        }
                        Ok(false) => fail("showcase panel: no second screen to probe".into()),
                        Err(e) => fail(format!("{e}")),
                    }
                }
            }
        }
        while runtime::running() {
            frame(game, renderer, audio, &idle);
        }
    }
}
