# ADR 0042: Dynamic meshes — geometry the game rewrites every frame, drawn as a model

Status: accepted (November 2026). Implemented in `trino-core` (`dynamic_mesh`: `DynVertex`,
`DynamicMesh::{new, textured, check, bounds, tmdl_len, write_tmdl}`, `DynamicMeshError`,
`smooth_normals`, `MAX_DYNAMIC_VERTICES`, `MAX_DYNAMIC_INDICES`; `Renderer::draw_mesh` with a
default that draws nothing; `Caps::max_dynamic_vertices`; `FrameStats::{dynamic_meshes,
dynamic_rejected}`), `trino-render-wgpu` (`PcRenderer::draw_mesh`, its slots,
`GpuTnl::write_mesh`, `PcRenderer::mesh_buffers_created`, `MAX_DYNAMIC_DRAWS`) and
castle64's self-test (`dynamic mesh`).

## Context

A game whose surfaces follow its simulation had only baked models to draw them with. Bongo
Kart draws the water a wave pushes up a road, the wet asphalt it leaves and the floor that
opens under a moon road as many model pieces, each placed by its own matrix:

- a sheet of water in four strips along a curved, banked road shows a line where two strips
  meet (each follows its own plane of the road), and the wet asphalt dries in bands;
- the moon road's floor is thousands of instances of one unit triangle, one matrix each, with
  one flat color per triangle.

What they need is one mesh whose vertices the game writes every frame: continuous (vertices
shared between neighbouring strips, so no seam), with a color and an alpha per vertex (a
gradient that dries, foam at the front), drawn like a model (the lighting, fog,
translucency and water every model gets).

## Decisions

### 1. One immediate call over borrowed data

`Renderer::draw_mesh(&DynamicMesh, &Mat34, &ModelParams)`. A `DynamicMesh` borrows the
game's vertices (`DynVertex`: position, normal, color with alpha, UV) and `u16` indices for
the call; the game may rewrite its buffers right after. The parameters are a model's
(`ModelParams::anim` aside: a dynamic mesh has no skeleton).

- **Not a model the game re-uploads.** A buffer per model id would allow one content per
  frame (the GPU reads it when the frame is submitted), and the CPU path's jobs read the
  blob at the batch flush: the same id drawn twice in a frame with different vertices would
  draw the last one twice. An immediate call can't get that wrong.
- **Normals are the game's.** `smooth_normals` computes area-weighted vertex normals from
  the triangles (shared vertices, one normal: no seam in the light) for games that don't
  have better ones.
- **A default method that draws nothing.** No backend has to implement it, and no ABI
  changes (a new `Renderer` method with a default doesn't bump `GAME_API_VERSION`).

### 2. The model path, through a TMDL blob

`DynamicMesh::write_tmdl` writes the mesh as a TMDL blob (header, positions, normals,
colors, indices, `BNDS` from the vertices as the model bake computes it, and `TXUV`/`TXID`
when it has UVs), into a buffer the backend owns (the core has no allocator).
`Mesh::from_tmdl` reads it back as the mesh a baked model with these vertices is. The PC
renderer then draws it with `draw_model_matrix`, so a dynamic mesh is a model for every
path and pass: the GPU T&L and the CPU T&L (both, `TRINO_PC_TNL=cpu`), translucent draws
sorted with the batch (ADR 0016 §13), the lit per-pixel pipelines, water, shadows, fog, the
draw report and the stereo recording. Nothing in the T&L knows dynamic meshes exist.

- **Slots.** The k-th `draw_mesh` of a frame takes slot k: a model key of its own
  (`ModelId::from_path("trino/dynamic-mesh/k")`) whose blob in the renderer's mesh table is
  rewritten in place (the `Vec` keeps its capacity) and whose GPU stream is rewritten in its
  buffer (`GpuTnl::write_mesh`: reused while it fits, else replaced by one of the next power
  of two). Each draw of a frame has its own slot, so a frame may draw any number of
  different dynamic meshes (up to `MAX_DYNAMIC_DRAWS`, 256), and nothing allocates once a
  slot has held a mesh that big (`mesh_buffers_created` stays put).
- **The GPU copy never lags.** The slot's GPU stream is rewritten with its blob while the
  GPU path is on, and dropped while it is off, so a later GPU draw re-uploads it rather than
  draw an older frame's.

### 3. Guards

`DynamicMesh::check` refuses a mesh before a byte is written: more vertices than the target
takes, more than 3 × 65 536 indices, a broken triangle, an index past the vertices, a NaN or
infinite position, normal or (read) UV. The renderer also refuses a non-finite matrix. A
refused draw draws nothing and is counted (`FrameStats::dynamic_rejected`); an empty mesh is
nothing to draw, not an error.

### 4. Tiers: the PC and the Web now, the consoles later

`Caps::max_dynamic_vertices` is 65 536 on the PC and the Web and 0 on the consoles (both
tiers), where the trait's default draws nothing. The PC's console profiles take the
consoles' caps: they draw none either (counted; strict mode panics with the reason), so
`n64_parity`/`n3ds_parity` and the editor's Game view keep showing what the console shows.

The consoles can draw them: a per-frame mesh is what their games did for water. The way in
is the CPU T&L the console profiles already run, with the blob in a buffer that lives until
the batch is drawn (the N64's RSP reads meshes by DMA after the call returns, and the 3DS
GPU path reads vertex buffers later too). That needs the N64 code-size and heap checks of a
T&L change, so it is a follow-up (BACKLOG).

## Consequences

- **The same pixels as a baked model** with the same vertices, on both PC T&L paths, byte
  for byte (`crates/platform-pc/tests/dynamic_mesh.rs::
  a_dynamic_mesh_draws_the_pixels_of_the_baked_model`: lit, flat, tinted, translucent,
  textured and water); the GPU path within the oracle's tolerance of the CPU path.
- **Cost** (`dynamic_mesh_cost`, release, a 64 × 64 sheet: 4 225 vertices, 8 192
  triangles, rewritten every frame; a Ryzen 7 5825U with its integrated Radeon, other
  builds running): the call is about 0.14 ms on the CPU path (write and read back the blob:
  0.04 + 0.06 ms) and 0.5 ms on the GPU path (plus the 24 576-vertex GPU stream and its
  upload); the same surface as one
  instance of a triangle per triangle costs 0.6 ms on the GPU path and 1.6 ms on the CPU
  path, and has no gradient.
- The blob is written and read back every call: cheap next to the T&L of the same
  triangles, and the price of having one draw path.
