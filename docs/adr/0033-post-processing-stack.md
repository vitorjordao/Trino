# ADR 0033: Post-processing per tier, and the daltonization filter

Status: accepted (October 2026, GitHub #64; the speed blur, §7, November
2026). Implemented in `trino-core` (`post`, `Caps::post_tier`,
`Renderer::set_post`, `Renderer::set_speed_blur`, `access::color::correction`,
`AccessSettings::cvd_filter`, `SceneNode`/`Entity::glow`, TSCN v6),
`trino-render-wgpu` (the HDR scene, bloom, the composite, the speed blur, the
correction filter), `trino-asset-pipeline` (`[luts.*]`, `grading`, `bake_grading`, graded
twins), `trino-scene` (`environment.post`, the `glow` component, codegen), the
PC/Web hosts, the editor, xtask and castle64's self-test.

## Context

The PC renderer wrote straight to an 8-bit target; its only full-screen pass was
the SSAO. Local lights and highlights clipped at white, there was no bloom, no
color grading, vignette or grain, and no plan for the consoles, which can't run a
full-screen pass at all. Issue #60 left a second item here: a **correction**
filter (daltonization) for players with a color-vision deficiency, built on the
simulation pass it shipped.

## Decisions

### 1. A tier per target (`Caps::post_tier`)

- `PostTier::Full` — the PC's native profile and the Web: the 3D draws into an
  HDR target, then bloom and one composite (exposure, tonemap, LUT, vignette,
  grain) write the picture the HUD draws on.
- `PostTier::Baked` — the N64, the 3DS and the PC's console profiles: no
  full-screen pass. The bake grades colors (§4) and `Glow` halos stand in for
  bloom (§5).

`Renderer::set_post(Option<&PostSettings>)` has a default no-op and is cleared at
`begin_frame`, like the AO: no `GAME_API_VERSION` bump.

### 2. The PC/Web stack

- **Off by default and allocation-free when off.** Without `set_post` no target,
  pipeline or pass exists and every frame is the old one (every existing golden
  passes unchanged; `post_off_is_the_old_frame_byte_for_byte` proves that a frame
  after a post frame is byte-identical to one before).
- **The scene targets.** A post frame draws its 3D into the render scale's scene
  targets (`SceneTargets`, which gained a format), in `Rgba16Float` where the
  adapter renders and blends it (`EXT_color_buffer_float` on WebGL2), else in the
  8-bit format (the stack still grades, vignettes, grains and blooms; only the
  range past 1 is lost). The split after the frame's last 3D draw (the AO's)
  keeps the HUD at LDR.
- **HDR twins.** A pipeline's color format must match its attachment, so every
  pipeline that writes the scene's color has an `Rgba16Float` twin per sample
  count (`post::HdrSet`: the four scene pipelines, the two water ones, the blob and
  planar shadows, the SSAO and the shadow map's composite; the GPU T&L's live in
  `GpuTnl`). They are built on the first post frame or by a precompile job
  (`JobKind::Hdr`, `HdrGpu`, `Post`), and the MSAA color target switches format
  with the post stack.
- **Order:** 3D (HDR) → shadow map → AO → bloom → composite (decode to linear →
  exposure → + bloom → tonemap → encode → LUT → vignette → grain), which also
  stretches a scaled scene onto the picture (the upscale's bilinear and CAS,
  sharpened on tonemapped values) → 2D/HUD → the correction filter → the CVD
  simulation view.
- **What is HDR in Trino.** The shaders write display-encoded color; vertex
  colors (both T&L paths) are 8-bit, so only per-pixel lights, highlights and the
  wet floor exceed 1 (`the_hdr_target_keeps_highlights_past_white`). Bloom
  therefore thresholds in linear light with a soft knee from half the threshold,
  and the default threshold (0.8) lets an unlit white glow.
- **Bloom** is a dual-filter chain (Bjørge 2015): a 4×4 prefilter with the
  threshold into level 0 (half or a quarter of the picture), 5-tap downs and
  8-tap tent ups added onto each level (`One + One`). The chain is sized from the
  picture, not the scene, so dynamic resolution allocates nothing per step.
- **The math is the core's.** `trino_core::post::{tonemap, tone, Lut::apply}`
  (no_std, own `log2`/`exp2`/`pow`) are the reference; the shader is held to them
  per tonemap and exposure, both T&L paths (≤ 2/255), and to the LUT (≤ 3/255).
  Tonemaps: none (clamp), Reinhard, Narkowicz's ACES fit, AgX (Wrensch's minimal
  fit, its published inset/outset matrices).
