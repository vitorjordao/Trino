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

### 3. The editor's layout is tested

The Inspector, the Assets strip and the toolbar overflowed at the editor's minimum
window (960x600) and with long paths. `layout_audit` walks the accessibility tree and
fails a test when a widget runs off the window or overlaps another; it runs over the
whole editor at 960x600 and 1600x940 and over each new panel at several widths. Every
new surface gets a pass of it.

## Consequences

- One scene file describes every platform; a diff shows what each console gets.
- Impostor atlases are generated files committed with the project: small, reviewable as
  images, and kept honest by `impostor check`.
- Exported code is per platform: a scene with variants is exported once per console.
- The editor resolves the scene twice per frame (Scene view on PC, Game view on its
  target); scenes are small, and the PC resolve is reused when the Game view is on PC.
