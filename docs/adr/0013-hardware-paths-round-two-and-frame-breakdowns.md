# ADR 0013: Fog on the PICA, why draws stay on the CPU, the PC's first frame breakdown, and an N64 change held back

Status: accepted (October 2026). Part of GitHub #31 (the hybrid hardware pipeline),
following [ADR 0012](0012-console-hardware-paths-and-honest-n64-timing.md). The handoff
on #31 asked for measurement first, then the cheap coverage gains; this round did both
on the 3DS and measured the PC. The N64 part was written and then held back (§4).

## Context

- **3DS:** with fog on, no draw took the GPU path: `ctx.fog_on` was `gpu_check`'s first
  per-draw rule. The showcase's top screen is fogged by default, so it ran all-CPU. The
  per-draw `Textured` rule was redundant: a mesh that reaches `gpu_check` has no UVs.
  Nothing reported *why* a draw stayed on the CPU, and the scene budget counted every 3DS
  draw as CPU work.
- **PC:** no frame breakdown had ever been measured, so the PC's GPU vertex path had no
  target to size against.
- **N64:** a colored or dimmed light sends every draw to the CPU (`ColoredLight`), and so
  do emissive and double-sided draws.

## Decisions

### 1. Fog in the 3DS vertex shader (`tnl3d.v.pica`)

- `GpuDraw` carries a fog row and a fog color. The row is `(view depth − near) /
  (far − near)` as a row on the model-space point: the shader needs one `dp4`, a clamp
  and a lerp. Without fog the row is zero. A draw is 44 floats of uniforms (was 36).
- Not the PICA fog LUT: it is indexed by the depth band, where a 16–42 m fog spans about
  0.0007 of the range.
- The GPU interpolates the fog perspective-correctly; the CPU path approximates it with
  depth splits, so the two differ slightly. The `gpu tnl` self-step compares two frames,
  castle64's hub and the showcase's fogged first frame (2 of 960 samples differ by more
  than 24, mean 0.32). The showcase's first frame went from 0 to 12 of 37 draws on the
  GPU (1 733 triangles); the rest are flat-shaded (15), textured (7) or skinned (3).
- The redundant `Textured` rule is gone (identical output).

### 2. Why a draw stays on the CPU, on the 3DS

- `N3dsRenderer` keeps a per-reason histogram of `GpuReject` per frame
  (`gpu_rejects`); `TRINO_FRAME` prints `gpu_tris`, `cpu_draws` and
  `off_gpu=<reason>:<n>,…`.
- A mesh whose linear-memory copy fails is logged (`TRINO_WARN`) and counted as
  `GpuReject::NoGpuMemory`; it used to stay on the CPU silently.
- `trino_scene::budget` splits 3DS draws by `gpu_check` (`gpu_draws`, `gpu_tris`,
  `off_gpu`); the editor's HUD line and `cargo xtask scene budget` show it.

### 3. The PC's frame breakdown (`PcRenderer::frame_times`)

CPU time in T&L, in model batches (painter sort and expansion into GPU vertices) and in
submit (uploads, encoding, present), plus GPU draw calls, bytes uploaded and the draws lit
per pixel. Measured on an external project's busiest scene (230 model draws, ~9 900
triangles, an AMD iGPU): T&L ~2.3 ms, batches ~1.0 ms, submit ~0.8 ms, 12 draw calls,
**~2.7 MB uploaded per frame** (288 B per triangle). Of the T&L, about 64 % is skinned
rigs, 27 % static meshes and 9 % translucent draws (a throwaway per-category timer). A
GPU path for static meshes alone (the handoff's first PC step) removes roughly a third;
the rest needs GPU skinning too. That is the next round.

### 4. The N64's colored, emissive and double-sided draws: written, held back

- The code is on branch `perf/n64-rsp-colored`: `rsp_check` accepts those draws; they go
  through a second overlay (`rsp_trino.S` assembled with `TRINO_COLORED`: per-channel
  ambient and diffuse terms and an emissive add); white draws keep the old overlay
  **byte for byte** (same `.text`/`.data`) and the old `RspDraw`. The oracle and the
  CPU-vs-RSP pixel checks pass (color 1/255; 0.00 and 0.33 mean pixel difference), and
  the N64 goldens match.
- Held back because it changes nothing anyone draws today (no gated scene routes a single
  draw differently), while an external project's busiest N64 scene measured **+1.0 ms on
  average over 8 code layouts** (`TRINO_N64_TEXT_PAD` 0…8192): 25.2 (23.9–25.9) against
  24.2 (23.4–24.6) ms, 2 of the 8 inside the baseline range. With the white overlay
  identical and the hot path unchanged, the executed work differs by well under 0.1 ms;
  the shift is most likely the code layout *inside* the Rust code, which
  `TRINO_N64_TEXT_PAD` (one block move) does not sample. The C-only part of the change
  moved the same scene −0.5 and +1.6 ms at two paddings.
- What the way there measured, all in ares:
  - one per-channel lighting loop for every draw: every white draw's RSP time grew
    (three DMEM loads and an add per vertex), +1.1–1.5 ms per frame;
  - the colored loop as a second loop in the same overlay: rspq reloads an overlay's code
    on every switch to it, so the bigger code cost white frames too;
  - a 4-byte `drain` field in `StageTimes` (inside the renderer's profile) moved
    castle64's throne room by +0.38 ms over 4 layouts on its own.

## Consequences

- The 3DS draws fogged scenes on the GPU, and every 3DS frame says what stays on the CPU.
- The next PC round has its target: the CPU T&L, batch and upload work (~4 ms of a
  ~5.5 ms frame at the busiest measured scene), mostly skinned rigs.
- **An N64 measurement limit:** a change that adds functions to the Rust code can move N64
  frames by 1–2 ms through layout alone, beyond what `TRINO_N64_TEXT_PAD` samples. The
  held-back branch's next step is a control: an inert, linked function of similar size on
  the baseline, measured over the same 8 paddings, to know how wide that axis is before
  judging the feature.
