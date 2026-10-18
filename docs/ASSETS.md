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
  n64/, 3ds/, pc/, web/ optional overrides: a file at the same path wins on that platform only
                       (the Web falls back to pc/ before shared/)
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
  baked for the consoles, which load every model at boot. The Web takes the PC's choices
  where it has none of its own: a list naming `pc` bakes for the Web too, and an
  `assets/pc/` override wins on the Web unless `assets/web/` has one (issue #47).

## What gets baked

| Master | PC | N64 | 3DS | Web |
|---|---|---|---|---|
| PNG sprite | `TSPR` (RGBA8), plus the 3DS preview | libdragon sprite (`mksprite`) in the ROM's DFS | `.t3x` (`tex3ds`, any format above) in RomFS | KTX2 (Basis Universal) or `TSPR` in the pack |
| WAV sound or music | `TSN2`: Ogg Vorbis (or PCM16) | `wav64`: VADPCM (or Opus, PCM), loops baked in | `T3AU`: DSP-ADPCM (or PCM16) | `TSN2`: Ogg Vorbis (or PCM16) in the pack |
| `assets/audio.toml` (the project mix) | `TMIX` (`audio/mixer`) | `TMIX` | `TMIX` | `TMIX` |
| glTF (`.glb`/`.gltf`) or `.prim.ron` model | TMDL | TMDL | TMDL | TMDL |
| Font (PNG grid or TTF) | `TFNT` table and an atlas sprite | `TFNT` table and an atlas sprite (`mksprite`) | `TFNT` table and an atlas `.t3x` | `TFNT` table and an atlas sprite |
| Scene (`*.scene.ron`) | `TSCN` blob | `TSCN` blob, N64 variants | `TSCN` blob, 3DS variants | `TSCN` blob, Web variants |
| Particle effect (`*.particles.ron`) | `TPFX` with the PC cap | `TPFX` with the N64 cap | `TPFX` with the 3DS cap | `TPFX` with the Web cap (else the PC's) |
| `assets/quality.ron` + `trino.toml [pc.quality]` (the presets) | `quality/presets` text blob | nothing (fixed quality) | nothing (fixed quality) | `quality/presets` in the pack |

`cargo xtask assets pc|n64|3ds|web` runs the bake alone. `cargo xtask build n64|3ds|web`
runs it as part of the build (the Web packs every asset of a game into one
`assets/<game>.pak`, fetched at boot), and the PC app bakes on start. With the editor open or
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
| Web | `KTX2-ETC1S`, `KTX2-UASTC` (Basis Universal, transcoded in the browser), `RGBA8`, `auto` | `auto` |

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

### Web: KTX2 (ADR 0017 §9)

| Format | Download | VRAM once transcoded | Use it for |
|---|---|---|---|
| `KTX2-ETC1S` | smallest (BasisLZ codebooks, often under 1 bit a texel) | BC1 / ETC2 RGB: ½ byte a texel; with alpha BC3 / ETC2 RGBA: 1 | most art: the download is what a page waits for |
| `KTX2-UASTC` | 1 byte a texel | BC7 / ASTC 4×4: 1 byte a texel | what ETC1S blurs (gradients, faces, UI) |
| `RGBA8` | 4 bytes a texel | 4 bytes a texel | sides not multiples of 4, pixel art that must stay exact |

- The bake writes `<id>.ktx2` beside the master (`ktx2` in the index), and `cargo xtask
  build web` packs the KTX2 instead of the master. The page transcodes each texture
  once, with Binomial's transcoder, to BC on desktops, ETC2 or ASTC on phones, RGBA8
  where the GPU has none (`TRINO_ASSETS: … target=BC` in the console says which).
- `auto` (the default) keeps the smallest download that reaches the threshold: ETC1S,
  else UASTC, else RGBA8. Its PSNR is measured on the transcoder's own decode, the
  pixels a browser shows.
- `texture inspect|report --platform web`: `bytes` is the download, the memory share is
  the VRAM against `Caps::WEB` (1 GiB). castle64's 33 KTX2 textures and the rest pack
  into a 944 KB `trino.pak`.

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

- `cargo xtask texture inspect sprites/x [--platform 3ds|web] [--json]`: per platform the
  format (and why `auto` chose it), every candidate with its bytes and PSNR, and the
  share of TMEM (N64), texture memory (3DS) or VRAM (PC, Web; the Web's bytes are its
  download). PSNR is computed on colors
  premultiplied by alpha, so invisible texels don't count. With devkitPro installed,
  the 3DS line also decodes the real `tex3ds` output.
- `cargo xtask texture report [--platform pc|n64|3ds|web] [--json]`: every texture of the
  project, biggest first, with per-platform totals against RGBA8; it fails when any
  texture fails its format or its `min_psnr`.
- The editor: select a sprite or font in **Assets**: its texture inspector shows the four
  platforms side by side, the decoded previews, the candidates, and writes format,
  `quality` and `min_psnr` back to the manifest.

## Audio: a codec per platform, streaming, buses (ADR 0021)

One WAV master (16-bit or float, mono or stereo: stereo is averaged to mono) bakes to
what each platform plays best:

| Platform | `auto` | Other codecs | Decoded by | Streams from |
|---|---|---|---|---|
| PC | Ogg Vorbis | `pcm` | the CPU (lewton) | RAM: the encoded file, a decoder thread fills a ring |
| N64 | VADPCM (`wav64`) | `opus`, `pcm` | the RSP (libdragon's mixer); Opus: CPU + RSP | the cartridge (every `wav64` does) |
| 3DS | DSP-ADPCM (`T3AU`) | `pcm` | the DSP, in hardware | RomFS, chunk by chunk into rotating wave buffers |
| Web | Ogg Vorbis (the PC's choice) | `pcm` | wasm (lewton) | RAM: the Ogg bytes, decoded in chunks scheduled 1 s ahead |

```toml
[music.theme]
file = "music/theme.wav"
loop = true
stream = true                  # music streams by default; sounds load whole
codec = { n64 = "vadpcm", 3ds = "dspadpcm", pc = "vorbis" }  # or "auto" (the default)
quality = 6                    # Vorbis quality 0..10 (default 6)
sample_rate = { n64 = 22050 }  # resample per platform (default: the master's; N64 at most 44100)
bus = "music"                  # music (default for [music.*]), sfx (sounds), voice, ambience, ui
```

A codec a platform can't play is a bake error that lists the ones it can; the Web takes the
PC's value when it has none. **Sizes:** VADPCM and DSP-ADPCM are about 28% of PCM16 at any
quality; Vorbis q6 is 18–30% on music. **Fidelity** (SNR of what plays against the master,
`cargo xtask audio report`): the ADPCMs keep 30–35 dB on music and tones, 24–25 dB on
noise (splashes, crowds); Vorbis spends its bits by audibility, so its waveform SNR (27–41 dB
on music, 15–20 dB on noise) understates how it sounds. Listen with `audio preview`.

**On the N64** the bake writes the `wav64` itself: VADPCM with Trino's encoder (the one
`audio inspect` and `preview` measure; libdragon's `audioconv64` clipped loud masters —
castle64's theme at 7.6 dB SNR, 30.8 dB now, same bytes) and raw PCM16 for `pcm`. Only
`opus` goes through `audioconv64` (`--wav-compress 3`, always 48 kHz). Masters above
44.1 kHz bake at 44.1 kHz, the mixer's channel limit. Every length becomes whole 16-sample
blocks, the only loops libdragon's VADPCM plays: a one-shot gains up to 15 samples of
silence, a loop drops up to 15 (one shorter than 16 repeats until it fits). An Opus asset
links libopus into the ROM (+99 KB) and raises every channel's rate limit to 48 kHz; a
project without one pays neither. At 60 Hz a 44.1 kHz VADPCM voice costs ~0.26 ms of the
frame and an Opus track ~4.6 ms (`audio inspect` quotes it), so keep Opus for long music
where ROM space matters.

**Streaming** keeps memory flat however long the track: the PC holds the encoded file and
~320 KB while it plays (a 10-minute track: 739 KB encoded instead of 115 MB of floats), the
Web the Ogg bytes and about 1 s of decoded chunks, the 3DS three 8 KB wave buffers, the N64
~1 KB per open `wav64` and the mixer's sample buffer per playing channel (48 KB for a
44.1 kHz track).

**Buses and the mix.** Every sound plays on a bus (`Bus::Master`, `Music`, `Sfx`, `Voice`,
`Ambience`, `Ui`): its manifest `bus`, or `SoundParams::on(bus)` per call. Gain is master ×
bus × ducking; each bus has a mute. `assets/audio.toml` sets each bus's starting volume and the
duck rules (the editor's Mixer panel writes it, keeping the comments at the top and right
above each table; `cargo xtask audio mix` validates it):

```toml
version = 1
[buses]
music = 0.8
[[duck]]
trigger = "voice"     # while anything plays on the voice bus...
target = "music"      # ...the music ducks to 0.4, in 150 ms, back in 300 ms
level = 0.4
attack_ms = 150
release_ms = 300
```

The player's own volumes come from the game (`Audio::set_bus_volume`/`set_bus_muted`, e.g.
an options menu): the PC keeps them per game in `target/save/<save_name>.audio`; console games
that want them across power cycles store `BusSettings::to_bytes()` (4 bytes) in their save.

**Checking audio:** `cargo xtask audio inspect music/theme [--platform n64] [--json]` (codec
and why, bytes against PCM16, SNR, RAM resident and while playing, who decodes it),
`cargo xtask audio report` (biggest first, totals per platform) and `cargo xtask audio preview
music/theme` (each platform's decoded result as a WAV, next to the master; the N64's Opus only
decodes on the console). The editor's Assets › Audio inspector shows the same numbers and
plays each platform's version.

## Groups, compression and memory (ADR 0022)

Every asset is in the `boot` group unless it names others. `boot` loads at boot and stays;
any other group loads when the game asks (`platform.load_group(GroupId::from_name("level1"))`,
polled each frame until `Ready`; its `progress` drives a loading bar) and frees its memory on
`unload_group`. Handles never change, and drawing an asset whose group is not loaded draws
nothing (counted as `not_loaded` in the frame stats).

```toml
[groups.boot]                        # optional: compress boot on the consoles
compression = { n64 = 1, 3ds = "lz4" }

[groups.level1]                      # loaded on demand; default N64 level 1, LZ4 elsewhere
[groups.level2]
compression = { n64 = 2 }            # aPLib: smaller, slower to decode

[models.boar]
file = "models/boar.glb"
groups = ["level1", "level2"]        # loaded once, freed with the last of them

[memory]                             # what is not the assets' (defaults measured in ares/Azahar)
reserve = { n64 = "2.2M" }
game = "256K"                        # the game's own heap
```

- **Bundles:** each group but `boot` bakes to `groups/<name>.tbnd` per platform (Web:
  `trino.<name>.tbnd` beside the pack, downloaded the first time the game loads it).
- **Compression:** N64 levels are libdragon's `mkasset` levels (0 none, 1 LZ4, 2 aPLib,
  3 Shrinkler); sprites are compressed by `mksprite` itself and `wav64` files stream as they
  are. The 3DS, the PC and the Web take `"lz4"` or `"none"`; the 3DS decodes on its worker
  core. `boot` is compressed only on the consoles (the 3DS packs it into `groups/boot.tbnd`);
  castle64's and Solid Eleven's DFS and RomFS are 21–31 % smaller with boot at N64 level 1 /
  LZ4 (ADR 0022 §3).
- **Scenes:** a scene's `requires` (default `["boot"]`) lists the groups it needs loaded, and
  a portal cell can name the groups its contents come from (`cell(size: …, groups: ["cellar"])`):
  `trino_core::CellGroups` keeps those of the rooms in view, and one doorway further, loaded.
- **Live reload** (`cargo xtask watch pc`) re-uploads changed `boot` assets; a grouped asset's new
  bytes are in its rebaked bundle and show the next time its group loads. The editor's play
  mode loads every asset, so it shows all edits at once.

**Checking memory:** `cargo xtask assets report [--platform n64] [--tier base] [--json]` prints
each console tier's RAM split (engine reserve, game, the rest for assets), every group's
assets, RAM and baked size, and each scenario (boot, boot + each group, each scene) against
the budget. `cargo xtask assets check` fails when a scene draws an asset none of its groups
loads, names an undeclared group, or would not fit a tier. `cargo xtask assets group
models/boar level1 [level2…]` sets an asset's groups (declaring new ones; `boot` alone
removes the key). Strict mode (`TRINO_STRICT=1` with an N64 or 3DS profile) panics on a load
past the console's RAM. The editor shows the same numbers in its Memory tab, filters the
Assets panel by group (drop a tile on a chip to move it), and picks a scene's `requires` and a
cell's groups in the inspector.

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
- **Quality presets** (`assets/quality.ron`, ADR 0023 §6): what Low, Medium, High and
  Ultra mean for the project on the PC and the Web, each preset naming only what it
  changes (`(version: 1, low: (render_scale: 0.6, frame_limit: "30"))`); `trino.toml`
  `[pc.quality] default = "auto"|"low"|…` picks the first launch. Every value is checked
  by the bake. `cargo xtask quality show|set|reset|check`, or the editor's Quality tab.
- **Scenes and prefabs** (`scenes/`, `prefabs/`): see the
  [scene-authoring](../.agents/skills/scene-authoring/SKILL.md) skill.
  `cargo xtask scene check` validates them, `scene export` writes one as Rust data,
  `scene budget` reports each console's frame cost.

## Checking your work

1. `cargo xtask assets pc` (and a console build when the toolchains are installed): format
   and size problems are bake errors.
2. `cargo xtask scene check` when scenes or prefabs reference the asset.
3. `cargo xtask model inspect <file>` for models, `cargo xtask texture inspect
   sprites/x` for textures, `cargo xtask audio inspect music/x` for sounds,
   `cargo xtask impostor check` for impostors, `cargo xtask quality check` for the
   presets.
4. `cargo xtask assets check` (and `assets report`) when groups or scenes changed.
5. `cargo xtask test`.
