# ADR 0016: PC hardware T&L

Status: accepted (October 2026, GitHub #51). Phase 1 (the rule, instancing,
reporting) landed first; Phases 2–6 (the WGSL vertex path and resident buffers,
skeletons and instanced draws, the parallel CPU fallback, the tooling, the
measurements) landed together and are §6–§11.

## Context

Every 3D draw on the PC native profile runs the CPU software T&L
(`trino_core::render3d`, ADR 0003) and uploads screen-space triangles every
frame. The N64 has an RSP path (ADR 0007) and the 3DS a PICA200 path (ADR 0012),
each picked per draw by a rule in the core (`rsp_check`, `gpu_check`) that also
names why a draw stays on the CPU. Issue #51 asks for the same on the PC: a WGSL
vertex stage for the draws it can take, resident mesh buffers, instancing, and a
parallel CPU fallback. It is split in phases so each lands with its own
evidence; this ADR records the first one and the shape the rest must keep.

## Decisions

### 1. The rule lives in the core: `pc_gpu_check`

- `render3d::pc_gpu_mesh_check(&Mesh)` (once per mesh) and
  `pc_gpu_check(&TnlContext, &Mesh, &Mat34, &ModelParams) -> Result<(), PcGpuReject>`
  (per draw), in `crates/core/src/render3d/pc.rs`, beside `rsp_check` and
  `gpu_check`. In the core so `scene budget` and the editor predict the split
  exactly, as they do for the consoles. `trino-core` stays `no_std` and
  dependency-free; no wgpu type appears.
