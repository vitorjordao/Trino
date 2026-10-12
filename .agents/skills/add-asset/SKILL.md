---
name: add-asset
description: Add an asset to a Trino game — sprite, sound, music, 3D model (glTF or a fake-object placeholder) or material — master placement, manifest declaration, per-platform formats, baking, validation and live reload.
---

# add-asset

Masters live under `assets/shared/<category>/`; `assets/manifest.toml` declares each one
under a **logical path** whose FNV-1a hash is the handle (`SpriteId::from_path("sprites/x")`).
A file at `assets/<platform>/<same path>` overrides the master for that platform only.

## Sprites, sounds, music

```toml
[sprites.enemy]
file = "sprites/enemy.png"      # PNG RGB/RGBA
formats = { n64 = "CI4" }       # REQUIRED for n64: CI4, CI8 or RGBA5551 (TMEM is 4 KB)

[sounds.jump]
file = "sounds/jump.wav"        # PCM 16-bit or float, mono/stereo

[music.theme]
file = "music/theme.wav"        # loops on the music channel (wav64 --wav-loop on N64)
```

```rust
pub const ENEMY: SpriteId = SpriteId::from_path("sprites/enemy");
pub const JUMP: SoundId = SoundId::from_path("sounds/jump");
pub const THEME: MusicId = MusicId::from_path("music/theme");
```

## 3D models

```toml
[models.robot]
file = "models/robot.glb"   # glTF 2.0 (.glb/.gltf), Y-up, meters
anim_fps = 20               # optional: clip sampling rate (default 15)
once = ["wave"]             # optional: clips that play once (default: loop)
```

- Vertex colors only (COLOR_0 → base-color texture sampled at vertex UV → base-color
  factor). Animated files bake a rigid skeleton (≤ 32 joints, heaviest weight wins).
  Full rules and the Blender workflow: the `blender-asset` skill.
- **Gate:** `cargo xtask model inspect assets/shared/models/robot.glb [--json]` — bakes
  like the engine and reports tris, joints, clips, size, N64 budget.
- Code: `ModelId::from_path("models/robot")`; animate with
  `ModelParams { anim: Some(AnimRef { clip: ClipId::from_name("walk"), time }), .. }`.

## Fake objects (placeholder models)

Need a crate/pillar/enemy *now*, art later? Create a `.prim.ron` and declare it as a model:

```ron
// assets/shared/models/proxies/crate.prim.ron
(
    shape: Box(size: (1.0, 1.0, 1.0)),   // Box Sphere Cylinder Cone Capsule Plane Ramp Torus
    color: (176, 124, 78, 255),
    top_color: Some((214, 168, 112, 255)),  // optional vertical gradient
    pivot: Bottom,                          // Center | Bottom
    segments: 12,                           // round shapes
    smooth: true,                           // false = faceted normals
)
```
```toml
[models.crate]
file = "models/proxies/crate.prim.ron"
```

It bakes to the same TMDL as a glTF on every target. Later point `file` at the real `.glb`:
the handle — and every scene entity, material and code reference — stays the same. The
editor does both (*Create › Fake object*, Shift+A; model Inspector › *Replace with real
art*) and writes these same files.

## Materials

`assets/materials/<name>.material.ron` (logical `materials/<name>`), no manifest entry:

```ron
(
    version: 1,
    tint: (255, 255, 255, 255),   // × vertex colors (alpha too)
    shading: Flat,                // Gouraud | Flat | Unlit
    emissive: (0, 0, 0),          // added after lighting
    double_sided: false,
)
```

Scenes/prefabs reference materials by logical path; games use the equivalent
`ModelParams` (or export the scene to code — `scene-authoring` skill).

## Verify

1. `cargo xtask assets pc` (and `cargo xtask build n64|3ds` when toolchains exist) — format
   or resolution problems are bake **errors**, never silent fallbacks.
2. `cargo xtask scene check` if scenes/prefabs reference the asset.
3. `cargo xtask test`.

## Rules

- A missing source anywhere is a bake error, never a fallback.
- Renaming a logical path changes the handle — breaking for scenes/code. Renames are refactors.
- With the editor open or `cargo xtask watch pc` running, saving a master rebakes and
  re-uploads it live under the same handle.
