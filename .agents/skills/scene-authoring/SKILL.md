---
name: scene-authoring
description: Create or edit Trino scenes, prefabs, materials and fake objects as text (RON) — the same files the editor reads and writes, hot-reloaded both ways — then validate with `cargo xtask scene check` and ship to consoles with `cargo xtask scene export`. Use for "build a level/scene", "place objects", "make a prefab", "change a material", or any edit an AI does without clicking in the editor.
---

# scene-authoring (text-first)

Everything the editor authors is a human-readable text file. Edit them directly; a running
editor (and a running `TRINO_SCENE` player) reloads them live. Formats are documented in
`crates/scene/src/lib.rs` (versioned; old versions migrate on load).

| File | Logical path | Purpose |
|---|---|---|
| `scenes/<name>.scene.ron` | — | entity tree + environment |
| `prefabs/<name>.prefab.ron` | `prefabs/<name>` | reusable linked entity tree |
| `assets/materials/<name>.material.ron` | `materials/<name>` | surface (tint, shading, emissive, sides) |
| `assets/shared/models/proxies/<name>.prim.ron` + manifest | `models/<name>` | fake object (see `add-asset`) |

## Scene (v3)

```ron
(
    version: 3,
    name: "level-1",
    environment: (                        // optional, all fields defaulted
        clear: (132, 178, 226),
        camera: (eye: (9.0, 7.0, -13.0), target: (0.0, 1.0, 0.0), fov: 55.0),
        light: (dir: (0.45, -0.75, 0.48), color: (255, 244, 225), ambient: 0.42, ambient_color: (190, 210, 255)),
        fog: Some((color: (132, 178, 226), near: 16.0, far: 42.0)),
        // Screen-space AO on the PC (consoles: bake `ao` into the models, add-asset).
        // Every field defaults: `ao: Some(())` works; `enabled: false` keeps the tuning.
        ao: Some((radius: 0.6, intensity: 1.2, bias: 0.025, quality: High)),  // Low|Medium|High = 8/16/32 samples
        // Shadows on every target (scene v7, ADR 0025): blobs on the N64, planar on the
        // 3DS, a map on the PC/Web. Every field defaults: `shadows: Some(())` works.
        shadows: Some((tier_max: Map, distance: 40.0, softness: 160, color: (0, 0, 0, 140), map_size: 0)),
        // Stereo 3D on the 3DS top screen (ADR 0032; no version bump). Every field
        // defaults: `stereo: Some(())` works. Convergence = the view depth on the
        // screen's plane (put it on the player); separation ≈ convergence / 20.
        stereo: Some((enabled: true, max_separation: 0.06, convergence: 4.0, policy: Auto, budget_us: 16667)),
    ),
    entities: [
        (
            name: "crate-a",
            transform: (pos: (-3.0, 0.0, 2.0), rot: (0.0, 15.0, 0.0), scale: (1.0, 1.0, 1.0)),  // rot in DEGREES
            tags: ["breakable"],
            model: Some((path: "models/crate", material: Some("materials/wood"), tint: Some((255, 220, 200, 255)))),
        ),
        (
            name: "robot",
            model: Some((path: "models/robot", anim: Some((clip: "walk", speed: 1.0)))),
        ),
        (
            name: "tree-1",
            transform: (pos: (8.0, 0.0, 5.0), rot: (0.0, 10.0, 0.0), scale: (1.2, 1.2, 1.2)),
            prefab: Some((path: "prefabs/tree", overrides: [(target: "leaves", material: Some("materials/autumn"))])),
        ),
        (name: "spawn", transform: (pos: (0.0, 0.0, -6.0), rot: (0.0, 0.0, 0.0), scale: (1.0, 1.0, 1.0)), tags: ["spawn"]),
        (name: "group", children: [ /* nested entities; transforms are parent-relative */ ]),
    ],
)
```

- World is Y-up meters. Sprites (`sprite: Some((path: "sprites/x"))`) are 2D screen-space:
  `pos.x/pos.y` are game pixels, rotation is `rot.z`.
