# ADR 0018 — N64: the RSP takes fog, colored light, textures, rigid skeletons and water

- Status: **accepted**, implemented (GitHub #69 phases 0–3; phase 4 and the issue's two
  budgets are open, §6).
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
| `rsp_trino_water` | `+ TRINO_WATER` | §4 | a 0xE0-byte water block per draw |

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
  (`rsp::ripple_delta`), DMA'd with each segment (`RIP_IN`).
- **Shading:** the normal and the view direction normalized by Newton steps, Fresnel
  (alpha toward opaque too), the glint `((c + s·n)/2)² / (2 (1 + t·s))` through `vrcp`
  raised by squaring, light from the world normal, sky color, fog; then S, T, W, 1/W as
  the textured overlay.
- **Eligibility** (`rsp_water_check`): the sphere and, if needed, every vertex raised and
  lowered by `1.45 · amplitude + ripple reach + 0.01`; the split rule with twice that
  slack.
- **Memory:** the water block and the ripple deltas live in a three-frame uncached
  arena (3 × 16 KB). The ring decals stay CPU triangles in the water bucket, after the
  surfaces.
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

## 6. What is still open

The issue's budgets are **not met**: the river is ~6 ms of CPU per frame (1.2 ms
asked), the showcase 75 ms (33.3 ms asked).

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
