//! Pipeline warm-up (issue #101, ADR 0031): the passes the renderer
//! otherwise builds the first time a frame needs them, built ahead on a
//! pool of worker threads (one job per frame on the Web, which has none).
//!
//! A job builds one pass's pipelines from cloned handles ([`Inputs`]) and
//! sends the result back; the render thread installs it
//! (`PcRenderer::warm_install`) where the lazy code would have built it, so
//! drawing never depends on whether a warm-up ran. A frame that needs a
//! pass still in flight waits for that job ([`Warmup::wait_for`]); one
//! still queued is taken off the queue and built inline, as before.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use trino_core::CvdMode;

use super::cvd::CvdKind;
use super::{MSAA_SAMPLES, WaterGpu, scene_pipelines, upscale};
use super::{
    OFFSCREEN_FORMAT, SceneSources, StereoView, ao, cvd, depth_blit, gpu_tnl, post, shadow,
    shadow_map, stereo,
};
use web_time::Instant;

/// One lazily built pass.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum JobKind {
    /// The four scene pipelines at 4× MSAA.
    MsScene,
    /// The blob/planar shadow pipelines at this many samples.
    ShadowPipes(u32),
    /// The SSAO pass (`true`: over the multisampled depth).
    Ao(bool),
    /// The shadow map's light and composite pipelines (`true`: MSAA).
    ShadowMap(bool),
    /// The render-scale upscale + sharpen.
    Upscale,
    /// WebGL2's depth copy by drawing.
    DepthBlit,
    /// A color-vision simulation view, or the players' correction filter.
    Cvd(CvdMode, CvdKind),
    /// The post stack's HDR twins of the scene's pipelines (`true`: at 4×
    /// MSAA; issue #64).
    Hdr(bool),
    /// The GPU T&L path's HDR twin, the path already built.
    HdrGpu(bool),
    /// The post stack's bloom chain and composite.
    Post,
    /// A composing stereo preview (the 3DS profile, issue #63).
    Stereo(StereoView),
    /// The GPU T&L path (with its MSAA twin when [`Inputs::gpu_ms`]).
    GpuTnl,
    /// The GPU T&L path's MSAA twin, the path already built.
    GpuTnlMs,
    /// The per-pixel water's MSAA twin.
    WaterMs,
}

/// What a job built.
pub(crate) enum Built {
    MsScene(Box<[wgpu::RenderPipeline; 4]>),
    ShadowPipes(u32, Box<shadow::ShadowPipes>),
    Ao(bool, Box<ao::AoPass>),
    ShadowMap(bool, Box<shadow_map::MapPass>),
    Upscale(Box<upscale::Upscale>),
    DepthBlit(Box<depth_blit::DepthBlit>),
    Cvd(CvdMode, CvdKind, wgpu::RenderPipeline),
    Hdr(bool, Box<post::HdrSet>),
    HdrGpu(bool, [wgpu::RenderPipeline; 2]),
    Post(Box<post::PostPass>),
    Stereo(StereoView, wgpu::RenderPipeline),
    GpuTnl(Box<gpu_tnl::GpuTnl>),
    GpuTnlMs([wgpu::RenderPipeline; 2]),
    WaterMs(wgpu::BindGroupLayout, [wgpu::RenderPipeline; 2]),
}

/// The handles every job builds from, cloned once per request (wgpu
/// handles are reference-counted and `Send + Sync` off the Web).
pub(crate) struct Inputs {
    /// The renderer's cache and counters (the jobs' [`super::pipes`] scope).
    pub entry: Arc<super::pipes::DeviceEntry>,
    /// Tests only: jobs wait here, already running, until it opens (no
    /// workers on the Web).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub hold: Option<Arc<Hold>>,
    pub device: wgpu::Device,
    pub sources: SceneSources,
    pub webgl2: bool,
    /// Groups 0 and 1 of the GPU T&L layout (globals, 3D texture).
    pub tnl_groups: [wgpu::BindGroupLayout; 2],
    /// Build the GPU T&L path's MSAA twin with it.
    pub gpu_ms: bool,
    /// The built GPU T&L path's layout and shader (for [`JobKind::GpuTnlMs`]).
    pub gpu_ms_source: Option<(wgpu::PipelineLayout, wgpu::ShaderModule)>,
    /// The color-vision pass's layout (the pass exists before its jobs run).
    pub cvd_layout: wgpu::PipelineLayout,
    /// The stereo preview pass's layout (made before its jobs run).
    pub stereo_layout: wgpu::PipelineLayout,
    /// Groups 0..2 of the water layout, and its single-sampled group 3.
    pub water_groups: [wgpu::BindGroupLayout; 3],
    pub water_bgl: wgpu::BindGroupLayout,
    /// The adapter renders `Rgba16Float` (the post stack's chain format).
    pub hdr: bool,
}

