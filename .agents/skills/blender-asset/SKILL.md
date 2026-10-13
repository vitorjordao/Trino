---
name: blender-asset
description: Build a low-poly 3D asset in Blender for Trino — static props or animated objects/characters — from blockout to a validated .glb in the manifest. Covers Trino's real limits (vertex colors, rigid skeleton, 32 joints, N64 budgets), vertex-color painting, rigid-part animation, QA renders, glTF export and the `cargo xtask model inspect` acceptance gate. Use when modeling, coloring, rigging, animating or exporting any model for a Trino game, or when replacing a fake object with real art.
---

# blender-asset: Blender → Trino

Every rule here comes from the engine's actual bake (`crates/asset-pipeline/src/model.rs`)
or from production lessons. **Follow the stages in order and do not skip the gates.**
If you learn something new, update this skill and `tools/blender/` in the same session.

## What Trino can and cannot draw (read first)

| Topic | Trino reality | Consequence in Blender |
|---|---|---|
| Color | **Vertex colors** by default. Bake priority per vertex: `COLOR_0` → base-color **texture sampled at the vertex UV** (nearest) → base-color **factor**, always × factor. **Textured models** (manifest `texture = "sprites/x"`): UVs (`TEXCOORD_0`) are kept and one small texture is sampled at runtime, modulated by the vertex color | Default: detail = geometry or per-face color; a palette atlas (flat color cells) works. Textured: one power-of-two texture per model, ≤ 4 KB on N64 (32x32 RGBA16, 64x64 CI4) — tiling surfaces (walls, floors, crates), not painted 512 px skins. |
| Lighting | Engine-side, per vertex: `Gouraud`, `Flat` or `Unlit` materials (`assets/materials/*.material.ron`) | Model for the faceted look; the material (not Blender) decides shading. |
| Animation | **Rigid skeleton**: each vertex follows exactly one joint. Skins: heaviest weight wins. No skin + animations: **every scene node is a joint** | Rigid parts (one object per moving part) or an armature with weight 1.0 per vertex. Blended weights crack at joints — or opt into `skinning = "smooth"` in the manifest (two joints per vertex, ~3% more CPU per draw; `model inspect --smooth` reports blended vertices) for meshes that must bend (tails, cloth strips). |
| Limits | ≤ **32 joints**, **one skin**, ≤ **65 535 vertices**, triangles only, no morph targets, CUBICSPLINE tangents dropped | Merge static parts into their parent; LINEAR keys. |
| Budget | The **N64 cost estimate** `model inspect` prints (CPU T&L ≈ 9 µs/vertex + 44 µs/triangle + 0.4 ms/draw, + ~30 µs per visible triangle) — not the `Caps` triangle percentage, which understates it ~10× | A 30 fps frame affords ~200–300 triangles of CPU T&L today: props tens of tris, characters ~100–150 (castle64's boar: 120 tris ≈ 9.7 ms). Heavier art needs a `lod` level, or waits for RSP T&L (BACKLOG). |
| Format | glTF 2.0 (`.glb`), Y-up, meters | Export with the exporter's Y-up option; 1 Blender unit = 1 m. |
| Facing | **Blender −Y = Trino +Z** (verified by `cargo xtask blender smoke`) | Model the front looking at Blender's −Y (Front view, numpad 1, looks at it). |
| Clip timing | Each clip starts at its **first keyframe** (Blender exports from frame 1 = 1/24 s; the bake removes the offset) | Key from frame 1 (or 0); close loops by repeating the first key `length` frames later. |
| Clips | Sampled at manifest `anim_fps` (default 15); loop unless listed in `once` | Close loops (last key = first). Name actions/NLA tracks exactly like the clips the game uses. |

castle64's characters and doors are built by a Blender script,
`tools/blender/castle64_models.py` (`cargo xtask blender castle64`), with only the faces
a camera can see. It is the reference for **budgeted static props and rigid parts**:
- the hand-rebuilt KayKit doorway: 1068 → 75 triangles;
- the boar, the stage doors and the player parts.

Reference models built exactly this way (generated in `xtask/src/showcase_assets.rs`):
`assets/shared/models/showcase/robot.glb` (body → head/arms/legs, clips `idle`, `walk`,
`wave`) and `windmill.glb` (tower + rotor, clip `spin`). Inspect them first:
`cargo xtask model inspect assets/shared/models/showcase/robot.glb --fps 20 --once wave`.

## Tools (`tools/blender/`)

Load once per Blender session (functions persist in `driver_namespace` between MCP calls):

```python
import bpy, os
ns = bpy.app.driver_namespace
ns["SCRATCH"] = r"<a scratch folder>"       # QA renders go here
ns["REPO"] = r"<path to the Trino repo>"
for f in ("qa_render.py", "color_tools.py", "rig_tools.py", "trino_export.py"):
    exec(open(os.path.join(ns["REPO"], "tools", "blender", f), encoding="utf-8").read(), ns)
result = {"ok": True}   # some Blender MCP servers require `result` to be a dict
```

| Script | For |
|---|---|
| `qa_render.py` | `shot`, `sheet`, `turntable`, `clip_sheet`, `orient_mode` (renderable face orientation), `vertex_color_mode` |
| `color_tools.py` | `Painter` (per-face colors by 3D rules: `fill`, `paint`, `gradient`, `noise`, `ambient_occlusion_floor`, `commit`), `palette_from_materials`, `report` |
| `rig_tools.py` | `part_report` (joints vs 32), `set_pivot`, `key_rot`, `key_loc`, `make_clip`, `rigid_check`, `snap_rigid`, `bake_to_deform` |
| `trino_export.py` | `stash_clips` (one NLA track per clip), `export_glb` (version-tolerant options), `inspect` (runs the engine's bake) |

> **Status:** validated end to end on **Blender 4.5.14 LTS and 5.2.2 LTS** (headless) by
> `cargo xtask blender smoke` (`tools/blender/smoke_test.py`): a static prop and a
> rigid-part character built from scratch with these tools, exported, and gated by the
> engine's bake (triangles, size, joints, clips, loop flags, facing), plus QA renders
> (turntable, orient mode, clip sheet) in `target/blender_smoke/`. The command skips
> cleanly where Blender is not installed (`$TRINO_BLENDER`, PATH, `~/.local/opt/blender*`).
> Exporter option names drift between versions — `export_glb` drops unknown options and
> reports them in `skipped_options` (4.5: only the legacy `export_colors`).
> **`inspect` and the editor's Scene view are the gates that count.**

Fixed while validating (keep in mind on other versions): after `stash_clips` and after
`export_glb` the parts are put back in their **rest pose** (`restore_rest`) — before, the
viewport and QA renders kept the last sampled pose; `clip_sheet` plays the clip on **every**
part that has a `<clip>__<object>` action, not only the object passed.

Working with an AI agent: any Blender MCP server that can run Python works
(`execute_blender_code`-style tools). Without MCP, run the same snippets in Blender's
Python console or `blender --background file.blend --python script.py`.

## Stage 0 — Inspect and plan

1. List objects, vertex/face counts, modifiers, materials, color attributes, armatures,
   actions. `ns["report"]("Obj")`, `ns["part_report"]("Root")`.
2. `turntable` + close-ups. Note **where the object faces**: models face **+Z in Trino**
   = **−Y in Blender** (verified: `cargo xtask blender smoke`, gate
   `facing_minus_y_is_plus_z`) — and its right side.
3. If it replaces a **fake object** (`assets/shared/models/proxies/*.prim.ron`), read the
   proxy's dimensions: the real model should occupy the same box so gameplay keeps working.
4. Decide: **static**, **animated prop** (rigid parts), or **character** (rigid parts or
   armature). Write the clip list with names and durations.

## Stage 1 — Mesh

- Apply Mirror (or let the exporter apply modifiers), merge by distance (1e-4), recalc
  normals outside. Gate: 0 non-manifold edges on closed parts, no stray vertices.
- Low-poly faceted style: details that are just color go to the color stage, not geometry —
  **anything that must read in-game needs geometry, face color or (textured models) the
  one small tiling texture**.
- Moving parts as **separate objects** with the origin at the pivot (`set_pivot`): shoulder,
  hip, hinge, axle. Static decorations: join them into their parent (Ctrl+J) to save joints.
- Scale: real meters (`proxies/hero-capsule` is 1.7 m). Root origin at the base (y = 0).
- Budget gate: `report` triangles; keep repeated props small.

### Pay only for faces a camera can see (N64)

The CPU T&L transforms and tests every triangle you ship, including:
- back faces, which are culled only after the transform;
- faces buried inside another part, which only the z-buffer hides.

Each one costs ~9 µs per vertex plus ~44 µs per triangle.

What worked on castle64 (`tools/blender/castle64_models.py`):
- **Drop buried faces:** a box face lying entirely inside another box goes. Parts may
  still interpenetrate, because every target z-buffers.
- **Drop faces no camera reaches:**
  - soles and bases on the ground;
  - the back of a prop set against a wall;
  - a creature's underside when the camera always looks down on it.

  Check with `orient_mode`. Red is fine only where no camera goes.
- **Rebuild heavy kit pieces by hand** at the same extents, origin and palette. The
  KayKit doorway (`door_kaykit.glb`) was 1068 triangles and cost ~64 ms per frame for
  ~20 px of screen. `door_castle.glb` is 75 triangles and looks the same in game (0.16%
  of pixels changed across nine views).
- **Results:**

  | Model | Before (tris) | After (tris) |
  |---|---|---|
  | boar | 120 | 70 |
  | stage door | 60 | 44 |
  | torso | 60 | 50 |
  | arm | 24 | 22 |
  | leg | 24 | 22 |

  The old boar's mane box was buried inside its body: 12 triangles nobody ever saw.

Traps, measured:
- **A boolean union of the parts makes it worse.** It cuts every face another part
  passes through, and the triangulation pays for the holes: the boar went 120 → 184
  triangles.
- **Blender's n-gon fill can mangle a concave outline.** A wall face with an arch
  notch came out with back-facing triangles across the opening. Build concave faces
  from convex pieces (fans from a corner), and check `orient_mode` from the front.
- **Flat-shaded faces export 3 vertices per triangle**: the glTF exporter shares no
  vertex even inside a quad (boar: 70 triangles → 210 vertices). Give every polygon
  its own vertices and mark the faces **smooth**. The normals are then exactly the
  face normal, and a quad exports 4 vertices.

## Stage 2 — Color

- Paint by **3D rules** with `Painter` (position, angle around the body axis, height
  gradients), never by UV. Region borders are smooth curves; continuous features (belts,
  stripes, mouths) go all the way around — check front, side **and from below**.
- Multi-material kits (Kenney/Quaternius/KayKit): `palette_from_materials` bakes material
  colors into the color attribute; or keep a palette texture and rely on the vertex-UV
  sampling rule (each face's UVs inside one flat cell).
- Gate: `vertex_color_mode(True)` + `turntable` — this is what Trino shows before lighting.
- **COLOR_0 is linear** (glTF spec). Blender exports the color attribute linear.
  Since 2026-10-13 the bake converts it back to display sRGB bytes
  (`color0_to_display`; Trino vertex colors are display bytes, like texels). Before
  that, every Blender-painted model came out much darker in game: (200, 120, 80) →
  (147, 48, 20). `Painter` already stores linear values, and Trino's own generators
  now write linear `f32` colors too.
- Don't export materials over vertex colors. The bake multiplies COLOR_0 by the
  material's base-color factor, so the color would apply twice. After
  `palette_from_materials`, clear the material slots, or write the colors directly as
  `castle64_models.py` does.

## Stage 3 — Rig (animated only)

- **Rigid parts:** hierarchy root → parts. `part_report` must say `ok` (≤ 32 nodes).
- **Armature:** auto weights, then `rigid_check` → `snap_rigid` so Blender previews the
  exact in-game result (heaviest group wins). Fix cracks with geometry (an extra loop that
  lets the seam hide inside a joint) or by moving the boundary to a natural seam.
- One skin per file. Non-deform helper bones (IK, poles) are fine while animating;
  `bake_to_deform` before export, and mute constraints afterwards.

## Stage 4 — Animate

- One clip = one action per animated object (`make_clip(name, objects, length, keys)`),
  LINEAR keys, loops closed (last key repeats the first). Then `stash_clips(clips, objects)`
  so the glTF exporter writes **one animation per clip name** merged across parts.
- Keep timing readable at 15–20 fps sampling (`anim_fps` in the manifest).
- Measure what must touch: lowest vertex ≈ 0 on support frames (no floating, no sinking).
- Gate: `clip_sheet` for every clip (side + 3/4, evenly spaced frames).

## Stage 5 — QA gates (mandatory)

1. `orient_mode(True, name)` renders from 6+ angles (incl. top and bottom): **no red**
   visible from outside — Trino culls back faces, red = a hole in game. (Thin cards that
   must show both sides: use a `double_sided: true` material instead of flipping faces.)
2. `turntable` with vertex colors, close-ups of every distinctive part.
3. Animated: `clip_sheet` per clip; `rigid_check` (armature) shows 0 blended vertices.

## Stage 6 — Export and accept

```python
ns["export_glb"](["Robot"], "assets/shared/models/robot.glb")
ns["inspect"]("assets/shared/models/robot.glb", fps=20, once=["wave"])
```

The bake reorders triangles and vertices for the engine's 32-slot vertex cache
(`MeshData::optimize_vertex_order`), so exporter order doesn't matter. Before that,
Blender's order missed the cache on every reference: 210 transforms for 140 vertices.
`model inspect` reports vertices; `perf_budget` reports the transforms (`verts`).

Gate (all must hold, from the `inspect` JSON): `ok: true`; triangle count as planned;
`joints` = expected parts; every planned clip present with the right duration and loop
flag; `size` matches the real-world size (or the fake object it replaces); no warnings
about the origin.

## Stage 7 — Bring it into the game

1. Manifest (`assets/manifest.toml`):
   ```toml
   [models.robot]
   file = "models/robot.glb"
   anim_fps = 20          # optional (default 15)
   once = ["wave"]        # optional: clips that play once
   ```
   Replacing a fake object: change only its `file` (or use the editor's *Replace with real
   art* on the model) — the logical path, every scene entity, material and line of code stay.
2. Editor: the model appears in **Assets › Models** with a thumbnail; drag it into the Scene
   view; pick the clip in the Inspector (*Animation*) and preview with the film-strip toggle.
   Give it a material (`Flat` for the faceted look) — materials are text files too.
3. `cargo xtask model inspect` again if anything changed; `cargo xtask test` before commit.
4. Game code: `ModelId::from_path("models/robot")`, pose with
   `ModelParams { anim: Some(AnimRef::new(ClipId::from_name("walk"), time)), ..ModelParams::DEFAULT }`
   — crossfade idle→walk with `AnimRef::new(idle, t).blend_to(walk, t, weight)`.
5. Textured model: add `texture = "sprites/<tile>"` to its `[models.*]` entry (UVs from
   `TEXCOORD_0`), declare the sprite (`formats = { n64 = "CI4" }` or `CI8`), or set
   `texture` in a material to swap it per entity.

## Variants

- **Static prop** (crate, rock, door): stages 0, 1, 2, 5, 6, 7. One object or a merged
  hierarchy — without animations every node merges into one mesh.
- **Animated prop** (windmill, door hinge, elevator): a static body + 1–3 moving parts as
  child objects with pivots at the axes; one clip per behavior. See `windmill.glb`.
- **Character / creature**: rigid parts (N64-authentic, see `robot.glb`) or an armature with
  `snap_rigid`. Idle, walk/run (speed matched to the game's movement speed), action clips.

## Closing

- Record what went wrong and how it was fixed in this skill (or in the game's AGENTS.md).
- Report: what changed (mesh/colors/rig/clips), the `inspect` numbers, what is left, and how
  to see it (editor Scene view, `TRINO_SCENE=... cargo run -p trino-app-pc`).
