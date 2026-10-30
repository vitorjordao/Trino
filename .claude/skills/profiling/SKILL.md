---
name: profiling
description: Record and read timeline traces of a Trino game on every target — zones, counters and frame marks (`trino_core::trace`), `--trace` builds on PC, N64 (ares/hardware), 3DS (Azahar/3dslink) and the Web, `cargo xtask trace collect|report|diff`, Perfetto/Chrome JSON, Tracy on the PC, GPU pass zones, the editor's Profiler tab — plus the N64's backend counters, per-command RSP cost (`--prof rsp`), CPU sampler (`--prof sample`), `rsp count`, `perf report|ab` over code layouts — and turn them into the evidence a perf PR quotes. Use for "profile", "trace", "timeline", "where does the frame go", "spike", "stall", "Tracy", "Perfetto", "compare PC and N64", or before/after numbers of an optimization.
---

# profiling: one trace format from every target

ADR 0030 (GitHub #62). The summaries (`TRINO_PERF_SCENE`, `TRINO_FRAME`) stay the gates;
a trace says **when** things happened: which stage stalled, frame-to-frame variance, spikes.

## Record

| Target | Command | Output |
|---|---|---|
| N64 (ares) | `TRINO_ARES=docker cargo xtask test n64 --trace` | `target/traces/n64-trace.json` (+ `.log`): one session per perf scene (`n64-hub`, `n64-cpu-hub`, `n64-micro-…`) |
| 3DS (Azahar) | `cargo xtask test 3ds --trace` | `target/traces/3ds-trace.json`: `3ds-<scene>`, 30 frames each |
| PC | `cargo xtask run pc --trace` (or `TRINO_TRACE=<file>\|stdout` on any `--features trino-core/trace` build) | `target/traces/pc.log` while it runs, `pc-trace.json` at exit; CPU zones + GPU passes |
| Web | `cargo xtask test web --trace` / `run web --trace` | lines on the console every 120 frames (`target/web-test/<run>/console.log` → `target/traces/web-<run>-trace.json`), `performance.measure` entries in the browser's profiler |
| Hardware | `run n64 --device sc64 --trace`, `run 3ds --device network --trace` | the lines arrive on the terminal: save it (`script -q log.txt …`) and `trace collect log.txt`. Never pipe the loaders (AGENTS pitfall). Not yet verified on a console |
| Every boot without `--trace` | `test n64\|3ds\|web` | the self-test's `selftest` session (format check) in `target/traces/<platform>.json` |

`--trace` is a **separate build** (`--features trino-core/trace`). On the consoles its perf
table is printed but never gated or blessed; the goldens are skipped.

**Writing the trace costs time on the consoles:** the N64's ISViewer prints 60 frames of
events in ~115 ms (measured in ares, 2026-10-24). The play loops flush every 60 frames and
record the flush as a `trace flush` zone, so that frame's spike is labelled; the
self-test flushes after each measured scene, outside its frames. Judge frame times on
the frames between flushes.

## Read

```
cargo xtask trace report target/traces/n64-trace.json --session n64-hub   # per zone: count mean p95 max per_frame
cargo xtask trace diff target/traces/pc-trace.json target/traces/n64-trace.json --session pc-castle64=n64-hub --json
cargo xtask trace collect saved-terminal.log --out target/traces/sc64.json
```

- Open any `.json` in <https://ui.perfetto.dev> (drag the file) or `chrome://tracing`: one
  process per session, one thread per track (`cpu`, `gpu`, `rsp`, …), counters as tracks.
- The editor: **Profiler** tab (`TRINO_EDITOR_TAB=profiler`, `TRINO_EDITOR_TRACE=<file>`,
  `TRINO_EDITOR_TRACE_B=<file>` to compare). It follows a growing `pc.log` (attach to a
  running `run pc --trace`).
- `diff` pairs sessions by name, else by the name past the platform prefix (`pc-hub` ↔
  `n64-hub`), else the only session of each; `--session a=b` otherwise.

What the N64 zones mean: `begin` = waiting for a framebuffer (vsync/RDP: never judge on it),
`draw` = one `draw_model*` call (RSP draws included: their CPU side), `sort`, `submit` (CPU
triangles to the RDP), `rsp queue` (RSP commands queued), `2d`, `end` (frame end; the
profiled drain is in it), host `update`/`render`/`audio`. Counters: `draws`, `tris`,
`rsp_draws`, `rsp_tris`.

## Tracy (PC)

`cargo run -p trino-app-pc --features tracy` (or `trino-runner-pc`'s `tracy` feature in a
project), then connect the Tracy **0.14** viewer (`tracy-client` 0.19 = Tracy 0.14.x; a
mismatched viewer refuses the connection). Headless capture, verified 2026-10-24:

```
git clone --depth 1 --branch v0.14.1 https://github.com/wolfpld/tracy.git && cd tracy
cmake -S capture -B capture/build -DCMAKE_BUILD_TYPE=Release -DNO_ISA_EXTENSIONS=ON && cmake --build capture/build -j8
cmake -S csvexport -B csvexport/build -DCMAKE_BUILD_TYPE=Release -DNO_ISA_EXTENSIONS=ON && cmake --build csvexport/build -j8
capture/build/tracy-capture -o run.tracy -a 127.0.0.1 -f &   # then start the game
csvexport/build/tracy-csvexport run.tracy                    # zone names, counts, mean ns
```

## Instrument

- `trino_core::zone!("name");` times the rest of the block; `let z =
  trace::Zone::begin("x"); …; z.end();` for a span inside a function;
  `zone!(trace::track::AUDIO, "mix")` on another track; `trace::counter("name", v)`;
  `trace::frame_mark(n)` (the renderers already mark frames at `begin_frame`).
- Names are `&'static str` literals (interned, ≤ 128 per ring).
- Put zones at stage boundaries and per draw call, **never inside the T&L** (the N64
  I-cache: `build n64` fails if `tessellate_joints::<false, false>` passes 16 KB or a build
  without `--trace` links the recorder).
- Out-of-line work only when tracing: `if trino_core::trace::ENABLED { out_of_line() }`
  with the callee `#[inline(never)]` (the N64 renderer's `trace_counters`).
- **Other threads (issue #130):** `let _lane = trace::bind_thread(track::on(track::WORKER,
  n))` at the top of a thread's closure, then plain `zone!`s. The thread records into a
  `Lane` only it writes (512 events, `&'static str` names), and the main thread merges
  every lane into the ring at `frame_mark`, `flush` and `with_ring`: no lock, nothing lost
  unless a lane fills between two merges (then `dropped`). A track byte is a kind (low 3
  bits) and an instance (`track::on`, shown as `worker 3` through `track::label`); a
  thread has one track, whatever track a `zone!` inside it names.
  - Bound today: the PC's CPU T&L workers (`worker 1..8`, `tnl chunk`), the audio stream
    (`audio`: `stream fill` > `stream job`), the group loader's decoders (`loader 1..`,
    `decode entry`, through `par::map_ordered_on`), the 3DS worker core's queued jobs
    (`worker`, `job`; `test 3ds --trace` checks the `3ds-worker` session).
  - Binding needs `ThreadHooks`: the PC/Web `TraceHost::start` sets a `thread_local!`
    (`trace_host::thread_hooks`), the 3DS's `trace_start` keys two static lanes by the TLS
    register. Without hooks (an editor bake, the N64) `bind_thread` binds nothing and the
    zones go to the global ring as before; an unbound thread that finds the ring busy
    still loses its event (`dropped`).
  - A job a console runs **inline** (no worker, or its ring full) must not use the
    worker's lane: the 3DS times only queued jobs, through a trampoline.
  - Scoped workers come and go: pass the track instance from the worker's index, never
    the lane's slot, or the rows shuffle between frames.
