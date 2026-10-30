# ADR 0038: Performance instrumentation — backend counters, RSP cost per command, a CPU sampler, A/B over layouts

Status: accepted (October 2026, GitHub #79, with #105's follow-ups). Builds on ADR 0030 (the
trace format and the Profiler) and ADR 0012/0013 (code layout moves N64 timings). Implemented
in `trino-platform-n64` (`prof`, zones in the renderer's hot paths, the C shim's counters),
`trino-core` (`perf::RenderProfile::write_report_with`, `memo::evicts`, `trace::{adopt,
forward_begin, forward_end, TraceAttach}`, `Platform::trace_hooks`), `trino-trace`
(`perf`: reports, the RSP fit, symbols, A/B), xtask (`--prof`, `perf report|ab`, `rsp count`,
`watch pc --trace`), `trino-platform-pc` (`trace_attach`) and the editor's Profiler.

## Context

Finding the last 9 ms of `micro-river` (#69) took mostly measurement work the engine had no
tools for: timers in static accumulators, a throw-away sampling profiler, a `memcpy` caller
hook, per-command RSP drains and A/B builds, all written by hand and deleted before the PR.
#62 (ADR 0030) gave a timeline; this ADR makes the rest permanent, with the same data for an
agent (CLI, JSON) and in the editor.

## Decisions

### 1. One instrumented build, not a second recorder

The issue's `TRINO_PROF=scopes` is `trino-core`'s `trace` feature (ADR 0030): scopes are
zones, counters are `trace::counter` samples. `--prof scopes|rsp|sample` (or `TRINO_PROF`,
read only by `build|run|test n64`) implies `--trace`; `rsp` and `sample` add the
`trino-platform-n64` features `prof-rsp` / `prof-sample`. A plain build stays what it was:
`build n64` still fails if a plain ROM links a recorder symbol or if
`tessellate_joints::<false, false>` passes 16 KB, and every new entry point is `if
trace::ENABLED { out_of_line() }` with the body `#[inline(never)]`. Counters live in
`static`s, never in the renderer's structs (a 4-byte field in a hot struct moved a scene
0.38 ms, ADR 0013). The C shim is compiled twice: `shim.o` as before and `shim_prof.o`
with `-DTRINO_PROF`; its `TRINO_PROF_*` macros are empty in the first, so `shim.o` is byte
for byte the old one. An instrumented ROM links `shim_prof.o`.

### 2. Zones in the N64 renderer's hot paths

`memo` (the draw memo's lookup and a hit's queue writes), `eligible` (`rsp_eligible_mv`),
`rsp cmd` (building an RSP command), `cpu tnl` (around `tessellate_with`, never inside it),
`rsp submit` (writing queued commands), `water frame`, `water rings`, `water submit`, beside
#62's stage zones. Explicit `Zone::begin`/`end` spans where an early return would otherwise
stretch a scope.

### 3. Backend counters, per frame

