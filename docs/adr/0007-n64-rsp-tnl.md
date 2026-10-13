# ADR 0007 — N64: vertex T&L on the RSP

- Status: **accepted**, and implemented (§7, results). §1–§6 were written as a
  proposal after the Phase 4 desk spike of `docs/plans/n64-throughput.md`; §7 records
  what was built and measured, and where it departed from the proposal.
- Date: 2026-10-13
- Context: Phases 1–3 of the throughput plan took castle64 from hub 76.1 / green-hills
  37.3 / lava-land 49.1 / sky-tower 31.2 / throne 43.4 ms to 39.4 / 19.8 / 21.7 / 18.9 /
  16.0 ms of emulated N64 time per frame, with every golden identical or reviewed. The
  four stages are now at 46–62 fps. The hub, at 25 fps, is the only scene still over a
  30 fps frame (33.3 ms).

## 1. Where the hub's 39.4 ms go

From `cargo xtask test n64` (`TRINO_PERF_SCENE:hub`, after 3d):

| Stage | ms | What it is |
|---|---|---|
| T&L | 24.7 | `tessellate` on the VR4300: 1 229 vertex transforms, 605 source triangles, 26 draws |
| submit | 8.0 | `rdpq_triangle`: 26 uncached rspq words per emitted triangle (310 triangles) |
| sort | 2.3 | painter keys |
| game, sprites, update | ~4 | castle64 render code, the HUD, physics |

- The micro scenes put a Gouraud vertex at ~7 µs, a front-facing emitted triangle at
  ~22 µs, a visible draw's setup at ~80 µs and submit at ~22 µs per triangle. Half of
  submit is `rdpq_triangle`'s uncached stores; the conversion work overlaps them (1f).
- CPU-only levers left (each measured or priced in the plan):
  - merging the hub's blocks (2b): ~1–2 ms;
  - simplifying the door frames and the player: a few ms, and it costs art.

  Together they don't reach 33.3 ms with margin. T&L plus submit (32.7 ms) is the gap.

## 2. What the pinned libdragon offers (trunk `2c3d5db`)

- **RSP overlays are supported.** `rspq_overlay_register` /
  `rspq_overlay_register_static` in `rspq.h`; `n64.mk` builds `.S` RSP sources with
  `-Wl,-Trsp.ld`; `rsp.inc`, `rsp_queue.inc`, `rsp_dma.inc` and `rsp_rdpq.inc` are in
  the image's include dir.
- **`rsp_rdpq_tri.inc` (803 lines) is the triangle setup an overlay needs.** It
  assembles an RDP triangle command from three DMEM vertices. Its vertex layout is
  overridable (`RDPQ_TRIANGLE_CUSTOM_VTX` plus `VTX_ATTR_*` offsets), and it already
  carries per-vertex clip and trivial-reject flags. This is how libdragon's own 3D
  pipelines are built.
- There is no OpenGL and no tiny3d on trunk.
- **The `preview` branch**, which tiny3d requires, has diverged from the pin: 4 609
  commits ahead and **272 behind** (GitHub compare, 2026-10-13). Moving there means
  losing or re-porting trunk commits, then re-verifying rdpq, display, audio, the
  shim's ABI (ADR 0002) and every N64 gate.

## 3. Decision (proposed)

Write a **small Trino overlay on the current pin** rather than moving to preview for
tiny3d:

1. **Vertex format.** The RSP has no float unit, so the overlay needs fixed-point
   vertices. Add a TMDL section with s16 positions (plus a per-mesh scale) and s8
   normals, baked next to the f32 data (TMDL grows by sections, as the AGENTS pitfall
   says). Meshes without it, and PC/3DS, keep the f32 path.
2. **Per draw**, the CPU writes one command: the model-view matrix and the
   model-space light (both already computed: `TnlContext`, 3c), the tint and emissive
   factors, and the RDRAM addresses of the vertex and index data. Today the CPU writes
   ~26 words per *triangle*; this is a fixed handful per *draw*.
3. **On the RSP**, the overlay DMAs vertices in batches of 32 (the size of the CPU
   vertex cache), then transforms, lights (white light, Gouraud), projects and sets
   clip flags. It walks the index list, culls back faces and calls the
   `rsp_rdpq_tri.inc` setup per triangle.
4. **CPU fallback per draw** for anything the overlay doesn't cover yet:
   near/guard-band clipping (a triangle with clip flags goes back to the CPU path),
   fog, colored light, Flat/Unlit, textured draws, skinned meshes, depth splits. The
   CPU `tessellate` stays the reference, and PC and 3DS keep it.

**Rejected for now: preview + tiny3d.** It is a full 3D stack with its own formats and
conventions, so Trino's `draw_model` contract would sit on top of a second
pipeline. It also needs the libdragon move above, and that cost lands before any
frame gets faster.

## 4. What changes in the gates (to settle before code)

