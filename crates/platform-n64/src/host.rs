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
    /// Per frame: model draws sent to the RSP overlay vs all model draws
    /// that reached the T&L (culled ones included).
    pub rsp_draws: u32,
    pub draws: u32,
    pub culled: u32,
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
        let mut host = N64Host {
            renderer: N64Renderer::new(),
            audio: N64Audio::new(),
            input: N64Input,
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

    /// One frame with real controller input.
    pub fn frame(&mut self, game: &mut dyn Game, dt: f32) {
        let state = self.input.poll();
        self.step(game, &state, dt);
    }

    /// One frame with scripted input (self-tests, bots).
    pub fn step(&mut self, game: &mut dyn Game, input: &InputState, dt: f32) {
        game.update_with(&mut self.platform, input, &mut self.audio, dt);
        game.render(&mut self.renderer);
        self.audio.poll();
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
        let mut sums = [0u64; 9];
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
        let n = (frames as u64).max(1);
        models[..n_models].sort_unstable_by(|a, b| b.us.cmp(&a.us));
        let mut top = [(0u32, 0, 0, 0); 8];
        for (t, m) in top.iter_mut().zip(models[..n_models].iter()) {
            let f = frames.max(1);
            *t = (m.model.0, m.calls / f, m.us / f, m.tnl.rsp_draws / f);
        }
        FrameReport {
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
            rsp_draws: (sums[6] / n) as u32,
            draws: (sums[7] / n) as u32,
            culled: (sums[8] / n) as u32,
        }
    }

    /// Print a [`FrameReport`] as `TRINO_FRAME:<scene> avg_us= max_us=
    /// frames=` — `cargo xtask test n64` gates it on `n64_max_frame_us`.
    pub fn report_frames(&self, scene: &str, r: &FrameReport) {
        runtime::log(&format!(
            "TRINO_FRAME:{scene} avg_us={} max_us={} frames={} update={} tnl={} sort={} \
             submit={} sprites={} end={} draws={} rsp={} culled={}\n",
            r.avg_us,
            r.max_us,
            r.frames,
            r.update_us,
            r.tnl_us,
            r.sort_us,
            r.submit_us,
            r.sprites_us,
            r.end_us,
            r.draws,
            r.rsp_draws,
            r.culled
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

    /// The real-time loop. Never returns.
    pub fn run(&mut self, game: &mut dyn Game) -> ! {
        let mut last = runtime::now_us();
        loop {
            let now = runtime::now_us();
            let dt = (now.wrapping_sub(last) as f32 / 1_000_000.0).min(0.1);
            last = now;
            self.frame(game, dt);
        }
    }

    /// The entry the [`n64_main!`](crate::n64_main) macro expands to: boot,
    /// build the game on the heap, run the self-test in test mode
    /// (reporting its verdict, then idling on neutral input), else play.
    pub fn main<G: Game>(
        make: impl FnOnce(Vec2) -> G,
        self_test: impl FnOnce(&mut N64Host, &mut G) -> Result<(), String>,
    ) -> ! {
        let mut host = N64Host::boot();
        let mut game = boxed(make);
        if Self::test_mode() {
            let verdict = self_test(&mut host, &mut game);
            // The stack guard of the engine's own self-test: an overflow
            // corrupts memory silently (AGENTS.md › Pitfalls).
            let stack = host.stack_used();
            match verdict {
                Ok(()) if stack > 60 * 1024 => runtime::log(&format!(
                    "TRINO_TEST_FAIL:stack {stack} bytes of 64 KB used\n"
                )),
                Ok(()) => runtime::log(&format!("TRINO_STACK:{stack}\nTRINO_TEST_PASS\n")),
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
    Box::new(make(N64Host::SCREEN))
}
