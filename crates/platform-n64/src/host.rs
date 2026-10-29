//! A ready-made N64 host: what a game's console crate uses instead of
//! writing its own `trino_rust_main` (castle64's `apps/n64` keeps its own,
//! because its self-test also drives the engine's perf and RSP gates).
//!
//! ```ignore
//! // n64/src/lib.rs of a project (staticlib crate, `cargo xtask build n64`)
//! #![cfg_attr(target_os = "none", no_std)]
//! trino_platform_n64::n64_main!(my_game::MyGame::new, my_game::selftest::run);
//! ```
//!
//! The self-test (`cargo xtask test n64`) receives the [`N64Host`] and the
//! game, drives frames with [`N64Host::step`], may report frame times with
//! [`N64Host::report_frames`] (`TRINO_FRAME:` lines, gated by the project's
//! `n64_max_frame_us`) and dump goldens with [`N64Host::dump_framebuffer`].
//! Its `Ok` / `Err(reason)` becomes `TRINO_TEST_PASS` / `TRINO_TEST_FAIL`.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use trino_core::perf::ModelCost;
use trino_core::replay::ChannelSession;
use trino_core::{Game, Input, InputState, Vec2};

use crate::{N64Assets, N64Audio, N64Input, N64Platform, N64Renderer, runtime};

/// Everything a running N64 game talks to.
pub struct N64Host {
    pub renderer: N64Renderer,
    pub audio: N64Audio,
    pub input: N64Input,
    pub platform: N64Platform,
}

/// Frame-time summary of [`N64Host::measure`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameReport {
    pub frames: u32,
    /// Mean of `update + render` minus `begin_frame`'s vsync/RDP wait, µs.
    pub avg_us: u32,
    pub max_us: u32,
    /// Mean per stage, µs: `Game::update_with`, and the renderer's T&L,
    /// sort, submit, sprites (2D + text) and `end_frame`.
    pub update_us: u32,
    pub tnl_us: u32,
    pub sort_us: u32,
    pub submit_us: u32,
    pub sprites_us: u32,
    pub end_us: u32,
    /// Per frame: the part of `end_us` the CPU waited for the RSP and RDP
    /// to finish the frame (high: the frame is bound by them).
    pub drain_us: u32,
    /// Per frame: model draws sent to the RSP overlay vs all model draws
    /// that reached the T&L (culled ones included).
    pub rsp_draws: u32,
    pub draws: u32,
    pub culled: u32,
    /// Per frame: 2D draws (sprite blits, rectangles and text glyphs), the
    /// work behind `sprites_us`.
    pub draws2d: u32,
    /// Vertical blanks (60 Hz) the run took: the real frame rate, waits
    /// for vsync, the RSP and the RDP included (`fps = frames × 60 /
    /// vblanks`).
    pub vblanks: u32,
    /// The costliest models over the run: (model handle, calls per frame, µs per
    /// frame of T&L, RSP draws per frame), most expensive first.
    pub top: [(u32, u32, u32, u32); 8],
}

impl N64Host {
    /// The picture size games are created with.
    pub const SCREEN: Vec2 = Vec2::new(320.0, 240.0);

    /// Boot libdragon, print `TRINO_BOOT` and load every baked asset.
    pub fn boot() -> Self {
        runtime::init();
        runtime::log("TRINO_BOOT\n");
        // `trino.toml` `[n64] rsp_arena_kb` (GitHub #110): the RSP copies'
        // region, first, while the heap is one block.
        if let Some(kb) = N64Assets::read("/rsp_arena")
            .and_then(|b| core::str::from_utf8(&b).ok()?.trim().parse::<u32>().ok())
        {
            let ok = N64Renderer::reserve_rsp_arena(kb * 1024);
            runtime::log(&format!("TRINO_RSP_ARENA:{kb}KB ok={ok}\n"));
        }
        let mut host = N64Host {
            renderer: N64Renderer::new(),
            audio: N64Audio::new(),
            input: N64Input::new(),
            platform: N64Platform::new(),
        };
        N64Assets::load_all(&mut host.renderer, &mut host.audio, &mut host.platform);
        host
    }

    /// `cargo xtask test n64` put a `test_mode` file in the DFS.
    pub fn test_mode() -> bool {
        N64Assets::exists("/test_mode")
    }

    pub fn log(&self, msg: &str) {
        runtime::log(msg);
    }

    /// One frame with real controller input: the four ports as players
    /// (`Platform::players`, issue #67), port 1 as the game's `input`.
    pub fn frame(&mut self, game: &mut dyn Game, dt: f32) {
        let players = self.poll_players();
        self.step(game, &players.players[0], dt);
    }

