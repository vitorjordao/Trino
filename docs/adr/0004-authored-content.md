# ADR 0004 — Authored content: scenes v2, prefabs, materials, fake objects, rigid animation

**Status:** accepted (2026-10-12)

## Context

Trino is about to host a real 3D project. Until now 3D content lived in game code
(`castle64/src/levels.rs`) and the editor only placed 2D sprites. A real project needs
levels, reusable objects, surfaces and animated models authored by humans **and** AI
agents, hot-reloaded, and shippable on N64/3DS — without breaking the two rules:
`trino-core` is `no_std` with zero dependencies, and nothing the N64 can't do is exposed.

## Decision

1. **Text is the source of truth.** Scenes (`scenes/*.scene.ron`, v2), prefabs
   (`prefabs/*.prefab.ron`) and materials (`assets/materials/*.material.ron`) are
   versioned RON owned by `trino-scene` (std). The editor reads/writes exactly these
   files; agents edit the same files; watchers reload both ways.
2. **Scene v2**: nested entity tree (parent-relative transforms, Euler XYZ in
   **degrees**), components `model` (+ material, tint, animation clip), `sprite`,
   `prefab` instance, `tags`, `visible`; an `environment` (camera, colored light, fog).
   v1 files migrate in `Scene::from_ron`.
3. **Prefabs = Blender linked collections.** An instance links a prefab by logical path;
   editing the prefab updates every instance; `Override`s (material / tint / visible by
   child name path) customize one instance; `Library::unpack` makes it real; nesting is
   allowed and cycles are reported (never followed).
4. **Materials are data, not shaders**: `MaterialDef` → `trino_core::ModelParams`
   (tint, emissive, `Shading`, double-sided) evaluated by the software T&L (ADR 0003),
   hence identical on every target. `Material::Named` stays reserved.
5. **Fake objects** are models: `*.prim.ron` parametric primitives bake to TMDL in the
   asset pipeline, so they work on every target and in every tool on day one. Replacing
   one with art changes only the manifest `file` — the logical path (handle) and every
   reference survive.
6. **Rigid animation** inside the mesh blob: glTF node animations / skins bake into TMDL
   `SKEL` + `ANIM` sections — each vertex follows exactly one joint (heaviest weight),
   ≤ 32 joints, clips sampled at a fixed rate. `Mesh::pose` evaluates on the stack in
   `trino_core`; `ModelParams::anim` selects the pose. No backend changes, no allocator.
7. **Getting scenes onto consoles without a runtime parser**: `cargo xtask scene export`
   flattens a scene (prefabs expanded, world matrices, resolved materials) into a
   `static` slice of `trino_core::SceneNode` that game crates `include!`. Drift tests keep
   generated files in sync; `xtask/tests/showcase_render.rs` asserts the runtime-resolved
   and exported scenes render pixel-identically. On PC, `TRINO_SCENE` plays the RON scene
   directly (reloading scene/prefab/material edits) and `TRINO_GAME=showcase` runs the
   exported version; console builds of the exported code are type-checked (not yet
   booted in an emulator).

## Consequences

- Games remain pure `no_std` crates; scenes reach them as data, not as a dependency.
- Editor features are thin: every UI action ends in one of these files, so humans and
  agents share one workflow and one undo-able history (snapshot undo in the editor).
- Rigid skinning cracks at bent joints by design (N64-era look). Smooth skinning would
  need per-vertex weights in TMDL and more CPU per vertex — revisit only with numbers.
- Codegen means a rebuild to change a console scene. The next step (docs/BACKLOG.md) is
  a baked binary scene asset (`TSCN`) loaded through a generic blob API on every
  backend, keeping codegen for compile-time-checked content.

## Update (October 2026)

- The baked `TSCN` scene asset and the `Platform::blob` API shipped as planned.
- Scenes v2 gained the optional `collider`, `model.lod`, `sound` and `vis` (cell/portal)
  components without a version bump.
- Opt-in smooth skinning (`SKW2`) exists for meshes that need it.

Formats and rules are in ADR 0005.
