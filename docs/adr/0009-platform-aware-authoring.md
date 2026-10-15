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

### 2. The editor's layout is tested

The Inspector, the Assets strip and the toolbar overflowed at the editor's minimum
window (960x600) and with long paths. `layout_audit` walks the accessibility tree and
fails a test when a widget runs off the window or overlaps another; it runs over the
whole editor at 960x600 and 1600x940 and over each new panel at several widths. Every
new surface gets a pass of it.

## Consequences

- One scene file describes every platform; a diff shows what each console gets.
- Exported code is per platform: a scene with variants is exported once per console.
- The editor resolves the scene twice per frame (Scene view on PC, Game view on its
  target); scenes are small, and the PC resolve is reused when the Game view is on PC.