- **Quality presets** (`PostSetting`, #55): Off keeps the tonemap and grading and
  drops bloom, vignette and grain; Low blooms from a quarter over ≤ 3 levels,
  Medium from half over 4, High from half over 6. Never applied = High (smoke
  runs and goldens pin High).
- **Tools:** `PostView` (game / off / on / bloom) for the editor and
  `TRINO_POST`; `PcFrameTimes::{post_passes, bloom_levels}` and `TRINO_FRAME`'s
  `post_frames= bloom=`.

### 3. LUTs as `TLUT` blobs

`[luts.<name>]` takes a strip PNG (`n²×n`: n slices of blue, red along x, green
down y; `cargo xtask lut neutral` writes the identity) or a `.cube`. The PC and
Web bakes write a `TLUT` blob (`b"TLUT"`, version 1, size 2..=64, `size³` RGB8,
red fastest). Hosts already load blobs; each one also hands any blob starting
with the magic to `PcRenderer::take_lut_blob`, which uploads a 3D texture
(trilinear in hardware). `PostSettings::grading` is a `LutId` (the logical
path's hash, the blob's id). Not a sprite: `auto` would compress it.

### 4. Baked grading on the consoles

- `grading = "luts/x"` on a `[models.*]` or `[sprites.*]` entry: the N64 and 3DS
  bakes grade its vertex colors (after the AO) or pixels with `Lut::apply_color`.
- The PC bake keeps the model ungraded (the stack grades the frame) and writes a
  **graded twin** (`graded` in the index, `EntryKind::ModelGraded` in bundles)
  that the PC's console profiles upload (`runner_pc::model_for`, the editor, the
  engine checks), so `TRINO_SIM=n64` and the Game view show what the console
  shows.
- `bake_grading = true` on `[scenes.*]`: the console TSCN bake grades the scene's
  clear, light, ambient and fog colors through its `post.grading`
  (`Environment::graded`); `scene export --bake-grading` does the same for code.
- Exact for unlit colors; lit colors differ by the order (graded before the light
  on the consoles, after it on the PC). The reference scene
  (`scenes/post-reference.scene.ron`) holds the unlit cards and the background
  within 14/255 on N64 and 3DS against the PC.

### 5. Glow, the consoles' bloom

`trino_core::Glow { sprite, radius, color, mode }` and `draw_glow` draw a
camera-facing billboard of the engine's `sprites/glow` (an I4 mask on the N64)
where the target has no bloom (`GlowMode::Baked`), or everywhere (`Always`).
Scenes carry it as an entity component; `SceneNode::glow` and `Entity::glow`
draw it from `draw_nodes` and `EntityStore::draw`; codegen writes it, and TSCN
v6 adds a 16-byte glow block to each node (flag bit 7). A component, not a
`ModelParams` field: no draw memo, RSP or N64 stack cost per draw.

### 6. Daltonization (from #60)

`access::color::correction(mode) = I + E·(I − S)`: the Machado 2009 simulation
`S`, the error it removes moved by `E` to the channels the viewer sees (Fidaner,
Lin and Ozguven 2005: a red-green loss into green and blue, a blue-yellow loss
into red and green). It is one 3×3 in linear RGB, drawn by the existing CVD pass
(`CvdKind::Correct`) over the whole picture after the HUD and before the
simulation view. A player setting, `AccessSettings::cvd_filter` (bit 1 of the
packed byte 2; old saves still load), opt-in because games that already swap
semantic palettes for `cvd` would be corrected twice. Hosts call
`PcRenderer::apply_access` every frame. Grays stay gray; `Off` and `Grayscale`
draw nothing; confused pairs end farther apart to their viewer (≥ 1.3× ΔE).

### 7. Speed blur (November 2026)

A racing game's feel of speed (Burnout's boost, Mario Kart 8's top speed): the
3D past an inner radius smeared along the line to a vanishing point.

- **API:** `trino_core::post::SpeedBlur { center, strength, inner_radius,
  feather }` through its own `Renderer::set_speed_blur(Option<&SpeedBlur>)` (a
  default no-op, cleared at `begin_frame`), not a `PostSettings` field. A game
  drives it every frame from its speed, while `PostSettings` is a scene's look
  (scene RON, codegen, the editor's inspector, 27 struct literals in this
  repository and more in games): the separate method is additive and follows
  `set_ambient_occlusion`/`set_shadows`. No `GAME_API_VERSION` bump.
- **Units:** `center` in the 3D's screen UVs (y down); `strength` the streak's
  length as a fraction of the pixel's distance to the center (0..=1; 0 is off);
  `inner_radius` and `feather` in the vignette's measure (0 at the center, 1 in
  a corner). `clamped()` fixes NaN and ranges; a strength of 0 keeps nothing, so
  nothing runs and the frame is byte for byte the one without the call.
- **Where:** in the composite, in linear light after exposure and before the
  tonemap, the LUT, vignette and grain; the bloom is added unblurred. The HUD
  after the last 3D draw is untouched. Without `set_post` the stack runs with
  `PostSettings::DEFAULT`; the `off` post level drops it like the vignette; the
  bloom view and the Baked tier never draw it.
