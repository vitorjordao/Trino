//! The browser self-test (`?selftest`): the consoles' self-test
//! (`apps/3ds`, `apps/n64`) on the Web backend, step for step, spread over
//! animation frames so the page stays live and the harness can watch.
//!
//! 1. `castle64::selftest::engine_checks` (text, blobs, a save to
//!    `localStorage`, textured 3D, texture formats, billboards, occlusion,
//!    two screens, skinning, palettes, instancing, local lights, AO, water,
//!    impostors, particles, portals), then audio and idle input. The
//!    probes read pixels back where the browser can wait for the GPU
//!    (WebGL2); on WebGPU they report themselves skipped (`TRINO_STEP:…
//!    skipped`), and the golden frames below hold the picture instead.
//!    Then `quality bench` (#55): the quality calibration as a page runs
//!    it on `?quality=auto`, one presented frame per animation frame timed
//!    by the intervals, done in 3 s with the renderer as it was. Then
//!    `audio codecs` (#53, `castle64::selftest::AudioCodecs`, a step
//!    per frame): Vorbis and PCM sounds play and end, a streamed track
//!    delivers samples with no underruns, a duck rule ramps. It needs the
//!    audio context running, so it waits for a gesture (the harness clicks
//!    the page); a context that never runs fails the step, and a browser
//!    without Web Audio skips it, saying so. Before it, `asset groups`
//!    (#56): the level groups' bundles download beside the pack while
//!    frames go on, `castle64::selftest::asset_groups` loads, draws and
//!    unloads them as on the consoles, then a soak of load/unload cycles
//!    runs over animation frames with the heap back at its baseline after
//!    (WebGL2 frees GPU objects only once the page's event loop turns).
//! 2. The showcase demo plays every coin to the goal through the river, and
//!    the same level baked as data plays through too.
//! 3. castle64: gravity, a walk, a jump and the bot's whole Green Hills run
//!    to its star.
//! 4. Solid Eleven's scripted match (its own asset pack): the sim hash must
//!    equal the PC's and the consoles' (`solid_eleven::selftest`).
//! 5. Golden frames read back at 640×480 — the showcase river and the
//!    castle64 hub — handed to the harness (`trino_frame`), which compares
//!    them with `tests/golden/web/`.
//!
//! `TRINO_TEST_PASS` or `TRINO_TEST_FAIL:<why>` ends the run.

use trino_core::{Button, Game, InputState, Platform, Vec2};
use trino_platform_web::js;
use trino_platform_web::{Flow, WebHost, run};
use trino_render_wgpu::PendingRead;

const DT: f32 = 1.0 / 60.0;
/// The golden frames' size (the Web profile's starting picture).
pub const GOLDEN: (u32, u32) = (640, 480);
/// Simulation time per animation frame for the long stages (ms).
const BUDGET_MS: u64 = 40;

/// Where the run is.
enum Stage {
    Engine,
    Bench(Box<BenchRun>),
    /// `asset groups`: frames spent waiting for the bundles' downloads.
    Groups(u32),
    GroupSoak(Box<GroupSoak>),
    Audio(Box<AudioRun>),
    Demos,
    Castle(Box<CastleRun>),
    LoadFootball,
    Football(Box<solid_eleven::SolidElevenGame>, u32),
    Golden(Vec<(&'static str, Option<PendingRead>)>),
    Done,
}

/// castle64's run, frame by frame.
struct CastleRun {
    game: castle64::Castle64Game,
    bot: castle64::bot::Bot,
    phase: u8,
    frames: u32,
    ground_y: f32,
    z0: f32,
    peak: f32,
}

/// Step `quality bench`, frame by frame.
struct BenchRun {
    bench: trino_render_wgpu::Bench,
    /// The page clock at the last bench frame (µs).
    last: Option<u64>,
    /// The renderer's render scale and MSAA before, which must come back.
    before: (f32, u32),
}

/// Step `audio codecs`, frame by frame.
struct AudioRun {
    step: castle64::selftest::AudioCodecs,
    /// When it started waiting for the context to run (page clock, µs).
    waiting_since: Option<u64>,
    /// Frames in a row that came on time while waiting to start.
    smooth: u32,
    frames: u32,
    /// The page clock at the step's start and its last frame, and the
    /// longest gap between two of its frames (µs): what a failure reports.
    started: Option<u64>,
    last: u64,
    max_gap: u64,
}

impl AudioRun {
    fn new() -> Self {
        AudioRun {
            step: castle64::selftest::AudioCodecs::new(),
            waiting_since: None,
            smooth: 0,
            frames: 0,
            started: None,
            last: 0,
            max_gap: 0,
        }
    }
}

/// Frames `asset groups` waits for its two bundles to download and decode.
const GROUP_WAIT_FRAMES: u32 = 600;

/// The page's backends and loader for `castle64::selftest::asset_groups`.
struct GroupHost<'a>(&'a mut WebHost);

impl castle64::selftest::GroupHost for GroupHost<'_> {
    fn parts(&mut self) -> (&mut dyn trino_core::Renderer, &mut dyn Platform) {
        (&mut self.0.renderer, &mut self.0.platform)
    }

