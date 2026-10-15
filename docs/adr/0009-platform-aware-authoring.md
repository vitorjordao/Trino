# ADR 0009: Platform-aware authoring in the editor

Status: accepted (October 2026)

## Context

A project that ships on PC, N64 and 3DS adapts its content per platform: a cheaper
model on the N64, a prop left out of the 3DS, a flatter material. Until now that lived in
game code (`cfg` blocks, per-platform tables), invisible to the editor and to an agent
reading the scene files. The editor should let a person do it visually, an agent do the
same thing by writing text, and both see each other's changes.

## Decisions

### 1. Per-platform variants on entities (scene v3, prefab v2)

- `Entity::platforms` holds an optional `Variant` per target (`pc`, `n64`, `n3ds` — RON
  names cannot start with a digit). A variant can replace the model and the material,
  replace the scale of the entity's own model (or instanced prefab), override
  `visible`, or drop the entity (`disabled`). Unset fields keep the entity's own values.
- **Variants are a look, not gameplay.** A collider keeps the PC model's bounds and the
  base scale, so a deterministic game simulates the same thing everywhere. Children and
  cells/portals hang off the base transform. Only `disabled` removes gameplay data.
- **Version bump although the field is optional.** The versioning contract allows adding
  a defaulted field without a bump, but an older engine ignoring `platforms` would ship
  the PC model to a console without a word. Scenes go to v3 and prefabs to v2, so such an
  engine refuses the file ("newer than this engine understands"). v2 scenes and v1
  prefabs load unchanged and save in the new version.
- **Resolution happens at the edges, per target.** `trino_scene::resolve_for(scene, lib,
  time, target)` applies the variants; `resolve` is the PC. Every consumer picks its
  target: the editor's Game view (its simulation profile) and Play here, the PC scene
  player (`TRINO_SIM`), `cargo xtask scene export --platform`, and the `[scenes.*]` bake
  of each platform (`bake_scene_for`). Runtime formats (`SceneNode`, `TSCN`) are
  unchanged: a console only ever sees its own variant.
- `cargo xtask scene check` resolves every scene for each target and fails when a target
  draws a model its manifest entry does not bake (`platforms = ["pc"]`).

### 2. LOD chains with impostors, baked by the engine

- **A LOD level can draw an impostor** (`LodDef::impostor`, `LodLevel::impostor`): a model
  pre-rendered from `K` yaws into one atlas and drawn as one camera-facing billboard (2
  triangles). The chain is full mesh → lighter meshes → impostor → nothing, and each level
  has a start per platform (`LodAt::At(d)`) or is skipped there (`Never`).
- **Runtime:** `trino_core::Impostor` is a small `Copy` descriptor (sprite, views, columns,
  tile, quad size, center). The view is the rendered yaw nearest to the camera's direction
  in model space (`view_for`, the core's deterministic `atan2`), so every target picks the
  same frame. Regions follow the grid; the `TIMP` blob also stores them so tools agree.
  Drawing is `draw_billboard` with a region — no new `Renderer` method, no backend code.
- **TSCN v2:** LOD records grow from 8 to 32 bytes to carry the descriptor inline (a
  console resolves no asset at draw time). v1 blobs are refused with "rebake"; TSCN was
  not frozen yet (BACKLOG P0 #2).
- **The engine bakes impostors itself**, with its own T&L (`tessellate`) and a CPU
  rasterizer: deterministic bytes, no GPU, no Blender, the game's projection and lighting.
  Settings are text (`assets/impostors/<name>.impostor.ron`); the outputs are ordinary
  assets (a sprite and a blob in the manifest), so every bake path already ships them.
  The N64 gets an indexed CI4 PNG whose tiles shrink until the atlas fits TMEM (4096
  texels with the palette); PC and 3DS share the RGBA atlas.
- **One bake, two front ends.** `cargo xtask impostor` and the editor's model inspector
  call the same `trino_asset_pipeline::impostor::bake`; a test asserts the CLI finds
  nothing to change after the editor baked. Each table records the model's hash, so a
  model edit that wasn't re-baked fails `impostor check` and shows "stale" in the editor.
- **Cost honesty:** `N64_COST.us_per_billboard` (44 µs, measured) enters the estimates. On
  the N64 most static meshes run on the RSP for less than a CPU-submitted billboard, so
  the tools say so and per-platform distances let the N64 skip impostors.

### 3. Animation checked in the editor, measured by the engine

- The Animation tab previews any model with a skeleton in its own renderer (the same
  `PcRenderer`, so the N64 and 3DS looks are real) and drives `AnimRef` directly: a time,
  a speed, a crossfade with its weight.
- **Measurements come from the T&L's own posed vertices** (`Mesh::posed_position`, the
  T&L's expression, so tools and draws agree): a foot is planted when it is the lowest on
  the floor in a frame; its contact vertex's move between frames gives the stride speed,
  and the worst deviation from it is the foot slide. A looping clip's last frame is its
  first again (time wraps at the duration), so only consecutive frames pair up.
- The same `trino_asset_pipeline::anim` backs `cargo xtask model inspect --anim` and
  `model pose`, and the tab's "Console pose" writes the same glTF and manifest entry.
  A pose model has no skeleton: a console variant that skips skinning.

### 4. Particle effects as assets, capped per platform

- An effect is text (`*.particles.ron`, a `ParticlesDef`) declared as `[particles.*]`, a
  new manifest kind baked per platform to a fixed-size `TPFX` blob: the `Emitter` and the
  cap of that platform (0 = off there). Systems stay fixed-capacity (`ParticleSystem<N>`,
  no heap); `limit` applies the cap at runtime.
- The editor previews an effect with the real renderer and a live system on each
  platform's cap; `cargo xtask particles inspect` runs the same simulation (30 fps,
  seeded) and prints a position hash, so determinism is checkable from text.

### 5. A scene's cost per platform, before the console

- `trino_core::render3d::rsp_check` returns the first rule a draw fails (`RspReject`);
  `rsp_eligible` keeps its meaning and is defined by it, so the console and the
  estimate cannot drift apart.
- `trino_scene::budget::estimate` resolves the scene for a platform (variants, LOD,
  impostors), runs the same `tessellate` work counters the PC renderer keeps, splits the
  N64 draws into RSP and CPU, and prices them with `N64_COST` (the CPU side; plus
  `us_per_rsp_tri` for the RSP side). The frame is the slower half. Draws whose whole
  mesh is outside the view are named: they cost the CPU and show nothing.
- The editor shows it in the Game view's HUD and the Inspector's scene settings;
  `cargo xtask scene budget` prints the same numbers for an agent. The 3DS has no
  calibrated time model yet: it gets triangles and draws.

### 6. The editor's layout is tested

The Inspector, the Assets strip and the toolbar overflowed at the editor's minimum
window (960x600) and with long paths. `layout_audit` walks the accessibility tree and
fails a test when a widget runs off the window or overlaps another; it runs over the
whole editor at 960x600 and 1600x940 and over each new panel at several widths. Every
new surface gets a pass of it. Overlaps are judged on the clipped rects egui keeps for
each widget (hidden rows of a scroll area are not on screen).

## Consequences

- One scene file describes every platform; a diff shows what each console gets.
- Impostor atlases are generated files committed with the project: small, reviewable as
  images, and kept honest by `impostor check`.
- Exported code is per platform: a scene with variants is exported once per console.
- The editor resolves the scene twice per frame (Scene view on PC, Game view on its
  target); scenes are small, and the PC resolve is reused when the Game view is on PC.
