//! Heavy-shader evidence for pipeline precompilation (issue #101, ADR 0031).
//! Ignored: it measures this machine's driver.
//!
//! ```text
//! cargo test --release -p trino-platform-pc --test precompile_bench -- --ignored --nocapture
//! ```
//!
//! Driver disk caches are turned off for the cold numbers
//! (`MESA_SHADER_CACHE_DISABLE`, `__GL_SHADER_DISK_CACHE`), or a second run
//! measures Mesa's or NVIDIA's cache instead of the work. It reports, for a
//! set of generated heavy fragment shaders (unrolled noise taps, branches,
//! transcendental math; every variant different so no driver dedupes them):
//!
//! - naga's CPU time (`create_shader_module`) apart from the driver's
//!   compile (`create_render_pipeline`): a pipeline cache only removes the
//!   second;
//! - the pipelines on 1 thread and on N threads (the thread budget);
//! - a second device created from the first one's `PipelineCache` data
//!   (Vulkan only);
//! - the engine's own warm-up (`precompile(EVERYTHING)`) on 1 and N threads.
//!
//! Numbers land in `target/evidence/precompile/bench.txt`.

use std::fmt::Write as _;
use std::time::Instant;

use trino_core::PrecompileRequest;
use trino_platform_pc::renderer::headless_flags;
use trino_platform_pc::{PcRenderer, SimProfile};

const VARIANTS: usize = 48;
const TAPS: usize = 96;

fn heavy_shader(variant: usize) -> String {
    let mut s = String::from(
        "@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {\n\
         let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));\n\
         return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);\n}\n\
         fn hash(p: vec2<f32>) -> f32 { return fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453); }\n\
         fn noise(p: vec2<f32>) -> f32 {\n\
         let i = floor(p); let f = fract(p); let u = f * f * (3.0 - 2.0 * f);\n\
         return mix(mix(hash(i), hash(i + vec2<f32>(1.0, 0.0)), u.x),\n\
                    mix(hash(i + vec2<f32>(0.0, 1.0)), hash(i + vec2<f32>(1.0, 1.0)), u.x), u.y);\n}\n\
         @fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {\n\
         var acc = vec3<f32>(0.0);\n",
    );
    for t in 0..TAPS {
        let k = (variant * TAPS + t) as f32;
        let (dx, dy) = ((k * 0.37).sin() * 3.0, (k * 0.61).cos() * 3.0);
        let _ = writeln!(
            s,
            "{{ let n = noise(pos.xy * {sc:.4} + vec2<f32>({dx:.4}, {dy:.4}));\n\
             if (n > {th:.4}) {{ acc += vec3<f32>(n, n * n, sin(n * {k2:.3})); }}\n\
             else {{ acc -= vec3<f32>(cos(n * {k3:.3}), pow(n, 1.7), 0.5) * 0.1; }} }}",
            sc = 0.01 + (k % 7.0) * 0.003,
            th = 0.3 + (k % 5.0) * 0.1,
            k2 = 1.0 + k % 11.0,
            k3 = 2.0 + k % 13.0,
        );
    }
    let _ = write!(s, "return vec4<f32>(acc / {TAPS}.0, 1.0);\n}}\n");
    s
}

fn device(features: wgpu::Features) -> Option<(wgpu::Adapter, wgpu::Device)> {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.flags = headless_flags();
    let instance = wgpu::Instance::new(desc);
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, _queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("precompile-bench"),
        required_features: adapter.features() & features,
        required_limits: wgpu::Limits::downlevel_defaults(),
        ..Default::default()
    }))
    .ok()?;
    Some((adapter, device))
}

fn pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    cache: Option<&wgpu::PipelineCache>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("heavy"),
        layout: None,
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache,
    })
}

fn modules(device: &wgpu::Device, sources: &[String]) -> (Vec<wgpu::ShaderModule>, f64) {
    let t = Instant::now();
    let m = sources
        .iter()
        .map(|s| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl(s.as_str().into()),
            })
        })
        .collect();
    (m, t.elapsed().as_secs_f64() * 1e3)
}

/// The pipelines of `modules` over `threads` workers; ms.
fn compile(
    device: &wgpu::Device,
    modules: &[wgpu::ShaderModule],
    threads: usize,
    cache: Option<&wgpu::PipelineCache>,
) -> f64 {
    let t = Instant::now();
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(m) = modules.get(i) else { break };
                    let _ = pipeline(device, m, cache);
                }
            });
        }
    });
    t.elapsed().as_secs_f64() * 1e3
}

