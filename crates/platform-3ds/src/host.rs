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
use trino_core::replay::ChannelSession;
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
    /// Frames that drew two eyes (issue #63), and the last frame's stereo
    /// (`stereo=on@N%` or why it was mono).
    pub stereo_frames: u32,
    pub stereo: trino_core::StereoStats,
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
            input: N3dsInput::new(),
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
        let players = self.poll_players();
        self.step(game, &players.players[0], dt);
    }

    /// Poll the controls as the one player (issue #67), apply the game's
    /// seat requests and publish the players to the platform.
    pub fn poll_players(&mut self) -> trino_core::PlayersInput {
        while let Some(r) = self.platform.players.take_request() {
            self.input.seats_mut().apply(r);
        }
        let players = self.input.poll_players();
        self.platform.players.publish(self.input.seats());
        players
    }

    /// One frame with scripted input (self-tests, bots). The asset groups'
    /// loader runs first (issue #56): what the game asked for last frame
    /// loads, what it unloaded is freed.
    pub fn step(&mut self, game: &mut dyn Game, input: &InputState, dt: f32) {
        N3dsAssets::pump(&mut self.renderer, &mut self.audio, &mut self.platform);
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
        let (mut stereo_frames, mut stereo) = (0u32, trino_core::StereoStats::default());
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
            stereo = stats.stereo;
            stereo_frames += (stereo.eyes == 2) as u32;
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
            stereo_frames,
            stereo,
        }
    }

    /// Print a [`FrameReport`] as a `TRINO_FRAME:` line (reported only):
    /// `fps=` from the average frame, `off_gpu=` listing why the CPU draws
    /// stayed off the GPU, most common first, and `heap=used/peak`.
    pub fn report_frames(&self, scene: &str, r: &FrameReport) {
        runtime::log(&format!(
            "TRINO_FRAME:{scene} avg_us={} max_us={} frames={} fps={} cpu_us={} gpu_draws={} \
             gpu_tris={} cpu_draws={} cpu_tris={} off_gpu={} {} stereo_frames={} {}\n",
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
            trino_core::Platform::memory(&self.platform).heap_field(),
            r.stereo_frames,
            stereo_field(&r.stereo)
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

    /// The replay or recording this app was built for (issue #67): a
    /// `/replay.trec` in the RomFS replays it (`/replay_until` stops early,
    /// `/replay_shot` dumps the last frame), a `/record` file (the game's
    /// name) records the live input over the debug channel (`cargo xtask
    /// replay run --platform 3ds`, `cargo xtask run 3ds --record`). `None`:
    /// play.
    pub fn channel_session(
        &mut self,
        game: &dyn Game,
    ) -> Option<ChannelSession<'static, fn(&str)>> {
        let log: fn(&str) = runtime::log;
        if let Some(bytes) = N3dsAssets::read("/replay.trec") {
            let bytes: &'static [u8] = alloc::boxed::Box::leak(bytes.into_boxed_slice());
            let until = N3dsAssets::read("/replay_until")
                .and_then(|b| core::str::from_utf8(&b).ok()?.trim().parse().ok());
            return match ChannelSession::replay(bytes, until, "3ds", log) {
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
        let name = N3dsAssets::read("/record")?;
        let name = core::str::from_utf8(&name).ok()?.trim();
        runtime::log(&format!("TRINO_RECORD: 3ds {name}\n"));
        Some(ChannelSession::record(
            game,
            name,
            (400, 240),
            1,
            "3ds",
            log,
        ))
    }

    /// The real-time loop, until the applet manager asks to quit (a replay
    /// or a recording when the app asks for one:
    /// [`channel_session`](Self::channel_session)).
    pub fn run(&mut self, game: &mut dyn Game) {
        self.run_named(game, "3ds-game");
    }

    /// [`run`](Self::run) with the trace session's name (`run 3ds
    /// --trace`; castle64's app names its own).
    pub fn run_named(&mut self, game: &mut dyn Game, trace: &'static str) {
        // A local-wireless session (issue #117): the whole run in lockstep.
        if let Some(plan) = N3dsAssets::read("/uds")
            .and_then(|b| crate::uds::UdsPlan::parse(core::str::from_utf8(&b).ok()?))
        {
            self.run_uds(game, plan);
            return;
        }
        let mut session = self.channel_session(game);
        let mut last = runtime::now_us();
        let mut trace = runtime::TraceLoop::start(trace);
        while runtime::running() {
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
                            if N3dsAssets::exists("/replay_shot") {
                                self.dump_framebuffer();
                            }
                            runtime::log("TRINO_REPLAY_DONE\n");
                            while runtime::running() {
                                game.render(&mut self.renderer);
                            }
                            return;
                        }
                    }
                }
            }
            trace.frame();
        }
        if let Some(s) = session {
            s.finish();
        }
    }

    /// A local-wireless session (issue #117, [`crate::uds`]): meet the
    /// other consoles (host or join), then play the game in delay-based
    /// lockstep from its first frame ([`trino_core::lockstep`]): each
    /// console's pad is its seat, every seat's input reaches every
    /// console, the game runs on a [`Neutral`](trino_core::replay::Neutral)
    /// platform with a fixed `dt` of 1/60 s, and
    /// [`local_seats`](trino_core::Platform::local_seats) is this console's
    /// seat. Logs `TRINO_UDS:` lines (the meeting, then the session every
    /// 5 s) and `TRINO_NET_HASH:<frame>:<hash>` on the plan's `hash_at`.
    /// A `/uds_script` recording makes this console play its seat of it.
    pub fn run_uds(&mut self, game: &mut dyn Game, plan: crate::uds::UdsPlan) {
        use trino_core::lockstep::{LockConfig, Lockstep, MAX_PACKET};
        let log = runtime::log;
        if let Err(r) = crate::uds::init() {
            log(&format!("TRINO_UDS: init failed ({r:#x})\n"));
            return;
        }
        if plan.host {
            if let Err(r) = crate::uds::host(plan.players) {
                log(&format!(
                    "TRINO_UDS: could not create the network ({r:#x})\n"
                ));
                crate::uds::exit();
                return;
            }
            log(&format!(
                "TRINO_UDS: hosting for {} players\n",
                plan.players
            ));
        } else {
            log("TRINO_UDS: looking for a host\n");
        }
        // The meeting: render the game's first frame meanwhile.
        let mut joined = plan.host;
        let mut wait = 0u32;
        let seat = loop {
            if !runtime::running() {
                crate::uds::exit();
                return;
            }
            if !joined && wait.is_multiple_of(30) {
                match crate::uds::try_join() {
                    Ok(true) => {
                        joined = true;
                        log("TRINO_UDS: joined\n");
                    }
                    Ok(false) => {}
                    Err(r) => log(&format!("TRINO_UDS: scan failed ({r:#x})\n")),
                }
            }
            let (node, total) = crate::uds::status();
            if joined && node > plan.players as u16 {
                // A console that rejoined got a node past the seats: it
                // would play someone else's seat.
                log(&format!(
                    "TRINO_UDS: node {node} has no seat among {} players\n",
                    plan.players
                ));
                crate::uds::exit();
                return;
            }
            if joined && node > 0 && total >= plan.players {
                break (node - 1) as u8;
            }
            game.render(&mut self.renderer);
            wait += 1;
        };
        let script = N3dsAssets::read("/uds_script").and_then(|b| {
            let mut r = trino_core::replay::Reader::new(&b).ok()?;
            let mut out = alloc::vec::Vec::new();
            while let Some(Ok(f)) = r.next_frame() {
                out.push(f.players);
            }
            Some(out)
        });
        let mut lock = Lockstep::new(LockConfig {
            delay: plan.delay,
            ..LockConfig::new(plan.players, seat)
        });
        log(&format!(
            "TRINO_UDS: player {} of {}, delay {}{}\n",
            seat + 1,
            plan.players,
            plan.delay,
            if script.is_some() { ", scripted" } else { "" }
        ));
        let mut out = [0u8; MAX_PACKET];
        let mut rx = [0u8; 1500];
        let (mut clock_ms, mut last_us) = (0u64, runtime::now_us());
        let mut frames = 0u32;
        let dt = 1.0 / 60.0;
        while runtime::running() {
            let now = runtime::now_us();
            clock_ms += now.wrapping_sub(last_us) as u64 / 1000;
            last_us = now;
            while let Some((n, _from)) = crate::uds::recv(&mut rx) {
                let _ = lock.receive(&rx[..n], clock_ms);
            }
            lock.poll(clock_ms);
            let live = self.poll_players();
            let local = match &script {
                Some(s) => {
                    let f = (lock.frame() + plan.delay as u32) as usize;
                    s.get(f)
                        .map_or(InputState::IDLE, |p| p.players[seat as usize])
                }
                None => live.players[0],
            };
            lock.add_local(&local);
            if let Some(p) = lock.advance() {
                self.platform.players.publish_net(&p, lock.local_seats());
                N3dsAssets::pump(&mut self.renderer, &mut self.audio, &mut self.platform);
                {
                    let mut neutral = trino_core::replay::Neutral(&mut self.platform);
                    game.update_with(&mut neutral, &p.players[0], &mut self.audio, dt);
                }
                let h = game.state_hash().unwrap_or(0);
                lock.note_hash(h);
                if Some(lock.frame()) == plan.hash_at {
                    log(&format!("TRINO_NET_HASH:{}:{h:#018x}\n", lock.frame()));
                }
            }
            game.render(&mut self.renderer);
            self.audio.poll();
            for s in 0..plan.players {
                if s != seat {
                    let n = lock.packet(s, &mut out);
                    crate::uds::send(s as u16 + 1, &out[..n]);
                }
            }
            frames += 1;
            if frames.is_multiple_of(300) {
                let st = lock.stats();
                log(&format!(
                    "TRINO_UDS: state={:?} seat={seat} frames={} stalls={} received={} \
                     refused={} checked={} left={:#06b}\n",
                    lock.state(),
                    st.frames,
                    st.stalls,
                    st.received,
                    st.refused,
                    st.checked,
                    lock.left()
                ));
            }
        }
        crate::uds::exit();
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

/// The `stereo=` field of a `TRINO_FRAME` line.
fn stereo_field(s: &trino_core::StereoStats) -> alloc::string::String {
    let mut out = alloc::string::String::new();
    let _ = s.write(&mut out);
    out
}
