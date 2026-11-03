# ADR 0035 — Physics 2.0: a broadphase, shape casts, triggers, a character controller, ramps and mesh colliders, a fixed-point mode

- Status: accepted (2026-10-25); extended by Physics 2.1 (2026-11-02, GitHub #115, §10–§15)
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
- **Not done in #66, done in #115 (§10–§15):**
  - shape casts against triangle meshes;
  - the capsule decision (§11: the body stays a box on every tier, declared in `Caps`);
  - grid buffers baked into TSCN;
  - per-cell physics for scene streaming (BACKLOG P2 #11).
- **Unmeasured:** real hardware (as with every number here).

## Physics 2.1 (GitHub #115)

### 10. Shape casts and overlaps, exact against meshes, ramps and one-way boxes

- **A small convex polytope** (`physics/convex.rs`): a triangle, the triangle extruded by
  ±`seg` along Y, a ramp's wedge, the wedge extruded. The constructors compute the face
  normals (Newell), the scale and the volume test once.
  - **Spheres:** a ray against the polytope grown by the radius. Its boundary is the
    faces moved out by `r` (both sides), a cylinder round each edge and a sphere round
    each vertex. Each piece lies on or inside the grown solid and together they cover its
    boundary, so the earliest hit among them is the exact first contact. Degenerate
    pieces are skipped.
  - **Y-capsules:** a sphere against the polytope extruded by the half segment (the
    Minkowski sum with the segment). This is not the box trick of §2: a capsule passing
    beside a box's top edge with its cap clears it (tested).
  - **Boxes:** the swept separating-axis test over the box's axes, the face normals and
    the box axes crossed with the edges; the contact is the latest entry.
- **Meshes:** `MeshInstance::shape_cast` / `overlap_shape` walk the BVH with the swept
  bounds in mesh space (grown by 1e-3 so the inverse transform's rounding drops nothing)
  and test each triangle's world corners. `shape_cast_mesh_scan` /
  `overlap_shape_mesh_scan`, the brute force, run the same per-triangle test on every
  triangle: the BVH path equals them bit for bit, hit and triangle (ties: the lowest).
  A triangle outside the swept bounds is skipped before the exact test (both paths).
  `shape_cast_meshes` picks the earliest over several meshes.
- **Ramps** are their wedge in shape casts, `overlap_shape` and `overlap_aabb` (a box
  meets the wedge where `q ∩ box` reaches below the slope's plane: no polytope needed).
  Rays already hit the slope (§2).
- **One-way boxes** stop a cast only when the shape's bottom starts at or above the top
  (within the contact skin): solid from above, like the controller's. A motionless
  overlap has no direction, so it still reports their box.
- **The contract** (grid == scan) holds with all of it: `overlap_solids`,
  `shape_cast_solids` and the new `overlap_shape_solids` use the same tests.
  `step_world` gathers its candidates by bounding boxes (`overlap_bounds`, a superset),
  so the controller saw no change and every course hash kept its bits.

### 11. Capsules: the body stays a box on every tier

- **Decision.** `Caps::capsule_bodies` is `false` on every table (N64, Expansion Pak,
  3DS, New 3DS, PC, Web).
- **Why.** A capsule body on the PC and the Web only would make one game move one way on
  the N64 and another on the PC. Replays (`TREC`, #67) and lockstep netplay are
  cross-target by construction, and castle64 is `move_aabb` bit for bit. A capsule body
  on every tier would cost the N64 the axis-resolved box mover the cost model is built
  on.
- **What games get instead.** Capsules and spheres are exact in every shape cast and
  overlap, against boxes, ramps and meshes, on every tier (§10).

### 12. Fixed-point meshes (`FxMesh`)

- `FxMesh::build` converts a `MeshInstance` once, at load, into caller buffers: world
  corners rounded to Q16.16 and a unit normal per triangle, plus the BVH's nodes with
  bounds recomputed bottom-up in Q16.16.
- **The normal** is the cross product of the edges in `i64` on the raw bits, shifted
  under 2^30 and normalized with an integer square root: no floats after the load.
- **Queries** (`MeshQuery<Fx>`): walkable support (the plane's height over the footprint)
  and steep-triangle blocking by separating axes, all `i64` on the raw bits. The
  conversion uses f32 once per vertex; f32 adds and products are IEEE on every target,
  and the self-test pins the course over it (`PHYSICS_FX_MESH_HASH` `0x5d3c8f29`) on PC,
  ares, Azahar and the browsers.
- **Range:** ±256 units (edges and coordinates); the fixed-point controller's own range
  (§5) is tighter.
- `CharacterController<Fx>` takes `&[&dyn MeshQuery<Fx>]` as the f32 one does: one
  compiled step per scalar (§9).

### 13. Physics per cell and the baked section (TSCN v9)

- **Parts.** `CellPart`: one cell's solids gridded (their own `StaticWorld`), its
  triggers, and fixed global indices (`base + k`). Parts ascend by cell, `NO_CELL` (the
  level outside any cell) last and always loaded.
- **`CellWorld`.** The parts, a loaded-cell mask (`set_loaded`) and a dynamic list.
  Rays, casts and overlaps visit the loaded parts and equal a scan of their solids in
  global order (tested over random masks). Indices never shift when a neighbour streams,
  so a `standing_on` or a trigger key stays valid.
- **The controller:** `step_in(dt, &dyn SolidSource, meshes)` takes a `World` or a
  `CellWorld`; `step_world` is it for a `World`.
- **Ground across a stream.** A body standing on an unloaded cell's floor loses it and
  falls. Games keep the cells under their bodies loaded: `loaded = groups ready |
  cells_under(body)`. `CellGroups::ready_cells` gives the first half.
- **Triggers across a stream.** `TriggerTracker::end_frame_held(out, live)` keeps a pair
  whose trigger's cell unloaded (no Exit). Back in a loaded cell, a body still inside is
  a Stay, never a second Enter; one that left meanwhile is an Exit. Held pairs take
  room in the tracker: size `N` for the live pairs plus the held ones. A full tracker
  drops a held pair (counted in `dropped`) silently, and it comes back as an Enter.
- **The section.** The bake gathers the nodes' colliders (`gather_nodes`), sorts them by
  cell, grids each cell at `grid_cell_size` and writes the result (`write_section`)
  after the data table. The environment's last 8 bytes point to it (TSCN v9).
  `BakedPhysics::decode` gives the parts back, gridded as baked (`StaticWorld::from_raw`
  validates every list).
- **The migration (ADR 0005 §3).** A v8 blob still parses: no section, and
  `physics::load_scene` gathers and grids its nodes at load, the same parts (tested). A
  scene in its own asset group reads through `TscnView` (`BakedScene` needs `'static`
  bytes, a `group_blob` doesn't live that long).
- **Tools.** `physics check` reports each cell's part, grid and the section's bytes;
  `physics query --cells <mask>` queries with only those cells loaded. The editor's Play
  › Physics cells toggles the mask, the Scene view grays unloaded cells' colliders and
  labels each cell with its physics, the budget HUD's collision line counts cells and
  the baked bytes.

### 14. castle64 on the controller's options

- `player_body()` turns on `step_height` and `snap_down` (0.3 m). castle64's levels had
  no ledge or drop that low, so the options alone changed nothing. A stone landing of
  two 0.25 m steps now sits on the hub's route to the green door: the bot walks up and
  down it without jumping, and the whole game (four stars and the throne room) still
  clears. The run climbs 3 ledges and snaps 4 times (`bot_plays_the_whole_game`).
- No slopes: castle64's levels are boxes drawn as cubes. Slopes stay in the physics
  playground and the course of §8.
- Every frame that shows the landing, the bot's recording and the perf baselines were
  re-blessed (the PR lists them).

### 15. Measurements (emulated; the self-test's `physics cells` and `physics casts`)

- **Load of a 200-solid venue** (`scenes/physics-venue.scene.ron`, four cells): its
  physics from the baked section against a v8 load (gather the nodes, grid each cell),
  the mean of 4 loads:

  | Target | Baked section (v9) | Gathered and gridded (v8) |
  |---|---|---|
  | PC (debug test build) | 7 µs | 25 µs |
  | New 3DS (Azahar) | 545 µs | 2 318 µs |
  | Old 3DS (Azahar) | 562 µs | 2 318 µs |
  | N64 + Expansion Pak (ares) | 3 354 µs | 6 982 µs, re-gridding only |

  On the N64 the "before" grids the decoded solids again but leaves out the node
  gather, whose code does not fit the base console's self-test heap (below): the real
  v8 cost there is higher. The N64 numbers come from the branch's 8-layout sweep; the
  final ROM leaves the venue out altogether (its code costs the base console's heap),
  and other hosts skip it with less than 256 KB of heap free.
- **Mesh casts on the 3DS:** 48 casts against the rock (28 triangles) take 20.1 ms
  through the BVH and 20.0 ms by brute force: about 0.42 ms a cast. The BVH pays off on
  bigger meshes; the rock is one leaf group wide for most casts.
- **The base N64's heap.** The self-test links every engine feature, and the new code
  took 65 KB off the base console's heap at first. What brought it back:
  - ramp overlaps by the slope's plane instead of the polytope;
  - the perf reports' model sort as an insertion sort (`perf::sort_models`, 6 KB of
    `sort_unstable` per caller);
  - `N64Renderer::trim_buffers` gives newlib's free top chunk back to sbrk
    (`runtime::heap_trim`, `malloc_trim`). newlib extends the heap by a request's whole
    size when its top chunk is too small, so a 68 KB free top chunk beside 57 KB of sbrk
    room made the micro scenes' 79 KB batch fail (GitHub #110);
  - the global allocator's `trino_memalign` (in the shim, so each allocation site stays
    one call) retries once after `malloc_trim`: CI's newer nightly left 1 KB less, and a
    45 KB block failed after the micro scenes without it;
  - `step_impl`, `Surface::top_at` and `normal` out of line again, as on main;
  - the N64 self-test leaves out the mesh, ramp and one-way casts and the venue's load
    (the same f32 code the controller's hash proves on the N64; PC, Azahar and the
    browsers run them).

  The base self-test passes with a peak of 2 038 288 of 2 195 792 bytes (main: 2 036 496
  of 2 213 392). libdragon's sbrk keeps a margin of its own (~32 KB) before the stack:
  `top=` counts space a block can't use.
- **The N64 table over 8 code layouts** (`TRINO_N64_TEXT_PAD` 0 … 8192, Expansion Pak,
  mean (min–max), emulated µs; ADR 0012 §3). `main` is 8bea708, the branch this PR:

  | Query | main | #115 |
  |---|---|---|
  | 64 rays over the 500-solid venue, scan | 116 117 (116 035–116 244) | 118 249 (118 119–118 410) |
  | the same, grid | 6 342 (6 248–6 546) | 6 388 (6 206–7 470) |
  | 16 controller steps over the venue, scan | 12 291 (12 156–12 597) | 11 304 (11 233–11 479) |
  | the same, grid | 2 939 (2 761–3 117) | 2 851 (2 738–3 010) |
  | castle64's hub frame (ray + step), scan | 170 (161–187) | 175 (169–191) |
  | the same, grid | 196 (179–207) | 220 (211–231) |
  | the venue's physics load, baked section | — | 3 329 (3 205–3 429) |
  | the same, re-gridded (v8 without the gather) | — | 6 922 (6 764–7 281) |
  | castle64's hub perf scene (`frame_us`) | 12 070 (11 873–12 584) | 12 419 (12 160–12 607) |

  The grid's 18× over the scan holds over every layout (#114 quoted one build). The
  hub's 170 vs 196 µs is within layout noise, as #114 suspected: castle64 keeps the
  scan. The branch's hub step runs with step height and snap on (§14), and the hub
  scene draws the landing's two blocks (+2 RSP draws, +24 triangles).
- **Not measured:** real hardware (no flashcart or 3DS here).
