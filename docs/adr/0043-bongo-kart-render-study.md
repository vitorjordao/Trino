# ADR 0043: What Bongo Kart's study asked of the renderer — mipmaps, additive blending, a two-color ambient with rim light, a mix of two LUTs

Status: accepted (November 2026). Bongo Kart's study (its epic #24) ranked the engine
changes the game needs by value; this ADR covers the renderer's: GitHub Bongo-Kart #178
(§1), #179's blend half (§2) and #181 (§3).

## 1. Mipmaps on the PC and the Web (Bongo-Kart #178)

### Context

Every sprite went to the GPU with one level (`mip_level_count: 1`). A texture that tiles
a large surface seen at a grazing angle — Bongo Kart's beach, harbor and swamp water
(its #168), a road, a checkered floor — then shimmers in moiré rings: each pixel samples
one texel out of the dozens it covers, and which one changes every frame as the camera
moves. It also costs cache misses on the GPU.

### Decisions

- **Opt-in per sprite:** `mipmaps = true` in the manifest. A chain costs a third more
  memory, and most of a game's sprites (the HUD, fonts, atlases) never get minified in
  3D. Opt-in also keeps every existing golden byte for byte.
- **Powers of two only:** the halving is exact, every level is whole 4×4 blocks (what BC
  and Basis encode) down to 4 texels on the smaller side, and the textures that tile are
  powers of two on every console anyway. Another size fails the bake with its size in the
  message.
- **One generator in the core** (`trino_core::mip`, `no_std`): the PC's bake (BC levels),
  the Web's (KTX2 levels), the renderer's uploads of plain pixels and the tests make the
  same chain. Each texel is the mean of its 2×2 parents in **linear light**, weighted by
  alpha (a transparent texel's color doesn't bleed into its neighbors); alpha is the plain
  mean. Averaging sRGB codes instead would darken a fine black and white pattern to code
  128 (22 % light) where the eye sees 50 % (code 188): far floors would look darker than
  near ones.
- **Generated in the bake, chosen by the sampler, ignored by the consoles** (the issue's
  wording):
  - the PC's `<id>.bc` holds every level after level 0 (`BcFormat::encode_chain`,
    `levels`); the master's magic is `TSPM` instead of `TSPR` (same layout), so a host
    without BC makes the chain from the pixels at upload;
  - the Web's KTX2 holds every level: the Basis encoder takes the chain as user mipmaps,
    `basis_to_ktx2` moves one image descriptor per level into the BasisLZ global data and
    writes the levels smallest first (as Binomial's own writer does); the browser
    transcodes each level to the GPU's format, or decodes level 0 and rebuilds the chain
    where it has none;
  - the renderer keeps two views of a chained texture: level 0 for 2D draws, `Point`
    draws and the console profiles, the whole chain for bilinear 3D draws where
    `Caps::mipmaps` is on (the PC's native profile, the Web). The samplers' mip filter is
    linear (trilinear); the anisotropic sampler of the quality presets already was;
  - the consoles bake one level: the N64's TMEM has no room for a chain worth having, and
    the 3DS's bakes stay as they are. `Caps::mipmaps` is false on both.
- **A switch for A/B:** `PcRenderer::set_mipmaps(false)` or `TRINO_MIPS=0` samples level
  0 everywhere; `samples_mipmaps()` and `sprite_levels()` report what a renderer does.

### Measurements

`crates/platform-pc/tests/mipmaps.rs`, a 40 × 120 m floor tiled with a 32-texel
one-texel checker, eye 1.2 m up looking at the horizon, 480 × 270 (RADV). The far band is
the 40 rows nearest the horizon; "step" is the mean luma step between horizontal
neighbors (the moiré), "mean" its mean luma (188 is right):

| Preset (anisotropy) | Without a chain: mean / step | With it: mean / step |
|---|---|---|
| none (the test's default sampler) | 127.5 / 51.1 | 187.0 / 0.5 |
| Low (1×) | 127.3 / 22.6 | 187.7 / 0.0 |
| Medium (4×) | 127.5 / 23.5 | 151.1 / 11.4 |
| High (8×) | 127.4 / 18.3 | 139.0 / 13.8 |
| Ultra (16×, 1.25× scale) | 127.4 / 12.9 | 127.8 / 12.8 |

The anisotropic presets sample a sharper level along the view; a one-texel checker sits
right at the Nyquist limit there, so some moiré stays at mid distance: the worst case.
Bongo Kart measures its own water (beach, harbor, swamp) in its PR. Memory: 32² RGBA8
4 096 → 5 440 bytes (+33 %).

### Tests

`trino_core::mip` (4 unit tests), `crates/asset-pipeline/tests/mipmaps.rs` (the PC's
`TSPM` + BC chain split by the loader, the Web's KTX2 levels, the consoles' bakes
unchanged, odd sizes refused), the KTX2 container (`a_mipmapped_texture_carries_its_chain_
level_by_level`), `crates/platform-pc/tests/mipmaps.rs` (the far floor with and without,
per preset; the console profiles and 2D draws unchanged; the BC chain within 2/255 of the
RGBA8 one; wrong chains refused), and castle64's `mipmaps` self-test step (the minified
checker reads its mean light on the PC's native profile, `engine_checks` also fails it
with the chain off; the consoles and the PC's console profiles report level 0).

## 2. Additive blending (Bongo-Kart #179, its blend half)

### Context

Sparks off a drifting kart, boost flames, glows, lasers: light added to the picture.
With only alpha blending a spark covers what is behind it with its own color and a cluster
of sparks looks like a pile of stickers; added, they brighten what they cross and
overlapping ones get hotter, in any order.

### Decisions

- **One field:** `ModelParams::blend: Blend::{Alpha, Add}` (default `Alpha`, the old
  behavior: every golden unchanged). `Add` is `dst + src·a`: depth tested, never written,
  ignored by water surfaces (they blend their own way). Materials have `blend: Add`
  (`BlendDef`, the editor's material inspector); baked scenes (TSCN) carry it in the byte
  the surface record kept reserved, so older files read `Alpha` and no version moves;
  scene codegen writes it.
- **Every target, the CPU T&L's triangles:** an additive draw never takes a hardware T&L
  path — the N64's RSP and the 3DS's PICA200 report it as `Translucent`, the PC's GPU T&L
  as a new `PcGpuReject::Additive` — and the N64's draw memo never keeps one. Effects are
  small; the CPU path already sorts translucent triangles, and it keeps one place where a
  blend mode is chosen per triangle.
  - **PC and Web:** the triangles sort among the batch's blended ones (back to front:
    adding commutes, blending over them doesn't) and take three new scene pipelines (tri,
    tri clamp, lit) with `SrcAlpha, One`. With the post stack's HDR target a sum past
    white keeps its light and the bloom picks it up. The N64 look keeps the full alpha
    for them (`fs_tri_add`, like `fs_shadow`): the RDP's additive blender weighs by the
    combiner's alpha, not the framebuffer's one bit.
  - **N64:** a bucket after the water, bound per texture, then `trino_3d_additive(1)`:
    `RDPQ_BLENDER_ADDITIVE`, z compared and not written, no alpha compare;
    `trino_3d_additive(0)` makes the next draw bind its own mode. The RDP doesn't saturate:
    a sum past white wraps toward black. Keep additive colors dim (half intensity or less)
    where they cross bright things on the N64; the docs say so on `Blend::Add`.
  - **3DS:** a bucket after the water; `trino_3d_material` bit 14 sets `GPU_BLEND_ADD` with
    `GPU_SRC_ALPHA, GPU_ONE` and depth writes off (and gives citro back its blend after).
- **Not done here:** additive billboards and particles (`draw_billboard`, `Emitter`) keep
  their blend; a game draws an additive spark as a model or a dynamic mesh. The other half
  of #179, positional voices, is ADR 0044.

### Tests

`crates/core/tests/blend.rs` (the three hardware paths reject an additive draw, the memo
skips it, water keeps the GPU path), the material round trip and the TSCN byte
(`trino-scene`, `baked_scene_reads_every_field`), `crates/platform-pc/tests/additive.rs`
(a spark adds its light, alpha ones cover; two sparks read the same in either depth order;
no depth written, hidden behind walls; the GPU T&L leaves it to the CPU path with the same
frame; the console profiles add), evidence `target/evidence/additive/` (a spark shower with
and without, per profile), the editor's material widget test, and castle64's `additive`
self-test step (the card added over a backdrop reads their sum; on PC, ares, Azahar and the
browsers, `engine_checks` on all three profiles and both T&L paths).

## 3. A two-color ambient, a rim light and a mix of two LUTs (Bongo-Kart #181)

### Context

Bongo Kart lights its karts under one ambient color: a kart's underside reads as bright as
its roof, and at dusk a kart melts into the track behind it. Two cheap terms fix both:

- a **two-color ambient** (the sky's color on what faces up, the ground's on what faces
  down), so the shading follows the shape;
- a **rim light**, so a silhouette separates from the background.

Its time of day and weather wanted a look that passes between two LUTs (#89 of the game)
without baking one per step.

### Decisions

- **`Light::ground: Option<Color>` and `Light::rim: Rim { color, strength }`**:
  - Defaults are `None` and `Rim::NONE` (strength 0). `DEFAULT_LIGHT` and `Light::DEFAULT`
    shade as before, and every golden is unchanged.
  - `Light::has_terms()` tells whether either is on.
  - With a ground, `ambient_color` is the sky's. The ambient is mixed from the ground to the
    sky by `n.y·0.5 + 0.5` of the world normal.
  - The rim adds `(1 − n·v)²` of its color × strength, where v points from the vertex to
    the eye. It is a per-vertex Fresnel-like term: no new pass, no new blend.
- **Where it is evaluated:**
  - **CPU T&L, every target:** `render3d/lit.rs`'s `Terms`. The per-vertex shade there is
    out of line (`Terms::light`), and the static path (`tessellate_joints`) is untouched.
    Its body stays within the N64's 16 KB I-cache guard.
  - **Dispatch:** `tessellate_with` sends every lit draw under such a light to the lit
    copy, as it does when a local light reaches one.
  - **PC and Web:** the GPU T&L evaluates the same terms in `mesh_vertex`
    (`MeshCtx::{ground, rim, eye}`). The tnl oracle holds the two paths to a mean of
    0.000/255.
  - **N64 and 3DS:** the RSP overlays and the PICA200's shader don't know the terms, so a
    lit draw under them leaves the hardware path (`RspReject::LightTerms`,
    `GpuReject::LightTerms`, "sky/ground or rim light"). Unlit draws stay on it, and water
    ignores the terms.
  - **N64 memo:** `set_light` bumps the memo's generation, so no old command answers a
    draw the terms moved to the CPU.
- **Scenes:** `environment.light` takes `ground_color` (written bare: `ground_color: (40, 30,
  20)`), `rim` and `rim_color`. Unset fields stay out of the file, codegen writes both
  fields, and the editor's Inspector › Light has Ground and Rim rows. Neither is in TSCN:
  `BakedScene::light()` returns the one-color ambient.
- **The rim lights everything under that light**, the ground too, which a camera sees at a
  grazing angle (`n·v` small). A game gives its characters a rimmed light and the level the
  plain one, with `set_light` between the two groups of draws. Two caveats:
  - **The N64 memo:** each light change bumps its generation, so on the N64 two lights a
    frame rebuild every RSP draw (no memo hits). There, keep one light, or give it the rim
    only where the characters are the whole picture.
  - **Big triangles:** a view-dependent color changes across one triangle. On a big
    triangle crossing the guard band, the CPU path's clipped polygon and the GPU's
    whole-triangle interpolation differed by up to 11/255. Tests and levels use subdivided
    floors (as fog already asked).
- **Two LUTs:**
  - `PostSettings::grading_to: Option<LutId>` and `grading_mix: f32` (0..=1, clamped;
    NaN is 0).
  - The composite samples the second LUT at the same tonemapped color and mixes:
    `trino_core::post::grade_mix` is the CPU reference. A first LUT that is `None` mixes
    from the ungraded color.
  - Weight 0, or a missing second LUT, is the old frame byte for byte. The composite has
    one more 3D binding (6), and its uniform grows 96 → 104 bytes.
  - The Full tier only: the consoles' bakes grade by `grading` alone, and the Baked tier
    ignores the post settings as before.
  - Scenes' `post` takes `grading_to` and `grading_mix`, the editor has "Mix to" and "Mix",
    and codegen writes them. They are not in TSCN, like the rest of `post`.

### Tests

- `crates/core/tests/light_terms.rs`:
  - the default light has no terms;
  - a ground of the sky's color shades within 1/255 of the old light;
  - sky on top, ground below;
  - the rim lights edge-on vertices, not facing ones, and its strength scales;
  - the RSP and the PICA200 reject lit draws, keep unlit ones, and the PC GPU keeps both.
- `post.rs`'s `two_luts_mix_by_the_weight`.
- `crates/platform-pc/tests/light_terms.rs`:
  - GPU against CPU T&L with the floor under another light;
  - the old light from a ground of the sky's color, on both paths;
  - top > middle > bottom and the middle at the expected share, a rim that lights the
    silhouette and spares the middle, on the native profile (both paths) and the N64 and
    3DS profiles;
  - evidence in `target/evidence/light-terms/`: dusk, + sky/ground, + rim on everything,
    + rim on the objects only, per profile.
- `post.rs`'s `two_luts_mix_like_the_cpu_reference`: every swatch at weights 0, 0.3, 0.7
  and 1 against `grade_mix`, from a first LUT or none; weight 0 and a missing LUT are
  byte for byte the first grading.
- `authoring.rs`'s round trip; the codegen test (`export.rs`); the editor's post UI test.
- castle64's `light terms` and `lut mix` self-test steps, on PC (`engine_checks`: three
  profiles, both T&L paths), ares, Azahar and the browsers.
