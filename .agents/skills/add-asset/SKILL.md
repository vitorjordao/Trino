---
name: add-asset
description: Add an asset to a Trino game — sprite, sound, music, 3D model (glTF or a fake-object placeholder, textured or not), material, bitmap font, data blob or baked scene — master placement, manifest declaration, per-platform formats, baking, validation and live reload.
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
# loop = true                   # looping SFX (scene `sound` sources / voices); default false

[music.theme]
file = "music/theme.wav"        # loops on the music channel (wav64 --wav-loop on N64)

[music.fanfare]
file = "sounds/win.wav"
loop = false                    # a stinger: Audio::play_stinger, music ducks under it
```

On the N64, looped wavs are trimmed to a multiple of 16 frames (wav64 requires it),
and a sound plays on one channel at a time: re-triggering restarts it.

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
platforms = ["pc"]          # optional: bake only for these (pc, n64, 3ds); default all
once = ["wave"]             # optional: clips that play once (default: loop)
```

- **Colors**: by default the color is baked into the vertices, from COLOR_0, else the
  base-color texture sampled at the vertex UV, else the base-color factor.
- **Textures**: add `texture = "sprites/<tex>"` and the mesh keeps its UVs and draws
  with that sprite (TMDL `TXUV`/`TXID`). One texture per draw, power-of-two, and it
  must fit TMEM on the N64 (`formats = { n64 = "CI8" }` for 32×32 or 64×32 tiles).
  Fake objects get planar UVs (one repeat per meter). A material's `texture` overrides
  it per draw.
- **Skeletons**: animated files bake a rigid skeleton (≤ 32 joints, heaviest weight
  wins). `skinning = "smooth"` keeps a second joint per vertex (TMDL `SKW2`) for meshes
  that crack. That costs ~3% CPU per draw; check with `model inspect --smooth`.
- **Color slots:** a glTF custom attribute `_SLOT` (one scalar per vertex, 0..4, float
  or integer) bakes to TMDL `SLOT`. Draw with
  `ModelParams { palette: Some([c1, c2, c3, c4]), .. }` and each slotted vertex's color
  is multiplied by its entry, before tint and lighting. Slot 0 keeps its color. One
  character model then gives a whole crowd. Paint slot regions white, and keep slot
  borders on split vertices (Blender: `paint_slots`, FACE domain). Without `_SLOT`,
  the bake output is byte-identical to before. `model inspect` reports the vertices
  per slot. On the N64 a palette draw runs on the CPU T&L, not the RSP.
