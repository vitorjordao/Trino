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
| Budget | The **N64 cost estimate** `model inspect` prints (CPU T&L ≈ 7 µs/vertex + 17 µs/triangle + 0.18 ms/draw, + ~37 µs per visible triangle) — `model inspect` no longer prints a triangle-cap percentage | A 30 fps frame affords roughly 550–650 triangles of CPU T&L today, fewer with many draws: props tens of tris, characters well under 100 (castle64's boar: 70 tris ≈ 2.5 ms; the player is 5 draws, 138 tris ≈ 5 ms of T&L). Heavier art needs a `lod` level, or waits for RSP T&L (BACKLOG). |
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
| `color_tools.py` | `Painter` (per-face colors by 3D rules: `fill`, `paint`, `gradient`, `noise`, `ambient_occlusion_floor`, `commit`), `paint_slots` (`_SLOT` color slots), `palette_from_materials`, `report` |
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

Each one costs ~17 µs per vertex plus ~26 µs per triangle. Back faces have been
rejected before clipping since 2026-10-13, but they still pay for their vertices and
the test.

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
- **Count vertices in the GLB (`model inspect`), not the transforms.** The glTF exporter
  shares a flat quad's 4 vertices fine (boar: 140 vertices for 70 triangles).
  `perf_budget`'s `verts` counts vertex-cache misses. Those were 210 (3 per triangle)
  until the bake started reordering for the cache, and the same file then dropped
  to 140.

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

### Color slots: one model, many color schemes

A character drawn many times in different colors (skin, hair, top, bottom) is ONE
model. Mark the recolorable regions with color slots, and the game passes the colors
per draw (`ModelParams::palette`).
- **Slots:** 0 keeps the baked color; 1..4 multiply it by palette entry slot-1, rounded
  `(a·b + 127) / 255`, before tint and lighting. The palette's alpha is ignored.
- **Paint slot regions white in COLOR_0**, with their AO or shading painted in. White
  times the entry gives exactly the palette color, and darker paint gives it shaded.
  Anything that must not change (eyes, shoes, a logo) stays in slot 0 with its real
  color.
- **Mark them** with `ns["paint_slots"](name, [(rule, slot), ...])`. It writes the INT
  FACE attribute `_SLOT` by the same 3D rules as `Painter`, and later rules win.
- **Keep the FACE domain.** The exporter then splits vertices on slot borders. A shared
  POINT-domain vertex has one slot for all its faces, so Gouraud blends the palette
  color into the neighbor region.
- **Export** with `export_glb`, which sets `export_attributes=True`. Without that option
  Blender drops every custom attribute.
- **Verified on Blender 5.2.2** (`cargo xtask blender smoke`, gate `color_slots_export`):
  INT, INT8 and FLOAT on POINT, INT on FACE and FLOAT on CORNER all exported as a
  float SCALAR `_SLOT`. The bake rounds floats and rejects values outside 0..4 with
  the file name.
- **Gate:** `model inspect` prints `color slots: N vertices keep their color; slots 1-4
  have a / b / c / d`. In the JSON it is `slot_vertices` (`null` without slots).
- **Cost:** a palette draw always runs the CPU T&L on the N64, never the RSP. On a
  crowd that is the bigger cost; the per-vertex multiply itself is small. Budget the
  crowd with `model inspect`'s per-draw estimate.

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
- Gate: `cargo xtask model inspect <file> --anim [--json]` measures every clip on the
  engine's own posed vertices: **stride speed** and distance per cycle (move the
  character at that speed so its planted feet stay put), **foot slide** (a planted foot
  drifting against that speed in one frame; warns over 2 cm), **lowest point** (warns
  under −0.5 cm) and the feet it found. The editor's **Animation** tab shows the same
  numbers live, with onion skin and a crossfade. A planted foot is the lowest one on the
  floor in a frame; a looping clip's last frame must repeat its first.
- Seen on the engine's sample robot: its walk swings both legs like a pendulum, so the
  "planted" foot travels back and then forward again on the floor (15 cm slide) and dips
  3.6 cm under it — the measurement is right; the art needs a real stance phase.
