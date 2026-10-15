# ADR 0003 — 3D via software T&L in core; backends only rasterize triangles

Status: accepted (Fase 7, 2026-07). On the N64, eligible static draws now run the
same T&L contract on the RSP instead (ADR 0007, 2026-10-13); this CPU path stays the
reference and the fallback.

## Context

Fase 7 needs `draw_model` (vertex-lit 3D) on PC, N64 and 3DS. The original
roadmap sketched three separate hardware stacks: tiny3d on the N64 (requires
libdragon's `preview` branch — a different pinned toolchain image), citro3d on
the 3DS (requires compiling picasso shaders into the build) and a wgpu
matrix/lighting pipeline on PC. Three T&L implementations, three shading
models to keep visually in sync, and a toolchain destabilization on the N64.

Meanwhile the engine's design ceiling caps 3D scene complexity at N64-class
budgets (`Caps::N64.max_tris_per_frame` = 4000), and mid-90s consoles did
exactly this workload with CPU transforms.

## Decision

Transform & lighting is **engine code, not backend code**
(`trino_core::render3d`, pure `no_std` f32):

- model/view transform (`Mat34`), perspective projection, gouraud
  directional lighting, backface culling and painter's depth sort run on the
  CPU, identically on every target (the module ships its own `sin/cos/sqrt`
  approximations — no libm);
- backends receive **screen-space colored triangles** and only rasterize:
  `rdpq_triangle(TRIFMT_SHADE)` on N64, `C2D_DrawTriangle` on 3DS, a
  vertex-color wgpu pipeline on PC (interleaved with sprites in one command
  list, so draw order is preserved);
- models are baked from glTF masters into **TMDL**, a tiny portable format
  parsed zero-copy by `render3d::Mesh` — the same blob ships on all three
  platforms, no per-console model container.

## Consequences

- One shading model, deterministic across targets (goldens + console
  self-tests can compare like-for-like); no new toolchains, the pinned
  libdragon image stays on `trunk`.
- v1 limits: no z-buffer (per-mesh painter's sort — meshes are sorted
  internally, but interpenetrating meshes will not sort against each other),
  no textured 3D (vertex colors only), CPU-bound triangle throughput.
- Update (2026-07, castle64 stress test): triangles crossing the near plane
  are now clipped (Sutherland-Hodgman) against the near plane plus a 1.5x
  guard-band frustum instead of being dropped — large ground planes stay
  visible under the camera, and projected coordinates stay bounded, which
  the N64 RDP's fixed-point edge walker requires. One triangle fans into at
  most 6; backends size their scratch as `index_count / 3 * 6`.
- Update (2026-07, second pass): `draw_model` takes `&ModelParams` with a
  per-draw `tint` (multiplied into vertex colors before lighting — color
  variants of a mesh no longer need separate bakes), and backends batch the
  triangles of consecutive `draw_model` calls, depth-sorting the whole batch
  before rasterizing (painter across meshes; the batch flushes on sprite
  draws, camera changes and `end_frame`). Games no longer sort model draws;
  the remaining limit is per-triangle: interpenetrating geometry can still
  mis-sort (that fix is a z-buffer, still deferred).
- Update (2026-07, third pass — play-testing found floors painting over the
  player and doors vanishing behind wall quads): painter keys were triangle
  centroids, which misrepresent large triangles. `tessellate` now (a)
  bisects any edge spanning more than 3 units of view depth (the rule is
  per-edge, so neighbors split shared edges identically — no cracks) and
  (b) keys the sort on the triangle's **farthest** vertex, so a surface
  extending behind an object always draws before it. Output count no longer
  has a static bound, so `tessellate` emits through a callback and backends
  push straight into the batch buffer.
- Update (2026-07, fourth pass — painter alone could never resolve
  interpenetrating meshes, e.g. a cap box through a head box): occlusion is
  now **z-buffered** on all three targets. `ScreenTri` carries per-vertex
  normalized depth (`1 - NEAR/z`, hyperbolic); the PC writes it to a wgpu
  Depth32Float attachment (LessEqual; sprites test Always/no-write), the
  N64 to the RDP hardware z-buffer (`TRIFMT_ZBUF_SHADE`, zbuf surface
  attached and cleared per frame — the shim builds vertices from the
  trifmt's own offsets), and the 3DS passes citro2d's per-draw depth
  (one value per triangle, banded so background sprites < 3D < HUD). The
  depth-sorted batch remains as tie-break and blending order. This ADR's
  original "no z-buffer" limit is retired; "vertex colors only" stands.
- Update (2026-10, fifth pass — editor + real-project readiness): the T&L
  grew a per-draw **surface** (`ModelParams`: tint, emissive,
  `Shading::{Gouraud, Flat, Unlit}`, double-sided), a configurable **light**
  (`Renderer::set_light`: colored directional + ambient, distance **fog**),
  **matrix draws** (`draw_model_matrix`, for hierarchies; the trait default
  decomposes to `Transform3` so third-party backends keep working) and a
  **rigid skeleton** (TMDL `SKEL`/`ANIM` sections, ≤ 32 joints on the
  stack; ADR 0004). All of it is evaluated here, so the three backends only
  had to pass `&ModelParams` and the light through. TMDL became extensible
  through tagged sections (`BNDS` = bounding sphere for whole-mesh frustum
  culling; unknown tags skipped). A per-draw 32-entry vertex cache lights
  shared vertices once. Measured with `xtask/tests/bench_tnl.rs` (castle64
  bot playthrough through a CPU-only backend): 0.428 ms/frame vs 0.419 before
  the pass on the host — parity while adding the features; console gains are
  expected (fewer software normalizations) but unmeasured on hardware.
- Strict mode enforces `max_tris_per_frame` so PC development stays honest
  about console budgets.
- If a future phase needs more (textured 3D, z-buffered scenes), the
  hardware stacks (tiny3d/citro3d) can be introduced per-backend behind the
  same `draw_model` contract — this ADR does not preclude them; it defers
  them until the engine actually needs that throughput.

## Update (October 2026)

Textured 3D landed **within this contract**. The core T&L still produces screen-space
triangles, and they now carry `uv` + `w`. Each backend rasterizes them with one texture
per draw:
- PC: the w trick in the triangle shader;
- N64: rdpq TEX_SHADE;
- 3DS: a small citro3d path.

The "vertex colors only" limit is retired. Opt-in smooth skinning (`SKW2`) added
about 3% host CPU per draw in the worst case (`bench_tnl`).

Color slots (TMDL `SLOT`) and a per-draw `ModelParams::palette` (2026-10-15, ADR 0005 §4)
also stay within this contract. The T&L multiplies a slotted vertex color before tint
and lighting, and the backends see ordinary colored triangles.

Measured on the console clock: a full castle64 hub frame spends about **670 ms** of
emulated N64 time in render (CPU T&L + submit), against a 33 ms budget at 30 fps
(`TRINO_PERF` in `cargo xtask test n64`). CPU T&L throughput is now the top N64
risk. See `docs/BACKLOG.md` and ADR 0005.