#[test]
#[ignore = "measures this machine's driver: run with --ignored --nocapture"]
fn heavy_shaders_cold_threaded_and_cached() {
    // SAFETY: set before any device exists; this binary runs one test.
    unsafe {
        std::env::set_var("MESA_SHADER_CACHE_DISABLE", "true");
        std::env::set_var("__GL_SHADER_DISK_CACHE", "0");
    }
    let n = trino_core::ThreadSetting::Auto
        .resolve(std::thread::available_parallelism().map_or(1, |n| n.get() as u32))
        as usize;
    let mut out = String::new();
    let sources: Vec<String> = (0..VARIANTS).map(heavy_shader).collect();
    let Some((adapter, dev)) = device(wgpu::Features::PIPELINE_CACHE) else {
        eprintln!("skipping: no adapter");
        return;
    };
    let info = adapter.get_info();
    let _ = writeln!(
        out,
        "adapter: {} ({:?}, {})  cpus: {}  threads (auto = all - 1): {n}\n\
         {VARIANTS} heavy fragment shaders x {TAPS} unrolled noise taps ({} KB of WGSL)\n",
        info.name,
        info.backend,
        info.driver_info,
        n + 1,
        sources.iter().map(String::len).sum::<usize>() / 1024
    );

    // Fresh modules for each measurement: a pipeline from a module the
    // driver already compiled could be deduplicated.
    let (m1, naga1) = modules(&dev, &sources);
    let serial = compile(&dev, &m1, 1, None);
    drop(m1);
    // N threads on a second device (the first one's driver state stays out).
    let parallel = match device(wgpu::Features::empty()) {
        Some((_, d)) => compile(&d, &modules(&d, &sources).0, n, None),
        None => f64::NAN,
    };
    let _ = writeln!(
        out,
        "naga (create_shader_module, all {VARIANTS}):  {naga1:8.1} ms\n\
         driver compile, 1 thread:                {serial:8.1} ms\n\
         driver compile, {n:>2} threads:              {parallel:8.1} ms  ({:.1}x)",
        serial / parallel
    );

    // The pipeline cache: fill it on one device, start a second from its
    // data (what the next launch does).
    if dev.features().contains(wgpu::Features::PIPELINE_CACHE) {
        let fresh = |d: &wgpu::Device, data: Option<&[u8]>| unsafe {
            d.create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
                label: None,
                data,
                fallback: true,
            })
        };
        let (_, dc) = device(wgpu::Features::PIPELINE_CACHE).unwrap();
        let cache = fresh(&dc, None);
        let (m, _) = modules(&dc, &sources);
        let filling = compile(&dc, &m, n, Some(&cache));
        let data = cache.get_data().unwrap_or_default();
        let (_, dw) = device(wgpu::Features::PIPELINE_CACHE).unwrap();
        let warm = fresh(&dw, Some(&data));
        let (m, _) = modules(&dw, &sources);
        let from_cache = compile(&dw, &m, n, Some(&warm));
        let _ = writeln!(
            out,
            "pipeline cache: filling {filling:8.1} ms, next launch {from_cache:8.1} ms \
             ({:.1}x, {} KB on disk)",
            filling / from_cache,
            data.len() / 1024
        );
    } else {
        let _ = writeln!(
            out,
            "pipeline cache: not offered by this adapter (wgpu 29: Vulkan only)"
        );
    }

    // The engine's own warm-up, cold.
    for threads in [1, n as u32] {
        let mut r = pollster::block_on(PcRenderer::new_headless(SimProfile::Native)).unwrap();
        r.set_hw_tnl(true);
        r.set_precompile_threads(threads);
        let s = r.precompile(&PrecompileRequest::EVERYTHING.blocking());
        let _ = writeln!(
            out,
            "engine precompile(EVERYTHING): {} passes, {} pipelines, {:7.1} ms on {} thread(s)",
            s.total,
            r.pipeline_counts().precompiled,
            s.elapsed_us as f64 / 1e3,
            s.threads
        );
    }
    println!("{out}");
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/evidence/precompile");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("bench.txt"), &out).unwrap();
}