- `visible: false` hides an entity and its children. Omitted fields take defaults.
- Entities with no model/sprite/prefab are **markers** (spawn points, triggers, waypoints):
  game code finds them by name or tag.
- v2 files load and save as v3 (v3 added `platforms`). Prefabs are v2 (v1 loads).
- **Stereo (issue #63):** `environment.stereo` reaches the game as codegen's `STEREO`
  (pass it to `set_stereo` right after `begin_frame`). TSCN carries none, so a baked
  scene's host sets it. `cargo xtask scene budget <scene> --platform 3ds --stereo`
  prints both eyes' cost. The editor's Game view (3DS) previews it as an anaglyph or
  side by side with a simulated 3D slider.
- **Asset groups (scene v6, issue #56, ADR 0022):** `requires: ["boot", "level1"]` (the
  default is `["boot"]`, omitted when so) names the groups the scene needs loaded, and a
  cell can stream its own: `vis: Some(Cell(size: (8.0, 4.0, 8.0), groups: ["cellar"]))`.
  The game loads `REQUIRES` (codegen) before the scene and lets `trino_core::CellGroups`
  (`Resolved::cell_streamer`) keep the cells in view, and a doorway further, loaded.
  `cargo xtask assets check` fails when a scene draws an asset none of its groups load
  (`cargo xtask assets group <asset> <group>` moves it), and checks each tier's RAM.
  Versions 1–5 migrate to 6 on save.

## Per-platform variants (`platforms`)

One scene, three looks — no `cfg` in game code. A platform without a variant draws the
entity as written ("same as PC"). The 3DS field is `n3ds` (RON names can't start with a
digit).

```ron
(
    name: "tree",
    model: Some((path: "models/tree", material: Some("materials/leaf"))),
    platforms: (
        n64: Some((model: Some("models/tree-low"), material: Some("materials/leaf-flat"), scale: Some((0.9, 0.9, 0.9)))),
        n3ds: Some((visible: Some(false))),     // hidden there (subtree too)
        // pc: Some((disabled: true)),          // not on PC at all: no draw, collider or node
    ),
),
```

- `model` / `material` replace the `model` component's; `scale` replaces the scale of this
  entity's own model (or instanced prefab) — its children keep theirs.
- **Variants are a look.** The collider keeps the PC model's bounds and scale (gameplay is
  the same on every target); only `disabled` removes it.
- Who resolves them: the editor's Game view (its N64/3DS/PC profile), Play here,
  `TRINO_SIM=n64 TRINO_SCENE=…` on PC, `cargo xtask scene export --platform n64|3ds`, and
  the `[scenes.*]` bake of each platform (`bake_scene_for`).
- `cargo xtask scene check` resolves every scene for each platform and fails when a
  platform draws a model the manifest doesn't bake for it (`platforms = ["pc"]`): give the
  entity a variant for that console.
