# The asset pipeline

You author one master per asset. At build time the engine bakes it into the format each
platform needs, and your game refers to it by a logical path. This page is the overview;
the [add-asset](../.agents/skills/add-asset/SKILL.md) skill has every manifest field, and
the [blender-asset](../.agents/skills/blender-asset/SKILL.md) skill covers modeling.

## Masters, overrides and the manifest

```
assets/
  manifest.toml        every asset, under a logical path
  shared/              masters: sprites/, sounds/, music/, models/, fonts/, data/, particles/, impostors/
  n64/, 3ds/           optional overrides: a file at the same path wins on that platform only
  materials/           *.material.ron (no manifest entry)
  impostors/           *.impostor.ron (impostor bake settings)
```

- The manifest declares each asset under a **logical path** (`sprites/hero`,
  `models/robot`). The handle is the FNV-1a hash of that path, computed the same way by
  the game at compile time, by the bake and by the platform loader:
  `SpriteId::from_path("sprites/hero")`. Renaming a path changes the handle.
- Kinds: `[sprites.*]`, `[sounds.*]`, `[music.*]`, `[models.*]`, `[fonts.*]`, `[blobs.*]`,
  `[scenes.*]`, `[particles.*]`.
- **Resolution rule:** an override wins, else the shared master, else a bake error. A
  format the target cannot represent is also an error, never a silent fallback. The N64
  has no default sprite format (TMEM is 4 KB), so each sprite declares one
  (`formats = { n64 = "CI4" }`; also CI8, RGBA5551, I4, I8).
- **Per-platform models:** `assets/n64/models/...` and `assets/3ds/models/...` ship console
  low-poly variants under the same handle. A model listed with `platforms = ["pc"]` is not
  baked for the consoles, which load every model at boot.

## What gets baked

| Master | PC | N64 | 3DS |
|---|---|---|---|
| PNG sprite | `TSPR` (RGBA8) | libdragon sprite (`mksprite`) in the ROM's DFS | `.t3x` (`tex3ds`) in RomFS |
| WAV sound or music | `TSND` | `wav64` (`audioconv64`), loops baked in | raw PCM16 |
| glTF (`.glb`/`.gltf`) or `.prim.ron` model | TMDL | TMDL | TMDL |
| Font (PNG grid or TTF) | `TFNT` table and an atlas sprite | `TFNT` table and an atlas sprite (`mksprite`) | `TFNT` table and an atlas `.t3x` |
| Scene (`*.scene.ron`) | `TSCN` blob | `TSCN` blob, N64 variants | `TSCN` blob, 3DS variants |
| Particle effect (`*.particles.ron`) | `TPFX` with the PC cap | `TPFX` with the N64 cap | `TPFX` with the 3DS cap |

`cargo xtask assets pc|n64|3ds` runs the bake alone. `cargo xtask build n64|3ds` runs it
as part of the console build, and the PC app bakes on start. With the editor open or
`cargo xtask watch pc` running, saving a master rebakes it and re-uploads it live.

## Models: glTF to TMDL

TMDL is the engine's portable mesh format: positions, normals, vertex colors, and
optional sections for UVs and a texture (`TXUV`, `TXID`), bounds (`BNDS`), a skeleton
(`SKEL`), clips (`ANIM`), smooth-skin weights (`SKW2`) and color slots (`SLOT`). New data
goes into new sections; unknown tags are skipped.

- Color is baked into the vertices by default (from `COLOR_0`, else the base-color
  texture at the vertex UV, else the base-color factor). `COLOR_0` is linear and the bake
  converts it.
- A model can keep one small power-of-two texture instead (`texture = "sprites/x"`),
  which must fit TMEM on the N64.
- Animation is a rigid skeleton (at most 32 joints, each vertex follows one), with
  opt-in smooth skinning (`skinning = "smooth"`) for meshes that must bend. Clips are
  sampled at `anim_fps`.
- Color slots (`_SLOT` in glTF) let one model be recolored per draw with
  `ModelParams::palette`.

**The gate:** `cargo xtask model inspect <file> [--anim] [--json]` bakes a model exactly
like the engine and reports triangles, joints, clips, bounds and the estimated N64 cost
per draw. On the N64 the budget is CPU and RSP time, not a triangle count.

## Fake objects

A fake object is a parametric placeholder model, a `*.prim.ron` file (box, sphere,
cylinder, cone, capsule, plane, ramp, torus, with size, colors, pivot and segments). It
bakes to the same TMDL as a glTF on every target, so you can build a level before the art
exists. When the real model is ready, point the manifest entry's `file` at the `.glb`:
the handle stays the same, so every scene, material and code reference keeps working.
The editor creates them too (Create › Fake object, Shift+A).

## Blender

`tools/blender/` holds Python helpers for building assets inside Trino's limits:
vertex-color painting (`color_tools.py`), rigid rigs and clips (`rig_tools.py`), QA renders
(`qa_render.py`) and an exporter that checks the result with the engine's own bake
(`trino_export.py`). They work from Blender's console or through an AI agent driving
Blender.

```sh
cargo xtask blender smoke        # build a prop and a rigid character, export, run the gates
cargo xtask blender castle64     # rebuild castle64's Blender-authored models from code
```

`blender smoke` skips cleanly when Blender is not installed; `blender castle64` fails.
castle64's models
(`tools/blender/castle64_models.py`) are the reference for budgeted props: only the faces
a camera can see, so the CC0 KayKit doorway went from 1068 to 75 triangles.

## Materials, impostors, particles, scenes

- **Materials** (`assets/materials/*.material.ron`, logical path `materials/<name>`):
  tint, shading, emissive, double-sided, texture, filter, detail layer, gloss. Scenes and
  prefabs reference them; code uses the equivalent `ModelParams`.
- **Impostors:** the engine renders a model from N angles into an atlas (its own T&L and
  a CPU rasterizer, so the bytes are deterministic) for a far LOD level drawn as one
  billboard. `cargo xtask impostor new|bake|check|list`, or the editor's Model › Impostor.
  On the N64 an impostor can cost more than the RSP draw it replaces, so give the N64 its
  own distance.
- **Particle effects** (`*.particles.ron`): emitter settings with a cap per platform.
  `cargo xtask particles new|check|inspect`.
- **Scenes and prefabs** (`scenes/`, `prefabs/`): see the
  [scene-authoring](../.agents/skills/scene-authoring/SKILL.md) skill.
  `cargo xtask scene check` validates them, `scene export` writes one as Rust data,
  `scene budget` reports each console's frame cost.

## Checking your work

1. `cargo xtask assets pc` (and a console build when the toolchains are installed): format
   and size problems are bake errors.
2. `cargo xtask scene check` when scenes or prefabs reference the asset.
3. `cargo xtask model inspect <file>` for models, `cargo xtask impostor check` for
   impostors.
4. `cargo xtask test`.
