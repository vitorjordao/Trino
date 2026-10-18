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
| PNG sprite | `TSPR` (RGBA8), plus the 3DS preview | libdragon sprite (`mksprite`) in the ROM's DFS | `.t3x` (`tex3ds`, any format above) in RomFS |
| WAV sound or music | `TSND` | `wav64` (`audioconv64`), loops baked in | raw PCM16 |
| glTF (`.glb`/`.gltf`) or `.prim.ron` model | TMDL | TMDL | TMDL |
| Font (PNG grid or TTF) | `TFNT` table and an atlas sprite | `TFNT` table and an atlas sprite (`mksprite`) | `TFNT` table and an atlas `.t3x` |
| Scene (`*.scene.ron`) | `TSCN` blob | `TSCN` blob, N64 variants | `TSCN` blob, 3DS variants |
| Particle effect (`*.particles.ron`) | `TPFX` with the PC cap | `TPFX` with the N64 cap | `TPFX` with the 3DS cap |

`cargo xtask assets pc|n64|3ds` runs the bake alone. `cargo xtask build n64|3ds` runs it
as part of the console build, and the PC app bakes on start. With the editor open or
`cargo xtask watch pc` running, saving a master rebakes it and re-uploads it live.

## Texture formats

Each sprite and font atlas declares a format per platform
(`formats = { n64 = "CI4", 3ds = "ETC1", pc = "BC7" }`), or `"auto"`; a format the
platform can't represent is a bake error that lists the ones it can.

| Platform | Formats | Default |
|---|---|---|
| PC | `RGBA8`, `BC1`, `BC3`, `BC7` | `auto` |
| N64 | `CI4`, `CI8`, `RGBA5551`, `I4`, `I8` (TMEM is 4 KB), `auto` | none: declare one (fonts: `CI4`) |
| 3DS | `RGBA8`, `RGB565`, `RGBA5551`, `RGBA4`, `LA8`, `L8`, `A8`, `ETC1`, `ETC1A4` (`tex3ds -f`), `auto` | `auto` |

### `auto`, `quality` and `min_psnr` (ADR 0017 §6)

`auto` measures every format of the platform against the master and keeps the
**smallest one whose PSNR reaches the threshold** (same size: the higher PSNR; on the
N64, a format that fits TMEM first). Nothing is guessed from the content: an ETC1 of
an image with alpha, or an A8 of a colored one, simply measures low. The PC bake, the
3DS and N64 builds, `texture inspect`, `texture report` and the editor run the same
code, so they agree on every texture.

```toml
[sprites.sky]
file = "sprites/sky.png"
formats = { n64 = "RGBA5551" }   # PC and 3DS: auto
quality = "high"                 # low 30 dB, normal 35 (the default), high 40

[sprites.hud_icons]
file = "sprites/hud.png"
formats = { 3ds = "RGBA4" }      # an explicit format...
min_psnr = 38                    # ...checked: below 38 dB the bake fails
```

- **Threshold:** `min_psnr` when set, else `quality`. Above ~35 dB a change is hard to
  see, below ~25 dB it is obvious.
- **Explicit formats are the author's call:** only a texture with `min_psnr` checks
  them, and a miss fails the bake naming the formats that would pass and what `auto`
  would pick.
- **Not measured:** an N64 palette the master doesn't fit losslessly (`mksprite`'s
  quantizer is not modeled) and a BCn texture whose sides are not multiples of 4.
  `auto` never picks those.
- **Full alpha ramps** (soft glows in color) stay `RGBA8` at the default 35 dB: 4-bit
  alpha (`RGBA4`, `ETC1A4`) keeps about 33 dB. `quality = "low"` accepts them.

### PC: BCn (ADR 0017 §7)

| Format | Bits a texel | Keeps | Use it for |
|---|---|---|---|
| `BC1` | 4 | color in 4×4 blocks, 1-bit alpha | opaque art, hard cutouts |
| `BC3` | 8 | BC1 color + interpolated 8-bit alpha | masks, soft alpha |
| `BC7` | 8 | color and alpha, best quality | anything BC1 blurs |
| `RGBA8` | 32 | everything | sides not multiples of 4, gradients that need it |

- The PC bake writes the blocks (`<id>.bc`, `compressed` in the index); the master
  stays beside it. The runner and the editor upload the blocks where the GPU samples
  BC (`TEXTURE_COMPRESSION_BC`, every desktop GPU) and the decoded blocks otherwise,
  so the picture is the same. `TRINO_BC=0` forces the fallback.