- In the editor: Inspector › **Platforms** (PC / N64 / 3DS columns; "= PC" is "same as
  PC"; the N64 column shows the variant's estimated N64 time per draw).

### Hardware tiers: `n64_expanded`, `n3ds_new` (scene v5, prefab v4, ADR 0020)

The N64 with the Expansion Pak (8 MB) and the New 3DS (2 cores at 804 MHz) have keys of
their own. A tier without a variant takes its console's, which takes the base entity:

```ron
platforms: (
    n64: Some((model: Some("models/crowd-low"))),
    n64_expanded: Some((model: Some("models/crowd-mid"))),   // the Pak's RAM
    n3ds_new: Some((scale: Some((1.0, 1.0, 1.0)))),          // else the 3DS's
),
```

- The same keys go in LOD levels (`platforms: (n64_expanded: Some(At(30.0)))`),
  particle caps (`n3ds_new: Some((max: Some(48)))`) and manifest model `platforms`
  (`["n64_expanded"]`: in the ROM, loaded with the Pak only).
- The console's ROM / app holds both resolutions; the runtime loads its tier's. Content
  without tier variants bakes exactly as before.
- Preview: the Game view's "N64 + Expansion Pak" / "New 3DS", `TRINO_SIM=n64x|3dsnew`,
  `scene export|budget --platform n64_expanded|n3ds_new`. `scene check` resolves the
  tiers too (a tier-only model drawn on the base console fails it).

## Gameplay components (all optional)

```ron
(
    name: "crate-a",
    model: Some((
        path: "models/crate",
        // Distance LOD chain: a lighter model (often a fake object), an
        // impostor (2 triangles, `cargo xtask impostor`), then nothing.
        // Each level can start elsewhere per platform, or not be used there.
        lod: [
            (distance: 14.0, path: Some("models/crate-far")),
            (distance: 30.0, impostor: Some("impostors/crate"), platforms: (n64: Some(Never))),
            (distance: 40.0, path: None, platforms: (n64: Some(At(30.0)))),
        ],
    )),
    // Collision for trino_core::physics. `Some(())` = Auto: the model's box.
    // Shapes: Box(size: (x, y, z)) (FULL size), Sphere(radius), Capsule(radius, height),
    // Ramp(size: (x, y, z), dir: PosX|NegX|PosZ|NegZ) (the top rises toward `dir`),
    // Mesh("models/x") (that model's `collision` mesh, ADR 0035). `layer: "pickup"`
    // (written BARE, a name from assets/physics.ron; default layer 0), `one_way: true`
    // (boxes only: jump up through, land on top).
    collider: Some((shape: Box(size: (1.0, 1.0, 1.0)), offset: (0.0, 0.5, 0.0))),
),
// A player's or NPC's body for the engine's CharacterController; origin = feet.
(name: "hero", tags: ["player"],
 character: Some((height: 1.7, radius: 0.3, max_slope_deg: 45.0, step_height: 0.3))),
(name: "coin", collider: Some((shape: Sphere(radius: 0.5), trigger: true)), tags: ["coin"]),
// Shadows (scene v7, prefab v5): cast None|Blob|Mesh|Auto (default Auto: the shape
// where the tier draws shapes, a `radius` m blob on the N64); `receive: true` makes the
// top of the entity's box (collider, else model bounds) a surface shadows land on.
(name: "ground", collider: Some((shape: Box(size: (40.0, 1.0, 40.0)), offset: (0.0, -0.5, 0.0))),
 shadow: Some((cast: None, receive: true))),
(name: "barrel", model: Some((path: "models/barrel")), shadow: Some((cast: Auto, radius: 0.4))),
// Positional looping sound (Ambience plays the nearest sources). On N64 the
// sound needs `loop = true` in the manifest.
(name: "waterfall", sound: Some((path: "sounds/water", radius: 9.0, volume: 0.8))),
// Interior visibility (trino_core::vis): a room is a Cell; everything under
// it belongs to it. A Portal joins two cells by name and should cover the
// doorway opening. Sizes are FULL box sizes around the entity origin.
(name: "kitchen", transform: (pos: (4.0, 2.0, 4.0), rot: (0.0, 0.0, 0.0), scale: (1.0, 1.0, 1.0)),
 vis: Some(Cell(size: (8.0, 4.0, 8.0))), children: [ /* the room's content */ ]),
(name: "door", transform: (pos: (8.0, 1.5, 4.0), rot: (0.0, 0.0, 0.0), scale: (1.0, 1.0, 1.0)),
 vis: Some(Portal(size: (0.4, 3.0, 2.0), a: "kitchen", b: "hall"))),
```

Each component ends up in `SceneNode` / `Entity`, and in TSCN and codegen. Keep these
rules in mind:
- **LOD levels resolve per platform** (`LodDef::distance_on`): `At(d)` moves a level,
  `Never` skips it (the previous level keeps drawing). An impostor level needs the
  impostor baked; otherwise `scene check` warns and that level draws nothing. Games draw a
  chain with `scene::draw_nodes` (or `SceneNode::lod_from(eye).draw(..)`), which handles
  impostor levels. TSCN is v2 since impostor levels (32-byte LOD records): rebake.
- **Blob and planar shadows need a receiver under the caster.** Only the PC map shades
  every surface; on the N64 and the 3DS a caster with no `receive: true` box top below
  its center draws nothing. A big receiver box above the floor (a bridge's collider)
  catches every shadow under it: give such entities a tight collider or no `receive`.
  `cargo xtask scene budget` prints each platform's `shadows:` line (casters, blobs,
  planar, mapped, over budget, receivers).
- **Coplanar surfaces z-fight on the N64.** Keep decals and water several cm above the
  ground (16-bit z); the showcase pond sits at 10 cm.
- **Portal visibility:** a camera outside every cell sees everything. Nodes outside
  all cells (`NO_CELL`: walls, sky, exterior) always draw.
- **Up to 64 cells** per scene. A cell's `groups` stream with it (see Asset groups above; up to 64
  distinct groups).

