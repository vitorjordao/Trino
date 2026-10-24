# ADR 0031: Shader precompilation and the CPU thread budget

Status: accepted (October 2026, GitHub #101). Implemented in `trino-core` (`precompile`,
`threads`), `trino-render-wgpu` (`renderer/pipes.rs`, `renderer/precompile.rs`), the PC and
Web hosts (`trino-runner-pc`, `apps/pc`, `trino-platform-web`), `trino-asset-pipeline`
(`par`, `bake_all_with`, `BundleLoad::step_many`) and xtask (`threads`, `[threads]` in
`trino.toml`, the N64 and 3DS shader assembly).

## Context

A GPU driver turns a pipeline's shaders into machine code when the pipeline is created.
The PC renderer created most of its pipelines at start, but about ten passes were built
**the first time a frame needed them**, which is a hitch in that frame:

- the MSAA twins (`ms_pipelines`, shadow pipes, AO, the shadow map, GPU T&L, water);
- the shadow map and SSAO passes;
- the render-scale upscale;
- every color-vision view;
- the GPU T&L path;
- WebGL2's depth blit.

Turning on MSAA, a quality preset with SSAO or a CVD mode in the options page compiled
in the middle of a frame. On the second launch everything compiled again, unless the
driver kept its own cache.

The consoles never had the problem:

- **N64:** the four RSP overlays are assembled by the build and registered once at boot
  (`trino_shim.c`). The RDP combiner is register state.
- **3DS:** the PICA200 shaders are assembled by picasso at build time and parsed at boot
  (`DVLB_ParseFile`). TexEnv is register state.

Their build did assemble those shaders one after the other.

What other engines do, and why it fits Trino:

- **Pipeline-state caches** (`VkPipelineCache`, D3D12 pipeline libraries, Metal binary
  archives) skip the driver's compile on a later launch. DX12, Metal, Mesa and NVIDIA
  also cache on disk themselves.
