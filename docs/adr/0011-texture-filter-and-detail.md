# ADR 0011: Texture filtering per draw and a detail layer

Status: accepted (October 2026)

## Context

Small console textures (32×32, 64×64) get magnified a lot. Games need two controls the
engine didn't give them:

- the filter, to choose between crisp pixel art and the N64's smooth look;
- a detail layer, a second small texture tiled finer over the first, which hides the
  magnification without a bigger texture. TMEM is only 4 KB.

## Decisions

### 1. `ModelParams::filter` and `Renderer::set_texture_filter`

- `TexFilter::{Default, Point, Bilinear}` per draw, in material files (`filter:`) and
  baked scenes.
- `Default` is the target's own filter:
  - the N64: the RDP's 3-point bilinear;
  - the 3DS: bilinear;
  - the PC native profile: nearest, as before.
- `set_texture_filter` changes what `Default` means for the next draws. It persists,
  like `set_light`.
- Backends resolve the filter per draw into a `render3d::TexBind` (sprite, clamp, filter,
  detail). Their batches group and bind by it.
  - N64: `rdpq_mode_filter(FILTER_POINT | FILTER_BILINEAR)`.
  - 3DS: `C3D_TexSetFilter(GPU_NEAREST | GPU_LINEAR)`.
  - PC: a nearest or linear sampler. The N64 look keeps filtering in the shader:
    3-point, or a point fetch for `Point`.

### 2. `ModelParams::detail`: the N64 detail tile

- `DetailTexture { sprite, scale_log2 }` is multiplied over the base texture and tiles
  `2^scale_log2` times per base repeat over the same UVs. It needs no new TMDL data.
- **N64:** `trino_3d_texture_ex` uploads both textures with `rdpq_tex_multi_begin`.
  - The detail goes in `TILE1`, its `scale_log` shifted by the size ratio, so the
    repeat count doesn't depend on the two textures' sizes.
  - The 2-cycle combiner computes `(TEX0 × SHADE) × TEX1`, keeping the base's alpha.
  - Both textures share TMEM's 4 KB, and a palette (CI4/CI8) takes TMEM's upper half.
    So the shim draws without the detail when the pair doesn't fit or either one is
    CI. PC strict mode panics on an oversized pair: a small I4 detail on a 16-bit or
    intensity base fits.
- **3DS:** texture unit 1 holds the detail. The vertex shader outputs `texcoord1` as
  `texcoord0` times a uniform scale, and TexEnv stage 1 multiplies by it.
- **PC:** the 3D bind group has the base texture, the detail texture and their samplers.
  The shader multiplies `uv × scale` (white when there is no detail).

### 3. Baked scenes: TSCN v3

Nodes end with 8 surface bytes: gloss, filter, the detail's scale and the detail sprite.
Version 2 blobs are refused (rebake). Gloss (ADR 0010) was missing from baked scenes too.

## Consequences

- No filter or detail set: every golden and every console frame is unchanged, apart from
  the showcase's pillars, which now use a detail material.
- The showcase's `tex_bricks` changed from CI8 to RGBA5551 on the N64 (2 KB) so the I4
  grain can share TMEM with it.
- Billboards keep their profile filter and have no detail layer.