See `scenes/interior.scene.ron` for a working portal example.

### Post-processing and glow (issue #64, ADR 0033)

- `environment.post: Some((exposure: 1.0, tonemap: AgX, bloom: Some(()), grading:
  Some("luts/dusk"), vignette: 0.2, grain: 0.0))` — every field defaults; `Some(())` is
  the neutral stack. An `Option` field: `Some((...))`, never a bare `(...)` ("Expected
  option" at parse). Codegen writes `POST`; TSCN carries none.
- An entity's `glow: Some((radius: 0.5, color: (255, 220, 160, 200)))` draws a halo of
  `sprites/glow` where the target has no bloom (`mode: Always` for every target);
  codegen and TSCN v6 carry it, `draw_nodes` and `EntityStore::draw` draw it.
- For the consoles: `bake_grading = true` on the scene's manifest entry and `grading` on
  its models; `cargo xtask scene export --platform n64 --bake-grading` for code.

### Accessibility in scenes (scene v8, prefab v6, TSCN v8; issue #96)

- A model's color slots: `model: Some((path: "models/player", palette: [(slot: 1, color:
  Semantic("team.home")), (slot: 2, color: Rgb((20, 20, 30)))]))`. `Semantic` names come
  from `assets/palettes.ron` (an unknown one: a warning, the slot stays white); the
  player's color-vision mode picks the alternative at run time. Slots 1..=4, the mesh's
  glTF `_SLOT`s.
- An entity's high-contrast ring: `outline: Some(())` (yellow, 2 px, the model's bounding
  sphere) or `Some((color: (255, 255, 255), pixels: 3.0, radius: 0.8))`. Drawn while the
  player's high contrast is on; one more unlit draw per part (on the N64's RSP).
- Game code draws both with `trino_core::scene::draw_nodes_with(.., &NodeAccess::new(
  data, &settings, Some(&camera)))`; plain `draw_nodes` keeps the base colors.

### Collision (issue #66, ADR 0035)

- **The worked example** is `scenes/physics_playground.scene.ron`: a walkable 30° ramp,
  a 50° one that slides, stairs of 0.25 m steps, a one-way platform, a mesh dome and a
  trigger on the `pickup` layer.
- **The prim `ramp`** rises toward +Z, so a `ramp` model wants `Ramp(..., dir: PosZ)`
  with `offset: (0, 0.5, 0)` (its pivot is the bottom).
- **Checking without the editor:**
  - `cargo xtask physics check <scene> [--json]` reports overlapping solids, unnamed
    triggers (`Entity …`), unknown layers, mesh colliders without a baked `collision`,
    ramps steeper than any character, and the collision budget per platform;
  - `cargo xtask physics query <scene> --ray x,y,z dx,dy,dz | --drop x,y,z |
    --overlap c h | --cast sphere:r from to` asks what the engine answers;
  - `--drop` settles a character at those feet.
- **Layers:** `cargo xtask physics layers [add|remove|set a b on|off]` edits
  `assets/physics.ron`, the same file as the editor's Project › Physics page.

## Prefab

```ron
(
    version: 2,
    root: (
        name: "lamp",
        children: [
            (name: "post", model: Some((path: "models/lamp-post", material: Some("materials/metal")))),
            (name: "bulb", transform: (pos: (0.0, 2.5, 0.0), rot: (0.0, 0.0, 0.0), scale: (1.0, 1.0, 1.0)),
             model: Some((path: "models/lamp-bulb", material: Some("materials/glow")))),
        ],
    ),
)
```

Instances are **linked** (edit the prefab → every instance follows). Overrides address
children by `/`-joined names below the root (`"trunk/leaves"`) and may set `material`,
`tint`, `visible`. Prefabs may nest; cycles are reported, not followed.

## Loop

1. Edit the files.
2. `cargo xtask scene check` (or `scene check scenes/x.scene.ron`) — parses everything,
   resolves prefabs/materials, prints missing references, prefab cycles, stale overrides.
   Exit code ≠ 0 on problems.
3. Price it per console: `cargo xtask scene budget scenes/x.scene.ron [--platform n64]
   [--json]` — draws, triangles, the N64 frame (CPU vs RSP, with every reason a draw
   stays off the RSP) and draws sent but outside the view, from the scene camera, with
   each platform's variants, LOD levels and impostors.
4. Look at it: open the editor (`cargo xtask editor`, `TRINO_EDITOR_SCENE=scenes/x.scene.ron`)
   or play it (`TRINO_SCENE=scenes/x.scene.ron cargo run -p trino-app-pc`). Screenshots for
   agents: the `editor` skill.
5. Console/game code, two ways (both checked to agree byte for byte):
   - **Code**: export the scene, then `include!` the output (see
     `examples/showcase/src/lib.rs`):
     ```
     cargo xtask scene export scenes/x.scene.ron examples/<game>/src/x_gen.rs
     ```
     The output gives you `NODES`, `CAMERA`, `LIGHT` and `CLEAR`. Add a drift test
     like `xtask/tests/scene_codegen.rs` for every exported scene. A scene with
     variants is exported once per console (`--platform n64 …/x_n64_gen.rs`), each
     behind its `cfg` in the game. Game code (UI, a second screen) goes in its own
     modules next to the generated file, never in it: the showcase's 3DS touch panel
     is `examples/showcase/src/panel.rs`, and it switches the exported `LIGHT`'s fog
     or the camera at draw time instead of editing the data.
   - **Data**: declare `[scenes.x] file = "scenes/x.scene.ron"` in the manifest. It
     bakes to a `TSCN` blob, which you load with
     `BakedScene::parse(platform.blob(BlobId::from_path("scenes/x"))?)`. Changing the
     level then needs no rebuild of game code.

   Then, from either source:
   - spawn live entities with `EntityStore::spawn_nodes` / `spawn_iter`;
   - draw with `scene::draw_nodes(r, nodes, &root, t, eye)`, where `eye` drives LOD;
   - cull interiors with `Cells::from_nodes(..)`, `cells.visible(&camera, screen)`
     and `draw_nodes_in` / `EntityStore::draw_in`;
   - query with `scene::find(NODES, "spawn")` and `scene::with_tag(NODES, "coin")`.

## Pitfalls

- A water material (`water:` in the material file, ADR 0015) needs a mesh with UVs and
  its flow in UVs per second. `scene check` reports out-of-range fields. Games call
  `Renderer::set_water(time, ripples)` every frame; a scene played by `apps/pc` or
  the editor gets the clock and the editor's ripples.

- Logical paths, not file paths: `models/crate`, `materials/wood`, `prefabs/tree`.
- Rotations are degrees in files, radians in `trino_core`.
- Removing/renaming a field is a format change: bump the version and add a migration
  (`Scene::from_ron`). Adding an optional defaulted field is not.
- The editor guards unsaved work: if you edit a scene the editor has dirty, it warns
  instead of reloading. Save in the editor first (or edit while it's clean).
