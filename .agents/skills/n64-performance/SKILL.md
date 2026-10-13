---
name: n64-performance
description: Measure and improve Trino's frame cost on the N64 — the TRINO_PERF_SCENE / TRINO_PERF_MODEL reports (emulated N64 time from ares, per stage and per model), the perf gates (PC work baseline + N64 ceilings), the calibrated cost model (trino_core::perf::N64_COST), how to compare before/after when code layout alone moves numbers, and what has and hasn't worked. Use for "make it faster on N64", "frame rate", "why is it slow on the console", T&L or backend changes that could cost CPU, or heavy new content.
---

# n64-performance

## The numbers that count

`cargo xtask test n64` runs every `castle64::perf::SCENES` scene (hub, green-hills,
lava-land, sky-tower, throne — each a **fresh** game at its spawn after 60 idle
updates, then 4 measured update+render frames) and prints, per frame:

```
TRINO_PERF_SCENE:hub frames=4 update=279 render=174865 begin=6892 tnl=129774 sort=9202
  submit=24122 sprites=688 end=30 game=4155 vblanks=44 verts=3260 tris_in=1770 clipped=40
  splits=504 backfaced=1204 emitted=1074 culled=1 draws=34
TRINO_PERF_MODEL:hub model=e2d8d1f0 calls=1 us=63603 verts=1790 tris_in=1068 emitted=552 splits=0
```

- **Times** (µs, emulated N64 clock: ares emulates the count register; the shim
  converts exactly with `TIMER_MICROS_LL`): `tnl` = `tessellate` for all models,
  `sort` = batch depth sort, `submit` = handing triangles to the RDP (`rdpq_triangle`
  is CPU-side C), `begin` = framebuffer acquire, **mostly waiting** (vsync/RDP — it
  swings 0.1–9 ms between runs, never judge on it), `game` = castle64's own render
  code, `vblanks` = VI interrupts during the frames (independent clock check).
- **Work** (deterministic, identical on every target): `verts` = vertex
  transforms (cache misses), `tris_in` source triangles, `clipped`, `splits`
  (DEPTH_SPLIT bisections), `backfaced`, `emitted`, `culled` (whole draws).
- **Per model** lines: the hot list. Map a handle to a name with
  `ModelId::from_path("models/<manifest name>")` (FNV-1a of the path) — or run the PC
  test below, which prints names.
- Code: `trino_core::perf` (`RenderProfile`, `CostModel`), the N64 renderer's
  `start_profile`/`take_profile`, `castle64::perf`, `apps/n64/src/lib.rs` step 5.

## The gates (keep the data honest over time)

| Gate | Where | Checks |
|---|---|---|
| work baseline | `xtask/tests/perf_budget.rs` (`cargo test --workspace`) | every scene's T&L counters **equal** `tests/perf/castle64_work.toml` — runs the real `tessellate` on PC with a counting renderer; no emulator |
| cross-target | `cargo xtask test n64` (`xtask/src/perf.rs`) | the ROM's counters equal the same file (T&L determinism across targets) |
| time ceilings | `cargo xtask test n64` | `frame_us` (= update + render − begin) ≤ ceiling (+20% headroom), `tnl_us`/`sort_us`/`submit_us` ≤ ceiling (+35%), from `tests/perf/castle64_n64.toml` |

Intentional change: `TRINO_BLESS=1 cargo test -p xtask --test perf_budget` (work),
then `cargo xtask test n64 --bless` (ceilings + golden). **Bless after an
optimization too** so the gain is locked in. Review both TOML diffs: a number going
up is a cost the N64 pays.

## Code layout moves the numbers — read them like this

- ares is **bit-reproducible** for the same ROM (two runs → identical µs).
- But layout is not neutral: adding an unrelated 1 KB function moved hub `tnl`
  −10% and `submit` +20% (16 KB direct-mapped I-cache, 8 KB D-cache), while the
  frame total moved 5%. Hence the loose stage ceilings.
- So: **never claim a <10% stage win from one build**. A/B on *identical* code
  except the change (e.g. the memcpy link was A/B'd by toggling only the linker
  line), compare all five scenes, prefer the frame total.