    fn pump(&mut self) {
        self.0.pump_groups();
    }

    /// No heap check here: inside one animation frame WebGL2 can't free
    /// the GPU objects an unload dropped (wgpu waits on GL fences, which
    /// signal once the event loop turns). [`soak_frame`] checks the heap
    /// over real frames instead.
    fn heap_tolerance(&self) -> u32 {
        u32::MAX
    }
}

/// Frames the soak loads and unloads `selftest_level` for, and the frames
/// it lets go by before reading the heap on either side.
const SOAK_FRAMES: u32 = 240;
const SETTLE_FRAMES: u32 = 20;
/// How far the heap may end above its baseline: wgpu's trackers grow a
/// little over frames, as on the PC.
const SOAK_TOLERANCE: u32 = 64 * 1024;

/// The `asset groups` soak over animation frames.
#[derive(Default)]
struct GroupSoak {
    frames: u32,
    cycles: u32,
    /// The heap after the first settle (none yet: settling).
    baseline: Option<u32>,
    /// The soak's frames are through: the last settle runs.
    soaked: bool,
    settle: u32,
}

/// One frame of the soak: settle, load/unload `selftest_level` for
/// [`SOAK_FRAMES`], settle, then the heap must be back (`Ok(true)`).
fn soak_frame(s: &mut GroupSoak, host: &mut WebHost) -> Result<bool, String> {
    use castle64::selftest::LEVEL;
    use trino_core::LoadState;
    host.pump_groups();
    let settling = s.baseline.is_none() || s.soaked;
    if settling {
        host.platform.unload_group(LEVEL);
        s.settle += 1;
        if s.settle >= SETTLE_FRAMES && host.platform.group_state(LEVEL) == LoadState::Unloaded {
            let heap = host.platform.memory().heap_used;
            s.settle = 0;
            match s.baseline {
                None => s.baseline = Some(heap),
                Some(base) => {
                    let drift = heap.saturating_sub(base);
                    js::log(&format!(
                        "TRINO_STEP:asset groups soak frames={} cycles={} heap_baseline={base} \
                         heap_drift={drift}",
                        s.frames, s.cycles
                    ));
                    if drift > SOAK_TOLERANCE {
                        return Err(format!(
                            "asset groups: the heap ended {drift} bytes above its baseline \
                             after {} load/unload cycles",
                            s.cycles
                        ));
                    }
                    return Ok(true);
                }
            }
        }
    } else {
        match host.platform.load_group(LEVEL) {
            LoadState::Ready => {
                host.platform.unload_group(LEVEL);
                s.cycles += 1;
            }
            LoadState::Failed => return Err("asset groups: a group failed during the soak".into()),
            _ => {}
        }
        s.frames += 1;
        s.soaked = s.frames >= SOAK_FRAMES;
    }
    // A loading screen's frame.
    let r = &mut host.renderer;
    let caps = *trino_core::Renderer::caps(r);
    let (w, h) = (caps.screen_width as f32, caps.screen_height as f32);
    trino_core::Renderer::begin_frame(r, trino_core::Color::BLACK);
    let done = w * 0.6 * (s.frames as f32 / SOAK_FRAMES as f32).min(1.0);
    trino_core::Renderer::fill_rect(
        r,
        trino_core::Rect::new(w * 0.2, h * 0.5, done, 8.0),
        trino_core::Color::rgb(80, 220, 120),
    );
    trino_core::Renderer::end_frame(r);
    Ok(false)
}

