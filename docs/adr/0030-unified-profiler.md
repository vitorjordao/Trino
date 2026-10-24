# ADR 0030: One trace format from every target — zones, export, Perfetto, Tracy, the Profiler

Status: accepted (October 2026, GitHub #62). Implemented in `trino-core` (`trace`), every
platform backend (zones at the renderers' stage sites, the hosts' rings), `trino-render-wgpu`
(`trace_host`, GPU pass zones, the `tracy` feature), the new host crate `trino-trace`
(collect, Chrome/Perfetto JSON, statistics, diffs), xtask (`trace`, `--trace`, the N64
hot-symbol guard) and the editor's Profiler tab. Related: #79 (instrumentation follow-ups).

## Context

Trino measured a lot (`perf::RenderProfile` stages and models, `TRINO_PERF_SCENE`,
`TRINO_FRAME`, the PC frame meter), but every number was a summary line printed by one
platform: no timeline (which stage stalled, frame-to-frame variance, a load spike), no
comparison across targets other than by hand, no PC profiler integration, and the editor
showed estimates, never a measured console frame.

## Decisions

### 1. Instrumentation in the core, compiled out by default

- `trino_core::trace` (`no_std`, zero deps): `zone!("name")` times the rest of its block,
  `trace::counter(name, value)`, `trace::frame_mark(n)`, `Zone::begin_on(track, name)` for
  another track (`track::{CPU, GPU, RSP, AUDIO, WORKER}`).
- **Off unless `trino-core`'s `trace` feature is on.** Then `Zone` is zero-sized with no
  `Drop`, the functions are empty `#[inline(always)]` bodies and a release build is the same
  machine code. The types, the encoder and the decoder are always compiled (the linker drops
  them when unused), so the self-test checks the format on every target and the host tools
  read traces with the same code.
- The recorder is a **global ring the host installs** (`trace::install(&'static mut
  GlobalRing, clock, platform)`; `GLOBAL_EVENTS` = 4096 events of 12 bytes, 48 KB, in a
  `static` on the consoles, leaked on the PC/Web). The clock is the host's microseconds (the
  N64's count register, `svcGetSystemTick` on the 3DS, `Instant` / `performance.now()`).
- Locking: the N64 has one thread and no zone in an interrupt, so a plain flag (no
  read-modify-write atomics on the VR4300 path). Elsewhere an atomic try-lock: an event from a
  thread that finds the ring busy is **counted** (`dropped`), never blocks. Only main-thread
  code is instrumented today.
- Under hot reload (`watch pc`) the game dylib links its own copy of the core: game zones go
  to a ring nobody installed. Traces come from statically linked builds (documented pitfall).
- Instrumentation sits at the existing stage sites (`tick`/`charge` on the N64, the 3DS and PC
  renderers' begin/draw/batch/submit/end, the hosts' update/render/audio), **never inside the
  T&L**. Counters per frame: draws, tris, `rsp_draws`/`rsp_tris` (N64), `gpu_draws` (3DS),
  `pc_gpu_draws` (PC).

### 2. One wire format: `TTRC` packets in `TRINO_TRACE:` lines

- A packet (little-endian, byte by byte, so the big-endian N64 writes the same bytes): magic
  `TTRC`, version 1, the platform, the names interned since the previous packet (`name_base`,
  so names are sent once per ring), the events lost, then 12-byte events (`kind, name, track,
  depth, ts u32 µs, value u32`: a zone's duration, a counter's value, a frame's number).
- Lines: `TRINO_TRACE:<session> <seq> <fletcher16> <base64 of ≤ 96 bytes>` over each
  target's existing debug channel (ISViewer/USB, `svcOutputDebugString`/3dslink, stdout or a
  file, the browser console). Every line has a sequence number and a check: a lost or garbled
  line is an error naming the session and the line, never a silently wrong timeline.
- Sessions: `selftest` (the self-test's step), `n64-<scene>`, `3ds-<scene>`, `pc-<game>`,
  `web`. A line numbered 0 after others of the same name starts a new run (`<name>#2`).

### 3. Host tools: `trino-trace` and `cargo xtask trace`

- `trino-trace` (host only, `serde_json`): `collect(log)` → `Trace` (sessions, zones with
  depth, counters, frame marks; u32 timestamps unwrapped; each session moved to 0), Chrome
  Trace Event JSON out and in (`chrome::write`/`read`/`load`: complete `X` events, `C`
  counters, `i` frame marks, one process per session and one thread per track; foreign `B`/`E`
  traces read too), `SessionStats` (count, mean, p50, p95, max, per frame), `Diff` and
  `pair_sessions` (same name, else the same name past the platform prefix).
- `cargo xtask trace collect <log..> [--out] [--json]`, `trace report <trace> [--session]
  [--json]`, `trace diff <a> <b> [--session a=b] [--json]`.
- `cargo xtask test n64|3ds|web` collect every run's lines (not echoed) into
  `target/traces/<platform>.json` (+ the raw `.log`); the engine's run fails when the
  `selftest` session is missing or wrong.
- `--trace` (`test|build|run n64|3ds|web`, `run pc`): a separate build with
  `--features trino-core/trace`. The N64 and 3DS self-tests then trace each perf scene as
  its own session; **a `--trace` ROM reports its perf table but is never gated or blessed**
  (it is not the gated binary). `run pc --trace` writes `target/traces/pc.log` while the
  game runs (`TRINO_TRACE=<file>|stdout`) and `pc-trace.json` at exit.

### 4. PC and Web extras

- **GPU zones** (`renderer/gpu_zones.rs`): only while a trace records or Tracy listens, on
  adapters with `TIMESTAMP_QUERY`: an empty compute pass writes a timestamp at each pass
  boundary of the main screen (scene, shadow map, AO, upscale, 2D, VI, CVD view), read back a
  frame or two later and recorded on the `gpu` track from the submit's CPU time (the clocks
  differ). Nothing is created otherwise: WebGL2 and lavapipe without timestamps show none.
- **Tracy** (`trino-render-wgpu` feature `tracy`, forwarded by `trino-platform-pc`,
  `trino-runner-pc`, `trino-app-pc`): `tracy-client` 0.19 (Tracy 0.14) with `enable`,
  `ondemand`, `broadcast`. Live hooks (`trace::set_hooks`: begin/end/counter/frame) turn
  zones into Tracy spans per thread, counters into plots, frame marks into frames; the GPU
  timestamps go to a Tracy GPU context. The Tracy client starts with the process (its C++
  static initializer), so the hooks keep their own "started" flag.
- **Web**: the same `TraceHost` (`trino_render_wgpu::trace_host`) prints the lines on the
  console every 120 frames, and the end hook adds a `performance.measure` per zone (the
  browser's own profiler shows them). Not Tracy: its client needs threads and sockets.

### 5. The guard on the N64's hot code

`build n64` writes the ROM's symbols (`mips64-elf-nm -S -C`) and `check_hot_symbols` fails
the build when `tessellate_joints::<false, false>` passes the 16 KB I-cache, or when a build
without `--trace` links any part of the recorder (`trace::global`, `Zone`, a host ring). The
ceiling is the I-cache, not today's exact size (CI's newer nightly may move it). Measured:
0x3f58 bytes on `main` and on this branch, with and without `--trace`.

### 6. The editor

The **Profiler** tab reads the same files (`target/traces/*.json|*.log`): a timeline per
track with zoom and pan, a per-frame flame view, a per-zone table and a second trace or
session to compare (the `Diff` table). It follows a file that grows (`run pc --trace` writes
`pc.log` live). Hooks: `TRINO_EDITOR_TAB=profiler`, `TRINO_EDITOR_TRACE`,
`TRINO_EDITOR_TRACE_B`.

## Consequences

- The standard evidence of a perf PR becomes `trace diff --json` beside the scene table
  (which stays the gate). The `n64-performance` skill says how.
- Real hardware: a flashcart's or 3dslink's log saved from the terminal is a `trace collect`
  input (the loaders' output must not be piped: AGENTS pitfall). Not yet verified on a
  physical console.
- `TraceRing` is 48 KB in a `--trace` console build's `.data`; the self-test's own ring is
  32 events on the stack.

## Alternatives considered

1. Text lines plus a script: no timeline, no editor integration.
2. Tracy everywhere with a custom console transport: Tracy's client needs std and threads; a
   binary ring with an offline conversion works on bare metal.
3. Puffin: fine for egui, but Perfetto/Chrome JSON is a neutral format every tool and agent
   reads.
