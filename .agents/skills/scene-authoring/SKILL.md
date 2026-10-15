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
    // Shapes: Box(size: (x, y, z)) (FULL size), Sphere(radius), Capsule(radius, height).
    collider: Some((shape: Box(size: (1.0, 1.0, 1.0)), offset: (0.0, 0.5, 0.0))),
),
(name: "coin", collider: Some((shape: Sphere(radius: 0.5), trigger: true)), tags: ["coin"]),
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
- **Coplanar surfaces z-fight on the N64.** Keep decals and water several cm above the
  ground (16-bit z); the showcase pond sits at 10 cm.
- **Portal visibility:** a camera outside every cell sees everything. Nodes outside
  all cells (`NO_CELL`: walls, sky, exterior) always draw.
- **Up to 64 cells** per scene.

See `scenes/interior.scene.ron` for a working portal example.

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
     behind its `cfg` in the game.
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

- Logical paths, not file paths: `models/crate`, `materials/wood`, `prefabs/tree`.
- Rotations are degrees in files, radians in `trino_core`.
- Removing/renaming a field is a format change: bump the version and add a migration
  (`Scene::from_ron`). Adding an optional defaulted field is not.
- The editor guards unsaved work: if you edit a scene the editor has dirty, it warns
  instead of reloading. Save in the editor first (or edit while it's clean).
