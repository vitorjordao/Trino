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
TRINO_PERF_SCENE:hub frames=4 update=272 render=78556 begin=2676 tnl=55014 sort=3808
  submit=12191 sprites=695 end=21 game=4148 vblanks=19 verts=1397 tris_in=661 clipped=20
  splits=199 backfaced=326 emitted=533 culled=1 draws=34
TRINO_PERF_MODEL:hub model=e2dd959e calls=3 us=8659 verts=264 tris_in=132 emitted=72 splits=0
```

- **Times** (µs, emulated N64 clock: ares emulates the count register; the shim
  converts exactly with `TIMER_MICROS_LL`): `tnl` = `tessellate` for all models,
  `sort` = batch depth sort, `submit` = handing triangles to the RDP (the CPU side of
  `rdpq_triangle`: at the pinned libdragon it is `rdpq_triangle_rsp`, so the CPU only
  converts floats and writes the rspq command, and the RSP does edge setup), `begin` = framebuffer acquire, **mostly waiting** (vsync/RDP — it
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

T&L ≈ **16.6 µs/vertex + 26.1 µs/source triangle + 278 µs/draw + 22.3 µs/split**, plus
**~30 µs per emitted triangle** for sort + submit. It was recalibrated on 2026-10-13
after back faces moved ahead of clipping: 44 per-model points, mean error 5%, scene
totals ±2% (sky-tower +11%).

Real meshes carry ~2 vertices per triangle, so the vertex and triangle terms trade off
in the fit. Trust their sum, ~60 µs per closed-mesh triangle, not the split between
them.

The model is used by:
- `cargo xtask model inspect`: "N64 cost estimate", with a warning above 25% of a
  30 fps frame;
- the editor Game view's budget HUD: the "N64 3D time" row, N64 profile;
- the `test n64` table: the `model est` column, plus a `note:` line when off by more
  than 25%.

### Measured fixed costs (micro scenes)

The fit can't separate per-draw from per-vertex cost. The **micro scenes** measure
them directly (`apps/n64/src/n64/micro.rs`, run by `test n64` after the castle64
scenes). They are synthetic draw sets built in code: 64 one-triangle draws on screen,
the same 64 culled behind the camera, the same 64 through `draw_model` (zero rotation
/ yaw), and one 16×16-vertex grid (256 vertices, 450 triangles, facing the camera, no
clipping or splits) in Gouraud, Unlit and Flat, plus the grid with every triangle
listed twice (900 triangles, same vertices).

- `xtask/src/perf.rs` (`is_micro`, `micro_report`) keeps `micro-*` lines out of the
  gates and the bless.
- It **fails** if a micro scene's work counters are off: a number from the wrong T&L
  path (clipping, splits, back faces, a rejected blob) is worse than none.
- It prints the derived table after the castle64 table. The per-draw, per-vertex and
  per-triangle terms come from three equations:
  `A/64 = d + 3v + t`, `G = d + 256v + 450t`, `T = d + 256v + 900t`.
  Sort and submit per emitted triangle are read from the 64- and 900-triangle
  batches.
- `outside tnl` is the scene's `game` field per draw: `Mesh::from_tmdl`, the mesh map
  lookup and `Transform3::matrix`, which the `tnl` timer doesn't cover.

2026-10-13, on the Phase 0 ROM (emulated µs; cycles at 93.75 MHz):

| Cost | µs | cycles |
|---|---|---|
| culled draw (bounding-sphere reject), T&L | 32.8 | 3 070 |
| visible draw **setup** before any vertex (fit `d`) | **280** | 26 300 |
| draw call outside the T&L timer (parse, lookup) | 35–51 | 3 300–4 800 |
| `Transform3::matrix` (zero rotation or yaw; same cost) | 12.5 | 1 180 |
| per vertex, Gouraud | 28.1 | 2 630 |
| per vertex, Unlit | 33.3 | 3 120 |
| per front-facing, emitted source triangle (includes the 88-byte push) | 28.8 | 2 700 |
| per triangle all-in, Flat (its 256 position-only vertices included) | 67.3 | 6 300 |
| sort per emitted triangle | 2.4 | 220–230 |
| submit per emitted triangle | 22–24 | 2 100–2 300 |

How to read them:
- The visible-draw setup (280 µs) dwarfs the culled floor (33 µs). What lies between
  the cull and the first vertex is the `Setup`, `light.dir.normalized()` and the
  initialization of ~4.5 KB of scratch (vertex cache, `poly_a/poly_b`, split stack)
  in an 8 KB direct-mapped D-cache.
- Per vertex and per triangle are higher here than the scene fit's split suggests,
  because every grid triangle is front-facing and emitted. Real meshes reject about
  half as back faces. Use the fit for scene totals and these numbers for "what does
  one more draw / vertex cost".
- Sort is cheap. Submit costs 10× the sort per triangle.
- Unlit costs *more* per vertex than Gouraud on this build. That points at
  `Setup::finish` (4 `/ 255.0` divisions per vertex) and code layout, not lighting.

Recalibrate after any T&L or backend change. The `note:` line tells you when.
1. Collect the `TRINO_PERF_MODEL` lines of a run.
2. Least-squares `us ≈ a·verts + b·tris_in + c·calls + d·splits` over every line with
   `verts > 0`. numpy isn't installed; normal equations in plain Python are enough.
3. Check the scene totals.
4. Update `N64_COST` and its calibration test in `crates/core/src/perf.rs`.

**`Caps::N64.max_tris_per_frame = 4000` is not a CPU-T&L budget.** At the measured
cost, a 30 fps frame affords roughly 300–400 triangles, fewer with many draws. The
original KayKit door (1068 tris) read "27% of budget" and cost ~64 ms, two whole
frames. Trust the cost model.

## Where the time goes (2026-10-13, after the content + back-face pass)

| Scene | Frame (ms) | ≈ fps | T&L (ms) | Draws |
|---|---|---|---|---|
| hub | 76.2 | 13 | 55.0 | 34 |
| green-hills | 37.3 | 27 | 28.1 | 23 |
| lava-land | 49.1 | 20 | 33.7 | 22 |
| sky-tower | 31.2 | **32** | 23.2 | 26 |
| throne | 43.4 | 23 | 28.7 | 19 |

- **The hub has no single culprit any more:**
  - door frames ×3: 8.7 ms;
  - brick blocks ×7: 7.8 ms;
  - coins ×8: 6.3 ms;
  - doorway: 5.2 ms;
  - grass (193 splits): 5.0 ms;
  - boar: 4.6 ms;
  - player, 5 draws: 9.3 ms.
- About 9.5 ms is per-draw overhead (34 × 0.28 ms).
- Sort + submit: 16 ms.
- **The enemy is not the problem.** A boar costs ~4.6 ms, and all game logic ~0.3 ms.

## What worked, what didn't (keep this list honest)

| Change | Result on the scenes |
|---|---|
| T&L scratch (`poly_a/poly_b/stack`, 3.8 KB) initialized once per draw, not per triangle | hub render 640 → 259 ms, goldens identical |
| Sort 8-byte keys `(group, painter_key)` instead of moving 88-byte `ScreenTri`s; explicit tie-break | 259 → 190 ms; same order on every backend |
| Link newlib's `memcpy/memset/memmove/memcmp` (Rust's weak generic ones won) | update −10%, T&L −0.4…−1.6% (kept; `check_mem_symbols` guards it) |
| Light declared by castle64 every frame (it inherited the showcase's fogged light) | hub 190 → 175 ms (fog/colored-light path off) — a correctness fix that also measured right |
| **Content rebuilt in Blender** (`tools/blender/castle64_models.py`): KayKit doorway 1068 → 75 tris, boar 120 → 70, stage door 60 → 44, player parts −10…20%, only faces a camera can see | frame: hub 174 → 88.5 ms, the stages −3…−14% (same ROM code: a clean A/B) |
| Bake reorders triangles/vertices for the 32-slot vertex cache (`MeshData::optimize_vertex_order`) | Blender exports: transforms 3/tri → 2/tri (boar 210 → 140) |
| **Back faces rejected before clipping/splitting** (view-space triple product, `tessellate`) | frame −8…−17% more (hub 88.5 → 76.2, throne 52.2 → 43.4); splits −60…−80%; `emitted` and every golden (PC, N64, 3DS) identical |
| Per-vertex cached projection + no-copy fast path in `tessellate` | neutral (±2%) — reverted |
| Hot loop compacted (`#[inline(never)]` vertex/flat/general paths) | +3…+17% worse — reverted |
| `opt-level = "s"` for the ROM | worse everywhere — not adopted |