- **Warm-up at a loading screen** (Unreal's PSO precaching, Unity's
  `ShaderVariantCollection.WarmUp`, Steam's Fossilize pre-caching) moves the compile to
  a moment the player expects to wait.
- **Asynchronous compilation with a fallback** (Dolphin's ubershaders,
  `VK_EXT_graphics_pipeline_library`) exists because those engines can't know their
  variants ahead of time.

Trino has **no user shaders**: a surface is `ModelParams` data (the N64 ceiling), so the
set of pipelines is the engine's own passes × MSAA × profile. It is finite and known, and
a complete warm-up is possible.

## Decisions

### 1. One thread budget: all CPUs − 1, at least 1

- `trino_core::ThreadSetting { Auto, Count(n), AllBut(n) }`, text `auto` / `N` / `-N`
  (`0` = `auto`). `resolve(cpus)` clamps to `1..=cpus`. `Auto` leaves one CPU to the
  game's own thread, the OS and the editor.
- **Build** (xtask): `--threads`, then `$TRINO_THREADS`, then `trino.toml [threads] build`,
  then `auto` (`xtask/src/threads.rs`). It sets:
  - the parallel bakes (`bake_all_with`);
  - `CARGO_BUILD_JOBS`, unless the user set it already;
  - the N64 shim + RSP overlays: a job limit `J` in the Docker script;
  - picasso (`par::map_ordered`).
- **Runtime** (PC/Web hosts): `$TRINO_THREADS`, then `RunConfig::threads` (a package's
  `threads=` marker line, from `[threads] runtime`), then `auto`. It sets the precompile
  workers and `GroupLoader`'s decoding.
- The per-frame CPU T&L pool (`TRINO_PC_TNL_THREADS`, ADR 0016) stays a separate knob: it
  runs inside every frame, and a frame's budget is a different decision from a load's.
- **One rule:** parallel work gives the serial result. `par::map_ordered` returns results
  in input order. A bake on N threads writes the same bytes, with the same report and
  error order, as a bake on 1. `BundleLoad::step_many(n)` yields the same assets in the
  same order as `n` × `step`, and the GPU and audio install stays on the render thread,
  in order.

### 2. Every pipeline through one helper

`renderer/pipes.rs::render(device, desc)` replaces the 14 `create_render_pipeline` sites.
It does two things:

- **It passes the renderer's `PipelineCache`.**
- **It counts creations in two counters:** `render_thread`, and `warm` (a precompile
  worker or a Web time slice).

The builders keep their `&Device` signatures:

- The renderer opens a thread-local `pipes::Scope` around the code that may build
  pipelines: `build`, `set_msaa`, `ensure_gpu_tnl`, `ensure_shadow_pipes`,
  `draw_commands`, and the GPU T&L creation sites.
- Each worker opens a `warm_scope` for its jobs.
- A pipeline built with no scope is counted as `unscoped`. The precompile test holds that
  count at 0, so a lazy site that lost its scope fails it.
- **The device is not the key.** `wgpu::Device` compares by an id that repeats across
  instances: a first registry keyed by it gave two live headless renderers one entry
  (12 phantom pipelines in a test, and one device would have used the other's cache).

`PcFrameTimes::pipelines` is the render thread's count **since the previous frame
ended**, so what `update` builds between frames (`set_msaa`, a preset applied by the
options page) counts toward the next frame. It reaches `TRINO_FRAME` as `pipes=` (a
total, so any hitch shows) and `PcRenderer::pipeline_counts`.

### 3. `precompile`: plan, build off-thread, install where the lazy code looks

- **The plan.** `PcRenderer::precompile_plan(all)` lists the passes not built yet that
  the profile can reach. `EVERYTHING` covers any preset, MSAA on and off, and every CVD
  view. `CURRENT` covers what the current settings draw.
- **The jobs.** A job clones `Inputs` (reference-counted handles: shader modules, layouts),
  builds one pass (`precompile::build`) and sends it back.
  - `PcRenderer::warm_install` puts the result where the lazy code looks
    (`get_or_insert`). A pass the frame already built stays, so drawing never depends on
    whether a warm-up ran, and the pixels are identical.
  - The CVD pass is created on the render thread before its jobs, so their pipelines use
    its own layout.
- **The workers.** Native builds use `min(threads, jobs)` worker threads. The Web has no
  threads: one job per `begin_frame`.
- **The modes.** `Blocking` returns built. `Background` returns at once.
- **Frames during a background warm-up.** `begin_frame` installs finished jobs. Before a
  lazy site builds a pass, `warm_wait(kind)` handles it:
  - a job still **queued** leaves the queue and the frame builds it, as before;
  - a job a worker is **running** is waited for (a worker sends its result before it
    leaves `running`, so the wait always ends).

  The sites are `set_msaa`, `ensure_gpu_tnl`, `ensure_shadow_pipes`, the GPU T&L draws,
  and `draw_commands` once it knows the frame's AO, shadow map, upscale, depth blit and
  CVD view.
- **Never in a constructor.** Eager pipelines in every renderer crashed parallel headless
  tests on RADV (#94). The editor's and the tests' renderers stay lazy.
- **The game API.** `Renderer::precompile`/`precompile_status` are trait **defaults**
  answering `PrecompileStatus::DONE` (0 jobs), so there is no `GAME_API_VERSION` bump.
  The N64 and the 3DS keep the default: they compile nothing at run time.

### 4. The persistent pipeline cache

- `RendererOptions::pipeline_cache_dir` turns it on.
  - The runner passes `save_dir()/shaders`: the player's folder, never the build tree.
  - The renderer then asks for `Features::PIPELINE_CACHE` where the adapter has it.
- **The file.** It is `<dir>/<wgpu::util::pipeline_cache_key>.bin` (vendor and device;
  the driver checks its version), opened with `fallback: true`, so a corrupt, foreign or
  outdated file starts an empty cache.
- **Saving.** `save_pipeline_cache` writes it atomically (a temp file, then a rename)
  once per finished warm-up (at the next `begin_frame`, however it ended), and when the
  runner and `apps/pc` exit. castle64's cache is 88 KB on RADV.
- **Scope.** wgpu 29 offers it on Vulkan only. DX12 and Metal drivers cache on their own,
  and the editor's eframe device has no cache. `has_pipeline_cache()` says which case
  applies; nothing pretends otherwise.

### 5. Hosts

- `trino_runner_pc::boot_precompile` starts a `Background` warm-up of `EVERYTHING` after
  the boot assets load: the first frames present while the workers compile.
  - `RunConfig::precompile` (`Background` / `Blocking` / `Off`) or
    `TRINO_PRECOMPILE=background|blocking|off` chooses.
  - `Blocking` prints `TRINO_PRECOMPILE: N passes in M ms on T threads`.
- `apps/pc` does the same.
- The Web host starts it after creating the renderer, at one pass per frame.
- A game can call `precompile` itself: at a loading screen, or after its options page
  changes a preset.

## Measurements

From `cargo test --release -p trino-platform-pc --test precompile_bench -- --ignored
--nocapture`, on an AMD Renoir iGPU (RADV, Mesa 26.0.8, Vulkan), 16 logical CPUs, with
Mesa's disk cache off. The test generates 48 heavy fragment shaders, each with 96
unrolled noise taps (915 KB of WGSL), every variant distinct.

| What | Time |
|---|---|
| naga (`create_shader_module`, all 48) | 335 ms |
| driver compile, 1 thread | 2 736 ms |
| driver compile, 15 threads (`auto`) | 1 063 ms (2.6×) |
| pipeline cache: filling it / the next launch | 1 110 ms / **14 ms** (79×, 1.4 MB) |
| engine `precompile(EVERYTHING)`, 1 thread (14 passes, 35 pipelines) | 205 ms |
| engine `precompile(EVERYTHING)`, 14 threads | 41 ms (5×) |

- **Threads.** They help because RADV compiles pipelines concurrently, but not linearly:
  wgpu-core and the driver serialize parts of the work.
- **The cache.** It removes only the driver's share; naga's translation stays.
- **The warm-up.** It moves the engine's ~10 hitches (up to ~40 ms each on this iGPU)
  out of gameplay.

### In the game, and for the other budgets

From `cargo test --release -p xtask --test precompile_shots -- --ignored --nocapture`,
same machine, castle64's hub at 960×540 on the native profile. MSAA, the four
color-vision views and a render scale of 0.5 switch on and off over 140 frames:

| What | Before | After |
|---|---|---|
| worst frame (the one MSAA turns on) | 119.7 ms | 6.1 ms |
| pipelines compiled inside frames | 20 | 0 |
| warm-up before frame 0 (a loading screen) | none | 35.9 ms |
| pixels of the compared frames | — | identical (518 400 of 518 400, 4 steps) |
| asset bake of the engine's assets, 1 → 15 threads | 1 042 ms | 530 ms |
| decode of a 315 KB group (154 assets), 1 → 15 threads | 1.6 ms | 1.5 ms |
| decode of a 69 MB group (48 textures), 1 → 15 threads | 453.5 ms | 128.5 ms |
| N64 rebuild after a shader edit, `--threads 1` → `auto` | 8.8 s | 5.9 s |
| 3DS rebuild after a shader edit (picasso, shim, cargo jobs) | 13.1 s | 4.6 s |

**Small groups.** The first version of `step_many` spawned a worker per asset of a step,
and the small group took **4.0 ms on 15 threads against 1.6 ms on one**: thread start-up
cost more than the decoding. A step now uses
`min(threads, bytes / MIN_BYTES_PER_WORKER)` workers (512 KB each), so a small group stays
on one thread and a large one spreads.

The charts and pictures are drawn by `tools/evidence/precompile_charts.py` from the
JSON the two tests write. The `loading.gif` is a loading screen whose bar is
`precompile_status()`, with one tick per pass.

## Consequences

- `TRINO_FRAME` gains `pipes=`. Older parsers ignore unknown keys.
- **A new lazily built pass needs three things,** and the precompile test fails if one is
  missing:
  - a `JobKind`;
  - a line in `precompile_plan` and `warm_install`;
  - a `warm_wait` before its lazy site.
- `PcRenderer` implements `Drop` (it stops the workers first), so a destructuring move
  out of it no longer compiles (there was none).
- The N64 and 3DS builds now assemble their shaders in parallel. Their output files are
  unchanged.

## Tests

- `crates/platform-pc/tests/precompile.rs`:
  - without a warm-up, the toggles compile in frames;
  - after `EVERYTHING`, MSAA, SSAO, the shadow map, every CVD mode and a render scale
    compile **0** pipelines in frames, with identical pixels;
  - a background warm-up keeps frames coming and ends;
  - a frame that needs a pass a worker holds in flight (a test gate,
    `PcRenderer::hold_precompile`) waits for it and builds nothing itself;
  - `CURRENT` plans fewer jobs than `EVERYTHING`;
  - the cache round-trips through its file and survives a corrupt, truncated or foreign
    (mismatched header) one;
  - after the warm-up the render thread builds nothing at all, between frames included;
  - nothing is built outside a scope;
  - a soak of 25 runs on RADV and 10 on lavapipe (CI's GPU, where the cache test runs
    too) had no failure.
- Smoke: castle64 (`TRINO_SIM=native TRINO_SMOKE_FRAMES=120`) reports `pipes=0` on RADV
  and lavapipe; the second launch's worst frame went 47.5 → 34.4 ms with the cache.
- `crates/asset-pipeline/tests/parallel_bake.rs`:
  - the PC and Web bakes are byte-identical on 1, 2 and 8 threads, with the same errors;
  - a bundle decodes to the same assets on 1 and 8 threads.
- `trino_core::threads`, `asset_pipeline::par` and `xtask::threads` unit tests;
  `xtask/src/project.rs::thread_budgets_parse_with_auto_by_default`.
- The `shader precompile` self-test step (castle64) on PC (`engine_checks`), in ares,
  in Azahar and in the browsers.