- Game zones under hot reload: `cargo xtask watch pc --trace` builds the app and every
  reloaded dylib with the zones; the dylib's own `trino-core` adopts the host's ring through
  `Platform::trace_hooks` (`export_game!` calls `trace::adopt` each update; the PC copies
  each name once, `trino_platform_pc::trace_attach`, because a reload unmaps the dylib).
  A host of your own: return `trino_core::trace::TraceAttach` from `trace_hooks` with
  forwarders to `trace::forward_begin` / `forward_end`.

## Counters, RSP cost, CPU samples, A/B (issue #79, ADR 0038)

| Want | Command | Reads |
|---|---|---|
| Hot-path zones + backend counters | `cargo xtask test n64 --trace` (= `--prof scopes`, `TRINO_PROF=scopes`) | counters appended to each `TRINO_PERF_SCENE` line and `TRINO_FRAME`; tables printed; `target/perf/n64-<scene>.json` |
| Each RSP command's µs | `cargo xtask test n64 --prof rsp` | per overlay: µs = per command + per vertex · v + per triangle · t (the fit), from `TRINO_PROF_RSP:` lines |
| Where the CPU goes | `cargo xtask test n64 --prof sample` | top functions per scene (`TRINO_PROF_SAMPLE:` PCs + `target/n64/<name>.syms`), with the sampler's measured cost |
| Static RSP instructions | `cargo xtask rsp count [--blocks] [--json]` | per overlay path, IMEM/DMEM; `test n64` holds `tests/perf/rsp_counts.toml`, `--bless` rewrites it |
| Is it real or layout? | `cargo xtask perf ab --base <rev> --control [--paddings N]` | mean (min–max) per scene and stage per arm, `same_work`; `target/perf-ab/ab.json` |
| Any report again | `cargo xtask perf report <file..> [--json]` | the same tables / JSON |
| In the editor | Profiler › Reports / A/B (`TRINO_EDITOR_PROFILER=reports\|ab`) | the same files, plus the Game view's PC frame live |