    /// Poll the ports, apply the game's seat requests and publish the
    /// players to the platform.
    pub fn poll_players(&mut self) -> trino_core::PlayersInput {
        while let Some(r) = self.platform.players.take_request() {
            self.input.seats_mut().apply(r);
        }
        let players = self.input.poll_players();
        self.platform.players.publish(self.input.seats());
        players
    }

    /// One frame with scripted input (self-tests, bots): the asset-group
    /// loader's step (issue #56), the game, the audio.
    pub fn step(&mut self, game: &mut dyn Game, input: &InputState, dt: f32) {
        N64Assets::pump(&mut self.renderer, &mut self.audio, &mut self.platform);
        let zone = trino_core::trace::Zone::begin("update");
        game.update_with(&mut self.platform, input, &mut self.audio, dt);
        zone.end();
        let zone = trino_core::trace::Zone::begin("render");
        game.render(&mut self.renderer);
        zone.end();
        let zone = trino_core::trace::Zone::begin("audio");
        self.audio.poll();
        zone.end();
        self.platform.sample_heap();
    }

    /// Run `frames` scripted frames (`input(i)` per frame) and time them
    /// in emulated N64 microseconds, excluding `begin_frame`'s vsync wait.
    pub fn measure(
        &mut self,
        game: &mut dyn Game,
        frames: u32,
        dt: f32,
        mut input: impl FnMut(u32) -> InputState,
    ) -> FrameReport {
        let mut total: u64 = 0;
        let mut max = 0u32;
        let mut sums = [0u64; 11];
        let v0 = runtime::vblanks();
        let mut models: [ModelCost; 48] = [ModelCost::default(); 48];
        let mut n_models = 0usize;
        for i in 0..frames {
            let state = input(i);
            self.renderer.start_profile();
            let t0 = runtime::now_us();
            game.update_with(&mut self.platform, &state, &mut self.audio, dt);
            let t1 = runtime::now_us();
            game.render(&mut self.renderer);
            self.audio.poll();
            let spent = runtime::now_us().wrapping_sub(t0);
            sums[10] += crate::renderer::take_drain_us() as u64;
            sums[9] += trino_core::Renderer::stats(&self.renderer).sprites as u64;
            let p = self.renderer.take_profile();
            let wait = p.as_ref().map_or(0, |p| p.stages.begin);
            let us = spent.saturating_sub(wait);
            total += us as u64;
            max = max.max(us);
            sums[0] += t1.wrapping_sub(t0) as u64;
            if let Some(p) = p {
                let st = p.stages;
                for (k, v) in [st.tnl, st.sort, st.submit, st.sprites, st.end]
                    .iter()
                    .enumerate()
                {
                    sums[1 + k] += *v as u64;
                }
                sums[6] += p.tnl.rsp_draws as u64;
                let draws: u32 = p.models[..p.model_count]
                    .iter()
                    .map(|m| m.calls)
                    .sum::<u32>()
                    + p.other.calls;
                sums[7] += draws as u64;
                sums[8] += p.tnl.culled as u64;

                for m in &p.models[..p.model_count] {
                    match models[..n_models].iter().position(|x| x.model == m.model) {
                        Some(i) => {
                            let x = &mut models[i];
                            x.calls += m.calls;
                            x.us += m.us;
                            x.tnl.rsp_draws += m.tnl.rsp_draws;
                        }
                        None if n_models < models.len() => {
                            models[n_models] = *m;
                            n_models += 1;
                        }
                        None => {}
                    }
                }
            }
        }
        let vblanks = runtime::vblanks().wrapping_sub(v0);
        let n = (frames as u64).max(1);
        models[..n_models].sort_unstable_by(|a, b| b.us.cmp(&a.us));
        let mut top = [(0u32, 0, 0, 0); 8];
        for (t, m) in top.iter_mut().zip(models[..n_models].iter()) {
            let f = frames.max(1);
            *t = (m.model.0, m.calls / f, m.us / f, m.tnl.rsp_draws / f);
        }
        FrameReport {
            vblanks,
            top,
            frames,
            avg_us: (total / n) as u32,
            max_us: max,
            update_us: (sums[0] / n) as u32,
            tnl_us: (sums[1] / n) as u32,
            sort_us: (sums[2] / n) as u32,
            submit_us: (sums[3] / n) as u32,
            sprites_us: (sums[4] / n) as u32,
            end_us: (sums[5] / n) as u32,
            drain_us: (sums[10] / n) as u32,
            rsp_draws: (sums[6] / n) as u32,
            draws: (sums[7] / n) as u32,
            culled: (sums[8] / n) as u32,
            draws2d: (sums[9] / n) as u32,
        }
    }

