# ADR 0003 — 3D via software T&L in core; backends only rasterize triangles

Status: accepted (Fase 7, 2026-07)

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
  no textured 3D (vertex colors only), no near-plane clipping (triangles
  touching the near plane are dropped), CPU-bound triangle throughput.
- Strict mode enforces `max_tris_per_frame` so PC development stays honest
  about console budgets.
- If a future phase needs more (textured 3D, z-buffered scenes), the
  hardware stacks (tiny3d/citro3d) can be introduced per-backend behind the
  same `draw_model` contract — this ADR does not preclude them; it defers
  them until the engine actually needs that throughput.
