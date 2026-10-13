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
  converts floats and writes the rspq command, and the RSP does edge setup). Those
  writes are the cost: 3 × 8 + 2 = 26 words per triangle through a `volatile` pointer
  into the rspq buffer, which libdragon allocates with `malloc_uncached`. Measured with
  a throwaway shim that skips `rdpq_triangle` during the perf scenes: submit fell from
  22.4 to 10.0 µs per triangle, so `rdpq_triangle` is ~12.4 µs (~1 160 cycles, ~45 per
  uncached word). The Rust repack and the shim's conversion are the other ~10 µs, but
  they overlap the write-buffer drain of the previous triangle: removing the 12
  `div.s` per triangle made submit slower, not faster (1f). Only fewer words per
  triangle would help. `begin` = framebuffer acquire, **mostly waiting** (vsync/RDP — it
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

T&L ≈ **8.1 µs/vertex + 27.5 µs/source triangle + 67.7 µs/draw + 25.5 µs/split**, plus
**~29.5 µs per emitted triangle** for sort + submit. It was recalibrated on 2026-10-13
after the throughput plan's Phase 1: 44 per-model points, mean error 6%, scene totals
−1…+4%. Before Phase 1 it was 16.6 / 26.1 / 278 / 22.3 / 30.

Real meshes carry ~2 vertices per triangle, so the vertex and triangle terms trade off
in the fit. Trust their sum, ~44 µs of T&L per closed-mesh triangle (~59 µs with its
share of sort + submit), not the split between them.

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
cost, a 30 fps frame affords roughly 450–550 source triangles (300–400 before Phase 1),
fewer with many draws. The
original KayKit door (1068 tris) read "27% of budget" and cost ~64 ms, two whole
frames. Trust the cost model.

## Where the time goes (2026-10-13, after Phases 1–3a)

| Scene | Frame (ms) | ≈ fps | T&L (ms) | sort + submit (ms) | game (ms) | Draws |
|---|---|---|---|---|---|---|
| hub | 44.0 | 23 | 30.0 | 10.4 | 2.7 | 26 |
| green-hills | 21.6 | **46** | 14.2 | 4.6 | 1.9 | 16 |
| lava-land | 23.7 | **42** | 15.7 | 5.2 | 1.9 | 16 |
| sky-tower | 20.4 | **49** | 13.3 | 4.2 | 1.9 | 19 |
| throne | 16.3 | **61** | 10.3 | 3.5 | 1.7 | 12 |

- Hub T&L by model (ms, calls):
  door_frame ×3: 6.3; block_brick ×7: 4.4; door ×1: 4.1; boar ×1: 3.7; player_leg ×2: 2.3; player_arm ×2: 2.0; player_torso ×1: 1.9; block_roof ×3: 1.7; block_castle ×3: 1.5; block_grass ×1: 0.8; star ×1: 0.6; shadow ×1: 0.6.
- A visible draw costs ~50 µs before its first vertex (280 µs at Phase 0) plus
  ~13–29 µs outside the T&L timer. A Gouraud vertex ~10 µs, a front-facing emitted
  triangle ~30 µs, submit ~22 µs per emitted triangle (26 uncached rspq words).
- No castle64 draw splits any more (3a). The remaining hub cost is the doors, the
  bricks, the boar and the 5-part player: per-vertex and per-triangle work.
- **The enemy is not the problem.** A boar costs ~3.5 ms, and all game logic ~0.3 ms.

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
| **3a** `split_policy`: DEPTH_SPLIT only for fogged, textured or translucent draws (tint or vertex alpha < 255); opaque draws go unsplit, since per-vertex z `1 − NEAR/z` is affine on screen | frame hub −17%, lava-land −30%, throne −45%, green-hills −7%, sky-tower +0.5%; castle64 splits 199/30/211/4/252 → 0, emitted hub 509 → 310, throne 341 → 89. Output-moving: work baseline, 3DS golden (mean 0.006/255, a few pixels); N64 showcase golden identical (fogged scene keeps its splits); N64 hub/lava/throne checked by eye in one-off dumps, no z-fighting |
| **1i** T&L code size: each `tessellate_joints` instance `#[inline(never)]` and the 5-plane clip in one `#[cold]` `clip_frustum` | `tessellate_with` was **60 KB** (static + two skinned paths + five inlined clip planes, ~4× the 16 KB I-cache); now 1.9 KB + 13.9 KB static path + 6 KB cold clip. Frame −4.5…−7.9%, T&L −8…−12% in every scene. Goldens identical. (Not the reverted 2026-10-13 attempt, which split the hot loop itself) |
| **2a** castle64 coins as billboards (`draw_billboard` of the HUD's `S_COIN`, width `0.8·max(|cos spin|, 0.15)`); `billboard_with` reuses the `TnlContext` | coins' T&L gone (hub 3.4 ms, stages 1.6–3.4 ms), sort/submit/`game` +0.3–0.4 ms each (textured batch). Frame: throne −8.9%, green-hills and lava-land −4.9%, hub +0.3%, sky-tower +0.9%. That build was an unlucky layout: the micro scenes, identical code with no billboards, got T&L +17…+34%. Output-moving: 3DS golden (round sprite coins), work baseline −coin counters |
| **1h** `Mat34::from_rotation_scale_translation` skips the software sin/cos for `+0.0` angles (`sin(0) = 0`, `cos(0) = 1` bit-exactly, pinned by a test) | `game` −15…−18% in every scene; `Transform3::matrix` 26 → 7 µs per draw with zero rotation, 26 → 12 µs with a yaw. T&L +4% in two scenes (layout). Frame −1.4…+1.6%. Kept on the `game` stage win |
| 1f One `trino_tris(ptr, n)` FFI call per batch (packed 48-byte triangles) and a 256-entry `c / 255.0f` table instead of 12 `div.s` per triangle in the shim | submit **+3…+6%** in every scene, frame −1.5…+4.6%: reverted. The submit is store-bound, not compute-bound: `rdpq_triangle_rsp` writes 26 words per triangle into the rspq buffer, which is `malloc_uncached`, and the divisions ran while the write buffer drained (likely; consistent with the skip-`rdpq_triangle` experiment above) |
| **1e** Tint quotients `tint / 255.0` hoisted from `Setup::finish` (per vertex) into `Setup` (per draw); same operations, same order | frame −8…−14%, T&L −12…−20% in every scene (hub 64.9 → 56.8 ms, sky-tower 25.2 → 21.6). `div.s` in the static vertex closure 15 → 3 (the normalize and fog remain). Goldens identical |
| **1d** Vertex fetch through fixed-size records (`as_chunks`: `&[[u8; 12]]` positions/normals, `[[u8; 4]]` colors, whole-triangle index records): one bounds check per attribute instead of one per byte, none per triangle | frame −5…−14% (green-hills 34.4 → 29.5 ms, sky-tower 28.8 → 25.2); T&L −10…−20%; Gouraud per vertex 21.7 → 15.8 µs. Goldens identical. The floats are still assembled from bytes (24 `lbu` + shifts per vertex, ~100 cycles of ~1 500); byte-swapping at register was not worth its RAM |
| **1c** `MeshLayout`: each TMDL validated once at register, the draw's `Mesh` rebuilt from byte spans (`Mesh::from_layout`) instead of `Mesh::from_tmdl` per draw (all three backends) | `game` −13…−20% in every scene; culled draw outside the T&L 25 → 16 µs. T&L +8…+12% with unchanged `tessellate` code and submit −17%: layout. Frame −0.1…+3.8% (green-hills). Kept on the `game` stage win |
| **1b** `TnlScratch`: vertex-cache payload, clip polygons, split stack and the skinned joint matrices (~9 KB) owned by the backend and never cleared; only the 32 cache tags reset per draw | **frame −12…−15% in every scene** (hub 80.0 → 70.2 ms, green-hills 38.8 → 33.2); T&L −19…−22%; visible-draw setup 248 → 58 µs. `submit` +20% from layout. Goldens identical |
| **1a** `TnlContext`: view, focal (software sin/cos), guard band, `ldir`, light factors once per `set_camera`/`set_light` instead of per draw (all three backends; `tessellate_with`) | culled draw T&L 33 → 16 µs, visible setup 280 → 248 µs; scene T&L −0.5…−2.6% (throne +1%), but `game` +15…+22% from layout: `Transform3::matrix`, which didn't change, went 12.5 → 36 µs per draw (micro-trs). Frame −1.2…+1.9%: neutral, kept because the work it removes is measured |

## Reading the ROM's machine code

Before guessing where CPU time goes, look at what the compiler emitted:

```sh
docker run --rm -v "$PWD/target/n64":/w -w /w trino-n64:latest \
  sh -c 'mips64-elf-objdump -d --no-show-raw-insn trino.elf' > /tmp/rom.S
```

Check code size first: `mips64-elf-nm -S --size-sort -C trino.elf | tail -30` in the
same image. A hot function near or above 16 KB (the I-cache) is a lever on its own:
`tessellate_with` was 60 KB until 1i. Code layout then moves less between builds too.

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