/// One frame of `asset groups`: until both level groups downloaded and
/// decoded, pump; then unload them and run the consoles' check on the
/// downloaded bundles (`Ok(true)` once it passed).
fn groups_frame(host: &mut WebHost, frames: u32) -> Result<bool, String> {
    use castle64::selftest::{LEVEL, LEVEL2, STRIDER_GROUP};
    use trino_core::LoadState;
    host.pump_groups();
    let states = [LEVEL, LEVEL2, STRIDER_GROUP].map(|g| host.platform.group_state(g));
    if states.contains(&LoadState::Failed) {
        return Err("asset groups: a bundle failed to download or decode".into());
    }
    if states != [LoadState::Ready; 3] {
        if frames > GROUP_WAIT_FRAMES {
            return Err(format!(
                "asset groups: the bundles were not in after {frames} frames ({states:?})"
            ));
        }
        return Ok(false);
    }
    js::log(&format!(
        "TRINO_STEP:asset groups downloaded in {frames} frames"
    ));
    host.platform.unload_group(LEVEL);
    host.platform.unload_group(LEVEL2);
    host.platform.unload_group(STRIDER_GROUP);
    host.pump_groups();
    let t0 = host.platform.now_micros();
    // The soak runs over animation frames next (`soak_frame`).
    let r = castle64::selftest::asset_groups(&mut GroupHost(host), 0)?;
    castle64::selftest::log_groups(&mut host.platform, &r);
    // The animation steps (issue #65) on the strider's downloaded bundle.
    castle64::selftest::anim_steps(&mut GroupHost(host))?;
    js::log(&format!(
        "TRINO_STEP:asset groups took {}ms",
        (host.platform.now_micros() - t0) / 1000
    ));
    Ok(true)
}

/// How long `audio codecs` waits for the audio context to run (µs).
const AUDIO_WAIT_US: u64 = 30_000_000;
/// The step counts frames (a one-shot must outlive its first frames), so
/// it starts once frames come steadily: this many in a row, each within
/// [`SMOOTH_GAP_US`] of the last (the engine checks' GPU work drained).
const SMOOTH_FRAMES: u32 = 10;
const SMOOTH_GAP_US: u64 = 50_000;

/// A fetch in flight: its bytes (or error) once the browser has them.
type PackSlot = std::rc::Rc<std::cell::RefCell<Option<Result<Vec<u8>, String>>>>;

struct Run {
    stage: Stage,
    failed: Option<String>,
    frame_no: u32,
    /// The football pack's load, started by `LoadFootball`.
    pack: Option<PackSlot>,
}

pub fn start(host: WebHost) {
    js::log("TRINO_STEP:web selftest start");
    let mut state = Run {
        stage: Stage::Engine,
        failed: None,
        frame_no: 0,
        pack: None,
    };
    run(host, move |host, _dt| state.step(host));
}

impl Run {
    fn fail(&mut self, why: String) {
        js::log(&format!("TRINO_TEST_FAIL:{why}"));
        self.failed = Some(why);
        self.stage = Stage::Done;
    }