## Reading the ROM's machine code

Before guessing where CPU time goes, look at what the compiler emitted:

```sh
docker run --rm -v "$PWD/target/n64":/w -w /w trino-n64:latest \
  sh -c 'mips64-elf-objdump -d --no-show-raw-insn trino.elf' > /tmp/rom.S
```

Then find the function (`grep` its mangled name, e.g. `tessellate_joints`) and count
`div.s` (~29 cycles each), `cvt`/`trunc`, `jal` targets (non-inlined helpers,
`panic_bounds_check`) and `__*sf*` libcalls. Findings on 2026-10-13:
- no soft-float;
- `rd_vec3` not inlined, with 12 bounds checks per call;
- 15 `div.s` in the static vertex closure;
- the shim's `trino_tri` divides 12 color bytes by 255 for every triangle.

They are in `docs/plans/n64-throughput.md`.

## Remaining levers (BACKLOG P0)

The ordered plan is `docs/plans/n64-throughput.md`.

- **RSP T&L**: a small ucode, or tiny3d's, behind the same `draw_model` contract
  (ADR 0003). It is still the lever for the hub (~2.3× short of 30 fps).
- **Fewer draws.** Each one costs 0.28 ms before any vertex:
  - merge each level's static blocks into one baked mesh (also drops buried faces
    between blocks);
  - draw coins as billboards (`draw_billboard`, no T&L);
  - make the player one rigid-skeleton model instead of 5 draws.
- **DEPTH_SPLIT policy**: every target z-buffers. The remaining splits are on front
  faces of big blocks (hub grass 12 → 198 tris). Relaxing them moves goldens, so it is
  a deliberate renderer change.
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