- Counters (`trino_platform_n64::prof::NAMES`; `trino_trace::perf::COUNTERS` holds the
  same list): `memo_hit/miss/culled/store/evict`, `water_hit/miss/evict`, `rsp_reject`,
  and from the shim (`shim_prof.o`, `-DTRINO_PROF`) `rspq_words`, `rsp_cmds`,
  `ovl_switch`, `rsp_flush`, `tex_upload`, `tex_resident`, `cpu_tris`. `memo_evict` counts
  a store over any other key, stale or live: once the memo is full most stores evict,
  so read it against `memo_hit`/`memo_miss`, not as thrashing by itself.
- Adding a counter: a `Counter` variant + its name in both lists (Rust side), or a `TP_*`
  slot + `TRINO_PROF_ADD` in the shim (C side, in the `TP_*` order); bump only through
  `prof::bump`/`add` (compiled out without `trace`); never a renderer field.
- `--prof rsp` drains around every command of a profile window: its frame times are not
  the scene's. The empty drain (30–33 µs in ares) is subtracted. Streamed water commands
  submitted inside `water_frame` are not timed.
- `--prof sample`: 100 µs period; the handler costs ~5.6 µs in ares (5.5 %, printed as
  `handler_us`/`overhead_pct`). The sampled build records no zones (`trace_start`
  refuses). `enable_interrupts` collects the time spent with interrupts off;
  `__rsp_check_assert` and `rspq_*sync*` are waits on the RSP.
- ISViewer costs ~310 µs per trace line in ares (`TRINO_TRACE_FLUSH:` lines of every
  `--trace` self-test); the play loop (`TraceLoop`) prints a 60-frame ring's lines over
  the next 60 frames (`trace encode`, then a `trace flush` zone per frame).
- `perf ab` builds each arm in `target/perf-ab/trees/<arm>` (its own `target/`), runs
  `test n64 --expansion-pak on` per padding, keeps `target/perf-ab/<arm>/pad_<p>.log`
  (`--reuse` re-reads them). A failing gate in an arm doesn't stop the sweep: its lines
  still count. The control arm appends `perf_cmd::CONTROL_PATCH` (linked, never called)
  to `crates/platform-n64/src/lib.rs`.

## Evidence for a perf PR

1. The gate stays the scene table over 8 paddings: `cargo xtask perf ab --base origin/main
   --control` (`n64-performance`).
2. Add `trace diff --json` of the scene before/after (`--trace` builds of both commits,
   each in its own worktree and `target/`) and a Perfetto screenshot of the changed frame.
3. Quote zones' mean and p95 per frame, not a single frame.

## Code map

`crates/core/src/trace.rs` (ring, packet, lines, global recorder, hooks, lanes);
`crates/core/tests/trace_threads.rs`, `crates/platform-pc/tests/trace_threads.rs` (other
threads);
`crates/trace/` (`collect`, `chrome`, `stats`, `perf`: reports, the RSP fit, symbols,
A/B); `crates/platform-n64/src/prof.rs` (counters, `--prof rsp|sample`);
`xtask/src/{perf_cmd.rs, rsp_count.rs}`; `xtask/src/trace.rs` (CLI, `Capture` for
the emulator runs); `crates/render-wgpu/src/{trace_host.rs, tracy.rs,
renderer/gpu_zones.rs}`; `crates/platform-{n64,3ds}/src/runtime.rs` (`trace_start`,
`trace_flush`); the renderers' zones; `crates/platform-web/src/runtime.rs` (`mod trace`);
`examples/castle64/src/selftest.rs::trace` (the step); `crates/editor/src/profiler.rs`.