    fn step(&mut self, host: &mut WebHost) -> Flow {
        // A click or key (the harness clicks the page) resumes the audio
        // context, as a game's first gesture does.
        if host.input.take_gesture() {
            host.audio.unlock();
        }
        let started = host.platform.now_micros();
        let in_budget = |host: &WebHost| host.platform.now_micros() - started < BUDGET_MS * 1000;
        match &mut self.stage {
            Stage::Engine => {
                let r = &mut host.renderer;
                if let Err(e) = castle64::selftest::engine_checks(r, &mut host.platform) {
                    self.fail(format!("engine: {e}"));
                    return Flow::Continue;
                }
                host.platform.log("TRINO_STEP:audio\n");
                if let Err(e) = castle64::selftest::audio_checks(&mut host.audio) {
                    self.fail(format!("audio: {e}"));
                    return Flow::Continue;
                }
                host.platform.log("TRINO_STEP:input\n");
                if let Err(e) = castle64::selftest::idle_input_checks(&mut host.input) {
                    self.fail(e.to_string());
                    return Flow::Continue;
                }
                host.platform.log("TRINO_STEP:ktx2 transcode\n");
                if let Err(e) = ktx2_check(host) {
                    self.fail(format!("ktx2: {e}"));
                    return Flow::Continue;
                }
                host.platform.log("TRINO_STEP:asset groups\n");
                // The animation steps' strider too (they run at the end of
                // the consoles' check, on the downloaded bundles).
                for g in [
                    castle64::selftest::LEVEL,
                    castle64::selftest::LEVEL2,
                    castle64::selftest::STRIDER_GROUP,
                ] {
                    host.platform.load_group(g);
                }
                self.stage = Stage::Groups(0);
            }
            Stage::Groups(frames) => {
                *frames += 1;
                match groups_frame(host, *frames) {
                    Ok(false) => {}
                    Ok(true) => self.stage = Stage::GroupSoak(Box::default()),
                    Err(e) => self.fail(e),
                }
            }
            Stage::GroupSoak(soak) => match soak_frame(soak, host) {
                Ok(false) => {}
                Ok(true) => {
                    host.platform.log("TRINO_STEP:quality bench\n");
                    let before = (host.renderer.render_scale(), host.renderer.msaa());
                    let bench = host.renderer.bench_begin(
                        &trino_core::PresetTable::BUILT_IN,
                        trino_platform_web::BENCH_TARGET_US,
                        trino_platform_web::BENCH_BUDGET,
                        true,
                    );
                    self.stage = Stage::Bench(Box::new(BenchRun {
                        bench,
                        last: None,
                        before,
                    }));
                }
                Err(e) => self.fail(e),
            },
            Stage::Bench(b) => {
                let now = host.platform.now_micros();
                let interval = b.last.map(|t| (now - t).min(u32::MAX as u64) as u32);
                b.last = Some(now);
                if let Some(cal) = host.renderer.bench_step(&mut b.bench, interval) {
                    js::log(&format!("TRINO_STEP:quality bench {}", cal.summary()));
                    let after = (host.renderer.render_scale(), host.renderer.msaa());
                    if cal.results.is_empty() || cal.elapsed.as_secs_f32() > 3.0 {
                        self.fail(format!("quality bench: {}", cal.summary()));
                    } else if after != b.before {
                        let before = b.before;
                        self.fail(format!(
                            "quality bench: the renderer came back at {after:?}, not {before:?}"
                        ));
                    } else {
                        host.platform.log("TRINO_STEP:audio codecs\n");
                        self.stage = Stage::Audio(Box::new(AudioRun::new()));
                    }
                }
            }
            Stage::Audio(a) => match audio_frame(a, host, started) {
                Ok(false) => {}
                Ok(true) => self.stage = Stage::Demos,
                Err(e) => self.fail(e),
            },
            Stage::Demos => {
                host.platform.log("TRINO_STEP:showcase demo\n");
                let screen = Vec2::new(GOLDEN.0 as f32, GOLDEN.1 as f32);
                let mut demo = showcase::ShowcaseGame::with_screen(screen);
                let mut n = 0u32;
                let (renderer, audio) = (&mut host.renderer, &mut host.audio);
                let result = showcase::run_demo(&mut demo, |g, input| {
                    g.update(input, audio, DT);
                    if n.is_multiple_of(32) {
                        g.render(renderer);
                    }
                    n += 1;
                });
                if let Err(e) = result {
                    self.fail(format!("showcase: {e}"));
                    return Flow::Continue;
                }
                if demo.splashes == 0 {
                    self.fail("showcase: the demo never entered the river".into());
                    return Flow::Continue;
                }
                host.platform.log("TRINO_STEP:baked scene\n");
                let mut n = 0u32;
                let (renderer, audio) = (&mut host.renderer, &mut host.audio);
                let result = showcase::check_baked(&host.platform, |g, input| {
                    g.update(input, audio, DT);
                    if n.is_multiple_of(32) {
                        g.render(renderer);
                    }
                    n += 1;
                });
                if let Err(e) = result {
                    self.fail(format!("baked scene: {e}"));
                    return Flow::Continue;
                }
                host.platform.log("TRINO_STEP:castle64\n");
                let screen = host.screen();
                self.stage = Stage::Castle(Box::new(CastleRun {
                    game: castle64::Castle64Game::new(screen),
                    bot: castle64::bot::Bot::new(castle64::bot::GREEN_RUN),
                    phase: 0,
                    frames: 0,
                    ground_y: 0.0,
                    z0: 0.0,
                    peak: 0.0,
                }));
            }
            Stage::Castle(c) => {
                while in_budget(host) {
                    match castle_frame(c, host, &mut self.frame_no) {
                        Ok(false) => {}
                        Ok(true) => {
                            js::log(&format!(
                                "TRINO_STEP:castle64 bot reached the star in {} frames",
                                c.frames
                            ));
                            self.stage = Stage::LoadFootball;
                            break;
                        }
                        Err(e) => {
                            self.fail(e);
                            break;
                        }
                    }
                }
            }
            Stage::LoadFootball => {
                // Solid Eleven's own pack, fetched while frames go on.
                match &self.pack {
                    None => {
                        host.platform.log("TRINO_STEP:solid eleven pack\n");
                        let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
                        let fill = slot.clone();
                        wasm_bindgen_futures::spawn_local(async move {
                            let r = trino_platform_web::fetch("assets/solid-eleven.pak").await;
                            *fill.borrow_mut() = Some(r);
                        });
                        self.pack = Some(slot);
                    }
                    Some(slot) => {
                        let Some(result) = slot.borrow_mut().take() else {
                            return Flow::Continue;
                        };
                        match result.and_then(|bytes| host.load_pack_bytes(&bytes)) {
                            Ok(_) => {
                                host.platform.log("TRINO_STEP:solid eleven match\n");
                                let mut g =
                                    Box::new(solid_eleven::SolidElevenGame::new(host.screen()));
                                solid_eleven::selftest::begin(&mut g);
                                self.stage = Stage::Football(g, 0);
                            }
                            Err(e) => self.fail(format!("solid eleven pack: {e}")),
                        }
                    }
                }
            }
            Stage::Football(g, frame) => {
                while in_budget(host) && *frame < solid_eleven::selftest::FRAMES {
                    let input = solid_eleven::selftest::input(*frame);
                    g.update(&input, &mut host.audio, solid_eleven::selftest::DT);
                    if frame.is_multiple_of(10) {
                        g.render(&mut host.renderer);
                    }
                    *frame += 1;
                }
                if *frame >= solid_eleven::selftest::FRAMES {
                    js::log(&format!(
                        "TRINO_SE_HASH:{:#010x}",
                        solid_eleven::selftest::hash(g)
                    ));
                    match solid_eleven::selftest::check(g) {
                        Ok(()) => self.stage = Stage::Golden(Vec::new()),
                        Err(e) => self.fail(format!("solid eleven: {e}")),
                    }
                }
            }
            Stage::Golden(reads) => {
                if reads.is_empty() {
                    host.platform.log("TRINO_STEP:golden frames\n");
                    let saved = host.renderer.internal_size();
                    let msaa = host.renderer.msaa() > 1;
                    // Single-sampled, like the PC goldens (WebGL2 has no MSAA).
                    host.renderer.set_msaa(false);
                    host.renderer.set_internal_size(GOLDEN.0, GOLDEN.1);
                    let screen = Vec2::new(GOLDEN.0 as f32, GOLDEN.1 as f32);
                    // The showcase river under its scene camera, the hero
                    // wading in (the 3DS's fourth golden, at the Web's size).
                    let mut g = Box::new(showcase::ShowcaseGame::blank());
                    showcase::perf::river_scene(&mut g, screen, &mut host.audio);
                    for _ in 0..3 {
                        g.render(&mut host.renderer);
                    }
                    reads.push(("showcase", Some(host.renderer.start_read_offscreen())));
                    // The castle64 hub (the first perf scene, a fresh game).
                    let hub = &castle64::perf::SCENES[0];
                    let mut g = Box::new(castle64::perf::scene_game(hub, screen, &mut host.audio));
                    for _ in 0..3 {
                        g.render(&mut host.renderer);
                    }
                    reads.push(("castle64_hub", Some(host.renderer.start_read_offscreen())));
                    // One card per texture format, as sprites and textured
                    // 3D: what the `texture formats` probes check where a
                    // backend reads pixels back (the 3DS's fifth golden).
                    castle64::selftest::draw_texture_formats(&mut host.renderer, true);
                    reads.push((
                        "texture_formats",
                        Some(host.renderer.start_read_offscreen()),
                    ));
                    host.renderer.set_internal_size(saved.0, saved.1);
                    host.renderer.set_msaa(msaa);
                    return Flow::Continue;
                }
                for (name, read) in reads.iter_mut() {
                    if let Some(r) = read
                        && let Some(px) = host.renderer.poll_read(r)
                    {
                        let (w, h) = r.size();
                        js::trino_frame(name, w, h, &px);
                        js::log(&format!("TRINO_STEP:golden frame {name} {w}x{h}"));
                        *read = None;
                    }
                }
                if reads.iter().all(|(_, r)| r.is_none()) {
                    self.stage = Stage::Done;
                }
            }
            Stage::Done => {
                js::log(&host.meter.line("selftest"));
                js::log(&host.audio_line());
                match self.failed.take() {
                    None => {
                        js::log("TRINO_TEST_PASS");
                        js::trino_finish(true, None);
                    }
                    Some(e) => js::trino_finish(false, Some(e)),
                }
                return Flow::Stop;
            }
        }
        host.end_frame();
        Flow::Continue
    }
}

