# ADR 0035 — Physics 2.0: a broadphase, shape casts, triggers, a character controller, ramps and mesh colliders, a fixed-point mode

- Status: accepted (2026-10-25)
- Issue: GitHub #66
- Supersedes: nothing. `trino_core::physics` keeps `Aabb`, `move_aabb`, `raycast` and
  `overlapping` unchanged; this adds to it.

## Context

`trino_core::physics` was the N64-era minimum: boxes, an axis-resolved mover
(`move_aabb`), a slab raycast and an overlap scan, every query a linear pass over a
slice. Each game rewrote the rest:

- castle64 had its own controller loop (gravity, substeps, moving platforms);
- slopes were boxes;
- Solid Eleven ran its own Q16.16 ball;
- a stadium or a Plan & Party venue paid O(n) per query;
- nothing gave triggers, shape casts, one-way platforms or steps.

## Decision

### 1. Broadphase: a static XZ grid in caller buffers (`StaticWorld`)

- **Shape.** Levels are wide and flat, so the grid is 2D (XZ) and a column of solids
  shares a cell list.
- **Memory.** `StaticWorld::build` writes two `u16` buffers the caller owns:
  - the sizes come from `StaticWorld::size`;
  - nothing is allocated, so a console keeps the buffers in its level state;
  - the limit is 65 535 solids and references.
- **Order.** Cells list their solids in ascending order.
- **Overlaps.** A query reports each solid once, in the first cell it shares with the
  query box. The results are sorted, and a short buffer keeps the lowest indices.
- **Rays** walk the grid with Amanatides–Woo. They stop once the next cell starts past
  the best hit. Ties go to the lowest index.
- **The contract.** Every grid query returns what a scan of the same slice returns, bit
  for bit:
  - the references are `overlap_solids` and `raycast_solids`;
  - `crates/core/tests/physics_world.rs` checks them on 500 solids, short buffers
    included.
- **Dynamic solids.** `World` pairs the grid with a small dynamic slice, scanned
  linearly. Dynamic solid `k` has index `statics.len() + k` in every result.
- **Layers.** A `Solid` has `layer` bits and every query a `mask`; a solid takes part
  when `layer & mask != 0`.

Not chosen:
- **A BVH for boxes:** more code, and on the N64 the uniform grid's cell walk is cheaper
  for axis-aligned content.
- **Rapier:** `std`/alloc-heavy, too large for 4 MB, and different results per
  target.

### 2. Shape casts and triggers

