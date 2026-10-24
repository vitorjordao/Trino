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
| UI screen (`ui/<name>.ui.ron`, no manifest entry) | `TUIL` blob `ui/<name>` | `TUIL` blob | `TUIL` blob | `TUIL` blob in the pack |
| String table (`assets/i18n/<lang>.ron`, no manifest entry; `[i18n]` optional) | `TSTR` blob `i18n/<lang>` + `i18n/languages` | `TSTR` blob (no pseudo-locales) | `TSTR` blob (no pseudo-locales) | `TSTR` blob in the pack |

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
- **Shadows** (scene v7, prefab v5, TSCN v5, [ADR 0025](adr/0025-shadows-per-tier.md)):
  `environment.shadows: Some((tier_max: Map, distance: 40.0, softness: 160, color: (0, 0,
  0, 140), map_size: 0))` (every field defaults, `enabled: false` keeps the tuning) and an
  entity's `shadow: Some((cast: Auto, radius: 0.5, receive: false))`. `cast` is `None`,
  `Blob`, `Mesh` or `Auto` (the shape where the tier draws shapes, a blob on the N64);
  `receive: true` makes the top of the entity's collider, or else its model's bounds, a
  surface blob and planar shadows land on. Older files load with no shadows. The bake
  writes them into the TSCN (`BakedScene::shadows`, `receivers`), and `scene export` into
  `SHADOWS` and `RECEIVERS`; a game hands both to the renderer each frame.
- **Stereo 3D** ([ADR 0032](adr/0032-stereoscopic-3d.md)):
  - **The block:** `environment.stereo: Some((enabled: true, max_separation: 0.06,
    convergence: 4.0, policy: Auto, budget_us: 16667))`. Every field defaults, and
    `stereo: Some(())` is a valid start.
  - **Where it applies:** the 3DS top screen draws one picture per eye while its slider
    is up. Every other target ignores the block.
  - **Code and bakes:** `scene export` writes `STEREO`, which the game passes to
    `Renderer::set_stereo` right after `begin_frame`. TSCN carries none.
  - **Cost:** `scene budget --stereo` prints the cost per eye.

