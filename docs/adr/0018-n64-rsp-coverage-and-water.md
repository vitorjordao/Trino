# ADR 0018 — N64: the RSP takes fog, colored light, textures, rigid skeletons and water

- Status: **accepted**, implemented (GitHub #69: phases 0–3 in #77; the ripples and rings
  on the overlay, the issue's two budgets and phase 4's cost model in its second part, §7;
  a measurement on a real N64 is open, §8).
- Date: 2026-10-18
- Context: after #49 (ADR 0015) the showcase river ran at **286.6 ms** per frame in ares
  with **0 of 46** draws on the RSP. The overlay of ADR 0007 took only static, opaque,
  untextured, white-light, fog-free Gouraud/Unlit draws, and the showcase has a colored
  sun, fog, textures, rigid characters and water. The river alone cost 11.2 ms of CPU
  T&L against #69's 1.2 ms.

## 1. Phase 0: measure the river alone

- `micro-river` (`showcase::perf::MICRO_RIVER`): the scene camera, the game's light and
  water, two ripples, only the water entities drawn (`micro_river_ids` finds them once).
  It runs in the ROM before `showcase-river` and on the PC's counting renderer, so its
  work counters are pinned in `tests/perf/castle64_work.toml` and its time in
  `castle64_n64.toml`, like the castle64 scenes.
- Every number below is the mean (min–max) over the 8 `TRINO_N64_TEXT_PAD` paddings
  (0, 352, 1024, 2048, 3072, 4096, 6144, 8192) against the baseline's 8.

## 2. Phase 2: four overlays, one per surface

`rsp_trino.S` is assembled four times (`xtask/src/n64.rs`). The white path keeps its
bytes, so castle64's timings and its hub golden don't move.