- **`CastShape::{Sphere, Capsule, Box}` swept from a point to another.**
  - A sphere against a box is exact: the earliest contact over three face slabs, twelve
    edge cylinders and eight corner spheres.
  - A Y-aligned capsule is a sphere against the box grown along Y by its half segment.
  - A box against a box is a ray against the Minkowski sum.
  - A shape that starts inside reports `inside` and the way out.
  - Ramps and one-way boxes are cast against as their box. A ray hits a ramp's slope
    (Cyrus–Beck over the wedge's seven planes).
- **`TriggerTracker<N>`** records (body, trigger) pairs per frame:
  - `end_frame` emits Enter / Stay / Exit by a sorted merge, ordered by (body, trigger),
    so every platform gets the same list;
  - a full tracker counts what it dropped.

### 3. The character controller (`CharacterController<S>`)

- **The body is a box on every target.** `ControllerConfig::capsule` maps a capsule to
  its box. This is the N64-era cost model, and it is deterministic: no target collides
  differently.
- **The move is `move_aabb`'s.** It goes one axis at a time (X, Z, then Y) with the same
  contact skin, over a slice of `Surface`s visited in index order.
- **With every option off, it is `move_aabb` bit for bit.** The operations are the same
  and in the same order:
  - `with_options_off_the_controller_is_move_aabb_bit_for_bit` replays castle64's own
    loop on 39 random worlds × 900 frames;
  - that is why castle64 moved onto the controller with every golden, perf counter and
    bot run unchanged.
- **The options, all data:**
  - **Steps:** a ledge within `step_height` is climbed if the body fits on top.
  - **Snapping:** a grounded body follows the ground down by up to `snap_down` per
    substep.
  - **Slopes:** see §4.
  - **One-way platforms:** a `SurfaceKind::OneWay` box is solid only to a body whose feet
    were at or above its top before the move.
  - **Moving platforms:** `carry(delta)` before `step`, while `standing_on` is the
    platform. `step_world` keeps `standing_on` in world indices between frames.
  - **Coyote time and jump buffering:** `jump(pressed, speed)`. With both off it is
    `pressed && on_ground`.
- **`step_world`** gathers the surfaces near the frame's move from a `World`, at most
  `STEP_WORLD_CANDIDATES` (64), and reports `truncated`.

### 4. Ramps and mesh colliders

- **Ramps (`Shape::Ramp { half, dir }`).** A box whose top slopes from its bottom on one
  side to its top toward `RampDir`:
  - **Height.** The body's footprint takes the slope's height at its highest point.
  - **Walkable ramps.** The walk lifts the body within the slope's rise, plus the skin.
  - **Steep ramps** (past the body's `max_slope_tan`) are walls. A body landed on one
    slides downhill and is not grounded.
  - **Turning.** A ramp turned with its entity picks the nearest world direction (exact
    for quarter turns).
- **Mesh colliders (`Shape::Mesh { mesh, half }`).** `mesh` is the blob
  `collision/<model>`, a `TCOL`:
  - **Contents:** vertices, triangles and a BVH. The core's `write_tcol` writes it into
    a caller buffer (median split over centroids, four triangles per leaf) and
    `MeshCollider::parse` validates it in place:
    - every index and range is checked;
    - children come after their parent, so the tree is acyclic;
    - its depth is bounded.
  - **Placement.** A `MeshInstance` places a mesh under its entity's matrix. Queries
    transform the query box into the mesh's space and the visited triangles out, so a
    rotated mesh works.
  - **Ground.** Walkable triangles (`normal.y >= min_normal_y`) hold a body up at the
    plane's highest point over its footprint, clamped to the triangle's height range.
  - **Walls.** Steep triangles block a body that overlaps them (Akenine-Möller
    triangle–box test).
  - **Rays** hit meshes too (Möller–Trumbore).
- **The manifest's `collision` key:**
  - a master path (a simplified mesh);
  - `"visual"` (the visual mesh's triangles, welded, degenerate ones dropped);
  - `"auto-hull"` (the convex hull: incremental, f64, deterministic).
  It bakes `collision/<name>` on every target. A model's per-platform variants never
  change it: collision is gameplay.
- **The budget.** `Caps::max_collision_tris` is 4096 on the N64, 16 384 on the 3DS and
  10⁶ on the PC and the Web. `physics check` and the editor's HUD hold a scene to it.

### 5. A fixed-point mode (`Scalar`)

- **The scalar.** The controller, its surfaces and its options are generic over `Scalar`:
  - `f32`, the default, which castle64 runs;
  - `Fx`, Q16.16 in an `i32`:
    - products go through `i64` and an arithmetic shift;
    - quotients truncate toward zero and saturate at ±`i32::MAX`;
    - `sqrt` is an exact integer floor.
- **Code size.** Only the instantiations a game uses are compiled.
- **Meshes.** Triangle meshes are `f32` only. `MeshQuery` is implemented for `f32`, so a
  fixed-point controller cannot be handed a mesh. `NoMesh` is the empty type for a level
  without any.
- **Range.** Q16.16 holds ±32 767, and products of two coordinates must fit, so a
  fixed-point level stays within ~150 units or is scaled down (Solid Eleven draws at
  1/64).

### 6. Formats

- **Scene RON:** no version bump; every new field is optional and defaulted.
  - `collider` gains `layer` (a name, written bare), `one_way`, and the shapes
    `Ramp(size, dir)` and `Mesh("models/x")`.
  - Entities gain `character: (height, radius, max_slope_deg, step_height)`.
- **`assets/physics.ron`** (v1): up to 16 layer names, bit 0 the default, and the
  `ignore` pairs that pass through each other.
  - The editor's Project › Physics page and `cargo xtask physics layers` write it.
  - The bake writes it as `physics/layers` (`TPHY`: a mask per layer,
    `LayerMatrix`).
- **TSCN v7:** each node ends with a 24-byte physics block:
  - the collider's layer, one-way, ramp direction and mesh blob;
  - the character (flag bit 16);
  - shapes 4 (ramp) and 5 (mesh).
  v6 blobs are refused, like every older version: rebake (ADR 0005 §3).
- **`SceneNode::character`**, `Collider::{layer, one_way}` and the codegen template all
  gained fields. `examples/showcase/src/showcase_gen*.rs` were regenerated (only the new
  fields moved).

### 7. Tools and the editor

- **`cargo xtask physics check [scene..] [--json]`** reports:
  - counts, the grid's size and the budget per platform;
  - problems: overlapping static solids, mesh colliders without a baked collision,
    triggers without a name of their own, unknown layers, ramps no character can walk.
- **`physics query <scene>`** answers what the engine's queries answer, for an agent
  without a GUI: `--ray`, `--cast`, `--overlap`, `--drop` (a character settles).
- **`physics layers`** prints and edits the matrix.
- **The editor:**
  - **Scene view gizmos per shape:**
    - boxes;
    - rings for round shapes;
    - one-way boxes with an up arrow;
    - ramps as their wedge;
    - mesh colliders as their triangles;
    - triggers filled;
    - character bodies with their step height.
  - **The Inspector:** Ramp and Mesh shapes, a layer from the project's list, one-way, a
    Character component.
  - **Project › Physics:** the layers and their matrix (`TRINO_EDITOR_PROJECT_PAGE=physics`).
  - **Simulate controller** (`TRINO_EDITOR_PHYSICS=sim`): Play here walks the `player`
    on the engine's controller (any scene's `character` body does it on its own) and
    draws the overlay:
    - its body;
    - the ground normal;
    - the step probe;
    - a status line.
  - **The budget HUD's collision line:** solids, one-way, ramps, triggers, mesh
    triangles against the console's limit, and an N64 estimate per body.

### 7a. The castle64 migration

- **What moved.** castle64's player jumps and moves through `CharacterController<f32>`
  with every option off.
- **What stayed.** Its `pos`/`vel`/`on_ground` fields stay the game's, because the bot,
  the HUD and the tests read them. The game hands them to the controller around the
  step. Its mover carry stays its own, applied before the step as before.
- **Why not more.** A nonzero `step_height` would change how its bot takes short box
  edges, so steps, slopes and snapping are shown in the physics playground, not in
  castle64's levels.

### 8. Measurements (emulated time, the self-test's `physics` step)

The 500-solid venue (400 × 400 m, four layers, a floor), and castle64's hub frame:

| Query | N64 scan | N64 grid | New 3DS scan | New 3DS grid |
|---|---|---|---|---|
| 64 rays | 116 021 µs | 6 024 µs (19.3×) | 20 375 µs | 1 089 µs (18.7×) |
| 16 controller steps | 11 984 µs | 2 445 µs (4.9×) | 1 112 µs | 389 µs (2.9×) |
| hub frame (17 solids: camera ray + player step) | 169 µs | 183 µs | 30 µs | 25 µs |

- **The venue.** The grid pays for itself on dense levels.
- **The hub.** Its 17 solids don't need it (on the N64 the grid is even slower there):
  the scan is what castle64 still does.
- **Scope.** One build, with the physics code `optimize(size)` on the consoles (§9).
  Both tiers of each console measure within 1 %. The ratios are far beyond the ~20 %
  that code layout moves.

### 9. The base N64's RAM

- **The problem.** The self-test ROM grew ~100 KB of `.text`, which lives in RDRAM, and
  main's base-N64 self-test had ~29 KB of slack over its 128 KB heap gate. The growth
  also brought out GitHub #110's fragmentation.
- **What fixed it:**
  - console `optimize(size)` on the physics code;
  - one compiled controller `step` per scalar (`Scalar::controller_step`), where a
    generic copy per crate and mesh type had cost 8 KB each;
  - meshes as `&[&dyn MeshQuery]`;
  - an insertion sort for the query buffers;
  - `ShowcaseGame::write_blank` / `EntityStore::write_new` instead of a 44 KB `static`
    blank;
  - `N64Renderer::trim_buffers` and the micro scenes' flat copy cached first (#110).
- **The result.** The base self-test ends with 166 KB free (main: 157 KB).
- **`N64_COST`** gains `us_per_collision_solid` (9.9 µs) and `us_per_grid_ray`
  (94 µs). The editor's estimate reads them.

## Consequences

- **Every target collides alike.** The course hash of the `physics` step is pinned to the
  PC's (`0xba581be1` f32, `0xffa322c7` fixed point) and reproduced bit for bit in ares
  and Azahar.
- **Rigid-body dynamics stay out of the core.** Games that need it, like Solid Eleven's
  ball, keep their sims on top of the queries. Solid Eleven evaluated the fixed-point
  mode: its `Fx` matches the engine's bit for bit over 20 000 random operations, so the
  sim could take the engine's controller and queries without its match hash moving. It
  keeps its own sim: the pitch has nothing to collide with but the goal frame, which its
  ball physics models in detail.
- **Not done:**
  - shape casts against triangle meshes;
  - capsules that collide as capsules (the body is a box everywhere);
  - grid buffers baked into TSCN: `StaticWorld::build` runs at load, in microseconds;
  - per-cell physics for scene streaming (BACKLOG P2 #11).
- **Unmeasured:** real hardware (as with every number here).
