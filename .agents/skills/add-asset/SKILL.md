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
formats = { n64 = "CI4", 3ds = "ETC1A4" }  # n64 REQUIRED: CI4, CI8, RGBA5551, I4, I8 (TMEM 4 KB)
                                # 3ds optional (default RGBA8): RGB565, RGBA5551, RGBA4,
                                # LA8, L8, A8, ETC1, ETC1A4 — see "3DS texture formats"

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
ao = { strength = 0.8, ground = true }   # optional: bake ambient occlusion (below)
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
- **Ambient occlusion (baked):** `ao = true` or `ao = { strength = 0.8, distance = 0.5,
  rays = 64, ground = true }` darkens the vertex colors where the model hides the sky
  from itself (ADR 0014). This is how the N64 and 3DS get AO: free at run time and the
  same on every target.
  - `distance` 0 (the default) = a fifth of the bounding-box diagonal.
  - `ground = true` adds a floor at the model's lowest point, for contact shading
    under props.
  - A convex model without `ground` has nothing to occlude, so it bakes byte-identical.
  - Occlusion comes from the model alone. A crate on a separate floor model needs
    `ground`; contacts between models are the PC's SSAO (`scene-authoring`).
  - Editor: Assets › model › Ambient occlusion (baked), which writes the same line.
  - Check the result in `cargo test -p xtask --test ao_shots -- --ignored`
    (`baked_*.png`), or draw the model unlit.
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

## 3DS texture formats (issue #52, ADR 0017)

- Pick by content: opaque art `ETC1` (4 bpp, 8× less than RGBA8, 4×4 blocks), cutouts
  and soft alpha `ETC1A4`, flat UI with alpha `RGBA4`, hard cutouts `RGBA5551`, gray
  `L8`/`LA8`, tinted masks (fonts, glows) `A8`. `A8` draws **white** with its alpha on
  every target (the shim fixes the PICA200's black A8), `L8` is the Rec. 709 luminance.
- **See it before the console:** `TRINO_SIM=3ds cargo run -p trino-app-pc` (or the
  editor's Game view in 3DS) draws each sprite's decoded 3DS format — the PC bake writes
  it as `<id>.3ds.sprite` (`preview_3ds`). Fonts get it too.
- **Measure it:** `cargo xtask texture inspect sprites/x [--platform 3ds] [--json]`:
  bytes, PSNR vs the master (alpha-premultiplied; < ~25 dB is visible, > ~35 dB is not),
  TMEM share on the N64, texture-memory share on the 3DS; `psnr_tex3ds` when devkitPro
  is installed (the real encoder, decoded).
- The PC bake validates the 3DS format too: a typo fails `cargo xtask assets pc` with
  the supported list, before any console build.
- Evidence pictures: `cargo test -p xtask --test texture_shots -- --ignored`
  (`target/evidence/texture_formats/`: every format of a photo-like master, and
  Azahar's frame vs the PC's).

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

## Particle effects

`assets/shared/particles/<name>.particles.ron` + `[particles.<name>] file =
"particles/<name>.particles.ron"` (an override in `assets/n64/particles/` wins on the N64,
like any master):

```ron
(
    version: 1,
    sprite: "sprites/spark",         // a flipbook left to right when `frames` > 0
    rate: 30.0, life: (0.6, 1.2), speed: (1.5, 3.0),
    direction: (0.0, 1.0, 0.0), spread: 35.0,   // cone half-angle, DEGREES
    gravity: (0.0, -6.0, 0.0), size: (0.3, 0.1),
    color: ((255, 230, 120, 255), (255, 80, 20, 0)),
    frames: 2, frame_size: (8.0, 8.0),
    max: 24,                                     // the most alive at once
    platforms: (n64: Some((max: Some(8))), n3ds: Some((disabled: true))),
)
```

- It bakes per platform to a `TPFX` blob with that platform's cap (0 = off there). Code:
  `let fx = Effect::parse(platform.blob(BlobId::from_path("particles/<name>"))?)?;
  let mut s: ParticleSystem<32> = ParticleSystem::from_effect(&fx, seed);` — the cap
  holds against bursts and emission (`ParticleSystem::limit`).
- Cost: 2 triangles per particle and ~44 µs each on the N64 (`N64_COST.us_per_billboard`):
  cap the N64.
- `cargo xtask particles new <name> --sprite sprites/x`, `particles check`,
  `particles inspect <name> [--seconds S] [--seed N] [--json]` (live and peak particles,
  peak tris, N64 time and a position hash per platform — the same seed, the same hash).
- Editor: Assets › **Particles** (+ creates one) › Inspector: every field, the cap per
  platform, a live preview with Burst on the PC/N64/3DS caps. Edits write the file once
  the user settles; a file an agent writes reloads in the inspector.

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

**Water** (ADR 0015): add `water: Some((flow_speed: (0.0, -0.3), detail_flow: (0.04, -0.55),
wave_amplitude: 0.04, wave_frequency: 0.55, ripple_responsiveness: 1.0))` (every field
optional). The `detail` texture is the second flowing layer, blended 50/50 with the
base. On the N64 both layers share TMEM: use I8 + I4 (or 16-bit) images, never a
palette (`assets/materials/water.material.ron`, `sprites/tex_water`, `tex_ripples`).
The mesh's vertex alpha sets the translucency (deep middle opaque, banks clear). The
showcase's river meshes come from `xtask/src/river_assets.rs` (`cargo xtask
gen-assets`), driven by the same `showcase::river` table as the game's floor.

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

## A game with its own asset set (Solid Eleven, 2026-10-16)

- **Its own `assets/`:** `examples/solid-eleven/assets/` has its own `manifest.toml`.
  `TRINO_GAME=solid-eleven` runs it on `trino-runner-pc` and bakes it to
  `target/solid-eleven/assets/pc` (`RunConfig::target`), and the football template
  copies it instead of the engine's. Sounds and sprites are generated
  by `tools/solid_eleven/make_{sounds,sprites}.py`.
- **TTF fonts on the N64:** the bake writes transparent texels black and the N64 stages
  I4/I8 atlases with intensity = coverage. Before that, a TTF atlas drew solid blocks
  on the console only.
- **Keep strings to what the font bakes:** list the characters the code writes in the
  font's `chars` (accents included) and test it, as `ui::FONT_CHARS` and
  `solid_eleven_layers::the_fonts_bake_the_characters_the_code_writes` do.

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

