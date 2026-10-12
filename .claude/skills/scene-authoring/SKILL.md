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

## Scene (v2)

```ron
(
    version: 2,
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

## Prefab

```ron
(
    version: 1,
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
3. Look at it: open the editor (`cargo xtask editor`, `TRINO_EDITOR_SCENE=scenes/x.scene.ron`)
   or play it (`TRINO_SCENE=scenes/x.scene.ron cargo run -p trino-app-pc`). Screenshots for
   agents: the `editor` skill.
4. Console/game code: `cargo xtask scene export scenes/x.scene.ron examples/<game>/src/x_gen.rs`
   and `include!` it (see `examples/showcase/src/lib.rs`): `scene::draw_nodes`,
   `scene::find(NODES, "spawn")`, `scene::with_tag(NODES, "coin")`, `CAMERA`, `LIGHT`, `CLEAR`.
   Add a drift test like `xtask/tests/scene_codegen.rs` for every exported scene.

## Pitfalls

- Logical paths, not file paths: `models/crate`, `materials/wood`, `prefabs/tree`.
- Rotations are degrees in files, radians in `trino_core`.
- Removing/renaming a field is a format change: bump the version and add a migration
  (`Scene::from_ron`). Adding an optional defaulted field is not.
- The editor guards unsaved work: if you edit a scene the editor has dirty, it warns
  instead of reloading. Save in the editor first (or edit while it's clean).