/// Build one pass. Runs on a worker (native) or the render thread (Web).
pub(crate) fn build(kind: JobKind, i: &Inputs) -> Built {
    let d = &i.device;
    match kind {
        JobKind::MsScene => Built::MsScene(Box::new(scene_pipelines(
            d,
            &i.sources,
            MSAA_SAMPLES,
            OFFSCREEN_FORMAT,
        ))),
        JobKind::ShadowPipes(n) => Built::ShadowPipes(
            n,
            Box::new(shadow::ShadowPipes::new(d, &i.sources, n, OFFSCREEN_FORMAT)),
        ),
        JobKind::Ao(ms) => Built::Ao(
            ms,
            Box::new(ao::AoPass::new(d, OFFSCREEN_FORMAT, ms, i.webgl2 && !ms)),
        ),
        JobKind::ShadowMap(ms) => Built::ShadowMap(
            ms,
            Box::new(shadow_map::MapPass::new(
                d,
                OFFSCREEN_FORMAT,
                ms,
                i.webgl2 && !ms,
            )),
        ),
        JobKind::Upscale => Built::Upscale(Box::new(upscale::Upscale::new(d))),
        JobKind::DepthBlit => Built::DepthBlit(Box::new(depth_blit::DepthBlit::new(d))),
        JobKind::Cvd(mode, kind) => Built::Cvd(
            mode,
            kind,
            cvd::build_pipeline_for(d, &i.cvd_layout, mode, kind),
        ),
        JobKind::Hdr(ms) => Built::Hdr(
            ms,
            Box::new(post::build_hdr_set(
                d,
                &i.sources,
                (&i.water_groups, &i.water_bgl),
                ms,
                i.webgl2,
            )),
        ),
        JobKind::HdrGpu(ms) => {
            let (layout, shader) = i
                .gpu_ms_source
                .as_ref()
                .expect("planned only with the GPU T&L path built");
            let samples = if ms { MSAA_SAMPLES } else { 1 };
            Built::HdrGpu(
                ms,
                gpu_tnl::pipelines(d, layout, shader, samples, post::HDR_FORMAT),
            )
        }
        JobKind::Post => Built::Post(Box::new(post::PostPass::new(d, i.hdr))),
        JobKind::Stereo(view) => {
            Built::Stereo(view, stereo::build_pipeline(d, &i.stereo_layout, view))
        }
        JobKind::GpuTnl => {
            let [g0, g1] = &i.tnl_groups;
            let mut gpu = gpu_tnl::GpuTnl::new(d, [g0, g1]);
            if i.gpu_ms {
                gpu.ensure_ms(d, MSAA_SAMPLES);
            }
            Built::GpuTnl(Box::new(gpu))
        }
        JobKind::GpuTnlMs => {
            let (layout, shader) = i
                .gpu_ms_source
                .as_ref()
                .expect("planned only with the GPU T&L path built");
            Built::GpuTnlMs(gpu_tnl::pipelines(
                d,
                layout,
                shader,
                MSAA_SAMPLES,
                OFFSCREEN_FORMAT,
            ))
        }
        JobKind::WaterMs => {
            let (bgl, pipes) = WaterGpu::ms_pipelines(d, &i.water_groups);
            Built::WaterMs(bgl, pipes)
        }
    }
}

/// A gate precompile workers wait at once a job is theirs (tests: a job
/// held "in flight" while a frame needs its pass).
#[derive(Default)]
pub struct Hold {
    open: Mutex<bool>,
    wake: std::sync::Condvar,
}

impl Hold {
    /// Let every held job go on.
    pub fn release(&self) {
        *self.open.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.wake.notify_all();
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn wait(&self) {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        while !*open {
            open = self.wake.wait(open).unwrap_or_else(|e| e.into_inner());
        }
    }
}

#[derive(Default)]
struct Queue {
    pending: VecDeque<JobKind>,
    running: Vec<JobKind>,
}

/// The state of the last request.
#[derive(Default)]
pub(crate) struct Warmup {
    queue: Arc<Mutex<Queue>>,
    inputs: Option<Arc<Inputs>>,
    #[cfg(not(target_arch = "wasm32"))]
    rx: Option<std::sync::mpsc::Receiver<(JobKind, Built)>>,
    #[cfg(not(target_arch = "wasm32"))]
    workers: Vec<std::thread::JoinHandle<()>>,
    total: u32,
    done: u32,
    threads: u32,
    started: Option<Instant>,
    elapsed_us: u32,
}

fn lock(q: &Mutex<Queue>) -> std::sync::MutexGuard<'_, Queue> {
    q.lock().unwrap_or_else(|e| e.into_inner())
}

impl Warmup {
    /// Jobs a worker holds right now.
    pub fn running(&self) -> u32 {
        lock(&self.queue).running.len() as u32
    }

    /// Nothing queued or running.
    pub fn is_idle(&self) -> bool {
        self.done >= self.total
    }

    pub fn status(&self) -> trino_core::PrecompileStatus {
        let elapsed_us = match (self.started, self.is_idle()) {
            (Some(t), false) => t.elapsed().as_micros().min(u32::MAX as u128) as u32,
            _ => self.elapsed_us,
        };
        trino_core::PrecompileStatus {
            done: self.done,
            total: self.total,
            elapsed_us,
            threads: self.threads,
        }
    }