- **Benches lie, scenes don't.** A synthetic `tessellate` loop on the door mesh
  (TEMP microbench) showed −15% for a restructure that was neutral-to-worse on the
  scenes. Judge only on `TRINO_PERF_SCENE`.

## The cost model (`trino_core::perf::N64_COST`)

T&L ≈ **9.0 µs/vertex + 44.2 µs/source triangle + 419 µs/draw + 14.7 µs/split**, plus
**~30 µs per emitted triangle** for sort + submit (calibrated 2026-10-13, ±11%).
Used by `cargo xtask model inspect` ("N64 cost estimate", warning above 25% of a
30 fps frame), the editor Game view's budget HUD ("N64 3D time" row, N64 profile)
and the `test n64` table (`model est` column; a `note:` line when off by >25%).

Recalibrate after any T&L/backend change: collect the `TRINO_PERF_MODEL` lines of a
run, least-squares `us ≈ a·verts + b·tris_in + c·calls + d·splits` (13+ points;
numpy isn't installed — normal equations in plain Python are enough), check the
scene totals, update `N64_COST` and its calibration test in `crates/core/src/perf.rs`.

**`Caps::N64.max_tris_per_frame = 4000` is not a CPU-T&L budget**: at the measured
cost a 30 fps frame affords ~200–300 triangles. The KayKit door (1068 tris) reads
"27% of budget" but costs ~64 ms ≈ two whole frames. Trust the cost model.

## Where the hub's time goes (2026-10-13, render 175 ms, update 0.3 ms)

T&L 130 ms (door 64 = half of it; door frames 13; one boar 8; the big grass block 8
from 464 depth splits), submit 24, sort 9, begin ~1–9 (waiting), game 4. **The enemy
is not the problem** (a boar ≈ 8 ms, all game logic ≈ 0.3 ms); the door asset and the
CPU T&L's per-vertex/per-triangle cost are.

## What worked, what didn't (keep this list honest)

| Change | Result on the scenes |
|---|---|
| T&L scratch (`poly_a/poly_b/stack`, 3.8 KB) initialized once per draw, not per triangle | hub render 640 → 259 ms, goldens identical |
| Sort 8-byte keys `(group, painter_key)` instead of moving 88-byte `ScreenTri`s; explicit tie-break | 259 → 190 ms; same order on every backend |
| Link newlib's `memcpy/memset/memmove/memcmp` (Rust's weak generic ones won) | update −10%, T&L −0.4…−1.6% (kept; `check_mem_symbols` guards it) |
| Light declared by castle64 every frame (it inherited the showcase's fogged light) | hub 190 → 175 ms (fog/colored-light path off) — a correctness fix that also measured right |
| Per-vertex cached projection + no-copy fast path in `tessellate` | neutral (±2%) — reverted |
| Hot loop compacted (`#[inline(never)]` vertex/flat/general paths) | +3…+17% worse — reverted |
| `opt-level = "s"` for the ROM | worse everywhere — not adopted |

## Remaining levers (BACKLOG P0)

- **RSP T&L** (a small ucode, or tiny3d's, behind the same `draw_model` contract,
  ADR 0003): the only lever of the needed size (~5×).
- **Content**: decimate or LOD (`model.lod`) the KayKit door — half the hub's T&L.
- **DEPTH_SPLIT policy**: every target z-buffers now, so splits mostly pay for
  painter-order quality (14.7 µs each, and they inflate emitted tris: grass 12 → 198,
  throne castle 60 → 273). Changing it moves goldens — a deliberate renderer change.
- **Caps recalibration** to a cost budget.

## Workflow for a perf change

1. Baseline on the base commit in a `git worktree` (never `git stash`).
2. Fast loop first: the PC work counters (`perf_budget`, seconds) tell you whether
   the change removes work; `TRINO_PERF_MODEL` says where the time is.
3. Hold the pixels: PC goldens and `tests/golden/n64/showcase.png` byte-identical,
   unless it is a deliberate renderer change (re-bless, review, explain).
4. Judge on the scene table (all five), on identical code apart from the change.
5. Bless both baselines, quote before/after in the commit and BACKLOG, update the
   "where the time goes" section and the table above.
