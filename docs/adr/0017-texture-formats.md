# ADR 0017: Texture formats — 3DS hardware formats, a decoded preview, `texture inspect`

Status: draft (October 2026, GitHub #52). Phase 1 of the issue's roadmap is
implemented; Phases 2–5 (`auto` and the PSNR guard, PC BCn, the editor's texture
inspector, Web KTX2) are not.

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

## Consequences

- No existing texture changed format; RomFS, linear memory and every older golden
  are unchanged. Switching castle64's and Solid Eleven's textures is Phase 2 (with
  `auto` and the PSNR guard).
- Two traps cost time and are in AGENTS.md › Pitfalls: `C3D_Tex::fmt` is a signed
  4-bit bitfield (GPU_A8 reads back as -8), and the engine's citro2d tint mode is
  multiply, so a black texel stays black under any tint.
