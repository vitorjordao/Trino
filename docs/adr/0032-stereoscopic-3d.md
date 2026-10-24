# ADR 0032: Stereoscopic 3D on the 3DS top screen

Status: accepted (October 2026, GitHub #63). Implemented in `trino-core`
(`stereo`, `Caps::stereo`, the `Renderer` defaults, UI depth),
`trino-platform-3ds`, `trino-render-wgpu` (the PC's 3DS profile as a preview),
`trino-scene` (scene and UI fields, the budget), the editor, xtask, castle64 and
Solid Eleven.

## Context

The 3DS top screen is autostereoscopic, but Trino always drew one picture. A game
could not use the 3D slider, and nothing in the engine knew about eye separation,
convergence or which 2D should float over the scene. Doing it per game would mean
drawing every frame twice by hand, which breaks batching, the draw memo, the
dual-screen order (top first, then bottom, once per frame) and every budget.

## Decisions

### 1. The game draws once; the backend replays the frame per eye

On a stereo frame, a backend with a stereo screen **records** the top screen's
`Renderer` calls instead of drawing them, then replays the recording once per eye
when the frame leaves the top screen (`on_screen(Bottom)`) or ends:

- The record is the call, not its triangles (`trino_core::stereo::StereoCmd`): the
  camera, light, local lights, wetness, water, shadows and receivers, the texture
  filter, the layer depth, and the draws (models as a matrix, billboards, sprites,
  rectangles, text). An eye changes the projection, so screen-space triangles
  recorded once would be wrong for the other eye.
- The core has no `alloc`, so it owns the command type and `stereo::replay`; slices
  (text, lights, ripples, receivers) are `Span`s into arenas the backend keeps in its
  own `Vec`s (`StereoData`). The 3DS and the PC each have a small `Recording` type
  for that storage, with capacity kept from frame to frame.
- Each eye starts from the state the recording began with (`TopState`: camera,
  light, local lights, filter, water, shadows), and the renderer ends in the state
  the recording ended with, mono. Shadow statistics add up over both eyes.
- Game code always sees `Eye::Mono` (`Renderer::stereo_eye`); only a backend's own
  code sees `Left` and `Right` while it replays.

### 2. Off-axis eyes in the T&L context

`TnlContext::with_eye(shift, convergence)` moves the eye `shift` world units along
the camera's right vector (the view's x translation and the context's camera) and
moves the picture's center by `shift · focal / convergence`. A point at the
convergence depth keeps its mono pixel in both eyes (zero parallax), nearer points
get crossed parallax (out of the screen), farther ones uncrossed, and screen y never
changes (no toe-in, no vertical parallax). Left is `-separation/2`, right
`+separation/2`.

- Everything that projects reads the context: the CPU T&L, the PICA200 path's
  `gpu_screen_matrix`, the water program, billboards, blob and planar shadows. No
  shader changed.
- A zero shift returns the context unchanged, so the mono frame is bit-identical
  (`a_zero_shift_keeps_the_mono_context_exactly`). The method is out of line and adds
  no field to `TnlContext`, which every N64 RSP check reads.
- It is applied before `with_local_lights`, which works in view space.

### 3. The API (no `GAME_API_VERSION` bump: every method has a default)

- `StereoSettings { enabled, max_separation, convergence, policy, budget_us, slider }`
  and `Renderer::set_stereo(Option<&StereoSettings>)`. It is **cleared at
  `begin_frame`**, like shadows, and must come **before the first top-screen draw**.
  A later call keeps the frame mono (`StereoMono::Late`; PC strict mode panics).
- The separation in use is the slider's position times `max_separation`. The slider
  at 0 is the mono path: nothing is recorded and nothing costs more. `slider: Some(s)`
  fixes the position for tests and captures; games leave it to the player.
- `Renderer::set_layer_depth(LayerDepth)`: the 2D drawn after it floats by
  `LayerDepth.0` pixels of parallax at the full slider (each eye moves half, the
  slider scales it; positive out of the screen). It is state that persists until
  changed and resets at `begin_frame`.
- `Caps::stereo`: true for the 3DS tables only. The 3DS boot clears it on an old 2DS
  or a New 2DS XL (`CFGU_GetSystemModel`, probe bit 2, `TRINO_HW … stereo=`).
- `FrameStats::stereo` (`StereoStats { eyes, slider_pct, mono, commands }`): how many
  pictures the top screen got, and why only one. The other counters sum both eyes.
  `TRINO_FRAME` prints `stereo=on@N%` or the reason (`off`, `none`, `slider0`,
  `budget`, `late`).
- `Renderer::read_eye_pixel(Eye, x, y)`: the self-test's read-back of either eye.

### 4. The 3DS backend

The shim gains `trino_stereo_slider` (`osGet3DSliderState` as 16.16),
`trino_stereo_mode` (`gfxSet3D`, only on a change, so a mono frame never shows a stale
right eye), `trino_stereo_eye` (the right eye's target, made on the first stereo
frame with the same 24-bit depth and stencil as the top's, output to `GFX_TOP`
`GFX_RIGHT`, cleared to the frame's color), and screen 2 in `trino_debug_pixel` and
`trino_debug_fb_dump` (the right eye's LCD buffer). All take at most two integer
arguments (ADR 0002). Azahar renders the right eye into its own framebuffer even with
its 3D factor at 0, so the self-test reads both eyes back in emulation.

Costs per eye: a PICA200 draw runs its vertex program again, a CPU T&L draw is
transformed again, the 2D is drawn again.

**`StereoPolicy::Auto`** (the default) drops to one picture when two cost too much.
`StereoGovernor` sees each frame's interval (begin to begin). After three stereo
frames in a row past `budget_us` plus a quarter, it draws 120 mono frames, then tries
stereo again. Separation is not reduced instead: a smaller separation costs the
same. `Always` keeps two eyes.

### 5. The PC's 3DS profile previews it

The PC renderer records and replays exactly as the 3DS does, on `SimProfile::N3ds`
only (`Caps::stereo`). Each eye is rendered into the main target, copied into its own
texture (`PcRenderer::read_eye`, `read_eye_pixel`), and the preview is composed back
into the main target, so the window, the editor and every read-back see it.

- `StereoView`: `Off` (the simulated slider down; a game that fixes its slider still
  draws both eyes, and the left one shows), `Anaglyph` (red from the left, green and
  blue from the right), `SideBySide`, `Left`, `Right`, `Flip` (the eyes in turn every
  8 frames).
- `set_stereo_view`, `set_stereo_slider` and `TRINO_STEREO=<view>[:slider]`. The
  composites are two pipelines built through `pipes::render` with a precompile job
  (`JobKind::Stereo`).
- The preview never drops an eye for the budget: the PC's frame time isn't the
  console's.
- The native profile, the N64 profile and the Web have no stereo screen and never
  record.

### 6. The N64 and the Web

`Caps::stereo` is false. The defaults are no-ops and neither backend has stereo code.
Game code is the same everywhere. A UI screen without depths makes no extra call (the
draw loop compares one byte per node).

### 7. Authoring

- **Scenes:** `environment.stereo: Some((enabled, max_separation, convergence, policy,
  budget_us))`. Every field has a default (0.06 m, 4 m, Auto, 16 667 µs). It is an
  optional field, so there is no version bump (scenes stay v7), with
  `deny_unknown_fields`.
  - Codegen writes `STEREO`.
  - TSCN carries none, as with AO: the host of a baked scene sets it, and the editor's
    Play here copies it from the scene.
  - The scene player and the showcase call `set_stereo` first thing.
- **UI:** a node's `depth: i8` (0 floats with its parent). Children inherit it at
  layout (`Slot::depth`), and the draw sets it only where it changes and resets it
  after the screen. TUIL v3 adds the byte; v2 and v1 still load with every depth 0.
  `ui export` writes it only when set.
- **CLI:**
  - `cargo xtask scene budget [--platform 3ds|n3ds] --stereo [--json]`: per-eye cost
    (`StereoBudget`: tris, GPU T&L vertex passes, CPU T&L draws and vertices, both
    eyes). Scenes that ask for stereo get it without the flag.
  - `cargo xtask ui shot --platform 3ds --stereo anaglyph[:slider]`.
- **Editor:** the Game view's stereo preview and 3D slider on the 3DS, Inspector ›
  Stereo 3D, and the Scene view's convergence plane (the `editor` skill).

## Deviations from the issue

- "Scenes (v4)": scenes were at v7. The field is optional, so the version stays.
- "2D draws take a `LayerDepth`": it is a state setter
  (`Renderer::set_layer_depth`). Changing `draw_sprite`/`fill_rect`/`draw_text` or
  `SpriteParams` would have touched every call site and backend for a value most
  draws leave at 0.
- "Reduce separation or drop to mono": only the drop. Separation doesn't change what
  a frame costs.
- `--platform n3ds`: accepted as another name for `3ds`, as before.

## Consequences

- Stereo frames double the top screen's draw work, and the 3DS's per-frame vertex
  buffer (`TRI3D_MAX_VERTS`) is shared by both eyes. A scene near that limit in mono
  overflows it in stereo (the shim warns once and drops triangles). The budget's
  `stereo` line shows the doubled numbers.
- A game must call `set_stereo` before its first top-screen draw, every frame.
- Draws on the touch screen are never stereo.
- `castle64` converges on the player at 1/20 of the camera's distance, the HUD 6 px
  out. Solid Eleven converges on the ball's focus the same way, its HUD 6 px out.
  Both keep their goldens: the PC goldens and Azahar run with the slider down.
