# ADR 0046: Render targets — a small scene drawn into a texture

Status: accepted (November 2026). Bongo Kart's study (its epic #24, GitHub Bongo-Kart #184)
asked for a texture the renderer draws into: the leader on a stadium screen, live monitors on
a desk, a rear-view mirror. Implemented in `trino-core` (`target`: `RenderTarget::{new,
texels, fits, sized}`; `Renderer::begin_target` / `end_target` with defaults that refuse;
`Caps::{max_target_texels, max_targets}`; `FrameStats::{targets, targets_rejected}`),
`trino-render-wgpu` (the PC's native profile and the Web: `PcRenderer::begin_target`,
`end_target`, `read_target`) and castle64's self-test (`render target`).

## Context

The issue said to take it with care: a target costs another render pass, so measure it on
the Radeon, and without one a game falls back to a live minimap as a mesh (`draw_mesh`,
`Unlit`) and a mirror as a schematic. A screen or a mirror needs the 3D seen from another
camera, at another size, in the same frame, then sampled like any texture: on a model (a
stadium screen) or as a sprite (a mirror in the HUD).

## Decisions

### 1. A target is a texture with a sprite handle

`RenderTarget::new("targets/mirror", 480, 160)` names a texture by a logical path, as an
asset is named. Its `sprite` handle is the path's asset id, and every texture slot takes it:
`draw_sprite`, `ModelParams::texture`, a `DynamicMesh::textured` mesh. There is no new
texture kind, so no draw call changes. A target keeps its picture until it is drawn again,
and a new size makes a new texture under the same handle.

### 2. Begin, draw, end; targets first

`Renderer::begin_target(&target, clear) -> bool` sends the next draws, 2D and 3D, into the
target. `end_target()` renders them. Draws made after `end_target` that sample the target show
this frame's picture.

- **Targets first.** A frame draws its targets before its first draw on the screen; a
  `begin_target` after one is refused, and strict mode panics with the rule.
  - The screen's frame is one pass split only where its own passes need it (the AO, the
    shadow map, the post stack's scene target, the water's depth copy).
  - A target in the middle would split it again: the screen's draws before it would have to
    be finished, post stack included, and the rest drawn over them.
  - Drawing targets first costs a game nothing: it knows what its mirror will show before it
    draws the screen.
- **Refused, not faked.** `false` means skip the draws and fall back. It answers so when:
  - the platform has no targets (`Caps::max_target_texels` 0);
  - the target doesn't `fit` (1..=`max_target_texels` pixels);
  - the frame already drew `Caps::max_targets` of them (PC and Web: 4);
  - another target is open.

  Refusals count in `FrameStats::targets_rejected`. `end_frame` ends a target left open.
- **The camera is the renderer's.** 3D in a target projects to its size. The game sets the
  target's camera before its 3D and the screen's after `end_target`, as with the 3DS's second
  screen. Renderer state outlives a target as it outlives frames.

### 3. What a target draws: one plain pass

On the PC and the Web a target is the second screen's pass at another size: one
single-sampled pass into an `Rgba8Unorm` texture with its own depth buffer and globals.

- **It has:** the CPU and GPU T&L paths, sprites, text, the lit, translucent and additive
  pipelines, and fog.
- **Water at the per-vertex tier.** The per-pixel tier's foam reads a depth copy of the
  screen's pass, and a target takes none. A water draw inside a target therefore takes
  the CPU path and the per-vertex pipeline, as on the 3DS profile's second screen. The GPU
  path's water pipelines are per pixel only, and its reject counts it as `Disabled`.
- **It leaves out:** MSAA, the render scale, the post stack, AO, shadows of any tier and the
  color-vision passes.
  - A caster in a target would join the screen's shadow map, so a target draws no shadows.
  - The color-vision passes run over the screen that shows the target.
  - A mirror the size of a HUD panel doesn't need the rest, and each would cost another pass.
- **Opaque, one level:** the clear's alpha is ignored, and the texture has no mip chain.
- **The same pixels:** a target draws byte for byte what a screen of its size draws, on both
  T&L paths (`a_target_draws_what_a_screen_of_its_size_draws`). The frame's T&L, light and
  water streams are shared: a target uploads them and the screen uploads them again, longer.
  Indices only grow within a frame, so the target's draws read what they wrote.

### 4. The cost, measured on the Radeon