/// One frame of `audio codecs`; `Ok(true)` when it is through (or skipped
/// on a browser without Web Audio).
fn audio_frame(a: &mut AudioRun, host: &mut WebHost, now_us: u64) -> Result<bool, String> {
    if !host.audio.has_web_audio() {
        js::log("TRINO_STEP:audio codecs skipped: no Web Audio in this browser (silent backend)");
        return Ok(true);
    }
    if !host.audio.running() {
        let since = *a.waiting_since.get_or_insert_with(|| {
            js::log(&format!(
                "TRINO_STEP:audio codecs waits for the audio context ({})",
                host.audio.state()
            ));
            now_us
        });
        if now_us - since > AUDIO_WAIT_US {
            return Err(format!(
                "audio codecs: the audio context never ran ({} after {} s): no audio \
                 output, or no gesture reached the page",
                host.audio.state(),
                AUDIO_WAIT_US / 1_000_000
            ));
        }
        return Ok(false);
    }
    let gap = now_us.saturating_sub(a.last);
    a.last = now_us;
    if a.started.is_none() {
        a.smooth = if gap < SMOOTH_GAP_US { a.smooth + 1 } else { 0 };
        let since = *a.waiting_since.get_or_insert(now_us);
        if a.smooth < SMOOTH_FRAMES && now_us - since < AUDIO_WAIT_US {
            return Ok(false);
        }
        if a.smooth < SMOOTH_FRAMES {
            js::log("TRINO_STEP:audio codecs starts on uneven frames (they never settled)");
        }
        a.started = Some(now_us);
    } else {
        a.max_gap = a.max_gap.max(gap);
    }
    a.frames += 1;
    let timing = |a: &AudioRun, host: &WebHost| {
        format!(
            "{} frames in {} ms (longest gap {} ms), audio clock {:.3} s",
            a.frames,
            (now_us - a.started.unwrap_or(now_us)) / 1000,
            a.max_gap / 1000,
            host.audio.current_time()
        )
    };
    match a.step.step(&mut host.audio) {
        Ok(false) => Ok(false),
        Ok(true) => {
            let s = a.step.streamed;
            js::log(&host.audio_line());
            js::log(&format!(
                "TRINO_STEP:audio codecs ok, {}: rate={} streamed_frames={} \
                 stream_bytes={} resident_bytes={} underruns={}",
                timing(a, host),
                host.audio.sample_rate(),
                s.streamed_frames,
                s.stream_bytes,
                s.resident_bytes,
                s.underruns
            ));
            Ok(true)
        }
        Err(e) => {
            js::log(&host.audio_line());
            Err(format!("{e} ({})", timing(a, host)))
        }
    }
}