`prof::NAMES`, in this order (the editor and `trino_trace::perf::COUNTERS` hold the same
list): `memo_hit`, `memo_miss`, `memo_culled` (a remembered whole-mesh cull),
`memo_store`, `memo_evict` (a store over another key; a moving object replaces its own last
entry too), `water_hit`, `water_miss`, `water_evict`, `rsp_reject` (an RSP mesh refused by the
eligibility test), then the shim's `rspq_words` (words the CPU wrote into the rspq buffer:
RSP commands exactly, rdpq triangles and blits by the write pointer's delta), `rsp_cmds`,
`ovl_switch` (consecutive commands for another overlay, rdpq's own counting as one),
`rsp_flush`, `tex_upload`, `tex_resident` (a bind TMEM already held), `cpu_tris`. They are
folded into totals at `end_frame` (and sampled on the trace's counter tracks), appended to
each `TRINO_PERF_SCENE` line of an instrumented build (`write_report_with`; parsers ignore
unknown keys, the gate compares named fields only) and to `TRINO_FRAME` (`N64Host`).

### 4. Reports as files

Every `cargo xtask test n64` writes `target/perf/n64-<scene>.json`
(`trino_trace::perf::PerfReport`, format `trino-perf-report` v1): the scene line's fields,
the models, the counters, the scene's trace session (`SessionStats::to_json`), the RSP fit
and the samples when the build has them. An instrumented run prints each report's table;
`cargo xtask perf report <file..> [--json]` prints any of them later. The editor reads the
same files.

### 5. The RSP cost per command (`--prof rsp`)

While a profile window is open, `submit_rsp` and the water bucket submit each command alone:
drain, write it, drain again, and charge the time minus the empty drain (the median of 16,
calibrated at the window's start: 30–33 µs in ares) to (overlay, vertices, triangles). The
work comes from the command's own segment table (16-byte entries, counts at bytes 6 and 8).
`TRINO_PROF_RSP:` lines carry one entry per distinct shape; the host fits µs = per command
+ per vertex · v + per triangle · t per overlay by weighted least squares, dropping terms
the data can't separate (one shape: the mean). Streamed water commands outside the batch
(`water_frame`'s own submit) are not timed. Measured in ares (2026-10-29, castle64 hub and
the showcase river): white 154 µs mean ≈ 15.2 + 3.0·v (−0.38·t: v and t move together on
those meshes); colored 260 µs ≈ 27.2 + 1.28·v + 2.51·t (r² 0.99); textured 116 + 2.47·v;
the river's water command 310 µs (44 vertices, 40 triangles).

### 6. `cargo xtask rsp count`

Static instruction counts per overlay path from the assembled overlays: `nm -n` of each
`target/n64/rsp/<overlay>.elf` (every `.text` label starts a block that runs to the next),
summed over named label spans (`rsp_count::PATHS`: command, palette entry, segment, vertex
pass, triangle, rdpq triangle; the water overlay's surface pass, ripples and ring pass,
marked by two code-free labels added to `rsp_trino.S`, `water_ripple_pass` and `water_lit`),
and IMEM/DMEM from the sections (`.text` includes rspq's kernel: water 4 028 / 4 096 bytes).
`tests/perf/rsp_counts.toml` pins them; `cargo xtask test n64` (CI's n64 job) fails when one
moves, `--bless` rewrites it. The counts are static (every instruction of a span, branches
not followed): ares charges ~4–5 cycles per RSP instruction (#69), so they are the model.

### 7. A sampling profiler that reports its own cost (`--prof sample`)

A continuous libdragon timer reads COP0 EPC (the interrupted PC: the handler runs with
interrupts off) into a 32 K-sample buffer over the profile window, every 100 µs. The first
start calibrates the handler: the same busy loop with and without sampling, divided by the
samples (5.3 µs in ares, so 5 % of the sampled time at 100 µs; 10.6 % at 50). The ROM prints
`TRINO_PROF_SAMPLE:` lines (counts per PC); xtask symbolizes them with the ELF's `nm -S -C`
(`target/n64/<name>.syms`) and prints the top functions. A sampled build records no zones
(`trace_start` refuses): the recorder would be in its samples. Time spent with interrupts
off is charged to `enable_interrupts`, and `__rsp_check_assert` is the CPU spinning on the
RSP: read those as waits.

### 8. `cargo xtask perf ab`

`--base <rev>` (and `--head <rev>`, default the working tree) each in its own git worktree
under `target/perf-ab/trees/` with its own `target/` (the AGENTS pitfall), `--control` adds
the base plus a no-op patch (`perf_cmd::CONTROL_PATCH`: a linked, never-called function in
the N64 backend). Padding-major runs of `cargo xtask test n64 --expansion-pak on` with the
same `TRINO_N64_TEXT_PAD` values (the skill's eight by default), logs kept per arm and
padding (`--reuse` reads them again), and `trino_trace::perf::AbReport` (`ab.json`, format
`trino-perf-ab`): per scene and stage each arm's mean, min, max and per-padding values, and
whether every arm did identical T&L work (`same_work`). Identical work plus a shift the
control matches = layout.

### 9. The editor

The Profiler tab has three views (`TRINO_EDITOR_PROFILER=timeline|reports|ab`):
**Timeline** (#62's), **Reports** (the Game view's PC frame live — its ms over the last 120
frames, draws, triangles, GPU T&L draws, the renderer's CPU split — then a report from
`target/perf/` with its stage bar and tables, compared with a report B;
`TRINO_EDITOR_PERF`, `TRINO_EDITOR_PERF_B`) and **A/B** (`ab.json`: per scene and stage,
each arm's spread as a strip with a dot per padding; `TRINO_EDITOR_AB`). Files reload when
they change. Play started while the tab shows builds the game with `trino-core/trace`, sets
`TRINO_TRACE=target/traces/pc.log` and follows that file into the Timeline (#105 item 5).

### 10. #105's follow-ups

- **Item 2, the N64 flush.** ISViewer costs ~310 µs per 96-byte trace line in ares
  (`TRINO_TRACE_FLUSH:` lines of every `--trace` self-test: 289–315 µs). The play loop
  (`TraceLoop`) now encodes the ring every 60 frames and prints its lines over the next 60
  frames (`trace encode` and per-frame `trace flush` zones), so the cost is a line or so per
  frame instead of a spike. The hardware's own cost per line is part of #123.
- **Item 4, game zones under hot reload.** `Platform::trace_hooks` (default `None`; no
  `GAME_API_VERSION` bump) hands the game a `TraceAttach`: hooks into the host's ring and
  its clock. `export_game!` calls `trace::adopt` each update in a traced build; a copy of
  `trino-core` that records itself (a static build) adopts nothing. The PC copies each dylib
  name once into leaked memory (a reload unmaps the dylib; the ring keeps names until it is
  flushed). `cargo xtask watch pc --trace` builds the app and every reload with the zones.
- **Item 5, attach to Play.** §9.
- **Item 1 (traces from real consoles)** needs the hardware: it runs with #123.
- **Item 3 (zones on other threads)** is filed as its own issue: a per-thread buffer merged
  at flush, or a track per thread, changes the ring's single-writer design.

## Consequences

- A perf PR quotes a `perf ab --control` table and the reports' JSON instead of hand-made
  timers. The pinned overlay counts put an RSP change's cost in the diff.
- An instrumented ROM is another binary: its numbers are reported, never gated or blessed.
  The zones themselves cost time (the river under `--trace` runs ~1 ms slower than plain).
- The counters see only what the backends call; rdpq's internal state changes and RDP
  timing stay outside (ares doesn't time the RDP; #123, #82).

## Alternatives considered

- A separate `TRINO_PROF=scopes` table keyed by `&'static str`: a second recorder beside
  the trace ring, with its own compile-out rules. Zones already are that table (`trace
  report` gives count, mean, p95 and per frame).
- Counters as renderer fields: smaller code, but hot-struct fields move timings (ADR 0013).
- Sampling at #69's 4–8 µs: four times the frame; 100 µs with the cost measured and printed
  keeps the perturbation at ~5 %.
- Counting RSP cycles by emulating the overlay on the host: exact, but a second RSP model to
  keep in step with ares; static counts plus measured per-command times are enough to rank
  changes.