- **Permissive:** a desktop GPU clips in hardware and lights in floats, so there
  is no near-plane, band, scale, colored-light, fog, texture, palette, flat or
  double-sided rule. `PcGpuReject` names only what the GPU path won't port, or not
  yet. Phase 1 had `Skinned` and `Blended`; Phases 2–3 ported both and replaced
  them (§7): the reasons are now `Empty`, `Water` (the core displaces it, ADR 0015), `Translucent` (tint or
  vertex alpha < 255: the CPU path's per-triangle painter order is the blending
  order), `Offscreen` (exactly the CPU path's sphere cull, tested against it) and
  `PerPixelLit` (a local light, gloss or wetness: the lit pipeline's surface
  varyings come from `tessellate_surface`; reported by the renderer, which knows
  the lights, and by the budget for glossy draws), `FlatSkinned`, `TextureAlpha`
  and `Disabled` (§7).
- The rule is a separate function from `rsp_check`/`gpu_check` and is never
  called on the consoles, so the N64's hot path and I-cache don't see it.

### 2. Phase 1 decided and reported; the CPU path still drew (superseded by §6)

- The PC native profile calls the rule on every model draw and counts the result;
  the draw itself still goes through `tessellate_with`/`tessellate_surface`. So
  every golden and every T&L work counter is unchanged, and the counters'
  meaning for Phase 2 is fixed now.
- `TnlStats` gains `pc_gpu_draws`, `pc_gpu_tris` (source triangles) and
  `instanced`. Unlike `rsp_draws`/`gpu_draws`, the PC GPU draws' work **is** still
  in `verts`/`emitted` during Phase 1 (documented on the field); Phase 2, which
  moves the work, makes them exclusive like the consoles' and says so here.
- The console simulation profiles (`SimProfile::N64`, `N3ds`) never call it:
  their counters stay zero and their RSP/PICA splits are what they report.
- `PcRenderer::pc_gpu_rejects()` is the last frame's per-reason histogram
  (`PcGpuReject::index`), like the 3DS's `gpu_rejects()`.

### 3. Reporting: `TRINO_FRAME` on the PC, the budget, the editor

- `trino_platform_pc::FrameMeter` records each frame and prints
  `TRINO_FRAME:<name> profile= avg_us= max_us= frames= fps= draws= tris=
  pc_gpu_draws= pc_gpu_tris= instanced= off_pc_gpu=reason:n,...` (per-frame
  averages, reasons most common first, rounded up so a reason seen at all shows).
  `apps/pc` and `trino-runner-pc` print it when a `TRINO_SMOKE_FRAMES` run ends.
- `trino_scene::budget::FrameBudget` gains `pc_gpu_draws`, `pc_gpu_tris` and
  `off_pc_gpu` for `Target::Pc`; `cargo xtask scene budget --json` writes them for
  every platform row (zero on the consoles), and the text output and the editor
  (the Game view HUD's platform lines, the Inspector's scene budget) show
  `budget::pc_gpu_line`: `PC GPU 42/46 draws · 2619 tris · 3 skinned mesh`.
  `xtask/tests/scene_budget.rs` holds the estimate equal to the renderer's
  counters and reasons.

### 4. Instancing: one portable call, a default loop

- `Renderer::draw_model_instances(model, &[Instance], &ModelParams)` and
  `draw_model_instances_with(model, &[Instance], &[Palette], &ModelParams)`.
  `Instance { matrix: Mat34, tint: Color, palette_index: u8 }`:
  - `tint` **multiplies** the call's tint per channel, alpha included, with the
    `(a * b + 127) / 255` rounding of material tint stacking (white keeps it).
    Multiplying rather than replacing keeps `Instance::new(m)` neutral, and a GPU
    path computes the same bytes on the CPU while filling the instance buffer.
  - `palette_index` 0 keeps the call's palette; `k` ≥ 1 takes `palettes[k - 1]`
    (past the table: the call's). A byte per copy, a table per call: what a GPU
    path uploads (one uniform table per batch). The palette alias `[Color; 4]` is
    16 bytes, too much per copy.
- **Default:** a loop over `draw_model_matrix` with `Instance::params`, as
  `Material::VertexLit`. Backends override `draw_model_instances_with` only.
  The N64 and the 3DS use the default (no backend code, no FFI, no change to
  their cost); the PC overrides it to count `instanced` and, from Phase 3, to
  issue one instanced draw. A default method doesn't change the game dylib ABI
  (`GAME_API_VERSION` unchanged, the rule in `crates/game-api/src/lib.rs`).
- **Equivalence is a test, not a hope:** the `instancing` step in
  `castle64::selftest` draws six slotted cards (each its own tint, two from the
  palette table, one index past it) as separate draws with the rules worked out
  by hand, then in one call, and requires identical pixels (48 grid probes and
  each card's center), identical `models`/`tris`/T&L counters (`instanced`
  aside: 0 or one per copy) and the first card's own tint. It runs on PC
  (`engine_checks`), in ares and in Azahar. Three mutations of the PC override
  (a copy dropped, the tint ignored, the table ignored) each fail it.

### 5. What Phases 2–6 had to keep (and did)

- `SimProfile::N64`/`N3ds` keep the CPU path; `n64_parity`/`n3ds_parity` hold.
- The CPU path stays the oracle: Phase 2's GPU frames are compared with it on
  every PC golden scene (≤ 1/255 mean, ≤ 0.5% of pixels > 8/255).
- A new `PcGpuReject` joins `ALL` (a const assert enforces it), gets a `label`
  and a `key`, and the budget learns to predict it.
- The Web backend (#47) reuses `pc_gpu_check` and the WGSL path; its tier is the
  same rule with its own reasons added if WebGL2 lacks something.

### 6. Phase 2: the WGSL vertex stage, resident meshes, the oracle

- `crates/platform-pc/src/renderer/shaders_tnl.wgsl` + `gpu_tnl.rs`. A mesh is
  uploaded once, at `upload_mesh`, as a vertex buffer of one vertex per triangle
  corner (`MeshVertex`, 56 bytes: position, normal, the triangle's face cross
  product for flat shading, color, UV, joint pair, slot, weight). Per draw the
  CPU writes one `GpuInstance` (128 bytes: model and model-view rows, tint,
  emissive, flags, detail scale, palette), a `MeshCtx` per camera/light (dynamic
  uniform offset, deduplicated against the previous one) and a joint block.
- The vertex stage repeats `tessellate_with` per vertex with the same operation
  order and truncations: model-view transform, Gouraud / Flat / Unlit with the
  white-light fast path, `palette_mul`'s `(c·e + 127) / 255`, tint × light
  truncated, emissive, then **fog per vertex** (`Setup::fog`). The fragment
  stage multiplies the texture and the detail layer like `fs_tri`.
- **Depth and clipping:** clip = `(x·2f/W, y·2f/H, z − NEAR, z)`, so the
  hardware's depth is the CPU path's `1 − NEAR/z` and the near plane is the CPU
  path's. Front faces are clockwise; double-sided draws use a no-cull pipeline.
- **Affine color:** the CPU path's untextured, unfogged triangles carry w = 1
  (affine color on screen). The GPU path emulates it as `(color·w)/w`, both
  interpolated perspective-correctly. `@interpolate(linear)` would be the direct
  way, but Mesa's llvmpipe/lavapipe (CI) drops clipped triangles that use it.
- **Fog per pixel was tried and dropped:** smoother on big floors, but the CPU
  path doesn't split triangles wholly outside the fog band, and on the showcase
  river the GPU frame missed the CPU frame by 2.70/255 with 23.6% of pixels
  over 8/255. Per vertex it is 0.25/255 and 0.002%.
- **The oracle is a test on every PC golden scene of the native profile:**
  `trino_platform_pc::tnl_oracle::compare` draws a frame on both paths with the
  same renderer, and the native goldens (`ao_*`, `lights_native*`,
  `texture_filter_native`, `water_native`, `showcase_river`,
  `solid_eleven_match_native`, `solid_eleven_teams_native`) and
  `crates/platform-pc/tests/gpu_tnl.rs` hold it to ≤ 1/255 mean and ≤ 0.5% of
  pixels over 8/255. The worst is the river, 0.252/255 and 0.002%. The
  goldens themselves are unchanged.
- **The counters are exclusive now,** like the consoles': a GPU draw counts
  `pc_gpu_draws`/`pc_gpu_tris` and nothing in `verts`/`emitted`; `FrameStats::tris`
  adds its source triangles. `scene budget`'s PC `cpu_draws`/`cpu_tnl` are the
  rejected draws only.

### 7. Phase 3: skeletons and instanced draws

- **Joints on the GPU:** a skinned draw's pose (`Mesh::pose`, the CPU path's
  own function, clip crossfades included) is uploaded as 32 × 3 rows; the vertex
  stage multiplies model-view × joint for the position and model × joint for the
  normal, and blends the second joint by its `SKW2` weight. `Skinned` and
  `Blended` are no longer reasons.
- **New reasons:** `FlatSkinned` (flat shading needs the posed face normal,
  which a vertex can't compute from its own data; rare, and only on PC),
  `TextureAlpha` (a texture with transparent texels needs the CPU path's
  triangle order to blend; found at upload, exactly for RGBA and BC1/BC3 and
  conservatively for BC7) and `Disabled` (`set_hw_tnl(false)`).
- **Instancing is automatic:** at each batch flush the GPU draws are sorted by
  (sidedness, texture, mesh, context, joints) and runs sharing all of them
  become one instanced call. `draw_model_instances` fills the instances
  directly; separate `draw_model` calls of the same model merge the same way, so
  castle64's blocks and Solid Eleven's body parts benefit without code changes.
  The instance stream is per frame, grow-only and written once
  (`queue.write_buffer`), not a mapped ring: wgpu stages the write, and the
  measurements show no upload cost worth a ring's complexity.
- **Order:** GPU draws are opaque and depth-tested, so they are recorded before
  the batch's CPU triangles; translucent and water triangles keep their painter
  order after them.

### 8. Phase 4: the parallel CPU fallback

- On the native profile the CPU draws of a batch are queued as jobs and run at
  the flush. With at least `PARALLEL_MIN_TRIS` (2048) source triangles they are
  split into contiguous chunks over `std::thread::scope` workers
  (`TRINO_PC_TNL_THREADS`, default the machine's parallelism up to 8;
  `set_tnl_threads`), each with its own scratch, and the chunks are
  concatenated in draw order. The batch's depth sort then sees the serial
  stream: `cpu_tnl::parallel_runs_emit_the_serial_stream` compares the
  `ScreenTri`s for 2, 3, 8 and 64 workers, and `gpu_tnl.rs` the pixels and
  counters of a whole frame for 1, 2 and 8.
- The console profiles keep the immediate, single-threaded path (the console
  cost model and the N64 split are theirs).

### 9. Phase 5: tooling

- `TRINO_PC_TNL=gpu|auto|cpu` (`cpu`, `0` or `off` keeps every draw on the CPU
  path) and `PcRenderer::set_hw_tnl` / `hw_tnl()`; `TRINO_POWER=low|high` picks
  the adapter on two-GPU machines for measurements.
- `FrameBudget::paths` gives each drawn entity its path per platform
  (`RenderPath::{Hardware, Cpu(reason), Impostor}`); `scene budget` prints the
  entities under each reason and writes `paths` in `--json`.
- The editor: the Model component's **Render path** row (PC / N64 / 3DS: GPU,
  RSP or CPU with the rule on hover and below), each reason in the Inspector's
  platform budget opens on the entities it keeps on the CPU (click one to
  select it), and the Game view HUD's platform lines open that section.

### 10. Phase 6: measurements

See the PR and `xtask/tests/pc_tnl_bench.rs` (castle64 hub, a Solid Eleven
match, 2000 instanced robots; 1080p and 4K; CPU vs GPU path):

Measured 2026-10-18 on one Windows machine (Ryzen 7000 series), release build,
Vulkan, through `TRINO_POWER=high` (discrete) and `low` (the CPU's iGPU):

Adapter: AMD Radeon RX 7800 XT (Vulkan), 4x MSAA, 240 measured frames per row (update + render + GPU)

| Scene | Size | T&L | Frame avg (ms) | p99 (ms) | CPU T&L (ms) | Draws | On GPU | Draw calls |
|---|---|---|---:|---:|---:|---:|---:|---:|
| castle64 hub | 1920x1080 | CPU | 0.79 | 0.96 | 0.03 | 26 | 0 | 6 |
| castle64 hub | 1920x1080 | GPU | 0.75 | 1.00 | 0.00 | 26 | 26 | 17 |
| castle64 hub | 3840x2160 | CPU | 2.08 | 2.25 | 0.03 | 26 | 0 | 6 |
| castle64 hub | 3840x2160 | GPU | 2.07 | 2.26 | 0.00 | 26 | 26 | 17 |
| Solid Eleven match | 1920x1080 | CPU | 1.78 | 2.44 | 0.67 | 234 | 0 | 10 |
| Solid Eleven match | 1920x1080 | GPU | 0.76 | 0.99 | 0.00 | 234 | 234 | 39 |
| Solid Eleven match | 3840x2160 | CPU | 3.34 | 3.78 | 0.73 | 234 | 0 | 10 |
| Solid Eleven match | 3840x2160 | GPU | 1.97 | 2.17 | 0.00 | 234 | 234 | 39 |
| 2000 robots (instanced) | 1920x1080 | CPU | 28.13 | 47.80 | 6.68 | 2001 | 0 | 1 |
| 2000 robots (instanced) | 1920x1080 | GPU | 0.46 | 0.64 | 0.00 | 2001 | 1990 | 2 |
| 2000 robots (instanced) | 3840x2160 | CPU | 25.16 | 46.50 | 6.24 | 2001 | 0 | 1 |
| 2000 robots (instanced) | 3840x2160 | GPU | 0.69 | 0.77 | 0.00 | 2001 | 1990 | 2 |

Adapter: AMD Radeon(TM) Graphics (Vulkan), 4x MSAA, 240 measured frames per row (update + render + GPU)

| Scene | Size | T&L | Frame avg (ms) | p99 (ms) | CPU T&L (ms) | Draws | On GPU | Draw calls |
|---|---|---|---:|---:|---:|---:|---:|---:|
| castle64 hub | 1920x1080 | CPU | 14.16 | 14.98 | 0.05 | 26 | 0 | 6 |
| castle64 hub | 1920x1080 | GPU | 13.91 | 14.64 | 0.00 | 26 | 26 | 17 |
| castle64 hub | 3840x2160 | CPU | 95.00 | 96.65 | 0.05 | 26 | 0 | 6 |
| castle64 hub | 3840x2160 | GPU | 92.89 | 97.81 | 0.01 | 26 | 26 | 17 |
| Solid Eleven match | 1920x1080 | CPU | 17.00 | 18.26 | 0.80 | 234 | 0 | 10 |
| Solid Eleven match | 1920x1080 | GPU | 15.87 | 16.97 | 0.00 | 234 | 234 | 39 |
| Solid Eleven match | 3840x2160 | CPU | 91.81 | 94.40 | 0.85 | 234 | 0 | 10 |
| Solid Eleven match | 3840x2160 | GPU | 91.30 | 93.10 | 0.00 | 234 | 234 | 39 |
| 2000 robots (instanced) | 1920x1080 | CPU | 40.50 | 63.16 | 7.37 | 2001 | 0 | 1 |
| 2000 robots (instanced) | 1920x1080 | GPU | 5.96 | 6.36 | 0.00 | 2001 | 1990 | 2 |
| 2000 robots (instanced) | 3840x2160 | CPU | 69.19 | 85.55 | 6.57 | 2001 | 0 | 1 |
| 2000 robots (instanced) | 3840x2160 | GPU | 15.23 | 15.68 | 0.00 | 2001 | 1990 | 2 |

- **Where the CPU path cost most, the GPU path wins most:** 2000 instanced
  robots fall from 28.1 to 0.46 ms on the discrete GPU (61×) and from 40.5 to
  6.0 ms on the iGPU; most of the CPU path's time there was not T&L (6.7 ms on
  8 workers) but sorting and uploading ~300 000 screen triangles a frame.
- **A real game:** the Solid Eleven match halves on the discrete GPU at 1080p
  (1.78 → 0.76 ms) and drops by 1 ms on the iGPU.
- **Fill-bound frames don't move:** castle64's hub (26 draws, 0.03 ms of T&L)
  and anything at 4K on the iGPU are bound by MSAA, SSAO and fill, the same on
  both paths. The GPU path issues more draw calls there (one instanced call per
  distinct mesh against a few big triangle runs) at no measurable cost.

### 11. Not done

- Scene v4 `instancing = false` per entity (optional in the issue): automatic
  instancing has no visible effect to opt out of.
- Translucent draws, water and per-pixel lit draws stay on the CPU path; a
  per-pixel lit GPU path would reuse the lit pipeline's fragment stage with
  GPU-computed surface varyings.

## Consequences

- The PC native profile draws most models with no per-frame vertex upload; its
  CPU T&L time falls to the rejected draws, and those run on several cores.
- The CPU path stays the reference: every native golden runs the oracle, and
  `TRINO_PC_TNL=cpu` brings the old path back for bisecting.
- The console profiles, the console goldens (ares, Azahar) and the N64 cost are
  untouched: the consoles never call the rule, and the core changed only in
  `pc.rs` (reasons) and documentation.
- `TnlStats` is three words larger on every target (Phase 1, measured then).
