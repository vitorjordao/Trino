//! A ready-made 3DS host: what a game's console crate uses instead of
//! writing its own `trino_rust_main` (castle64's `apps/3ds` keeps its own
//! for the engine's gates). Same shape as `trino_platform_n64::N64Host`:
//!
//! ```ignore
//! // n3ds/src/main.rs of a project (bin crate, `cargo xtask build 3ds`)
//! #![cfg_attr(target_os = "horizon", no_std)]
//! #![cfg_attr(target_os = "horizon", no_main)]
//! trino_platform_3ds::n3ds_main!(my_game::MyGame::new, my_game::selftest::run);
//! ```
//!
//! The self-test's `Ok` / `Err(reason)` becomes `TRINO_TEST_PASS` /
//! `TRINO_TEST_FAIL` over `svcOutputDebugString`. Unlike the N64, the loop
//! returns when the applet manager asks (HOME / power), so the shim can
//! tear the OS services down.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use trino_core::render3d::GpuReject;
use trino_core::{Game, Input, InputState, Vec2};

use crate::{N3dsAssets, N3dsAudio, N3dsInput, N3dsPlatform, N3dsRenderer, runtime};

/// Everything a running 3DS game talks to.
pub struct N3dsHost {
    pub renderer: N3dsRenderer,
    pub audio: N3dsAudio,
    pub input: N3dsInput,
    pub platform: N3dsPlatform,
}

/// Frame-time summary of [`N3dsHost::measure`]. Azahar is not cycle
/// accurate: report these, never gate on them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameReport {
    pub frames: u32,
    /// Wall clock per frame, vsync and the wait for the GPU included.
    pub avg_us: u32,
    pub max_us: u32,
    /// Per frame: the ARM11's own time (frame time minus `begin_frame`'s
    /// wait for the previous frame's GPU work and vsync).
    pub cpu_us: u32,
    /// Per frame: model draws the GPU transformed and lit (ADR 0012) and
    /// their source triangles, the model draws left on the CPU T&L, and
    /// the triangles the CPU T&L handed over.
    pub gpu_draws: u32,
    pub gpu_tris: u32,
    pub cpu_draws: u32,
    pub cpu_tris: u32,
    /// Per frame: why the CPU draws stayed off the GPU
    /// ([`GpuReject::index`]; `Offscreen` ones are culled, not drawn).
    pub off_gpu: [u32; GpuReject::COUNT],
}

impl N3dsHost {
    /// The top screen: the picture games are created with.
    pub const SCREEN: Vec2 = Vec2::new(400.0, 240.0);

    /// Boot libctru, print `TRINO_BOOT` and load every baked asset.
    pub fn boot() -> Self {
        runtime::init();
        runtime::log("TRINO_BOOT\n");
        let mut host = N3dsHost {
            renderer: N3dsRenderer::new(),
            audio: N3dsAudio::new(),
            input: N3dsInput,
            platform: N3dsPlatform::new(),
        };
        N3dsAssets::load_all(&mut host.renderer, &mut host.audio, &mut host.platform);
        host
    }

    /// `cargo xtask test 3ds` put a `test_mode` file in the RomFS.
    pub fn test_mode() -> bool {
        N3dsAssets::exists("/test_mode")
    }

    pub fn log(&self, msg: &str) {
        runtime::log(msg);
    }

    /// The applet manager has not asked the app to quit.
    pub fn running(&self) -> bool {
        runtime::running()
    }

    /// One frame with real controller + touch input.
    pub fn frame(&mut self, game: &mut dyn Game, dt: f32) {
        let state = self.input.poll();
        self.step(game, &state, dt);
    }

    /// One frame with scripted input (self-tests, bots). The asset groups'
    /// loader runs first (issue #56): what the game asked for last frame
    /// loads, what it unloaded is freed.
    pub fn step(&mut self, game: &mut dyn Game, input: &InputState, dt: f32) {
        N3dsAssets::pump(&mut self.renderer, &mut self.audio, &mut self.platform);
        game.update_with(&mut self.platform, input, &mut self.audio, dt);
        game.render(&mut self.renderer);
        self.audio.poll();
        self.platform.sample_heap();
    }

