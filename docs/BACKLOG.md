# Trino — ranked backlog for a real 3D project

Written after the October 2026 pass (editor overhaul, scenes v2, prefabs, materials, fake
objects, rigid animation; ADR 0004). Ranked by **what blocks shipping a 3D game first**,
then by cost. Each item says why, the N64-safe shape of the solution, and rough size
(S ≤ 1 day, M ≤ 1 week, L > 1 week).

## P0 — needed before production starts

1. **Collision from scenes** (M). Today collision lives in game code (`castle64::physics`,
   `trino_core::collide` for tilemaps). Add a `collider` component (box / sphere / capsule,
   and "use the fake object's shape") to scenes v2, export it in `SceneNode`, and move a
   generalized version of castle64's axis-resolved AABB mover + raycast into `trino_core`
   (fixed-size, no alloc). Editor: draw colliders as wireframes, toggle in the view menu.
2. **Runtime entities for gameplay** (M). Exported scenes are static data; games need to
   spawn/despawn, move and query entities. A fixed-capacity entity store in core
   (`EntityStore<const N: usize>`: transform, model, params, tags, user data index)
   instantiated from `SceneNode`s, with `find`/`with_tag` queries. Keeps games `no_std`.
3. **Bitmap text** (S–M). HUDs and menus need text; castle64 uses one sprite per digit.
   A bitmap-font asset kind (PNG atlas + glyph table baked by the pipeline, CI4 on N64)
   and `Renderer::draw_text` built on `draw_sprite`. Editor: font preview.
4. **Validate the Blender toolkit for real** (S). `tools/blender/*.py` were adapted but not
   run (no Blender in CI). Run the `blender-asset` skill end to end once (static prop +
   rigid-part character), fix API drift, record the facing convention, and add a
   `blender --background` smoke script for machines that have Blender.
5. **Clip crossfade** (S). `Mesh::pose` blends two frames already; blending two clips
   (`AnimRef` + `blend: Option<(ClipId, f32 time, f32 weight)>`) removes popping between
   idle/walk/run at almost no CPU cost.

## P1 — the next quality jump

6. **Baked scene asset (`TSCN`) + generic blob API** (M). Load scenes at runtime on every
   target (data-driven levels, no rebuild): a `blobs` asset kind (bytes by handle) in the
   PC/N64/3DS asset loaders and a `Platform::blob(id)`-style accessor; `SceneNode` parsed
   zero-copy like TMDL. Keep codegen for compile-time-checked content.
7. **Textured 3D, budgeted** (L). The biggest visual gap. N64: one ≤ 4 KB texture per draw
   through `rdpq_triangle` with texture coords (the shim already builds trifmt vertices);
   3DS: citro2d can't, needs a small citro3d path; PC: UVs in the tri pipeline. TMDL gains
   a `UV` section; materials gain `texture: Option<sprite path>`. Strict mode checks TMEM.
8. **Budget HUD in the Game view** (S). Live tris / draw calls / texture memory vs the
   selected console's `Caps`, red when over. Makes N64 limits visible while designing.
9. **Distance LOD + fake-object impostors** (M). `lod: [(distance, model)]` on model
   components; fake objects are natural far LODs. Essential to fit open areas in 4 000
   tris/frame.
10. **Billboards / particles** (M). Sprites in 3D space (camera-facing quads through the
    T&L, z-tested) for coins, smoke, sparks. Prerequisite for most game feel.
11. **Editor productivity** (M, incremental): multi-select + box select; snap-to-surface
    placement; array/duplicate tool; align/distribute; "apply instance overrides to
    prefab"; in-place prefab editing; drag a `.glb` onto the window to import (writes the
    manifest entry); command palette (Ctrl+K) mirroring every menu (same commands an AI
    would issue); multi-scene tabs.
12. **Play in editor** (M). Run the scene player inside the Game view (same process,
    `ScenePlayer` already exists in `apps/pc`) for instant iteration; keep the separate
    process for crash isolation of real game code.
13. **Emulators in CI** (M). Boot ares/Azahar headless in CI for the `TRINO_TEST_*`
    self-tests; add mupen64plus golden screenshots + the VI stage (dedither/divot) the
    N64 look still lacks.

## P2 — later, when the game asks for it

14. **Save data** in the `Platform` trait (EEPROM/SRAM on N64, `fs` on 3DS, files on PC).
15. **Positional audio** (pan/attenuation from scene audio sources) and music stingers.
16. **Smooth skinning** (two-weight blend per vertex) — only with profiling numbers; the
    rigid model is the N64-era look and much cheaper.
17. **Portals / cells** for interiors (per-room visibility) when a level busts the budget
    even with LOD.
18. **Standalone projects**: publish engine crates and make `cargo xtask new` work outside
    this repo (templates already ship AGENTS.md).

## Engineering hygiene

- Console performance numbers: `xtask/tests/bench_tnl.rs` measures host CPU only; add an
  N64 self-test that times a fixed castle64 frame via the RCP counter and prints it over
  ISViewer, so T&L changes are judged on the real ceiling.
- Editor tests: the doc/undo and gizmo math are unit-tested; add a headless egui test
  harness (egui_kittest when it supports our egui) for panel flows.
- Keep `docs/adr/` current: every format change gets a version bump + migration + ADR line.