/// The browser's transcoder against the bake's: each Basis mode's fixture
/// (`xtask/src/web.rs` `ktx2_fixtures`) decodes to RGBA8 exactly as
/// Binomial's native transcoder did at build time, and transcodes to the
/// GPU format this browser uses.
fn ktx2_check(host: &mut WebHost) -> Result<(), String> {
    use trino_core::BlobId;
    /// The transcoder's RGBA32, and the GPU targets by name.
    const RGBA32: u32 = 13;
    let gpu = match host.texture_target {
        trino_platform_web::TextureTarget::Bc => Some(6),
        trino_platform_web::TextureTarget::Etc2 => Some(1),
        trino_platform_web::TextureTarget::Astc => Some(10),
        trino_platform_web::TextureTarget::Rgba8 => None,
    };
    for mode in ["etc1s", "uastc"] {
        let file = host
            .platform
            .blob(BlobId::from_path(&format!("web/ktx2_{mode}")))
            .ok_or("fixture missing from the pack")?;
        let want = host
            .platform
            .blob(BlobId::from_path(&format!("web/ktx2_{mode}.rgba")))
            .ok_or("expected decode missing from the pack")?;
        let out = js::trino_transcode_ktx2(file, RGBA32).map_err(|e| format!("{mode}: {e:?}"))?;
        let got = js_sys::Uint8Array::new(&out.get(3)).to_vec();
        if got != want {
            let differ = got.iter().zip(want).filter(|(a, b)| a != b).count();
            return Err(format!(
                "{mode}: the browser's decode differs from the bake's ({differ} bytes of {})",
                want.len()
            ));
        }
        if let Some(f) = gpu {
            let out = js::trino_transcode_ktx2(file, f).map_err(|e| format!("{mode}: {e:?}"))?;
            let len = js_sys::Uint8Array::new(&out.get(3)).length();
            if len == 0 {
                return Err(format!(
                    "{mode}: empty {} transcode",
                    host.texture_target.name()
                ));
            }
        }
    }
    js::log(&format!(
        "TRINO_STEP:ktx2 transcode etc1s+uastc decode exact, gpu target {}",
        host.texture_target.name()
    ));
    Ok(())
}