- Full rules and the Blender workflow: the `blender-asset` skill.
- **Gate:** `cargo xtask model inspect assets/shared/models/robot.glb [--json]` — bakes
  like the engine and reports tris, joints, clips, size and an **N64 cost estimate per
  draw** (calibrated `trino_core::perf::N64_COST`; warns above 25% of a 30 fps frame;
  `--json` has `n64_draw_us` and `n64_frame_percent`). The N64 budget is CPU time, not
  a triangle count: the 1068-tri KayKit door read "27% of budget" under the old
  triangle cap and cost ~64 ms (two frames) on the N64's CPU T&L. It was
  rebuilt in Blender at 75 tris (`blender-asset` › "Pay only for faces a camera can
  see").
  Adding a model to castle64 changes `tests/perf/castle64_work.toml` (bless it,
  `n64-performance`).
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

## Fonts, blobs, baked scenes

```toml
[fonts.hud]
file = "fonts/hud.png"        # PNG glyph grid in codepoint order…
cell = [6, 10]                # …of these cells, from `first` (default 32 = space)
# proportional = true         # trim glyphs to their opaque columns (+ `spacing`)
# or: file = "fonts/x.ttf" + size = 12 (+ chars, antialias) — rasterized at bake time

[blobs.dialogue]
file = "data/dialogue.txt"    # any bytes: Platform::blob(BlobId::from_path("blobs/dialogue"))

[scenes.level1]
file = "scenes/level1.scene.ron"   # path from the PROJECT ROOT; bakes to a TSCN blob
```

- **Fonts**: code uses `renderer.draw_text(FontId::from_path("fonts/hud"), pos, "TEXT",
  &TextParams::DEFAULT)` and formats numbers with `TextBuf<N>` (no alloc). The atlas
  is a sprite (CI4 on N64). Missing characters draw as `?`.
- **Blobs**: they are `'static`, read once at load.
- **Baked scenes**: load with `BakedScene::parse(platform.blob(BlobId::from_path("scenes/level1"))?)`
  (see `scene-authoring`).

## Impostors (a model as a far-away billboard)

A model pre-rendered from N yaws into one atlas, drawn as **one billboard, 2 triangles**
(a LOD level). The engine renders it itself (its own T&L + a CPU rasterizer: no Blender,
no GPU, deterministic bytes), and the editor and the CLI run the same bake:

```
cargo xtask impostor new tree --model models/tree --views 8 --pitch 20 --tile 64
cargo xtask impostor bake tree | --all        # after editing the settings or the model
cargo xtask impostor check                    # fails on a missing or stale impostor
cargo xtask impostor list --json
```

Editor: Assets › the model › Inspector › **Impostor** (Create and bake; then tune views,
pitch, tiles and Bake). Both write:

| File | What |
|---|---|
| `assets/impostors/<name>.impostor.ron` | the settings (`ImpostorDef`): model, views, pitch, tile, n64_tile, light, clip/time |
| `assets/shared/impostors/<name>.png` + `.timp` | PC/3DS atlas (RGBA) and its `TIMP` table |
| `assets/n64/impostors/<name>.png` + `.timp` | N64 atlas: an **indexed CI4 PNG** (index 0 clear) whose tiles shrink to fit TMEM, and its table |
| manifest `[sprites.impostor-<name>]` (`n64 = "CI4"`) + `[blobs.impostor-<name>]` | added once |

- Use it from a scene LOD level: `(distance: 30.0, impostor: Some("impostors/<name>"))`
  (`scene-authoring`), or in code: `Impostor::parse(platform.blob(BlobId::from_path(
  "blobs/impostor-<name>"))?)?.draw(r, &matrix, eye, tint)`.
- The view shown is the rendered yaw nearest to the camera's direction in model space
  (`Impostor::view_for`, deterministic). The quad covers the bounding sphere; the light is
  baked in (billboards are unlit) and turns with the object.
- Each `.timp` records a hash of the model it came from: `impostor check` (and the
  editor's "stale" badge) catch a model edit that wasn't re-baked.
- Textured models render with their vertex colors only (the bake ignores textures).
- **N64 cost:** textured triangles are CPU-submitted (~44 µs a billboard,
  `N64_COST.us_per_billboard`). Most static meshes run on the RSP for less, so an impostor
  can make the N64 *slower* — give the N64 its own distance or `Never` in the LOD level.

## Materials

`assets/materials/<name>.material.ron` (logical `materials/<name>`), no manifest entry:

```ron
(
    version: 1,
    tint: (255, 255, 255, 255),   // × vertex colors (alpha too)
    shading: Flat,                // Gouraud | Flat | Unlit
    emissive: (0, 0, 0),          // added after lighting
    double_sided: false,
    texture: Some("sprites/brick"),  // optional: per-draw texture for meshes with UVs
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

## Formats and fonts added for external projects (2026-10-14)
- **N64 sprite and font formats** include `I4` and `I8`: intensity, with alpha equal
  to intensity. They suit white masks tinted at draw time (fonts, UI) and upload no
  palette per blit. The font bake writes transparent texels **black** for this reason.
- **PNG grid fonts:**
  - `chars = "..."` lists each cell's character in order, so accents, `¿`, `€` and
    friends work;
  - with `proportional = false`, `advance = N` sets a pitch wider than the cell (for
    example 5-wide cells at 6 px);
  - a font atlas that fits TMEM (I4: ≤ 4096 bytes) takes the N64 resident text path,
    which is several times faster per glyph.
- **Per-platform model overrides** (`assets/n64/models/...`, `assets/3ds/models/...`)
  are the way to ship console low-poly variants.