    /// Print a [`FrameReport`] as `TRINO_FRAME:<scene> avg_us= max_us=
    /// frames=` — `cargo xtask test n64` gates it on `n64_max_frame_us`.
    pub fn report_frames(&self, scene: &str, r: &FrameReport) {
        runtime::log(&format!(
            "TRINO_FRAME:{scene} avg_us={} max_us={} frames={} update={} tnl={} sort={} \
             submit={} sprites={} end={} drain={} draws={} rsp={} culled={} draws2d={} vblanks={} \
             fps={} {}\n",
            r.avg_us,
            r.max_us,
            r.frames,
            r.update_us,
            r.tnl_us,
            r.sort_us,
            r.submit_us,
            r.sprites_us,
            r.end_us,
            r.drain_us,
            r.draws,
            r.rsp_draws,
            r.culled,
            r.draws2d,
            r.vblanks,
            (r.frames * 60).checked_div(r.vblanks).unwrap_or(0),
            trino_core::Platform::memory(&self.platform).heap_field()
        ));
        for (id, calls, us, rsp) in r.top.iter().filter(|t| t.1 > 0) {
            runtime::log(&format!(
                "TRINO_FRAME_MODEL:{scene} {id:08x} calls={calls} tnl_us={us} rsp={rsp}\n"
            ));
        }
    }

    /// Print the last presented frame (`TRINO_FBR:` rows): the next entry
    /// of the project's `n64_goldens`.
    pub fn dump_framebuffer(&mut self) {
        self.renderer.dump_framebuffer();
    }

    /// Deepest main-stack use since boot, bytes (the stack is 64 KB).
    pub fn stack_used(&self) -> u32 {
        runtime::stack_used()
    }

    /// What rollback netplay would cost on this console (issue #67): per
    /// frame, a snapshot of `game` ([`Game::save_state`]), one sim update
    /// with no drawing, and a load of the snapshot back; `window` updates
    /// in a row is a full rollback of that many frames. The sim runs on a
    /// [`Neutral`](trino_core::replay::Neutral) platform (no save writes)
    /// with no sound. `None` when the game takes no snapshots.
    pub fn rollback_cost(
        &mut self,
        game: &mut dyn Game,
        frames: u32,
        dt: f32,
        mut input: impl FnMut(u32) -> InputState,
    ) -> Option<trino_core::perf::RollbackCost> {
        let size = game.state_size();
        if size == 0 || frames == 0 {
            return None;
        }
        let mut buf = alloc::vec![0u8; size];
        let mut c = trino_core::perf::RollbackCost {
            snapshot_bytes: size as u32,
            ..Default::default()
        };
        let mut mute = trino_core::perf::Silent;
        for i in 0..frames {
            let state = input(i);
            let t0 = runtime::now_us();
            game.save_state(&mut buf)?;
            let t1 = runtime::now_us();
            {
                let mut neutral = trino_core::replay::Neutral(&mut self.platform);
                game.update_with(&mut neutral, &state, &mut mute, dt);
            }
            let t2 = runtime::now_us();
            game.load_state(&buf);
            let t3 = runtime::now_us();
            c.save_us += t1.wrapping_sub(t0);
            c.update_us += t2.wrapping_sub(t1);
            c.load_us += t3.wrapping_sub(t2);
            // Move on (the next frame's state is the updated one).
            let mut neutral = trino_core::replay::Neutral(&mut self.platform);
            game.update_with(&mut neutral, &state, &mut mute, dt);
        }
        c.save_us /= frames;
        c.update_us /= frames;
        c.load_us /= frames;
        Some(c)
    }

