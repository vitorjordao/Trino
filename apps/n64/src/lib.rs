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
            N64Assets::pump(&mut renderer, &mut audio, &mut platform);
            game.update_with(&mut platform, &state, &mut audio, dt);
            game.render(&mut renderer);
            audio.poll();
            platform.sample_heap();
        }
    }

    /// The `asset groups` step's backends and the loader's frame.
    struct GroupHost<'a> {
        renderer: &'a mut N64Renderer,
        audio: &'a mut N64Audio,
        platform: &'a mut N64Platform,
    }

    impl castle64::selftest::GroupHost for GroupHost<'_> {
        fn parts(&mut self) -> (&mut dyn trino_core::Renderer, &mut dyn trino_core::Platform) {
            (self.renderer, self.platform)
        }

        fn pump(&mut self) {
            N64Assets::pump(self.renderer, self.audio, self.platform);
            self.audio.poll();
            self.platform.sample_heap();
        }
    }

    /// Asset groups (issue #56): load behind a loading bar, draw, unload,
    /// the heap back to its baseline, then 1 000 frames of load/unload.
    #[inline(never)]
    fn asset_groups(renderer: &mut N64Renderer, audio: &mut N64Audio, platform: &mut N64Platform) {
        runtime::log("TRINO_STEP:asset groups\n");
        let t0 = runtime::now_us();
        let mut host = GroupHost {
            renderer,
            audio,
            platform,
        };
        match castle64::selftest::asset_groups(&mut host, 1_000) {
            Ok(r) => {
                castle64::selftest::log_groups(host.platform, &r);
                runtime::log(&format!(
                    "TRINO_STEP:asset groups took {}ms\n",
                    runtime::now_us().wrapping_sub(t0) / 1000
                ));
            }
            Err(e) => {
                runtime::log(&format!("TRINO_TEST_FAIL:{e}\n"));
                loop {
                    trino_core::Renderer::begin_frame(host.renderer, trino_core::Color::BLACK);
                    trino_core::Renderer::end_frame(host.renderer);
                }
            }
        }
    }

    /// The heap's high-water mark (issue #56), like [`check_stack`]: the
    /// peak since boot must leave [`HEAP_MARGIN`] of libdragon's heap free —
    /// what a game's next allocation (a level, a display list) needs to
    /// not fail at a random point.
    fn check_heap(after: &str, platform: &N64Platform, sampled: u32) -> bool {
        const HEAP_MARGIN: u32 = 128 * 1024;
        let mut m = trino_core::Platform::memory(platform);
        m.heap_peak = m.heap_peak.max(sampled);
        runtime::log(&format!(
            "TRINO_HEAP:{}/{} peak={} after {after}\n",
            m.heap_used, m.heap_total, m.heap_peak
        ));
        if m.heap_peak + HEAP_MARGIN > m.heap_total {
            runtime::log(&format!(
                "TRINO_TEST_FAIL:heap: peak {} of {} bytes after {after} — less than {} KB \
                 left (move assets to a group, or shrink them: cargo xtask assets report)\n",
                m.heap_peak,
                m.heap_total,
                HEAP_MARGIN / 1024
            ));
            return false;
        }
        true
    }

    // Steps 0b–0d share one ShowcaseGame (~25 KB), boxed once and reset in
    // place by each step: libdragon's main stack is 64 KB, and a game built
    // by value per step (constructor frame + the caller's slot) stacked
    // main + run_self_test + check_baked + from_baked past it — the
    // overflow corrupted memory and hung the ROM at random points. Each
    // step still runs out of line; `check_stack` guards the limit.

    /// An empty showcase game, copied from ROM into its box: never on the
    /// stack (`b.write(ShowcaseGame::with_screen(..))` still built it there
    /// first, 63.5 KB of the 64). The steps reset it in place.
    static BLANK_SHOWCASE: showcase::ShowcaseGame = showcase::ShowcaseGame::blank();

    #[inline(never)]
    fn boxed_showcase() -> alloc::boxed::Box<showcase::ShowcaseGame> {
        let mut b = alloc::boxed::Box::<showcase::ShowcaseGame>::new_uninit();
        // SAFETY: a bitwise copy of a valid value with no `Drop`, into
        // memory sized and aligned for it.
        unsafe {
            core::ptr::copy_nonoverlapping(&BLANK_SHOWCASE, b.as_mut_ptr(), 1);
            b.assume_init()
        }
    }

    #[inline(never)]
    fn showcase_demo(
        demo: &mut showcase::ShowcaseGame,
        renderer: &mut N64Renderer,
        audio: &mut N64Audio,
    ) {
        runtime::log("TRINO_STEP:showcase demo\n");
        // 0b) Collision from scenes + runtime entities: the showcase demo
        // (editor scene exported to code) plays through every coin to the
        // goal, wading across the river (ripples, splashes, the water draws).
        demo.reset(Vec2::new(320.0, 240.0));
        let mut n = 0u32;
        let result = showcase::run_demo(demo, |g, input| {
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
        if demo.splashes == 0 {
            runtime::log("TRINO_TEST_FAIL:showcase: the demo never entered the river\n");
        }
    }

    #[inline(never)]
    fn showcase_baked(
        game: &mut showcase::ShowcaseGame,
        renderer: &mut N64Renderer,
        audio: &mut N64Audio,
        platform: &mut N64Platform,
    ) {
        runtime::log("TRINO_STEP:baked scene\n");
        // 0c) The same level baked as data ([scenes.showcase] -> TSCN blob),
        // parsed at runtime, must equal the exported code and play through.
        let mut n = 0u32;
        let result = showcase::check_baked_in(platform, game, |g, input| {
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

    #[inline(never)]
    fn showcase_golden(g: &mut showcase::ShowcaseGame, renderer: &mut N64Renderer) {
        // 0d) Emulator golden image: the showcase's first frame (fixed state,
        // deterministic T&L), dumped over ISViewer; `cargo xtask test n64`
        // compares it with tests/golden/n64/showcase.png (--bless writes it).
        runtime::log("TRINO_STEP:golden frame\n");
        g.reset(Vec2::new(320.0, 240.0));
        g.render(renderer);
        renderer.dump_framebuffer();
    }

    /// The `showcase-river` perf scene (`showcase::perf`): its frames timed
    /// per stage and per model on the console's clock.
    #[inline(never)]
    fn water_perf(renderer: &mut N64Renderer, audio: &mut N64Audio) {
        // Streamed RSP draws (`set_rsp_stream`, as the football template's
        // host sets them): the river's surfaces go to the overlay as they
        // are issued, so the RSP draws while the CPU builds the next.
        let stream0 = renderer.rsp_stream();
        renderer.set_rsp_stream(true);
        let mut g = boxed_showcase();
        // `micro-river` first: the river alone (issue #69's 1.2 ms budget
        // read directly), gated like the scenes.
        showcase::perf::micro_river_scene(&mut g, Vec2::new(320.0, 240.0));
        let (ids, n_ids) = showcase::perf::micro_river_ids(&g);
        // Unmeasured frames first: the draw memo is built on the first.
        for _ in 0..showcase::perf::WARM_FRAMES {
            showcase::perf::micro_river_render(&g, &ids[..n_ids], renderer);
        }
        let mut times = trino_core::perf::FrameTimes::default();
        let v0 = runtime::vblanks();
        renderer.start_profile();
        for _ in 0..showcase::perf::MEASURE_FRAMES {
            let t0 = runtime::now_us();
            showcase::perf::micro_river_update(&mut g);
            let t1 = runtime::now_us();
            showcase::perf::micro_river_render(&g, &ids[..n_ids], renderer);
            let t2 = runtime::now_us();
            times.update += t1.wrapping_sub(t0);
            times.render += t2.wrapping_sub(t1);
        }
        times.vblanks = runtime::vblanks().wrapping_sub(v0);
        if let Some(p) = renderer.take_profile() {
            let mut report = alloc::string::String::new();
            let _ = p.write_report(showcase::perf::MICRO_RIVER, &times, &mut report);
            runtime::log(&report);
        }
        showcase::perf::river_scene(&mut g, Vec2::new(320.0, 240.0), audio);
        for _ in 0..showcase::perf::WARM_FRAMES {
            g.render(renderer);
        }
        let idle = InputState::default();
        let mut times = trino_core::perf::FrameTimes::default();
        let v0 = runtime::vblanks();
        renderer.start_profile();
        for _ in 0..showcase::perf::MEASURE_FRAMES {
            let t0 = runtime::now_us();
            g.update(&idle, audio, showcase::perf::DT);
            let t1 = runtime::now_us();
            g.render(renderer);
            let t2 = runtime::now_us();
            times.update += t1.wrapping_sub(t0);
            times.render += t2.wrapping_sub(t1);
            audio.poll();
        }
        times.vblanks = runtime::vblanks().wrapping_sub(v0);
        if let Some(p) = renderer.take_profile() {
            let mut report = alloc::string::String::new();
            let _ = p.write_report(showcase::perf::NAME, &times, &mut report);
            runtime::log(&report);
        }
        // The third emulator golden: the river under the scene camera
        // (`tests/golden/n64/showcase_river.png`).
        runtime::log("TRINO_STEP:river golden frame\n");
        g.render(renderer);
        renderer.dump_framebuffer();
        renderer.set_rsp_stream(stream0);
    }

    /// The fourth emulator golden: blobs on a 256 m floor out to 100 m
    /// (`tests/golden/n64/shadows_far.png`), where the RDP's compressed z
    /// is coarsest: none may fight the floor.
    #[inline(never)]
    fn shadows_golden(renderer: &mut N64Renderer) {
        runtime::log("TRINO_STEP:shadows golden frame\n");
        castle64::selftest::shadow_far_scene(renderer);
        renderer.dump_framebuffer();
    }

    /// Step `shadow cost`: the 22 blob casters of `shadow_crowd_scene`
    /// timed with their blobs and without, per stage on the console's
    /// clock; the difference per blob is `N64_COST`'s blob term. Reported
    /// as `TRINO_SHADOW_COST:`; the frame with every blob must fit 30 fps
    /// (a Solid Eleven match's crowd, ADR 0025).
    #[inline(never)]
    fn shadow_cost(renderer: &mut N64Renderer) {
        use castle64::selftest::{SHADOW_CROWD, shadow_crowd_scene};
        const FRAMES: u32 = 16;
        runtime::log("TRINO_STEP:shadow cost\n");
        let mut measure = |shadows: bool| {
            for _ in 0..4 {
                shadow_crowd_scene(renderer, shadows);
            }
            renderer.start_profile();
            let t0 = runtime::now_us();
            for _ in 0..FRAMES {
                shadow_crowd_scene(renderer, shadows);
            }
            let total = runtime::now_us().wrapping_sub(t0) / FRAMES;
            let p = renderer.take_profile();
            let stage = |f: fn(&trino_core::perf::StageTimes) -> u32| {
                p.as_ref().map_or(0, |p| f(&p.stages) / FRAMES)
            };
            let blobs = trino_core::Renderer::stats(renderer).shadows.blobs;
            // The frame without its vsync wait (`begin`), as the perf
            // ceilings count it.
            (
                total.saturating_sub(stage(|s| s.begin)),
                stage(|s| s.tnl),
                stage(|s| s.sort + s.submit),
                blobs,
            )
        };
        let (off, off_tnl, off_submit, _) = measure(false);
        let (on, on_tnl, on_submit, blobs) = measure(true);
        let per = |a: u32, b: u32| a.saturating_sub(b) / (blobs as u32).max(1);
        runtime::log(&format!(
            "TRINO_SHADOW_COST: casters={SHADOW_CROWD} blobs={blobs} frame_us={off}->{on} \
             blob_us={} tnl_us_per_blob={} submit_us_per_blob={}\n",
            per(on, off),
            per(on_tnl, off_tnl),
            per(on_submit, off_submit),
        ));
        if blobs as usize != SHADOW_CROWD {
            runtime::log("TRINO_TEST_FAIL:shadow cost: not every caster drew its blob\n");
        }
        if on > trino_core::perf::FRAME_30FPS_US {
            runtime::log(&format!(
                "TRINO_TEST_FAIL:shadow cost: {SHADOW_CROWD} blob casters take {on} us, past \
                 the 30 fps frame\n"
            ));
        }
    }

    /// The hub perf scene's first frame, dumped for the second golden.
    #[inline(never)]
    fn castle64_golden(renderer: &mut N64Renderer, audio: &mut N64Audio) {
        // The RSP T&L path against the CPU path on a real frame: the hub
        // drawn both ways, 40x30 samples. Different cells mean the two
        // paths disagree (depth ties, lighting, a dropped mesh).
        let mut maps = alloc::vec![[[0u8; 40]; 30]; 2];
        for (k, rsp) in [false, true].into_iter().enumerate() {
            renderer.set_rsp(rsp);
            let mut g = alloc::boxed::Box::new(castle64::perf::scene_game(
                &castle64::perf::SCENES[0],
                Vec2::new(320.0, 240.0),
                audio,
            ));
            g.render(renderer);
            for y in 0..30u32 {
                for x in 0..40u32 {
                    let c = trino_core::Renderer::read_pixel(renderer, x * 8 + 4, y * 8 + 4)
                        .unwrap_or(trino_core::Color::BLACK);
                    maps[k][y as usize][x as usize] =
                        ((c.r as u32 + c.g as u32 * 3 + c.b as u32) / 5) as u8;
                }
            }
        }
        let mut differ = 0;
        for y in 0..30 {
            for x in 0..40 {
                differ += (maps[0][y][x].abs_diff(maps[1][y][x]) > 20) as u32;
            }
        }
        runtime::log(&format!(
            "TRINO_RSP:hub frame, RSP vs CPU T&L: {differ} of 1200 samples differ\n"
        ));
        if differ > 24 {
            for y in 0..30 {
                let line: alloc::string::String = (0..40)
                    .map(|x| {
                        if maps[0][y][x].abs_diff(maps[1][y][x]) > 20 {
                            '#'
                        } else {
                            '.'
                        }
                    })
                    .collect();
                runtime::log(&format!("TRINO_RSP:diff {y:02} {line}\n"));
            }
            runtime::log("TRINO_TEST_FAIL:rsp: the hub frame differs from the CPU path\n");
        }
        renderer.set_rsp(true);
        runtime::log("TRINO_STEP:castle64 golden frame\n");
        let mut g = alloc::boxed::Box::new(castle64::perf::scene_game(
            &castle64::perf::SCENES[0],
            Vec2::new(320.0, 240.0),
            audio,
        ));
        g.render(renderer);
        renderer.dump_framebuffer();
    }

    /// One empty frame: the audio polled, a vblank passes.
    fn audio_frame(renderer: &mut N64Renderer, audio: &mut N64Audio) {
        audio.poll();
        trino_core::Renderer::begin_frame(renderer, trino_core::Color::BLACK);
        trino_core::Renderer::end_frame(renderer);
    }

    /// Step `audio codecs` (issue #53): each codec plays and finishes, the
    /// stream delivers with no underrun, a duck rule ramps (castle64's
    /// `AudioCodecs`), then the counters as `TRINO_AUDIO:`.
    #[inline(never)]
    fn audio_codecs(renderer: &mut N64Renderer, audio: &mut N64Audio) {
        runtime::log("TRINO_STEP:audio codecs\n");
        match castle64::selftest::audio_codecs(audio, |a| audio_frame(renderer, a)) {
            Ok(s) => runtime::log(&format!(
                "TRINO_AUDIO: voices={} streams={} resident_bytes={} stream_bytes={} \
                 underruns={} streamed_frames={}\n",
                s.voices,
                s.streams,
                s.resident_bytes,
                s.stream_bytes,
                s.underruns,
                s.streamed_frames
            )),
            Err(e) => {
                runtime::log(&format!("TRINO_TEST_FAIL:audio codecs: {e}\n"));
                loop {
                    trino_core::Renderer::begin_frame(renderer, trino_core::Color::BLACK);
                    trino_core::Renderer::end_frame(renderer);
                }
            }
        }
    }

    /// The audio's cost on the console's clock (issue #53, the audio terms
    /// of `N64_COST`): `poll` timed with nothing playing, the theme
    /// (VADPCM, 44.1 kHz) alone, with two more VADPCM loops (22.05 kHz), and
    /// the Opus track; per second of output, so the frame rate drops out.
    /// Reported as `TRINO_AUDIO_COST:`, never gated.
    #[inline(never)]
    fn audio_cost(renderer: &mut N64Renderer, audio: &mut N64Audio) {
        use castle64::selftest::{AUDIO_OPUS, AUDIO_STREAM, WATER};
        use trino_core::{Audio, SoundId, SoundParams};
        const FLOW: SoundId = SoundId::from_path("sounds/river_flow");
        const WARM: u32 = 8;
        const FRAMES: u32 = 60;
        runtime::log("TRINO_STEP:audio cost\n");
        let mut measure = |name: &str, audio: &mut N64Audio, frames: u32| {
            for _ in 0..WARM {
                audio_frame(renderer, audio);
            }
            let c0 = audio.cost();
            for _ in 0..frames {
                audio_frame(renderer, audio);
            }
            let c1 = audio.cost();
            let samples = c1.samples().saturating_sub(c0.samples()).max(1);
            let per_s = |us: u32| us as u64 * c1.rate as u64 / samples;
            runtime::log(&format!(
                "TRINO_AUDIO_COST:{name} us_per_s={} rsp_us_per_s={} voices={} buffers={}\n",
                per_s(c1.poll_us.wrapping_sub(c0.poll_us)),
                per_s(c1.rsp_us.wrapping_sub(c0.rsp_us)),
                audio.audio_stats().voices,
                c1.buffers.wrapping_sub(c0.buffers)
            ));
        };
        audio.stop_music();
        measure("idle", audio, FRAMES);
        audio.play_music(AUDIO_STREAM, true);
        measure("vadpcm-1", audio, FRAMES);
        let a = audio.play_voice(WATER, &SoundParams::DEFAULT, true);
        let b = audio.play_voice(FLOW, &SoundParams::DEFAULT, true);
        measure("vadpcm-3", audio, FRAMES);
        for v in [a, b].into_iter().flatten() {
            audio.stop_voice(v);
        }
        audio.stop_music();
        // The Opus track is 0.66 s: measured while it plays.
        audio.play_music(AUDIO_OPUS, false);
        measure("opus-1", audio, 30);
        audio.stop_music();
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
        runtime::watchdog_start();
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
        asset_groups(renderer, audio, platform);
        runtime::log("TRINO_STEP:audio\n");

        if let Err(e) = castle64::selftest::audio_checks(audio) {
            runtime::log(&format!("TRINO_TEST_FAIL:audio: {e}\n"));
            loop {
                trino_core::Renderer::begin_frame(renderer, trino_core::Color::BLACK);
                trino_core::Renderer::end_frame(renderer);
            }
        }
        audio_codecs(renderer, audio);
        audio_cost(renderer, audio);
        runtime::log("TRINO_STEP:input\n");
        if let Err(e) = castle64::selftest::idle_input_checks(&mut N64Input) {
            runtime::log(&format!("TRINO_TEST_FAIL:{e}\n"));
            loop {
                trino_core::Renderer::begin_frame(renderer, trino_core::Color::BLACK);
                trino_core::Renderer::end_frame(renderer);
            }
        }
        // Its own frame: the showcase games (~22 KB each) must be gone from
        // the stack before castle64 renders on top of this function.
        let mut showcase = boxed_showcase();
        showcase_demo(&mut showcase, renderer, audio);
        showcase_baked(&mut showcase, renderer, audio, platform);
        showcase_golden(&mut showcase, renderer);
        drop(showcase);
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
        let heap_peak = core::cell::Cell::new(0u32);
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
            // The heap's peak for `check_heap`, every 8th frame (outside the
            // timed part: `mallinfo` walks the free lists).
            if frame_no % 8 == 0 {
                heap_peak.set(heap_peak.get().max(runtime::heap().0));
            }
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
                    // RSP T&L on (the default, gated), then the same scenes
                    // on the CPU path as `cpu-<scene>` (same ROM, ungated):
                    // the A/B without code-layout noise.
                    for (prefix, rsp) in [("", true), ("cpu-", false)] {
                        renderer.set_rsp(rsp);
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
                                let name = format!("{prefix}{}", scene.name);
                                let _ = p.write_report(&name, &times, &mut report);
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
                    }
                    renderer.set_rsp(true);
                    force_render.set(false);
                    // A second emulator golden: castle64's hub through the
                    // RSP T&L path (`tests/golden/n64/castle64_hub.png`).
                    castle64_golden(renderer, audio);
                    // 6) Fixed costs of the T&L (synthetic draw sets,
                    // ungated): after the castle64 scenes, so their camera
                    // and light cannot leak into those.
                    micro::run(renderer, castle64::perf::MEASURE_FRAMES);
                    // 7) Water (ADR 0015): the showcase's river under its
                    // scene camera, the hero wading in (gated like the
                    // castle64 scenes; the river's own line is `models/river`).
                    water_perf(renderer, audio);
                    // 8) Shadows (ADR 0024): the far-blob golden and what
                    // a blob costs (ungated, the `N64_COST` blob term).
                    shadows_golden(renderer);
                    shadow_cost(renderer);
                    // High-water marks of the whole run (bot, perf scenes).
                    if check_stack("the bot and the perf scenes")
                        && check_heap("the bot and the perf scenes", platform, heap_peak.get())
                    {
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