- **Root motion** (issue #65): a walk that moves its root (the hips) forward with the
  feet planted bakes with `root_motion = ["walk"]`; `model inspect models/x --anim` then
  reports the slide carried by the track (0 for planted feet) beside the constant-speed
  one. Give the root a non-constant speed only if the gait has one; a constant-speed walk
  needs no root motion. The generated `models/strider` (`xtask/src/showcase_assets.rs::
  strider`) solves its legs per key by the law of cosines — the recipe for planted feet.
- **Foot IK**: name the leg joints so the manifest can list them (`ik = { pelvis = "hips",
  left = ["thigh_l", "shin_l", "foot_l"], right = [..] }`, glTF node names); each leg must
  descend hip → knee → foot, and the ankle is the foot joint's origin.
- A console that should skip the skeleton: `cargo xtask model pose models/x --clip walk
  --time 0.3 --name x-pose` (or Animation › Console pose) bakes a static glTF of that
  pose and declares it; point the entity's N64 variant at it (`platforms`).

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
   anim_compression = true            # optional: ANMC tracks, far less console RAM
   events.walk = [{ at = 0.0, name = "foot_l" }]   # optional: timeline events
   ```
   Clip logic (which clip when, crossfades, blend by speed) is an animation graph:
   `cargo xtask anim new robot --model models/robot`, then the `animation` skill.
   Replacing a fake object: change only its `file` (or use the editor's *Replace with real
   art* on the model) — the logical path, every scene entity, material and line of code stay.
2. Editor: the model appears in **Assets › Models** with a thumbnail; drag it into the Scene
   view; pick the clip in the Inspector (*Animation*) and preview with the film-strip toggle.
   Give it a material (`Flat` for the faceted look) — materials are text files too.
3. `cargo xtask model inspect` again if anything changed; `cargo xtask test` before commit.
4. Game code: `ModelId::from_path("models/robot")`, pose with
   `ModelParams { anim: Some(AnimRef::new(ClipId::from_name("walk"), time)), ..ModelParams::DEFAULT }`
   — crossfade idle→walk with `AnimRef::new(idle, t).blend_to(walk, t, weight)`.
   Color slots: `ModelParams { palette: Some([skin, hair, top, bottom]), ..ModelParams::DEFAULT }`.
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

## Lessons from an external project's asset set (2026-10-14, 64 models + 38 console variants)

- **Export and inspect:**
  - export with `export_materials='NONE'` (and `export_vertex_color='ACTIVE'`): Trino
    multiplies COLOR_0 by the material base color, so PBR master materials come out
    double-colored;
  - `inspect` run from inside Blender finds the system cargo: put `~/.cargo/bin` first
    on PATH.
- **QA renders:** Workbench STUDIO light plus Blender 5's AgX view transform darken and
  tint the colors (white cloth renders gray). Use the Standard view transform, FLAT
  light, and a temporary color attribute that applies Trino's formula
  `ambient + (1 − ambient)·max(0, n·l)`.
- **Baked light** (pre-lit meshes drawn `Unlit`, e.g. a roof that is stretched
  non-uniformly): multiply the light factor in display (sRGB) space, then convert to
  linear for COLOR_0. Trino lights display bytes.
- **Geometry:**
  - the "buried face" culling in `castle64_models.py` deletes recessed geometry (a sink
    basin, planter soil): give boxes an opt-out (`occlude=False`);
  - cards and rotated slivers dip a few millimetres below y = 0: snap anything within
    5 mm up to the floor;
  - coplanar details (hair on a head, papers on a desk) z-fight on the N64: stand them
    about 2 cm proud, or cut them into the face;
  - an octagon wheel's lowest point is `r·cos(22.5°)` below its center;
  - world positions are stale until `view_layer.update()`.
- **Code and checks:**
  - `castle64_models.py` runs `main()` on import, so it can't be used as a library;
    copy its Model class;
  - two coincident opposite faces read red in the orientation check: offset them 2 mm.
- **Console variants:** the pipeline resolves `assets/<platform>/<same path>` over the
  master, so low-poly N64/3DS versions need no game code. In one 64-model set they
  cost 44% fewer triangles.


## Lessons from Solid Eleven (2026-10-16, issue #39)

- **A small prop held in a hand (2026-10-19, the referee's card):** draw it at the
  same anchor as the linesman's flag (`FOREARM_R`, 0.3 m along it; `ARM_R`, 0.6 m on
  the consoles) and build it out along the arm's −y. A thin card is seen edge-on
  from the broadcast camera: turn it about the arm (40°) and make it larger than
  life (16 × 22 cm) or it reads as a stick. White vertex colors, tinted per draw
  (yellow or red), 12 triangles: it stays on the RSP. Force the body's arms
  (`Detail::Near`) while it shows: the far console body has none (`Detail::Far` poses
  `mask::NO_ARMS`), and the broadcast camera already shows the referee far;
  `solid_eleven_budget::the_referees_card_stays_on_the_rsp_near_and_far` fails
  without it.

- **One generator, two tiers:** `tools/blender/solid_eleven_models.py`
  (`cargo xtask blender solid-eleven [--only a,b]`) builds console and PC models from
  one table. Keys tagged `@pc` are written as `<name>_hd` and listed with
  `platforms = ["pc"]` in the manifest, so the consoles never load them. `--only`
  takes those `_hd` names.
- **Rigid parts posed from a clip table instead of a skinned rig:** the players are 7
  parts on the consoles (11 on PC), each drawn with a matrix the game computes from
  `render/clips_gen.rs`. `tools/blender/solid_eleven_anim.py` holds the clip table, which
  rigs the Blender preview and writes the Rust table (`--rust`, and `--check` in
  `solid_eleven_layers`). One source keeps the art and the game in step, and a part
  costs one draw with no skinning.
- **Big flat ground under a low camera:** split it into coarse far tiles and fine
  near tiles. One 20 m quad under the camera can leave the N64's RSP (clip band).
- **Rigid parts need a fit test, not eyeballing** (iteration 2: legs out of the shorts,
  arms out of the sleeves, a floating head, all worse in motion). The body is a table
  (`BODY` in `solid_eleven_anim.py`: frustums with skipped faces, a tint, tiers), so
  `render/fit.rs` can pose it through every clip and crossfade and check five rules:
  each part stays attached, its plug (the end inside the parent) stays hidden, no
  holes, nothing sinks into a neighbor of another tint, feet on the turf. Put a joint's
  pivot at the center of the parent's opening (the head pivots at the neck top), make
  the plug a frustum that reaches deeper than the parent's rim at the widest bend, and
  close every face a camera can see in some pose (the boot's back, the hair).
- **The fit test never judges two solids on one joint against each other,** so a part
  that crosses another on its own joint passes unseen. Solid Eleven's hair cap and skull
  were two solids of one head: the forehead and temples showed through the hair for two
  iterations. Make anything that must cover another surface on the same bone (hair,
  a cap, a collar) a part of its own, built **around** the surface: the surface's own
  solids pushed out by a margin (a frustum's sides stay parallel, so the gap holds over
  the top), plus locks that stand off its widest section. Then check it statically
  (`fit.rs` rule 6: every outer face ≥ 2 mm off) and pose every variant through the
  clips at once, with variants never judging each other (`Body::judges`). Look at
  `heads_<tier>.png` / `hair_lineup_<tier>.png` from `solid_eleven_poses head_closeups`.
- **A taper that narrows to a waist needs whatever rides inside it narrowed too:**
  the shorts taper in at the top, and a straight shorts leg on the thigh poked 9 mm out
  of them at the hip (`fit.rs::the_shorts_legs_stay_inside_the_shorts_sides`).
- **Ground every key by forward kinematics, per tier:** the generator computes each
  key's root height from the lowest point of the posed body, once with the PC body
  (`root`) and once with the console body (`lo`); the game also plants the feet per
  frame from generated contact points. Hand-written root heights floated or sank.
- **Lying clips need a pelvis roll,** or the torso goes through the turf; give falls
  and slides a get-up clip, and route a fall to it instead of crossfading to a run.
- **A pose can pass every fit rule and still look wrong:** the slide's hips sat 0.45 m
  up on a straight leg, a flying kick, with nothing poking through. Look at the pose
  sheets (`cargo test --release -p xtask --test solid_eleven_poses -- --ignored`) of
  both bodies: the consoles' legs have no knee, so a folded leg sticks up into the air
  there. Search a pose with the generator's FK (`lowest`, `frames_of`) against what
  should touch the turf, on both tiers, then fit-test it.
- **Generators that write Rust read and write with `encoding="utf-8", newline="\n"`.**
  Windows Python defaults to cp1252. CI's Windows job runs the clip table's
  `--check`, which then saw the em dash in the generated header as drift, on a file
  identical on disk.
- **Merge what's always together:** the advertising boards became part of the stands
  (fewer draws; on the N64, draw count costs more than triangles).