- **How:** two steps, no full-size target (on the integrated GPUs games target,
  an extra full-size target costs as much as the scene). A gather at half the
  picture's size takes `GATHER_TAPS` (12) jittered taps over the first
  `1/COMPOSITE_TAPS` of a full-strength streak from each pixel; the composite
  reads it `COMPOSITE_TAPS` (8) times along the pixel's own streak. The feather
  grows the streak's length, not its opacity: only streaks under `FADE_PIXELS`
  (4) fade in from the sharp pixel, which hides the half-size gather's softness.
  Taps are spaced geometrically
  (`center + (uv - center) * reach^t`), so the two steps tile a streak exactly:
  96 samples for 20 taps. Light is averaged as the square of the display value
  (gamma 2: one multiply a tap instead of three `pow`, within a few levels of
  the sRGB blend), and the gather stores the square root (an 8-bit chain, WebGL2
  without float targets, keeps the darks). Inside the inner radius the
  composite runs the old code; where the blur covers the pixel fully it skips
  the sharp pixel's tonemap and sharpening.
- **Two traps:** the gather first used each pixel's own weight. In the feather
  the sub-streaks of the pixels farther out were then too short and left gaps:
  eight stepped copies at strength 0.8. Full strength everywhere overlaps them a
  little instead. And the composite first cross-faded the sharp and the blurred
  picture by the weight: through the feather every edge showed twice, a sharp
  ghost over its blur. The weight now only sets the streak's length.
- **Reference:** `SpeedBlur::gather` (one pass, any number of taps) and
  `SpeedBlur::mix`. The GPU frame is held to it over a whole frame (mean under
  1.5 levels, under 2% of pixels past 24; measured 0.5–0.8 and 0.5% on RADV and
  lavapipe), and the CPU reference with a mirrored center must miss.

## Measurements

castle64's hub with the runner's 4× MSAA and its SSAO, `PostSettings::FILMIC` with the `dusk`
LUT, on an AMD Renoir iGPU (RADV), blocks of 10 frames alternating off/on, the median of the
paired differences (`cargo test --release -p xtask --test post_bench -- --ignored --nocapture`;
wall-clock frame including update and `wait_gpu`, two runs):

| Size | `post` level | Off frame | Δ with post |
|---|---|---|---|
| 1920×1080 | Off (tonemap + LUT) | 8.7–9.1 ms | +1.7 ms |
| 1920×1080 | Low (¼, 3 levels) | 8.6–9.0 ms | +2.3–2.5 ms |
| 1920×1080 | Medium (½, 4) | 8.7–9.4 ms | +3.0 ms |
| 1920×1080 | High (½, ≤ 6) | 8.6–12.2 ms | +2.9–4.0 ms |
| 3840×2160 | Off … High | 37–51 ms | +9–14 ms |

Most of the Off level's cost is the half-float multisampled scene (twice the bytes per sample),
not the composite.

The speed blur (§7) on the same hub, `FILMIC` + `dusk`, 4× MSAA, the dynamic resolution pinned
at 1 so the renderer times each frame on the GPU (timestamp queries), center (0.5, 0.45), 40 pairs
of 10-frame blocks, the median of the paired differences
(`post_bench.rs::speed_blur_cost_and_shots`; a shared machine, load 5–10):

| Size | `post` level | GPU frame with post | Δ GPU, strength 0.3 | Δ GPU, strength 0.8 |
|---|---|---|---|---|
| 1920×1080 | Low | 12.6–12.8 ms | +1.5 ms | +2.3 ms |
| 1920×1080 | High | 15.4–16.6 ms | +1.6 ms | +1.8 ms |
| 3840×2160 | Low | 54–56 ms | +8.3 ms | +11.2 ms |
| 3840×2160 | High | 52–56 ms | +7.9 ms | +9.5 ms |

Seven more 1080p High runs gave +1.5 to +2.1 ms, up to +3.4 while other builds loaded the
machine. The taps are not the cost: 1 gather tap instead of 12, no jitter, or a quarter-size
gather all measured within the noise of the full pass, while skipping the gather's draw took
about 0.9 ms off. On this iGPU, which shares its memory with the CPU, the cost is one more read
of the half-float scene and the composite's reads of the gather, not arithmetic.

## Deviations from the issue

- **Scenes "v4":** scenes were already v7; `post` and `glow` are optional fields,
  no bump. TSCN did bump (v5 → v6) for the glow; the post settings stay out of
  TSCN, like the AO and stereo (a baked scene's host sets them).
- **3DS fog/TexEnv tint per scene:** not added; `bake_grading` grades the fog and
  light colors, which is the same mood without a TexEnv stage.
- **LUT size per preset:** a LUT's size is the asset's; presets change bloom only.
- **The PC's console profiles** show the graded models and environment, not the
  graded 3DS texture previews (`grading` on sprites reaches the consoles' bakes
  but not the PC's `.3ds.sprite` preview).
- **Chromatic aberration** (in the issue's problem list, not its API) is not
  implemented.

## Consequences

- Every pipeline that writes the scene's color now takes a format; a new one gets
  an HDR twin in `post::HdrSet` (or `GpuTnl`), or post frames fail validation.
- A frame toggling post on and off reallocates the scene targets each time; games
  set it every frame alike.
- The N64 and 3DS draw nothing new unless content asks: a glow is one billboard.