/// One castle64 frame of the run (the 3DS self-test's script): settle,
/// walk toward the castle, jump and land, then the bot to the star.
/// `Ok(true)` when the bot is done.
fn castle_frame(c: &mut CastleRun, host: &mut WebHost, frame_no: &mut u32) -> Result<bool, String> {
    let idle = InputState::default();
    let mut fwd = InputState::default();
    fwd.set(Button::DpadUp, true);
    let mut jump = InputState::default();
    jump.set(Button::A, true);
    let input = match c.phase {
        // 0) 60 frames: gravity settles the player on the hub floor.
        0 => idle,
        // 1) 30 frames walking toward the castle (+Z).
        1 => fwd,
        // 2) 14 frames holding A (variable-height jump), 3) falling.
        2 => jump,
        3 => idle,
        _ => c.bot.drive(&c.game),
    };
    c.game.update(&input, &mut host.audio, DT);
    if (*frame_no).is_multiple_of(4) {
        c.game.render(&mut host.renderer);
    }
    *frame_no += 1;
    c.frames += 1;
    let g = &c.game;
    match c.phase {
        0 if c.frames == 60 => {
            if !g.on_ground || g.vel.y != 0.0 {
                return Err(format!(
                    "castle64 did not settle: on_ground={}",
                    g.on_ground
                ));
            }
            (c.ground_y, c.z0, c.peak) = (g.pos.y, g.pos.z, g.pos.y);
            (c.phase, c.frames) = (1, 0);
        }
        1 if c.frames == 30 => {
            let walked = g.pos.z - c.z0;
            if walked < 1.5 {
                return Err(format!("castle64 walked only {walked}"));
            }
            (c.phase, c.frames) = (2, 0);
        }
        2 if c.frames == 14 => (c.phase, c.frames) = (3, 0),
        3 => {
            c.peak = c.peak.max(g.pos.y);
            if g.on_ground {
                if c.peak < c.ground_y + 1.0 {
                    return Err(format!(
                        "castle64 jump peak {} vs ground {}",
                        c.peak, c.ground_y
                    ));
                }
                if (g.pos.y - c.ground_y).abs() > 0.01 {
                    return Err(format!("castle64 landing y {} vs {}", g.pos.y, c.ground_y));
                }
                (c.phase, c.frames) = (4, 0);
            } else if c.frames > 150 {
                return Err("castle64 never landed".into());
            }
        }
        4 => {
            if c.bot.done() {
                if g.star_count() != 1 || g.level != 0 {
                    return Err(format!(
                        "castle64 bot run ended with {} stars, level {}",
                        g.star_count(),
                        g.level
                    ));
                }
                return Ok(true);
            }
            if c.bot.frames_in_step() > 1800 || c.frames > 60 * 100 {
                return Err(format!("castle64 bot stuck at step {}", c.bot.step_index()));
            }
        }
        _ => {}
    }
    Ok(false)
}