| Overlay | Define | Adds | Command |
|---|---|---|---|
| `rsp_trino` | — | ADR 0007 | 27 words |
| `rsp_trino_color` | `TRINO_COLORED` | per-channel light color, emissive, double-sided (cull off), per-vertex fog (a fourth row of the matrix gives view depth; the factor goes into the shade like the CPU path's) | +13 color words |
| `rsp_trino_tex` | `+ TRINO_TEXTURED` | 40-byte vertices with u, v (s5.10); S, T = uv · size/32 + offset (s10.5), W and 1/W for `RDPQ_Triangle_Send_Async`'s textured triangles | +4 texture words |
| `rsp_trino_water` | `+ TRINO_WATER` | §4, §7 | a 0x240-byte water block per draw (two DMAs) |

- **Flat shading** takes a *flat copy* of the mesh: each face's vertices duplicated with
  the face normal (`flat_ratios`, keyed by the model scale's ratios in 1/256 steps), up
  to `RSP_FLAT_SEG_TRIS` faces per segment.
- **Rigid skeletons** (`rsp_rigid_draws`): one command per joint that has triangles,
  each part a static copy moved by `model · skin`. Every part must pass the static rules,
  or the whole draw stays on the CPU.
- **The split rule.** The CPU path bisects edges of fogged, textured or translucent draws
  spanning `DEPTH_SPLIT` of view depth. The RSP can't, so such a draw takes the overlay
  only when no edge could be split: `max_edge2 · |depth row|²` or the per-axis
  `edge_box` bound (tall thin pillars pass the second). Otherwise `RspReject::Fog` /
  `Textured` names the reason.
- **One answer for every predictor.** `render3d::rsp_plan` (commands, triangles, or the
  reason) is what `perf_budget`, `scene budget`, the editor's budget and the PC's strict
  N64 estimate call. The ROM's counters equal the PC baseline exactly.

## 3. Phase 1: content the RSP can take

- **The terrain in tiles** (`xtask/src/river_assets.rs`): a quadtree around the scene
  camera, 8 → 1 cells of 2 m, 52 `terrain_<x>_<z>_<size>` meshes in a `ground` group.
  A tile under the camera is small and culled; the far ones are whole in view, so
  `rsp_hull_check` keeps them on the RSP. 738 splits → 0.
- **The river in pieces** (`river_<a>_<b>`): halves of ring ranges while the camera
  would clip one, so the far reach is one piece wholly in view.
- **One ring per ripple.** `over_surface`: a ripple's ring decal is drawn by the piece
  whose triangles lie under it.
- **Lighter props:** the windmill wall in two storeys; the coin ring at 10 segments.
- The white-light stopgap of the issue was not needed: Phase 2 landed first.

## 4. Phase 3: the water overlay

Per vertex, on the vector unit, what `render3d/lit.rs`' `WATER` path does on the CPU:

- **The swell:** both waves' phases as two more columns of the vertex product,
  `water::sin_turns`' polynomial on four lanes (sin and cos of each), height and half
  gradient.
- **Displacement:** clip position, fog depth and world y gain `h · UP` in the
  accumulator.
- **The ripples:** height and gradient computed by the CPU per vertex
  (`rsp::ripple_delta`), DMA'd with each segment (`RIP_IN`); on the RSP since §7.
- **Shading:** the normal and the view direction normalized by Newton steps, Fresnel
  (alpha toward opaque too), the glint `((c + s·n)/2)² / (2 (1 + t·s))` through `vrcp`
  raised by squaring, light from the world normal, sky color, fog; then S, T, W, 1/W as
  the textured overlay.
- **Eligibility** (`rsp_water_check`): the sphere and, if needed, every vertex raised and
  lowered by `1.45 · amplitude + ripple reach + 0.01`; the split rule with twice that
  slack.
- **Memory:** the water block and the ripple deltas live in a three-frame uncached
  arena (3 × 16 KB). The ring decals stay CPU triangles in the water bucket, after the
  surfaces (RSP triangles since §7).
- **Oracle:** the self-test's sheet under a warm light with fog, a swell and a glint,
  then with a ripple: xy ≤ 2/4 px, z ≤ 1/32767, color ≤ 3/255, S/T ≤ 1/32 texel. A
  CPU-vs-RSP pixel check covers the same draw.

### Traps (all hit)

- `RDPQ_Triangle_Send_Async` keeps only the fraction of `minW · (1/W)`. `vrcp` runs up
  to 0.2 % high, so the product passed 1.0 and every pixel sampled texel (0, 0). W is
  stored 2^-7 smaller.
- `VADD` adds the VCO carry a previous `VADDC` left: clear it first.
- `w + w/2` saturates s0.15 in a Newton step: subtract first. The view direction's
  depth needs three steps.
- LLVM hoisted float math out of an `Option<f32>` payload; the VR4300 trapped on the
  uninitialized denormal. Fog is `RspFog { on, .. }`, plain fields.

## 5. Measurements (ares, 8 paddings)

| Scene | Before | After | RSP draws |
|---|---|---|---|
| `micro-river` frame | 19.6 (19.1–20.5) | **10.5 (10.1–11.0)** | 0/1 → 3/6 |
| `micro-river` T&L stage | 11.2 | 6.4 | |
| `showcase-river` frame | 286.6 (280.8–291.4) | **75.2 (73.8–76.2)** | 0/46 → 85/113 |
| `showcase-river` T&L / sort / submit / game | 154.2 / 31.2 / 87.0 / 11.8 | 25.9 / 0.7 / 31.0 / 14.9 | |
| `showcase-river` splits | 738 | 0 | |
| hub | 11.1 (10.8–11.6) | 11.4 (10.9–11.8) | 25/26 |
| green-hills | 6.8 | 6.7 | |
| lava-land | 7.1 | 7.4 | |
| sky-tower | 6.6 | 6.8 | |
| throne | 17.0 (16.4–17.6) | 17.8 (17.2–18.4) | 22/28 |
| `micro-flat-rsp` | 37.3 (on the CPU) | 10.9 | 0/1 → 1/1 |

- castle64's work counters and its N64 golden are unchanged. Its frames moved within
  ±0.8 ms, inside the paddings' spread for every scene but throne, whose range moved
  up by ~0.7 ms (more code in the draw path ahead of the overlay choice).
- `micro-river` per draw (CPU time charged to the model): the near piece with its two
  ring decals 3.4 ms, the far piece 1.2 ms, a small piece 0.8 ms, the piece under the
  camera (clipped, CPU) 0.9 ms.
- `scene budget --platform n64` for the showcase: before 46 draws, all on the CPU
  (34 colored light, 9 textured mesh, 3 skinned), estimate 209.9 ms; after 102 draws,
  74 on the RSP, estimate 25.4 ms. The estimate is low against ares' 75 ms: the cost
  model's RSP term is still ADR 0012's 90 µs per draw (§6).

## 6. What was open after #77

The issue's budgets were **not met** then: the river was ~6 ms of CPU per frame (1.2 ms
asked), the showcase 75 ms (33.3 ms asked). §7 is how the second part closed them.

1. **Ripples and rings on the overlay.** The ripple deltas cost a `ripple_sample` per
   vertex on the VR4300, and each ring is a CPU-tessellated decal: most of the near
   piece's 3.4 ms. The overlay already has `vrsq`; the ripple sum per vertex and the
   ring as triangles of the same command are the next step.
2. **RSP throughput.** `submit` is 31 ms for 4 231 RSP triangles: the showcase is RSP
   bound once its CPU work is gone. Fewer triangles (LOD per platform, BACKLOG P2 #17)
   or a leaner triangle path.
3. **Per-draw CPU.** 113 draws at ~0.2 ms of eligibility, command and memo work each
   (AGENTS pitfall). Static draws of one material could share a command.
4. **Phase 4.** An RSP term in `N64_COST` priced per triangle and per surface, and a
   measurement on a real N64 (ares doesn't time the RDP; 2-cycle translucent water fills
   at half rate).

## 7. Part 2: the river at 1.2 ms and the showcase at 30 fps

### The water overlay

- **Ripples on the RSP.** The block carries up to four ripples (x, z, 128 · radius,
  strength · response · decay, and each band's d² bounds). Per vertex the overlay gets
  `d` from `vrsq` (R = 128 / d in s15.16), `q = (d − r) / W`, `e = max(0, 1 − q²)`, the
  ring's phase through `sin_turns`, and adds `h = s e² cos` and its gradient to the
  swell's. A vertex pair outside every band skips the sum. `rsp::ripple_delta` and the
  per-segment deltas are gone.
- **Rings on the RSP.** A ring is a command of the same overlay over `rsp::ring_segment`s
  (22 world-space vertices, 20 triangles each, written by the CPU into the arena); the
  block's ring flag keeps the vertex color, fogged, without light, Fresnel or glint.
  `ring_on_rsp` checks each ring's sphere against the band; a ring that fails it stays a
  CPU decal (`tessellate_ripples_except`). `perf_budget` and the PC's estimate count
  them the same way (`rsp_water_rings`).
- **Two vertices per pass,** A in lanes 0..3, B in 4..7, like the colored overlay: the
  world x, z come from a second product (the block's columns, both halves), the sums
  use `.h` broadcasts, the ripples fill all eight lanes. `water_sin` and
  `water_newton` are subroutines (three and five calls a pass) to fit IMEM: the overlay
  is 4 028 of 4 096 bytes, so the test triangle and its vertices are left out of it.
- **The block in two parts.** The static part (0x180 bytes: world columns, UP, the swell
  terms, a/b, the view's rotation, sun, sky, glint) is built once per memoized draw and
  stays in RDRAM with it (`WaterCmd::block`, written back once; a replaced command waits
  four frames in `water_grave` before it is freed). The frame's part (0xC0 bytes: the
  swell's phases, the ripples, their count) is written once a frame per material
  (`WaterNow::publish`) and shared by every draw and ring; the overlay DMAs both
  (`DRAW` +0xB0 and +0xB4).

### The CPU side

- **Streaming that starts the RSP.** `set_rsp_stream` queued commands but never called
  `rspq_flush`: an idle RSP sleeps until the frame's end, so the streamed river was
  drawn after the CPU was done (0.8 ms of drain). `trino_rsp_flush` after each streamed
  chunk and water command: 0.35 ms. The river's surface goes out before its rings are
  built. The showcase's perf scenes run with streaming on, as the football template's
  host does.
- **The water texture stays in TMEM** (`trino_3d_water`): the same layers and flags only
  move the detail tile's origin (`SET_TILE_SIZE` with the upload's own `translate · 4`),
  instead of an upload every frame (~0.25 ms).
- **Arena lines claimed without a fill** (`trino_cache_create`, Create Dirty Exclusive):
  every byte of an arena allocation is written before the RSP reads it, so a miss needn't
  read RDRAM first (~80 µs a frame of the river's writes).
- `rings_of` takes the surface's rest height (`water_plane`, kept in the command), so a
  memo hit builds no `Mesh`; key comparisons are `memo::words_eq` (`==` on arrays is
  newlib's byte-wise `memcmp` on the N64).
- **The water memo drops what the last two frames didn't draw** when full. Clearing it
  whole, mid-frame, rebuilt the river every frame after a scene that had filled it.

### Measurements (ares, mean (min–max) over the 8 paddings)

| Scene | After #77 | Now |
|---|---|---|
| `micro-river` frame | 10.48 (10.14–10.97) | **1.78 (1.73–1.82)** |
| `micro-river` T&L (the river's draws, the gate) | 6.39 (5.95–6.72) | **0.94 (0.88–0.98)** |
| `showcase-river` frame | 75.17 (73.81–76.17) | **26.21 (25.89–26.74)** |
| `showcase-river` T&L | 25.92 | 11.91 |
| hub | 11.37 (10.93–11.78) | 12.09 (11.80–12.31) |
| green-hills | 6.75 | 7.22 |
| lava-land | 7.38 | 8.19 |
| sky-tower | 6.77 | 7.03 |
| throne | 17.85 (17.16–18.44) | 18.67 (18.19–19.31) |

- **The river's RSP time** (each command drained in turn, less a drain's own 45 µs):
  the three surfaces 0.6 ms (48 vertices, 52 triangles: ~8.8 µs a surface vertex, ~2.6 µs
  a triangle, ~40 µs a command), the two rings 0.28 ms (~3 µs a vertex). The RSP runs
  beside the CPU's 0.94 ms; the frame's 1.78 ms is those plus the empty frame's 0.31 ms
  (begin, end, camera, light) and the scene's draw calls.
- **Gates:** `perf::BUDGETS` caps the blessed ceilings at the issue's numbers:
  `micro-river` `tnl_us` 1 200 and `showcase-river` `frame_us` 33 300.
- **castle64** moved +0.3…+0.8 ms with its work counters and its hub golden unchanged.
  It is code layout: the baseline with a six-line counter patch outside the draw path
  measures hub 12.15, lava-land 7.93 and `micro-draws-rsp` 9.16 ms, the same as this
  tree (12.09, 8.19, 9.62), and its memo hits and misses are identical.
- **In ares the RSP costs about 4–5 cycles per instruction, dependent or not.** Pairing
  the water vertices cut ~20% (fewer instructions per vertex), not the 2× a stall model
  predicts; count instructions, not dependency chains.
- **Goldens:** `showcase.png` (3 127 px: the N64's light river, as after #77's
  variants) and `showcase_river.png` re-blessed; `castle64_hub.png` byte-identical. The
  water oracle keeps xy ≤ 2/4 px, z ≤ 1/32767, color ≤ 3/255, S/T ≤ 1/32 texel.

### Phase 4: the cost model

`N64_COST` prices a water surface on the RSP apart: `us_per_rsp_water_tri` 9.5 µs of RSP
and `us_per_rsp_water_draw` 150 µs of CPU (`RspWork` carries the water draws and
triangles; `scene budget`, the editor's budget and the PC's strict estimate use it).
`scene budget` also adds the RSP draws' CPU side, which it had left out: the showcase's
estimate is 25.8 ms against ares' 26.2 (it was 25.4 against 75).

## 8. Still open: a real N64

ares doesn't time the RDP. The river is translucent, 2-cycle and covers much of the
screen, and 2-cycle fills run at half rate on hardware. `run n64 --device sc64|everdrive`
(`docs/HARDWARE.md`) prints the same `TRINO_PERF_SCENE` lines; nobody has run it on a
cart yet, so the 1.2 ms and 33.3 ms are emulator numbers (BACKLOG P0 #1a).