`xtask/tests/target_bench.rs` on the Radeon the issue named: AMD Radeon Graphics (RADV RENOIR, the
Ryzen 7 5825U's iGPU), Vulkan.

**Setup.** castle64's hub at 1920×1080 as the runner draws it: 4× MSAA, the game's SSAO,
the GPU T&L path. Each target holds the game's whole render: its update once, then its
renders.
- The mirror looks back from the player.
- The map looks down from above the player.
- The monitors show the screen's own view, the worst case: the whole scene twice.

Every layout is warmed for 120 frames, then the layouts run interleaved over 8 rounds of 40
frames each. A frame's time covers the update, the renders and the GPU's work, waited for.
Medians in ms.

**The last run:**

| Frame | Targets | Median | p95 | Added | Draws | Triangles |
|---|---:|---:|---:|---:|---:|---:|
| screen only | 0 | 8.69 | 9.10 | — | 27 | 629 |
| rear-view mirror 480×160 | 1 | 8.97 | 9.33 | +0.29 | 54 | 653 |
| live map 256×256 | 1 | 8.96 | 9.46 | +0.27 | 54 | 1224 |
| monitor 480×270, the same view | 1 | 9.04 | 9.49 | +0.35 | 54 | 1258 |
| monitor 960×540, the same view | 1 | 9.21 | 9.67 | +0.52 | 54 | 1258 |
| 4 monitors 256×144, the same view | 4 | 9.76 | 10.33 | +1.07 | 135 | 3145 |

**Across four runs**, the iGPU sat in one of two clock states for a whole run:

| State (screen only) | One target | Four monitors |
|---|---|---|
| fast (≈ 8.0 ms) | +0.05 to +0.17 ms | +0.53 to +0.57 ms |
| slow (≈ 8.7–9.0 ms) | +0.25 to +0.52 ms | +1.05 to +1.07 ms |

**What it means:**
- **Each target costs a pass.** Four 256×144 monitors (147 456 pixels) cost twice one
  960×540 monitor (518 400 pixels). Their fixed cost is a render pass, a submit and the
  frame's T&L streams uploaded again, and it outweighs the pixels at these sizes.
- **The CPU's share is nothing to speak of.** The game's draws are on the GPU path: 0.01
  to 0.02 ms of CPU T&L a frame with four monitors.
- **The budget at 60 fps.** A kart's mirror (one small target) costs under 0.3 ms of a
  16.7 ms frame on this iGPU, even in its slow state. That's 2 % or less. Four stadium
  screens showing the race cost about 1 ms.
- **What a game does with that.** It can keep targets small and few, update a slow one
  every other frame (the picture persists), and draw a target's far scenery with its
  LODs.

The evidence images (each layout's frame and its first target's picture) are written to
`target/evidence/render-target/`.

### 5. The consoles: refused for now

`Caps::max_target_texels` is 0 on the N64 and the 3DS, and on the PC's console profiles,
which draw what the consoles draw. `begin_target` answers `false` there (the trait's
default), and a game takes its fallback.

- **The N64 could:** the RDP draws into any RDRAM surface (`rdpq_attach`), and a texture that
  fits TMEM (32×32 RGBA16, 64×32 CI8) could then be sampled. That costs a second
  `rdpq_attach` per frame, the RSP's projection retargeted, and a 32×32 mirror's worth of
  detail.
- **The 3DS could:** citro3d renders into a texture (`C3D_RenderTargetCreateFromTex`).

Both are follow-ups (BACKLOG). The API doesn't change for them: they will raise their caps.

## Tests

- `crates/core/src/target.rs`: names, sizes, `fits` against every table.
- `crates/core/tests/renderer_defaults.rs`: the default refuses.
- `crates/platform-pc/tests/water.rs::water_in_a_render_target_is_the_per_vertex_tier_on_both_paths`:
  no GPU draw, the same bytes on both paths. Without the rule, the GPU path took it.
- `crates/platform-pc/tests/render_target.rs`:
  - a target equals a screen of its size, byte for byte, on both paths;
  - the screen samples it as a sprite and on a textured quad, keeps it a frame it isn't
    drawn, and follows a resize;
  - the caps and the order are refused and counted, strict mode says why, and `end_frame`
    ends an open target;
  - the console profiles refuse.
- **Self-test step `render target`** (`castle64::selftest::render_target`) on the PC
  (`engine_checks`, both paths), ares, Azahar and the browsers.
  - Where targets draw: a 64×32 target with a red rectangle and a green 3D quad over a blue
    clear, read on a sprite at 2× and on a textured quad, then again a frame later. An
    oversized target is refused.
  - On the consoles and the PC's console profiles: `begin_target` refuses.
  - The probe isn't compiled for the consoles, where it can't run, so it costs no N64 heap.
- `xtask/tests/target_bench.rs` (ignored, release): §4's measurements.