- **Work counters.** `castle64_work.toml` stays the PC truth. The ROM can't report
  `verts`/`emitted` for RSP draws unless the overlay counts them, so either the
  overlay writes counters back to RDRAM (preferred: the cross-target check survives
  for the CPU-fallback draws and the RSP's own counts), or the ROM check covers
  CPU-path draws only.
- **N64 golden.** Fixed-point T&L won't be pixel-identical to the f32 path. Re-bless
  `tests/golden/n64/showcase.png` once and keep `n64_parity` (PC vs ares, mean diff
  ≈ 2.5/255 today) as the tolerance check that PC and console still agree.
- **Determinism (ADR 0003).** It becomes "deterministic per target". The N64 RSP path
  is reproducible in ares, but it is not bit-equal to PC. Game logic never reads T&L
  output, so gameplay determinism is unaffected.
- **Perf gates.** `tnl` on the CPU shrinks to the fallback draws. The RSP's time
  shows up as `submit`/`end` waiting. Add an `rsp` stage so the scene table still
  says where the time goes: in a profiling build, the renderer waits on the queue
  (`rspq_wait`) after the batch and charges that wait to it.

## 5. Expected gain and how to measure it

- Estimate: the hub's T&L (24.7 ms) and most of submit (8.0 ms) leave the CPU, so the
  CPU frame drops toward ~8–10 ms (game, update, sort of what remains, command
  writes). The RSP then has to do ~1 200 vertices and ~600 triangle setups per frame.
  At a few hundred RSP cycles per vertex and per triangle (62.5 MHz), that is several
  ms, well under a frame. **Unverified**: the first deliverable is a measured hub
  frame.
- Spike acceptance, next session: an overlay that draws one static Gouraud mesh
  (castle64's door frame) through `rspq`, matching the CPU path within `n64_parity`
  tolerance, timed in the micro scenes (`micro-gouraud` on the RSP vs the CPU).

## 6. Consequences

- New: an `.S` overlay in `crates/platform-n64/shim/`, a TMDL section, a shim entry
  that stays within the FFI rules (≤ 4 scalar/pointer arguments: the command is
  built in a struct passed by pointer), an RSP self-test step in `castle64::selftest`.
- The N64 renderer keeps `trino_core::tessellate` for every draw the overlay
  declines, so features never regress; coverage grows draw type by draw type.

## 7. As built (2026-10-13, branch `n64-rsp-tnl`)

- **Overlay** `crates/platform-n64/shim/rsp_trino.S`, built by xtask like `n64.mk`
  builds ucode. It takes 2.4 KB of IMEM and 3.4 KB of DMEM with the rspq kernel.
  - `TrinoCmd_Draw` takes a 27-word command (§3.2).
  - Segments of ≤ 32 vertices: s15.16 positions with a 32×32 multiply, s0.15 normals,
    u8 indices. A position format with a per-mesh shift was dropped because it
    overflowed for distant objects.
  - Lighting is the model-space white light of throughput item 3c; Unlit is ambient 1.
  - Triangles go through `RDPQ_Triangle_Send_Async`.
- **Eligibility**: `trino_core::render3d::{rsp_mesh_supported, rsp_eligible,
  rsp_split}`, in the core so the PC predicts the console's split. A draw qualifies
  when it needs nothing the overlay lacks, and its bounding sphere is inside the guard
  band and past NEAR. So instead of a per-triangle CPU fallback (§3.4), whole draws
  that could clip stay on the CPU.
- **Gates (§4, as decided)**:
  - work counters gain `rsp_draws`/`rsp_tris`, and ROM = PC stays exact;
  - the showcase golden is unchanged (fogged, so it takes the CPU path);
  - a second N64 golden, `castle64_hub.png`, covers the RSP path;
  - an oracle, a pixel-parity check and a hub CPU-vs-RSP check run in the self-test;
  - measured frames drain the queue, so RSP time is counted.
- **Measured traps** (the `n64-performance` skill has them): the reciprocal's 1/2w,
  z from 1/depth with CPU-matching rounding, a single staging buffer, the cull mode,
  submitting RSP draws after the batch's CPU triangles, and SYNC_PIPE before mode
  changes.
- **Results**, same ROM, emulated time, frames drained:

  | Scene | CPU T&L (ms) | RSP T&L (ms) |
  |---|---|---|
  | hub | 38.0 | **13.1** |
  | green-hills | 19.4 | **7.3** |
  | lava-land | 21.1 | **7.3** |
  | sky-tower | 18.5 | **7.2** |
  | throne | 15.4 | **8.4** |

  Hub vblanks: 11 → 4 over the 4 measured frames, i.e. vsync-bound at 60 fps.
- **Not modeled**: ares doesn't time the RDP's fill, so hardware may be RDP-bound.
  `N64_COST`, the HUD and strict mode still describe CPU T&L only.
- **Determinism (§4)**: as decided. The PC is still bit-exact against itself. The N64
  RSP path is reproducible in ares and within 1/4 px, 1/32767 z and 1/255 color of the
  CPU path. Gameplay never reads T&L output.