- **Post-processing** ([ADR 0033](adr/0033-post-processing-stack.md)):
  - **The block:** `environment.post: Some((exposure: 1.0, tonemap: AgX, bloom:
    Some((threshold: 0.8, intensity: 0.6, radius: 0.6)), grading: Some("luts/dusk"),
    vignette: 0.2, grain: 0.0))`. Every field defaults (`post: Some(())` is the
    neutral stack) and `enabled: false` keeps the tuning.
  - **Code and bakes:** `scene export` writes `POST` for `Renderer::set_post`; TSCN
    carries none. `scene export --platform n64 --bake-grading` grades the scene's
    colors; `bake_grading = true` on `[scenes.*]` does it for the consoles' TSCN.
  - **Glow:** an entity's `glow: Some((radius: 0.5, color: (255, 255, 255, 255),
    sprite: None, mode: Baked))` draws a halo of `sprites/glow` where the target has
    no bloom (`Always`: everywhere); TSCN v6 and codegen carry it.
  - **Before/after:** `cargo xtask scene shot <scene.ron> --platform pc|n64|3ds|web
    --post game|on|off|bloom`; `cargo xtask post check` validates the references.

## Grading LUTs

`[luts.<name>]` (issue #64): `file = "luts/dusk.png"` is a strip PNG `n²×n` — `n`
slices of blue side by side, red along x inside a slice, green down y, 2..=64 texels
per axis — or an Adobe/Resolve `.cube`. `cargo xtask lut neutral [--size 32]` writes
the identity to edit in any image tool, `cargo xtask lut inspect <file|luts/x>` says
how far one moves colors.

- **PC and Web:** baked to a `TLUT` blob (logical `luts/<name>`, the blob and
  `LutId` share the hash); hosts upload it to the renderer; `PostSettings::grading`
  names it.
- **Consoles:** no LUT at run time. `grading = "luts/<name>"` on a `[models.*]` entry
  grades its vertex colors in the N64 and 3DS bakes (on a `[sprites.*]` entry, its
  pixels); the PC bake keeps the model as authored and adds a graded twin that the
  PC's console profiles draw. Unlit colors match the PC's run-time grading exactly;
  lit ones differ by the order (graded before the light).

## UI screens

A screen is a RON file in the project's `ui/` folder (ADR 0024); the bake turns it into the
`ui/<name>` blob on every target and checks its structure (a duplicate id or a navigation
target that does not exist fails the bake). Every new project starts with `title`, `pause`,
`options` and `controls`.

```ron
(
    version: 1,
    name: "pause",
    on_back: "resume",                 // what B / Escape sends
    theme: (font: "fonts/trino5x7"),   // and colors: panel, button, focus, focus_text...
    root: (
        children: [
            (kind: Rect(), anchor: Fill, bleed: true, color: Some((0, 0, 0, 160))),
            (
                kind: Stack(gap: 6),
                anchor: Center,
                children: [
                    (kind: Text, key: "pause.title", text: "Paused"),
                    (kind: Button, id: "resume", key: "pause.resume", text: "Resume", on_click: "resume", size: (Px(120), Px(18))),
                    (kind: Toggle, id: "music", text: "Music", bind: "music"),
                    (kind: Slider(min: 0, max: 10), id: "volume", text: "Volume", bind: "volume"),
                    (kind: Text, text: "Press START", visible_on: [N64, N3ds]),
                    (kind: Button, id: "map", text: "Map", on_click: "map", screen: Bottom),
                ],
            ),
        ],
    ),
)
```

- **Kinds:** containers `Overlay`, `Stack()`, `Grid()`, `Scroll()`, `List()`; leaves `Text`,
  `Image(sprite: "sprites/x")`, `Rect()`, `Button`, `Toggle`, `Slider()`, `Model3d(model:
  "models/x")`. A kind with settings needs its parentheses even without any (`Stack()`).
- **Layout:** `size: (Fit | Px(n) | Percent(n) | Fill, ...)`, `anchor` (nine points or
  `Fill`), `offset`, `padding`, `min`/`max`, in virtual units against `reference` (320×240
  by default; whole-pixel scaling). `bleed: true` ignores the safe area; `screen: Bottom`
  is the 3DS touch screen (the main screen elsewhere); `visible_on: [N64, N3ds, Pc, Web]`.
- **Stereo depth:** `depth: 6` floats a node and its children 6 px of parallax out of
  the 3DS top screen at the full 3D slider; negative values sink it. It is TUIL v3, and
  `ui shot --platform 3ds --stereo anaglyph` shows it.
- **Data:** `text` (`{}` takes the bound value), `key` (a string table entry, `text` is
  the default), `bind` (a value the game sets: `ui.set("volume", 7)`), `visible_if` and
  `enabled_if` (a bound value), `overflow: Visible | Clip | Ellipsis | Wrap`, `nav: (down:
  "id")` overrides, `preview: {"volume": Int(7)}` for the tools.
- **String tables:** `assets/i18n/en-US.ron` is `Strings(lang: "en-US", fallback: "",
  entries: {"pause.title": "Paused"})`; `i18n/<lang>` blobs, read with `StringsBlob` or a
  `Localizer` (plurals, formats, per-language fonts: [Localization](#localization)).

The game keeps a `ScreenBlob::new("pause")`, fetches it in `update_with` (it re-parses after
a live edit), and drives a `Ui<N>` with it; a game without blobs (tests, a server) uses
`cargo xtask ui export ui/pause.ui.ron src/ui/pause_gen.rs` and `include!`s the file.

```sh
cargo xtask ui check [screen..] [--platform n64] [--lang pt-BR] [--json]   # every target × language
cargo xtask ui shot pause --platform 3ds --lang pt-BR --focus quit --out pause.png
cargo xtask ui export pause src/ui/pause_gen.rs
cargo xtask ui list --json
```

`ui check` fails on any issue: a control the pad cannot reach (an error on the N64 and the
3DS), two controls whose boxes overlap, a screen over `Caps::max_ui_nodes` or
`max_ui_draws`, a text that overflows its box in any language, a glyph the font lacks, a
font, sprite or model not in the manifest, a key in no table. The editor's
UI tab edits the same file and shows the focus order, the overflows and each target.

## Accessibility data (ADR 0027)

Three sources bake into one generated blob, `access/data`, on every target (no manifest
entry; `AccessData` / `AccessBlob` read it):

- `assets/palettes.ron`: semantic colors with an alternative per color-vision mode
  (`protanopia`, `deuteranopia`, `tritanopia`, `grayscale`; a mode left out keeps the
  `base`), `distinct` groups that must look apart in every mode (`min_delta_e`, default
  20 CIE76 ΔE) and `contrast` pairs (`min`, default 4.5:1). A group a mode can't tell apart
  fails the bake.
- The manifest: `caption`, `important`, `caption_priority`, `direction` and
  `caption_seconds` on a `[sounds.*]` or `[music.*]` entry; `cvd_icon = "sprites/x"` on a
  `[sprites.*]` entry (the sprite a color-vision mode shows with it).
- `ui/themes/high_contrast.ron`: the UI's high-contrast theme (a screen `theme`; what it
  leaves out is the engine's `Theme::HIGH_CONTRAST`).

A UI node takes `cvd_icon: "sprites/x"` too (TUIL version 2). Caption keys live in the
same `assets/i18n/<lang>.ron` tables as the screens' keys. `cargo xtask access check`
validates all of it and lays every screen out at 200 % text;
[ACCESSIBILITY.md](ACCESSIBILITY.md) shows the game side.
## Achievements (ADR 0029)

`assets/achievements.ron` (achievements, stats, rich-presence keys; the editor's Project
› Achievements page) bakes to the `services/achievements` blob (`TACH`) on every target,
with no manifest entry, beside the UI blobs. An achievement's bit in a save is its place
in the file, frozen by `assets/achievements.lock` (`cargo xtask services lock`). Names and
descriptions are string keys; icons are sprites. `cargo xtask services check` validates,
`services export steam` writes Steamworks' files. See [SHIPPING.md](SHIPPING.md).

## Localization

One file per language, `assets/i18n/<lang>.ron`, named after its `lang` ([ADR
0026](adr/0026-localization.md), issue #59; the `localization` skill has the loop):

```ron
Strings(
    lang: "pt-BR",
    fallback: "en-US",                 // keys this table lacks come from en-US
    name: "Português (Brasil)",        // for a language menu (Localizer / StringTable::name)
    locale: (currency: "R$"),          // optional: overrides of the tag's defaults, written bare
    entries: {
        "menu.play": "Jogar",
        "guests.count": Plural(one: "{n} convidado", other: "{n} convidados"),
        "money": Format("{amount:currency}"),
    },
)
```

- **Entries:** text, `Plural(zero:, one:, two:, few:, many:, other:)` (the forms the
  language's CLDR rule uses; `plural: "Slavic"` overrides the rule) or `Format("…")`.
  Placeholders `{n}`, `{n:int}`, `{y:raw}`, `{x:num}`, `{amount:currency}`, `{d:date}`,
  `{s:str}`; `{{` and `}}` are braces. Number, money and date formats come from the
  language's `Locale` (`R$ 1.200,00`, `$1,200.00`, `1200,00 €`; dates `31/12/2026`,
  `12/31/2026`, `2026/12/31`).
- **Bake:** every shipped language → `i18n/<lang>` (`TSTR` v2: the locale, plural entries,
  the fonts it draws with) and `i18n/languages` (the tags, the default first), on every
  target, no manifest entry. A `qps-*` pseudo-locale bakes on the PC and the Web only.
- **Manifest:**

  ```toml
  [i18n]
  default = "en-US"
  languages = ["en-US", "pt-BR"]   # shipped (default: every table but pseudo-locales)
  extra_chars = "0123456789"       # every auto font also bakes these
  groups = true                    # per-language atlases in group i18n-<lang>
  check_fonts = ["fonts/ui"]       # what `i18n check` holds to each language
  blobs = false                    # the game keeps its tables in code (i18n export --rust)

  [i18n.overrides.ja-JP]
  "fonts/ui" = { pc = "fonts/ui-ja", web = "fonts/ui-ja" }

  [fonts.ui]
  file = "fonts/Roboto.ttf"
  size = 11
  chars = "auto"                   # exactly the glyphs the languages and the screens use
  extra_chars = "#*"
  per_language = true              # fonts/ui (the default's) + fonts/ui@pt-BR, ...
  ```

  `chars = "auto"` (TTF/OTF) bakes the shipped languages' glyphs, the UI screens' literal
  texts, the extras, the space and `?`. A glyph nothing uses is not baked.
  `per_language` gives each language its own atlas, smaller and N64-TMEM friendly; its
  `TSTR` maps the font to it and the UI swaps it with no game code.
- **Runtime:** `Localizer::load(platform)` reads every language; `pick(saved,
  platform.system_language())`; `tr(StrKey::new("menu.play"))`, `tr_fmt(key, &[Arg::money
  ("amount", 120_000)], &mut TextBuf)`, `plural`, `font`, `group`; the UI takes a
  `Localizer` as its `UiStrings`.

```sh
cargo xtask i18n check [--platform n64] [--strict] [--dir examples/x] [--json]
cargo xtask i18n pseudo [--from en-US] [--expand 40]      # assets/i18n/qps-ploc.ron
cargo xtask i18n export --csv strings.csv                  # import --csv to read it back
cargo xtask i18n export --rust src/strings_gen.rs          # static StringTable code
cargo xtask i18n stats | fonts [--platform n64] | list [--json]
```

`i18n check` fails on missing keys no fallback covers, placeholders that differ from the
default language's, glyphs a font lacks for a language on a target, and UI text that
overflows in a language on a target. It warns on plural forms a rule needs, keys only one
language has and atlases past the N64's TMEM, and notes keys nothing uses. The editor's
**Localization** tab edits the same files.

## Checking your work

1. `cargo xtask assets pc` (and a console build when the toolchains are installed): format
   and size problems are bake errors.
2. `cargo xtask scene check` when scenes or prefabs reference the asset.
3. `cargo xtask model inspect <file>` for models, `cargo xtask texture inspect
   sprites/x` for textures, `cargo xtask audio inspect music/x` for sounds,
   `cargo xtask impostor check` for impostors, `cargo xtask quality check` for the
   presets.
4. `cargo xtask assets check` (and `assets report`) when groups or scenes changed.
5. `cargo xtask ui check` when screens or string tables changed.
6. `cargo xtask access check` when palettes, captions, the high-contrast theme or screens
   changed (every screen at 200 % text).
7. `cargo xtask test`.