    /// Start `jobs` on up to `threads` workers (the Web runs them in
    /// [`poll`](Self::poll), one per call). The previous request must be
    /// finished ([`finish`](Self::finish)).
    // One thread on the Web, whose wgpu handles are not `Send`.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub fn start(&mut self, jobs: Vec<JobKind>, inputs: Inputs, threads: u32) {
        debug_assert!(self.is_idle(), "finish the last request first");
        self.total = jobs.len() as u32;
        self.done = 0;
        self.started = Some(Instant::now());
        self.elapsed_us = 0;
        let inputs = Arc::new(inputs);
        lock(&self.queue).pending = jobs.into();
        self.inputs = Some(inputs.clone());
        #[cfg(target_arch = "wasm32")]
        {
            let _ = threads;
            self.threads = (self.total > 0) as u32;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let n = threads.max(1).min(self.total);
            self.threads = n;
            let (tx, rx) = std::sync::mpsc::channel();
            self.rx = Some(rx);
            self.workers = (0..n)
                .map(|w| {
                    let (queue, inputs, tx) = (self.queue.clone(), inputs.clone(), tx.clone());
                    std::thread::Builder::new()
                        .name(format!("trino-precompile-{w}"))
                        .spawn(move || worker(&queue, &inputs, &tx))
                        .expect("spawn a precompile worker")
                })
                .collect();
        }
        self.settle();
    }

    /// Finished results so far (the Web builds one queued job first).
    pub fn poll(&mut self) -> Vec<Built> {
        let mut out = Vec::new();
        #[cfg(target_arch = "wasm32")]
        if let Some(inputs) = self.inputs.clone() {
            let next = lock(&self.queue).pending.pop_front();
            if let Some(kind) = next {
                let _scope = super::pipes::warm_scope(&inputs.entry);
                out.push(build(kind, &inputs));
                self.done += 1;
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(rx) = &self.rx {
            while let Ok((_, b)) = rx.try_recv() {
                out.push(b);
                self.done += 1;
            }
        }
        self.settle();
        out
    }

    /// The render thread is about to build `kind` itself: a queued job
    /// leaves the queue (counted done: the frame builds it), a running one
    /// is waited for. Returns every result received meanwhile.
    pub fn wait_for(&mut self, kind: JobKind) -> Vec<Built> {
        if self.is_idle() {
            return Vec::new();
        }
        let running = {
            let mut q = lock(&self.queue);
            if let Some(at) = q.pending.iter().position(|k| *k == kind) {
                q.pending.remove(at);
                self.done += 1;
                false
            } else {
                q.running.contains(&kind)
            }
        };
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut out = Vec::new();
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(rx) = &self.rx {
            if running {
                // A worker sends before it leaves `running`: the result is
                // on its way.
                while let Ok((k, b)) = rx.recv() {
                    out.push(b);
                    self.done += 1;
                    if k == kind {
                        break;
                    }
                }
            }
            while let Ok((_, b)) = rx.try_recv() {
                out.push(b);
                self.done += 1;
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = running;
        self.settle();
        out
    }

    /// Run every remaining job to the end (blocking requests, a new
    /// request, drop) and return the results.
    pub fn finish(&mut self) -> Vec<Built> {
        let mut out = Vec::new();
        #[cfg(target_arch = "wasm32")]
        while !self.is_idle() {
            let before = self.done;
            out.extend(self.poll());
            if self.done == before {
                break;
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(rx) = &self.rx {
                while self.done < self.total {
                    match rx.recv() {
                        Ok((_, b)) => {
                            out.push(b);
                            self.done += 1;
                        }
                        // Every worker gone (one panicked): stop waiting.
                        Err(_) => break,
                    }
                }
            }
            self.done = self.total;
            self.settle();
        }
        out
    }

    /// Done: freeze the elapsed time, let the workers and inputs go.
    fn settle(&mut self) {
        if !self.is_idle() {
            return;
        }
        if let Some(t) = self.started.take() {
            self.elapsed_us = t.elapsed().as_micros().min(u32::MAX as u128) as u32;
        }
        self.inputs = None;
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.rx = None;
            for w in self.workers.drain(..) {
                let _ = w.join();
            }
        }
    }
}

impl Drop for Warmup {
    fn drop(&mut self) {
        lock(&self.queue).pending.clear();
        let _ = self.finish();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn worker(queue: &Mutex<Queue>, inputs: &Inputs, tx: &std::sync::mpsc::Sender<(JobKind, Built)>) {
    let _scope = super::pipes::warm_scope(&inputs.entry);
    loop {
        let kind = {
            let mut q = lock(queue);
            let Some(k) = q.pending.pop_front() else {
                break;
            };
            q.running.push(k);
            k
        };
        if let Some(h) = &inputs.hold {
            h.wait();
        }
        let built = build(kind, inputs);
        let _ = tx.send((kind, built));
        lock(queue).running.retain(|k| *k != kind);
    }
}
