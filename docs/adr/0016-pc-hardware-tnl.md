# ADR 0016: PC hardware T&L — the decision, instancing, and reporting first

Status: draft (October 2026, GitHub #51). Phase 1 of the issue's roadmap is
implemented; Phases 2–6 (the WGSL vertex path, resident buffers, skeletons, the
instance ring, the parallel CPU fallback, the evidence) are not.

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
  yet: `Skinned` and `Blended` (joint matrices and `SKW2` in WGSL are Phase 3),
  `Empty`, `Water` (the core displaces it, ADR 0015), `Translucent` (tint or
  vertex alpha < 255: the CPU path's per-triangle painter order is the blending
  order), `Offscreen` (exactly the CPU path's sphere cull, tested against it) and
  `PerPixelLit` (a local light, gloss or wetness: the lit pipeline's surface
  varyings come from `tessellate_surface`; reported by the renderer, which knows
  the lights, and by the budget for glossy draws).
- The rule is a separate function from `rsp_check`/`gpu_check` and is never
  called on the consoles, so the N64's hot path and I-cache don't see it.

### 2. Phase 1 decides and reports; the CPU path still draws

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

### 5. What Phases 2–6 must keep

- `SimProfile::N64`/`N3ds` keep the CPU path; `n64_parity`/`n3ds_parity` hold.
- The CPU path stays the oracle: Phase 2's GPU frames are compared with it on
  every PC golden scene (≤ 1/255 mean, ≤ 0.5% of pixels > 8/255).
- A new `PcGpuReject` joins `ALL` (a const assert enforces it), gets a `label`
  and a `key`, and the budget learns to predict it.
- The Web backend (#47) reuses `pc_gpu_check` and the WGSL path; its tier is the
  same rule with its own reasons added if WebGL2 lacks something.

## Consequences

- The PC native profile pays one rule evaluation per model draw (a view·model
  product for the sphere test) for numbers it doesn't use yet; the console
  profiles pay nothing.
- `TnlStats` is three words larger on every target. The N64 sums it per draw;
  the N64 frame was measured over code layouts before and after (see the PR).
- Games get instancing on every target today, at separate-draw cost; the speed
  comes with Phase 3 on the PC only.
