# ADR 0017: Texture formats — hardware formats per platform, `auto`, PC BCn

Status: accepted (October 2026, GitHub #52). Phases 1–4 of the issue's roadmap are
implemented (§1–5: 3DS formats and `texture inspect`; §6: `auto` and the PSNR guard;
§7: PC BCn; §8: the editor's texture inspector). Phase 5, Web KTX2, belongs to the web
target and moves to its issue (#47).

## Context

The 3DS bake called `tex3ds` with `RGBA8` or `RGB565` only. The PICA200 samples
ETC1 (4 bits a texel) and ETC1A4 (8), RGBA4, RGBA5551, LA8, L8 and A8 natively:
a 256×256 RGBA8 texture is 256 KB of linear memory, its ETC1 is 32 KB. Authors
had no way to see what a lossy format would do before booting Azahar, and no tool
said what a texture costs on each platform.

## Decisions

### 1. The manifest names the hardware format; `tex3ds` encodes it

- `formats = { 3ds = "ETC1" }` accepts `RGBA8`, `RGB565`, `RGBA5551`, `RGBA4`,
  `LA8`, `L8`, `A8`, `ETC1` and `ETC1A4` (`Platform::supported_texture_formats`);
  an unknown one fails the bake with the list. RGBA8 stays the 3DS default: no
  silent quality change for existing content.
- `trino_asset_pipeline::N3dsFormat` owns the names, the `tex3ds -f` spelling,
  bits per texel and the linear-memory size (power-of-two sides, at least 8). The
  3DS bake keeps `tex3ds` (the `.t3x` container the shim loads through
  `C2D_SpriteSheetLoad`), now for every format.
- The PC bake validates the 3DS format too, so a typo fails on the PC before a
  console build.

### 2. Channel semantics are the engine's, not the PICA200's

The same texture must look the same on every target, so the formats mean:

| Format | Sampled as |
|---|---|
| RGBA8, RGB565, RGBA5551, RGBA4 | the color (565 opaque, 5551 alpha ≥ 128) |
| ETC1 | the color, opaque |
| ETC1A4 | the color with 4-bit alpha |
| L8 | gray (Rec. 709 luminance of the linear color, back to sRGB), opaque |
| LA8 | gray with alpha |
| A8 | **white** with alpha: a mask tinted at draw time (fonts, glows) |

The PICA200 samples A8 as (0, 0, 0, a). The shim makes it white
(`trino_shim_3ds.c`, `tex_is_mask`): a 3D TexEnv takes such a texture through
`GPU_TEVOP_RGB_ONE_MINUS_SRC_COLOR` (on the base, detail and water stages), and a
sprite blit of one switches citro2d to `C2D_TintSolid` for that draw (the tint's
color, the texel's alpha; the engine's tint mode is otherwise `C2D_TintMult`).

### 3. The PC previews the decoded result, from the bake

- `N3dsFormat::preview` reproduces `tex3ds`' result in Rust: 16-bit formats
  truncate (`>> 3`, `>> 4`), RGBA5551 keeps alpha ≥ 128, a fully transparent
  texel becomes 0 in every channel, luminance as above, n-bit channels expand by
  bit replication like the PICA200. ETC1 has its own encoder (both flips,
  individual and differential modes, five base candidates per half, every table),
  close to `tex3ds`' medium quality.
- The PC bake writes `<id>.3ds.sprite` next to `<id>.sprite` when a sprite's (or
  a font atlas's) 3DS format is not RGBA8, and the index records it as
  `preview_3ds`. `LoadedSprite::pixels_for(Platform::N3ds)` picks it;
  `trino-runner-pc`, the editor's Game view (3DS) and the PC tests on the 3DS
  profile upload it, so `TRINO_SIM=3ds` shows ETC1 blocks and 4-bit banding. The
  PC renderer is untouched: it receives other pixels.
- **Verified against the real encoder**
  (`crates/asset-pipeline/tests/texture_formats.rs::preview_matches_tex3ds`, run
  where devkitPro is installed): every uncompressed format texel for texel
  (luminance within 1), ETC1/ETC1A4 within 1.5 dB of `tex3ds`' own PSNR (measured:
  28.75 vs 28.77 dB on the opaque fixture). `texture::decode_tex3ds_raw` reads
  `tex3ds -r` output (8×8 Morton tiles, ETC1 blocks little-endian in Z order,
  ETC1A4's alpha first, column-major nibbles).

### 4. `cargo xtask texture inspect`

`texture inspect <sprites/x|fonts/x> [--platform pc|n64|3ds] [--json]` prints per
platform the format, the bytes, the PSNR against the master (alpha-premultiplied
color plus alpha, so invisible texels don't count) and the share of the platform's
texture memory: TMEM on the N64 (texel rows padded to 8 bytes, a palette's TLUT in
the upper half), `Caps::N3DS.texture_memory_bytes` on the 3DS. With devkitPro
installed the 3DS line adds the PSNR of the real `tex3ds` output
(`psnr_tex3ds`). The N64's palette and intensity conversions (`mksprite`) are not
modeled: their PSNR is reported only when lossless.

### 5. The self-test proves the shim samples every format

`castle64::selftest::texture_formats` draws one 32×32 card per 3DS format, each
holding quadrants its format stores exactly, as a sprite and as a textured 3D card,
and probes every quadrant (a transparent one must show the background). The N64
bakes the cards as RGBA5551, so the same table holds on every target. The 3DS
self-test dumps the frame, with a photo-like card in RGBA8, ETC1 and RGBA4, as the
`tests/golden/3ds/texture_formats.png` golden, and `n3ds_parity` holds the PC's 3DS
profile to it.

### 6. `auto` measures; the threshold is the author's

- `formats = { <platform> = "auto" }` measures every format of the platform against
  the master (the same PSNR as §4) and takes the smallest whose PSNR reaches the
  threshold. Ties: the higher PSNR; on the N64, formats that fit TMEM come before
  smaller ones that don't. No content heuristics decide (an ETC1 of an image with
  alpha simply measures low); the content class (`white mask`, `opaque color`, …) is
  only reported.
- The threshold is `min_psnr` when set, else `quality` (`low` 30, `normal` 35,
  `high` 40 dB). An explicit format is the author's call: it is checked only when the
  texture sets `min_psnr`, and a miss fails the bake with the formats that would pass
  and what `auto` would pick. A texture nothing keeps fails with the best measured.
- **Defaults change:** the PC and the 3DS default to `auto`; the N64 keeps "declare
  one" (fonts `CI4`), because N64 textures are drawn for TMEM by hand and `mksprite`'s
  palette quantizer is not modeled. The N64 model: `I4`/`I8` keep a gray master's
  level, otherwise Rec. 601 luma × alpha stretched to min–max, sampled as
  (i, i, i, i); `CI4`/`CI8` are measured only when the master's colors after RGBA5551
  truncation fit the palette.
- One function decides (`texture::choose`): the PC bake (which also writes the 3DS
  preview of the 3DS choice), the 3DS and N64 stagers, `texture inspect`,
  `texture report` and the editor call it on the same decoded master, so they never
  disagree. `report` fails (exit 1) when any texture fails.
- Consequence for content: a full alpha ramp in color keeps ~33 dB in 4-bit alpha, so
  such textures stay RGBA8 at the default; opaque art goes to ETC1/BC1 when it holds
  35 dB, and photo-like masters often don't (RGB565 then).

### 7. PC BCn: blocks baked, uploaded where sampled, decoded elsewhere

- `BC1`, `BC3` and `BC7` (`BcFormat`). BC1/BC3 are encoded by `texpresso` (cluster
  fit); BC7 by the pipeline's own mode-6 encoder (principal-axis endpoints, p-bits,
  two least-squares refits) — one mode, but always at least BC1's quality at BC3's
  size. Every decode (PSNR, previews, the fallback) is `bcdec_rs`.
- BC4/BC5 are not offered: wgpu has no texture swizzle, so a one-channel texture
  can't be sampled as gray or as a white mask like the other targets.
- Sides must be multiples of 4 (wgpu's rule for block formats); others are not
  measured and stay RGBA8.
- The bake writes `<hash>.bc` (`TBCN`, width, height, format code, blocks) beside the
  master, `compressed` in the index. `PcRenderer` asks for
  `TEXTURE_COMPRESSION_BC` when the adapter has it (unless `TRINO_BC=0`) and
  `upload_sprite_bc` creates a `Bc*RgbaUnorm` texture (unorm like the RGBA8 sprites).
  Without BC, `trino_runner_pc::upload_sprite` uploads the decoded blocks: the same
  picture, logged once. `PcRenderer::sprite_memory` and the smoke-exit
  `TRINO_TEXTURES` line report the VRAM. The console profiles of the PC keep drawing
  the console's own texture (`pixels_for`).

### 8. The editor's texture inspector

Assets › Texture shows a sprite or font on the three platforms side by side: the
choice and why, every candidate with bytes and PSNR, the decoded preview against the
master, and the share of the platform's texture memory. Its controls write the
manifest through `manifest::edit::set_texture_format` / `set_texture_key`, which touch
one key and keep the rest of the file as written, and the watcher rebakes.

## Consequences

- Phase 1 changed no texture. Phase 2's `auto` default moves the engine's and Solid
  Eleven's 3DS and PC textures to smaller formats where they keep 35 dB; the 3DS
  goldens that show them moved within their tolerance (the PR quotes the numbers).
- Two traps cost time and are in AGENTS.md › Pitfalls: `C3D_Tex::fmt` is a signed
  4-bit bitfield (GPU_A8 reads back as -8), and the engine's citro2d tint mode is
  multiply, so a black texel stays black under any tint.