- BC4/BC5 (one and two channels) are not offered: wgpu has no texture swizzle to
  sample them as gray or white.
- A run with `TRINO_SMOKE_FRAMES` prints `TRINO_TEXTURES: textures= bytes= bc=
  bc_bytes= bc_supported=`: castle64's 31 textures take 19,904 bytes of VRAM instead
  of 147,712.

### On the 3DS (ADR 0017)

| Format | Bits a texel | Keeps | Use it for |
|---|---|---|---|
| `ETC1` | 4 | color in 4×4 blocks, opaque | opaque art: terrain, walls, skies (8× less than RGBA8) |
| `ETC1A4` | 8 | ETC1 plus 4-bit alpha | foliage, decals, soft cutouts |
| `RGBA4` | 16 | 4 bits a channel | UI with alpha, flat-colored art |
| `RGBA5551` | 16 | 5-bit color, 1-bit alpha (≥ 128) | hard cutouts |
| `RGB565` | 16 | 5/6-bit color, opaque | opaque art without blocks |
| `LA8` | 16 | gray with alpha | gray masks with a shade |
| `L8` | 8 | gray, opaque | light maps, detail layers |
| `A8` | 8 | alpha; drawn **white** with that alpha | fonts, glows: masks tinted at draw time |
| `RGBA8` | 32 | everything | what nothing smaller keeps |

- Gray is the Rec. 709 luminance of the linear color, back to sRGB (what `tex3ds`
  computes). A fully transparent texel becomes 0 in every channel, so in `L8`/`ETC1`
  it is black.
- **The PC shows the result:** for each sprite whose 3DS format loses something, the
  PC bake also writes what the 3DS samples (`<id>.3ds.sprite`, `preview_3ds` in the
  index), and the PC's 3DS profile (`TRINO_SIM=3ds`, the editor's Game view in 3DS)
  draws it: ETC1 blocks and 4-bit banding appear on the PC as in Azahar. The preview
  equals `tex3ds`' output texel for texel for the uncompressed formats; ETC1 uses the
  pipeline's own encoder, within 0.1 dB of `tex3ds`' quality.
- `cargo xtask build 3ds` prints the linear memory the textures take, and what they
  would take as RGBA8, per format.

### On the N64

`auto` is opt-in (an N64 texture is sized for TMEM by hand). The pipeline models
`mksprite`: `I4`/`I8` keep a gray master's level, otherwise Rec. 601 luma × alpha
stretched to the full range, and the RDP samples **(i, i, i, i)**, so an intensity
texture is also translucent: right for glows and fonts, wrong for opaque gray art
(`auto` picks `CI8` there). `CI4`/`CI8` are measured only when the master fits the
palette losslessly after RGBA5551 truncation.
- **Masks in `I4`/`I8` are staged by their coverage** (`texture::n64::stage`):
  `mksprite` takes a gray master's red channel whatever its alpha, so a white glow
  became a solid white disc on the console (Solid Eleven's glow and rain, until
  2026-10-17). A font atlas is staged as its alpha; an all-gray sprite with alpha
  takes its color or its coverage, whichever previews closer. (i, i, i, i) still
  attenuates a white mask twice (about 20 dB): the engine has no IA format.

### Checking textures

- `cargo xtask texture inspect sprites/x [--platform 3ds] [--json]`: per platform the
  format (and why `auto` chose it), every candidate with its bytes and PSNR, and the
  share of TMEM (N64) or texture memory (3DS). PSNR is computed on colors
  premultiplied by alpha, so invisible texels don't count. With devkitPro installed,
  the 3DS line also decodes the real `tex3ds` output.
- `cargo xtask texture report [--platform pc|n64|3ds] [--json]`: every texture of the
  project, biggest first, with per-platform totals against RGBA8; it fails when any
  texture fails its format or its `min_psnr`.
- The editor: select a sprite or font in **Assets**: its texture inspector shows the three
  platforms side by side, the decoded previews, the candidates, and writes format,
  `quality` and `min_psnr` back to the manifest.

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
3. `cargo xtask model inspect <file>` for models, `cargo xtask texture inspect
   sprites/x` for textures, `cargo xtask impostor check` for impostors.
4. `cargo xtask test`.