    /// The replay or recording this ROM was built for (issue #67): a
    /// `/replay.trec` in the DFS replays it (`/replay_until` stops early,
    /// `/replay_shot` dumps the last frame), a `/record` file (the game's
    /// name) records the live input over ISViewer/USB (`cargo xtask replay
    /// run --platform n64`, `cargo xtask run n64 --record`). `None`: play.
    pub fn channel_session(
        &mut self,
        game: &dyn Game,
    ) -> Option<ChannelSession<'static, fn(&str)>> {
        let log: fn(&str) = runtime::log;
        if let Some(bytes) = N64Assets::read("/replay.trec") {
            let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
            let until = N64Assets::read("/replay_until").and_then(|b| decimal(&b));
            return match ChannelSession::replay(bytes, until, "n64", log) {
                Ok(mut s) => {
                    s.start(game);
                    Some(s)
                }
                Err(e) => {
                    runtime::log(&format!("TRINO_REPLAY: {}\n", e.message()));
                    None
                }
            };
        }
        let name = N64Assets::read("/record")?;
        let name = core::str::from_utf8(name.trim_ascii()).ok()?;
        runtime::log(&format!("TRINO_RECORD: n64 {name}\n"));
        Some(ChannelSession::record(
            game,
            name,
            (320, 240),
            1,
            "n64",
            log,
        ))
    }

    /// The real-time loop (a replay or a recording when the ROM asks for
    /// one: [`channel_session`](Self::channel_session)). Never returns.
    pub fn run(&mut self, game: &mut dyn Game) -> ! {
        self.run_named(game, "n64-game")
    }

    /// [`run`](Self::run) with the trace session's name (`run n64
    /// --trace`; castle64's ROM names its own).
    pub fn run_named(&mut self, game: &mut dyn Game, trace: &'static str) -> ! {
        let mut session = self.channel_session(game);
        let mut last = runtime::now_us();
        let mut trace = runtime::TraceLoop::start(trace);
        loop {
            let now = runtime::now_us();
            let dt = (now.wrapping_sub(last) as f32 / 1_000_000.0).min(0.1);
            last = now;
            match &mut session {
                None => self.frame(game, dt),
                Some(s) => {
                    let live = self.poll_players();
                    match s.input(&live, dt) {
                        Some((players, dt)) => {
                            if s.is_replay() {
                                self.platform.players.publish_players(&players);
                            }
                            self.step(game, &players.players[0], dt);
                            s.after(game, &players, dt);
                        }
                        None => {
                            // The replay ended: its verdict, its last frame.
                            if let Some(s) = session.take() {
                                s.finish();
                            }
                            if N64Assets::exists("/replay_shot") {
                                self.dump_framebuffer();
                            }
                            runtime::log("TRINO_REPLAY_DONE\n");
                            loop {
                                game.render(&mut self.renderer);
                            }
                        }
                    }
                }
            }
            trace.frame();
        }
    }

    /// The entry the [`n64_main!`](crate::n64_main) macro expands to: boot,
    /// build the game on the heap, run the self-test in test mode
    /// (reporting its verdict, then idling on neutral input), else play.
    pub fn main<G: Game>(
        make: impl FnOnce(Vec2) -> G,
        self_test: impl FnOnce(&mut N64Host, &mut G) -> Result<(), String>,
        setup: impl FnOnce(&mut N64Host),
    ) -> ! {
        let mut host = N64Host::boot();
        setup(&mut host);
        let mut game = boxed(make);
        if Self::test_mode() {
            runtime::watchdog_start();
            let verdict = self_test(&mut host, &mut game);
            // The stack guard of the engine's own self-test: an overflow
            // corrupts memory silently (AGENTS.md › Pitfalls).
            let stack = host.stack_used();
            match verdict {
                Ok(()) if stack > 60 * 1024 => runtime::log(&format!(
                    "TRINO_TEST_FAIL:stack {stack} bytes of 64 KB used\n"
                )),
                Ok(()) => {
                    let (used, total) = runtime::heap();
                    runtime::log(&format!(
                        "TRINO_STACK:{stack}\nTRINO_HEAP:{used}/{total}\nTRINO_TEST_PASS\n"
                    ))
                }
                Err(e) => runtime::log(&format!("TRINO_TEST_FAIL:{e}\n")),
            }
            loop {
                host.step(&mut *game, &InputState::default(), 1.0 / 60.0);
            }
        }
        host.run(&mut *game)
    }
}

/// Build the game straight into a box from a frame that is popped right
/// after: `Box::new(big())` in the caller would keep the temporary in its
/// frame for the whole run (the 64 KB stack pitfall).
#[inline(never)]
fn boxed<G>(make: impl FnOnce(Vec2) -> G) -> Box<G> {
    // Allocate first and write the game straight into the box: the
    // compiler can then build it in place instead of on the stack and copy
    // it over (a large game is tens of KB of a 64 KB stack).
    let mut b = Box::<G>::new_uninit();
    b.write(make(N64Host::SCREEN));
    // SAFETY: just written.
    unsafe { b.assume_init() }
}

/// A file's decimal number (`replay_until`), by hand: `str::parse` and
/// `trim` would link their generic code into every N64 image.
fn decimal(bytes: &[u8]) -> Option<u32> {
    let digits = bytes.trim_ascii();
    if digits.is_empty() {
        return None;
    }
    digits.iter().try_fold(0u32, |n, &b| {
        b.is_ascii_digit()
            .then(|| n.checked_mul(10)?.checked_add(u32::from(b - b'0')))
            .flatten()
    })
}
