# ADR 0014: Ambient occlusion per tier

Status: accepted (October 2026, GitHub #42)

## Context

Trino lights a draw with one directional light, an ambient term, fog and local lights.
Without contact shadows, models look pasted onto floors and creases look flat. The
only tool so far was a Blender helper (`tools/blender/color_tools.py`'s
`ambient_occlusion_floor`), outside the engine and the editor.

Screen-space AO (SSAO) needs a depth read-back and many texture fetches per pixel.
Neither the N64 (RDP, 4 KB TMEM) nor the 3DS (PICA200, fixed-function fragment stage)
can afford it. Both draw vertex colors for free, though.

## Decisions

### 1. Two tiers, chosen by `Caps::ao_tier`

`trino_core::ao::AoTier` has two values:

- `ScreenSpace` on the PC native profile: a post pass over the depth buffer.
- `Baked` on the N64, the 3DS and the PC's console profiles: the asset bake writes the
  occlusion into vertex colors (§3). No run-time pass.

The console profiles show what the consoles show. A sim profile never runs SSAO, so the
N64 and 3DS goldens, `n64_parity` and `n3ds_parity` are unaffected.

### 2. `Renderer::set_ambient_occlusion(Option<&AmbientOcclusion>)`

- The settings are `radius` (meters), `intensity` (0..2), `bias` (meters) and
  `quality`: `AoQuality::{Low, Medium, High}` = 8/16/32 samples. `clamped()` keeps
  them in range.
- The method has a default no-op, like `set_local_lights`, so `GAME_API_VERSION` is
  unchanged. Baked-tier targets ignore it.
- **Cleared at `begin_frame`.** A game sets it every frame, so AO never leaks from the
  self-test into a game, or from one game into the next.
- Scenes carry it in `Environment::ao: Option<AmbientOcclusionDef>`.
  - Every field defaults, so `ao: Some(())` is valid. `enabled: false` keeps the
    tuning and drops the pass. A missing block writes nothing (no scene version
    bump).
  - Codegen exports `pub const AO: Option<AmbientOcclusion>`.
  - **TSCN does not carry it.** Its environment header is fixed-size, and only PC
    games would read it. A PC game that loads a baked scene sets AO in code, as the
    editor's Play here does.

### 3. Baked vertex AO: `ao = { ... }` on a `[models.*]` entry

- `ao = true`, or `ao = { strength, distance, rays, ground }`. The defaults are
  strength 0.8, distance 0 (a fifth of the bounding-box diagonal), 64 rays and no
  ground.
- Each vertex casts cosine-weighted rays around its normal against the model's own
  triangles, through a BVH. Nearer hits weigh more. RGB is multiplied by
  `1 - strength × occlusion`; alpha stays.
- `ground = true` adds a floor at the model's lowest point, so props darken where they
  stand.
- Rays start a hair above the surface and a hair behind it along the ray. A vertex
  lying on another surface's plane, such as a floor meeting a wall, still sees that
  surface.
- The bake is deterministic. Directions come from `trino_core::math3d`'s
  sin/cos/sqrt, and the BVH sort is stable. Same bytes on every host.
- AO is per model: self-occlusion only. A box on a separate floor model gets nothing
  from the floor, unless `ground` is set. The SSAO tier covers contacts between models
  on the PC.
- It costs nothing at run time on any backend, including the N64's RSP path, which
  already reads vertex colors. `castle64` content does not opt in, so every console
  golden stays identical. The self-test probe does opt in (§6).

### 4. The PC pass

`renderer/ao.rs` and `ao.wgsl` hold the pass. It runs only on frames with AO set, and
its pipelines are built on first use.

1. **Where it goes in the frame.** The main pass ends after the frame's **last 3D
   draw**. The AO passes run there. The commands after it (HUD, text) draw on top with
   `LoadOp::Load`. A HUD is never darkened.
   - Sprites drawn *before* the last 3D draw are inside the 3D part.
   - A sprite over cleared background stays as it is (depth 1 = no occlusion).
   - A sprite over geometry is darkened like that geometry.
2. **The occlusion.**
   - The depth texture gained `TEXTURE_BINDING`. The pass reads depth with
     `textureLoad` and inverts the T&L's projection (`d = 1 - NEAR/z`), using the
     focal length of the current camera. The camera at the end of the frame is the
     one that counts.
   - The normal is rebuilt from the neighbor nearer in depth on each axis.
   - Samples form a cosine-weighted golden-angle spiral, turned per pixel by a 4×4
     Bayer pattern. They are range-checked and faded with the light's fog.
3. **The blur.** A separable 5-tap bilateral blur with weights [0.5, 1, 1, 1, 0.5].
   It spans the Bayer period exactly.
4. **The composite.** A multiply blend (`Dst × Src`) into the offscreen target, so
   `read_offscreen`, `read_pixel` and the editor all see the result. The mask view
   replaces the frame with the occlusion instead.
5. **Fixed per pixel.** There is no per-frame noise: the same frame renders the same
   pixels. The PC goldens `ao_native` and `ao_mask` compare with a tolerance (lavapipe
   in CI).
6. **Pictures taller than 540 lines** compute the occlusion and the blur at half
   resolution, one texel per 2×2 block. The composite upsamples it, weighting the four
   nearest texels by depth similarity. On a Renoir iGPU (castle64 hub, whole frame,
   fastest of 6 alternating rounds), AO off → on:

   | Resolution | Off (ms) | On (ms) |
   |---|---|---|
   | 640×360 | 0.30 | 1.46 |
   | 1280×720 | 0.46 | 1.76 |
   | 1920×1080 | 0.45 | 3.47 |
   | 3840×2160 | 1.46 | 10.88 |

   Before the half-resolution path, 1080p cost 6.7 ms and 4K 24.9 ms. The 4×4 pattern
   was first a dynamically indexed local array, which went to scratch memory per pixel
   and nearly doubled the pass. It is bit arithmetic now.

### 5. Debug views: `AoView` and `TRINO_AO`

- `PcRenderer::set_ao_view(AoView::{Game, Off, On, Mask})`:
  - `Game` (the default): as the game sets it;
  - `Off`: never;
  - `On`: the game's settings, or the defaults when it set none;
  - `Mask`: the occlusion alone.
- A game window reads `TRINO_AO=off|on|mask`. Headless renderers ignore it, so tests
  never depend on the environment.
- The editor's Scene view has the same four modes in its toolbar and in View ›
  Ambient occlusion. `TRINO_EDITOR_AO` sets the mode at start.

### 6. Editor and tests

- **Inspector › scene settings › Ambient Occlusion:** a checkbox, Radius, Intensity,
  Bias and Quality, live in both views and undoable. Play here, `apps/pc`'s scene
  player and the Scene view all use the scene's own settings. In near-orthographic
  mode, radius and bias are scaled by `RenderCam::ao`.
- **Inspector › model › Ambient occlusion (baked):** writes the manifest's `ao` line
  through `manifest::edit::set_model_ao` (comment-preserving). The watcher then
  rebakes. A convex model bakes to the same bytes, so the inspector reads the setting
  back from the manifest, not from the bake.
- **The `ambient occlusion` self-test step** on PC, in ares and in Azahar:
  - It draws `models/selftest_ao` (a panel between two fins, baked with AO) and its
    unbaked twin `selftest_ao_raw`, unlit.
  - It asserts that the panel is darker by the fins, unchanged in the middle, and not
    tinted.
  - Commenting out the bake call makes it fail.

## Consequences

- PC games get contact shadows with one call per frame, and authored scenes with one
  RON block. Consoles get the same cues on any model that opts into the bake, at no
  run-time cost.
- Baked AO is in the model, so it also shows on the PC native profile. A game that
  enables both gets both: baked creases plus screen-space contacts.
- Follow-ups:
  - Scene-level baking: occlusion between static models, baked per instance.
  - AO in TSCN for PC games that load levels as data.
  - An AO term on the 3DS GPU path, if per-vertex AO ever needs to vary at run time.