    /// Run `frames` scripted frames and time them (wall clock, vsync
    /// included).
    pub fn measure(
        &mut self,
        game: &mut dyn Game,
        frames: u32,
        dt: f32,
        mut input: impl FnMut(u32) -> InputState,
    ) -> FrameReport {
        let mut total: u64 = 0;
        let mut cpu: u64 = 0;
        let (mut gpu_draws, mut gpu_tris, mut cpu_draws, mut cpu_tris) = (0u64, 0u64, 0u64, 0u64);
        let mut off = [0u64; GpuReject::COUNT];
        let mut max = 0u32;
        let _ = self.renderer.take_wait_us();
        for i in 0..frames {
            let state = input(i);
            let t0 = runtime::now_us();
            self.step(game, &state, dt);
            let us = runtime::now_us().wrapping_sub(t0);
            total += us as u64;
            cpu += us.saturating_sub(self.renderer.take_wait_us()) as u64;
            let stats = trino_core::Renderer::stats(&self.renderer);
            let tnl = stats.tnl;
            gpu_draws += tnl.gpu_draws as u64;
            gpu_tris += tnl.gpu_tris as u64;
            cpu_draws += stats.models.saturating_sub(tnl.gpu_draws) as u64;
            cpu_tris += tnl.emitted as u64;
            for (o, n) in off.iter_mut().zip(self.renderer.gpu_rejects()) {
                *o += n as u64;
            }
            max = max.max(us);
        }
        let per = |v: u64| v.checked_div(frames as u64).unwrap_or(0) as u32;
        FrameReport {
            frames,
            avg_us: per(total),
            max_us: max,
            cpu_us: per(cpu),
            gpu_draws: per(gpu_draws),
            gpu_tris: per(gpu_tris),
            cpu_draws: per(cpu_draws),
            cpu_tris: per(cpu_tris),
            // Rounded up: a reason seen at all shows.
            off_gpu: off.map(|n| n.div_ceil(frames.max(1) as u64) as u32),
        }
    }

    /// Print a [`FrameReport`] as a `TRINO_FRAME:` line (reported only):
    /// `fps=` from the average frame, `off_gpu=` listing why the CPU draws
    /// stayed off the GPU, most common first, and `heap=used/peak`.
    pub fn report_frames(&self, scene: &str, r: &FrameReport) {
        runtime::log(&format!(
            "TRINO_FRAME:{scene} avg_us={} max_us={} frames={} fps={} cpu_us={} gpu_draws={} \
             gpu_tris={} cpu_draws={} cpu_tris={} off_gpu={} {}\n",
            r.avg_us,
            r.max_us,
            r.frames,
            fps(r.avg_us),
            r.cpu_us,
            r.gpu_draws,
            r.gpu_tris,
            r.cpu_draws,
            r.cpu_tris,
            off_gpu_list(&r.off_gpu),
            trino_core::Platform::memory(&self.platform).heap_field()
        ));
    }

    /// Print the last presented frame: the next entry of the project's
    /// `n3ds_goldens`.
    pub fn dump_framebuffer(&mut self) {
        self.renderer.dump_framebuffer();
    }

    /// Print the last presented touch screen (the next golden).
    pub fn dump_framebuffer_bottom(&mut self) {
        self.renderer.dump_framebuffer_bottom();
    }

    /// The real-time loop, until the applet manager asks to quit.
    pub fn run(&mut self, game: &mut dyn Game) {
        let mut last = runtime::now_us();
        let mut trace = runtime::TraceLoop::start("3ds-game");
        while runtime::running() {
            let now = runtime::now_us();
            let dt = (now.wrapping_sub(last) as f32 / 1_000_000.0).min(0.1);
            last = now;
            self.frame(game, dt);
            trace.frame();
        }
    }

    /// The entry the [`n3ds_main!`](crate::n3ds_main) macro expands to.
    pub fn main<G: Game>(
        make: impl FnOnce(Vec2) -> G,
        self_test: impl FnOnce(&mut N3dsHost, &mut G) -> Result<(), String>,
    ) {
        let mut host = N3dsHost::boot();
        let mut game = boxed(make);
        if Self::test_mode() {
            match self_test(&mut host, &mut game) {
                Ok(()) => runtime::log("TRINO_TEST_PASS\n"),
                Err(e) => runtime::log(&format!("TRINO_TEST_FAIL:{e}\n")),
            }
            while runtime::running() {
                host.step(&mut *game, &InputState::default(), 1.0 / 60.0);
            }
            return;
        }
        host.run(&mut *game);
    }
}

/// Frames per second from the average wall-clock frame (vsync included):
/// what the screen showed, 60 at most.
fn fps(avg_us: u32) -> u32 {
    (1_000_000 + avg_us / 2).checked_div(avg_us).unwrap_or(0)
}

/// `Reason:n,…` by count (most first), `-` when nothing stayed off.
pub fn off_gpu_list(off: &[u32; GpuReject::COUNT]) -> String {
    let mut by: alloc::vec::Vec<(u32, GpuReject)> = GpuReject::ALL
        .iter()
        .map(|&r| (off[r.index()], r))
        .filter(|&(n, _)| n > 0)
        .collect();
    by.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    if by.is_empty() {
        return String::from("-");
    }
    let mut s = String::new();
    for (i, (n, r)) in by.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("{r:?}:{n}"));
    }
    s
}

/// Build the game straight into a box from a frame popped right after.
#[inline(never)]
fn boxed<G>(make: impl FnOnce(Vec2) -> G) -> Box<G> {
    Box::new(make(N3dsHost::SCREEN))
}
