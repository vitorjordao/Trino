# ADR 0022: Quality presets for the PC and the Web

Status: accepted (October 2026, GitHub #55). Every phase of the issue is implemented: the
settings and presets in the core (§1), the calibration bench (§2), dynamic resolution
with the upscale and sharpening passes (§3), frame pacing and 1% lows (§4), the Steam
Deck, phones and the consoles (§5), the project's presets (§6), the start and the
player's file (§7), the content multipliers (§8), the CLI, the editor and the options
page (§9), and the Web (§10).

## Context

The PC drew one way on every machine: 4× MSAA, the game's AO, every LOD at its authored
distance, every particle, at the window's full resolution. That is right for a desktop
GPU and wrong for the machines a PC or browser game actually meets — a laptop's
integrated GPU, a Steam Deck at 800p on battery, a Chromebook, a phone. The consoles
never had the problem: their hardware is one table (`Caps`) and the engine's paths are
tuned to it (ADR 0012, 0020). Commercial PC releases need what every engine offers:
named presets, a first launch that picks one, a resolution that bends to hold the frame
rate, a frame cap, and a Graphics page.

## Decisions

### 1. One `QualitySettings`, four named presets, in the core

`trino_core::quality` (`no_std`, zero deps) defines `QualitySettings`: `render_scale`,
`dynamic_resolution: Option<DynRes>` (target µs — 0 is 90% of the frame interval — and a
min/max scale), `msaa`, `ao`, `shadows`, `post`, `lod_bias`, `particle_scale`,
`texture_filter`, `anisotropy`, `sharpen` and `frame_limit` (`VSync`, `Fps(n)`,
`Unlimited`). `shadows` and `post` are levels the renderer stores for #57 and #64.

| Preset | 3D scale | Dynamic res. | MSAA | AO | LOD bias | Particles | Aniso | Sharpen |
|---|---|---|---|---|---|---|---|---|
| Low | 0.75 | 50–100% | off | off | 0.6 | 0.5 | 1 | 0 |
| Medium | 1.0 | 60–100% | off | ≤ Low | 0.8 | 0.75 | 4 | 0.2 |
| High | 1.0 | off | 4× | the game's | 1.0 | 1.0 | 8 | 0.4 |
| Ultra | 1.25 | off | 4× | the game's | 1.5 | 1.0 | 16 | 0.5 |

High is the PC's look before presets existed (the golden frames prove it, §Verification);
Ultra is High supersampled. `PresetTable` holds a project's four (§6), `classify` names a
custom set back (ignoring the frame cap, a display choice), `clamped` keeps every value
in range. The `Platform` trait gains five default methods — `quality`, `set_quality`,
`quality_preset`, `quality_info`, `detect_quality` — and `Renderer` gains `lod_bias`: no
`GAME_API` bump, and every console keeps its behavior.

### 2. A first launch picks its preset: the calibration bench

`PcRenderer::calibrate` draws a built-in scene — 64 lit spheres (~100k triangles) on a
floor, four point lights, AO, a HUD bar — at each preset, Low first, on the renderer the
game will use at its picture size. Each preset draws two warm-up frames and up to twelve
timed ones, each from `begin_frame` to the GPU's idle, unpresented (a FIFO swap chain
would round every frame up to the refresh). The walk stops at the first preset whose
median misses the target (90% of the display's frame), or when the 2 s budget is spent;
the pick is the highest that held, Low when none did, and always Low on a software
adapter (llvmpipe, lavapipe, SwiftShader, WARP). The renderer's state comes back as it
was. On this repo's CI adapter the whole walk takes about 0.5 s; the acceptance bar is 3 s.

`bench_begin` / `bench_step` are the same walk one frame at a time: the PC loops over
them, the Web draws one per animation frame (§10). "Detect again" (`detect_quality`) runs
it from an options page, keeping the player's frame cap.

### 3. Render scale, upscale and dynamic resolution

Below or above a scale of 1 the native profile draws the frame's 3D into its own color
and depth targets at the scaled size, runs the AO there, and stretches it onto the
picture: bilinear, plus contrast-adaptive sharpening (`sharpen`, CAS-style) when the 3D
is smaller than the picture. The 2D after the last 3D draw — the HUD — draws on the
picture at its own resolution, so text stays crisp at any scale. At 1 nothing of this
exists and the frame is the one the PC always drew.

Dynamic resolution is a PI controller on the log of the rendered area (`scale²`): a
frame's GPU time is roughly a fixed part plus a part proportional to the pixels, so a
ratio of times maps onto a ratio of areas. Every 8 frames it averages the measurements,
takes `e = ln(target / measured)` and steps by `0.55·e + 0.25·(e − e_prev)` — the
velocity form, nothing to wind up at a limit — with a ±3% dead band and at most ×1.35 a
step. It measures the GPU's time where the adapter has timestamp queries (two empty
passes bracket the frame, read back a frame or two later), else the host's frame
intervals (`report_frame_time`). An interval can't show headroom under VSync, so on that
fallback a frame on target probes 2% up per period and a missed frame comes back down.

Under dynamic resolution the 3D's targets (color, depth, the MSAA and water copies, the
AO's) are allocated once, at the largest size the range reaches, and each frame draws
into their top-left corner at the current size (a viewport; the upscale and the AO read
only that corner). Allocating per step stalled the frame it landed on — +25 ms with 4×
MSAA, +2–10 ms without, on llvmpipe at 1080p. The price is a full-size clear and MSAA
resolve: 1–3 ms on llvmpipe with 4× MSAA at half scale, nothing measurable without
MSAA (Low and Medium, the presets with dynamic resolution on).

Rejected: temporal upscalers (FSR 2/3, DLSS) — they need motion vectors, history and
jitter the engine's T&L paths don't produce, and they are heavy for WebGL2. Spatial
scaling and sharpening first; revisit when a game needs more.

### 4. Frame pacing

`FrameLimit` picks the present mode: `VSync` presents FIFO; a cap at or under the
display's rate presents FIFO and sleeps to the cap (30 on 60 Hz shows each frame for two
refreshes); a cap over it, and `Unlimited`, present mailbox, else immediate, and the
limiter alone holds the cap. The limiter keeps deadlines an interval apart and sleeps to
1.5 ms before each, then spins: OS sleeps overshoot by up to a millisecond (more on
Windows), which alone would miss ±2% at 120 fps. A frame later than a whole interval
restarts the cadence instead of rushing the next ones.

`TRINO_FRAME` gains the 1% lows (`low1_us`, `low1_fps`: the mean of the slowest 1% of
frames, kept for about half an hour) and, on the PC and the Web, the quality in effect:
`quality=low scale=0.82 quality_from=detected detected=low gpu_us=…`.

### 5. Machines: the Steam Deck, phones, the consoles

- **Steam Deck** (`SteamDeck=1`, or the DMI board name `Jupiter`/`Galileo`): its first
  launch takes `QualitySettings::STEAM_DECK` — the project's Medium capped at 60 fps —
  without a bench, saved as the player's; 40 and 30 fps are offered for battery.
- **Phones and tablets** on the Web (Client Hints' `mobile`, the user agent, an iPad in
  desktop mode): the project's Low, not saved, so a later project default still applies.
- **Consoles:** fixed. `QualitySettings::console(caps)` is Low on the N64 (the Expansion
  Pak adds memory, not speed) and the 3DS, Medium on the New 3DS; `fixed` scales
  nothing (LOD bias and particle scale 1), `set_quality` and `detect_quality` are
  documented no-ops, `quality_preset` and `quality_info` are `None`. The N64 is the
  ceiling (the core's rule): nothing in a console's frame changes. The self-test logs it
  (`TRINO_STEP:quality low fixed lod=100 particles=8` on the N64) and `cargo xtask test n64|3ds`
  requires the console's preset.

### 6. The project's presets: `assets/quality.ron` and `[pc.quality]`

A project changes what a preset means in `assets/quality.ron` (version 1, RON with
`implicit_some`): each of `low`, `medium`, `high`, `ultra` names only what it changes from
the built-in values, and every value is checked (render scale 0.5–2, dynres 0.25–2 with
min ≤ max, MSAA 1 or 4, levels off/low/medium/high, multipliers 0.1–4, anisotropy
1/2/4/8/16, sharpen 0–1, a frame limit). `trino.toml` takes:

```toml
[pc.quality]
default = "auto"                 # or low|medium|high|ultra: the first launch
presets = "assets/quality.ron"   # where the file is
```

The PC and Web bakes turn both into one `quality/presets` blob (a `default=` line, then
`[low]`…`[ultra]` sections of `key=value` lines), only when the file exists or the default
isn't `auto`; the consoles' bakes carry nothing.

### 7. The start, and the player's file

What a run starts with, first match wins (`trino_render_wgpu::quality::start`):

1. forced for the run, never saved: `TRINO_QUALITY=<preset>|auto` (`cargo xtask run pc
   --quality low`), the Web's `?quality=`;
2. the player's saved settings: `<save_name>.quality` next to the save on the PC
   (`key=value` text, hand-editable, every value clamped on read), `localStorage`
   `trino-quality:<save_name>` on the Web;
3. the machine (§5);
4. the project's default: a preset, or `auto` — the bench, whose pick is saved.

A smoke run (`TRINO_SMOKE_FRAMES`, the Web's self-test and `?frames=`) is deterministic:
High unless forced, nothing read or written. `TRINO_QUALITY_BENCH=1` benches, prints
`TRINO_QUALITY: bench …` and exits (`cargo xtask quality bench`). A game's `set_quality`
is applied after the frame, classified (a named preset or Custom) and saved.

### 8. What the presets change in content

`lod_bias` multiplies every LOD start (`SceneNode::lod_from_biased`, `draw_nodes`,
`EntityStore::draw` read `Renderer::lod_bias`); `particle_scale` multiplies every
effect's cap (`ParticleSystem::scale_limit`, never past the system's storage, an effect
that is on keeps one particle). The AO setting is a cap on the game's own AO: Off drops
the pass, Low and Medium lower its quality, High keeps what the game set. Anisotropy
applies to the 3D's textures, the texture filter where the game leaves the default.

### 9. Tools: the CLI, the editor, a Graphics page

- **CLI:** `cargo xtask quality show [--json]`, `quality check`, `quality set <preset>
  key=value… [--json]`, `quality reset [presets…]` show and edit `assets/quality.ron` with the same checks;
  `quality bench [--json]` runs the game's PC host in bench mode; `run pc --quality <p>`.
- **Editor:** a Quality tab (one column per preset, one row per setting, Reset, an eye to
  preview a preset; edits written once the user settles, outside edits live, an invalid
  file shown and never written back) and, in the Game view's PC and Web profiles, a
  preset picker and a dynamic-resolution toggle; the budget HUD says
  `Quality Low · 3D 0.81× (dynres 0.50-1.00) · MSAA off · AO off`. Play runs the PC at the
  previewed preset. Hooks: `TRINO_EDITOR_TAB=quality`, `TRINO_EDITOR_QUALITY=low[,dynres]`.
- **Graphics page:** `trino_core::GraphicsOptions` — preset, render scale, dynamic
  resolution, frame limit, anti-aliasing, AO, Detect again, Back, with the live 3D scale,
  the GPU time and the calibration's pick — driven by the d-pad, drawn with the game's
  font, only `Platform` calls underneath. castle64 opens it on Select (PC and Web).

### 10. The Web

The same presets, start and requests (`trino-platform-web::quality`): `WebPlatform`
holds a `HostedQuality` like `PcPlatform`. A page can't wait for its GPU, so the bench
draws one presented frame per animation frame and times the intervals: under the
browser's VSync a frame that holds 60 fps comes one refresh after the last, one that
misses two, so the target is 1.25 refreshes at 60 Hz. A frame cap skips animation frames
(40 on a 60 Hz screen alternates one and two refreshes); dynamic resolution follows the
game frames' intervals. WebGL2 never gets MSAA (ADR 0019).

## Consequences

- A PC or Web game ships presets, a calibrated first launch, dynamic resolution, a frame
  cap and a Graphics page without code of its own; a project tunes them in one text file
  that the CLI, the editor and an agent all edit.
- High draws what the PC always drew, and the consoles draw exactly what they did: every
  golden frame (PC, N64, 3DS, Web) is unchanged.
- The bench's scene is the engine's, not the game's: a game much heavier than it can pick
  a preset it can't hold. Dynamic resolution absorbs some of that; "Detect again" and
  the page cover the rest.

## Verification

- Core: the preset table, `classify`, `clamped`, the text round trip, the AO cap, the
  LOD bias through scene and entity draws, the particle cap, the Graphics page row by
  row against a fake host (`core/tests/quality.rs`, `core/tests/options.rs`).
- PC renderer (`platform-pc/tests/quality.rs`): each preset draws its own frame; Low and
  High match `tests/golden/pc/quality_{low,high}.png`; High equals the frame the native
  profile always drew; a scaled 3D under a crisp HUD; the AO cap; dynamic resolution
  within its range; the console profiles ignore quality; the bench picks in ≤ 3 s and
  restores the renderer; dynamic resolution's steps allocate nothing and draw what a
  fixed scale draws. `dynres` and `pacing` unit tests: the PI settles a modeled frame
  on 16.6 ms within ±1 ms, its scale within ±5%, from any start; the limiter holds 30,
  60 and 120 fps within ±2%.
- Measured (`xtask/tests/quality_shots.rs`, castle64's hub): on an AMD Radeon
  integrated GPU (Vulkan, Windows) at 2560×1440, where full scale takes 24.7 ms,
  dynamic resolution settles at 0.59× with a GPU median of 16.71 ms and a frame median
  of 17.28 ms for the 16.6 ms target, with no hitch at its steps; the limiter measured
  30.00, 60.00 and 120.00 fps; the bench picked Medium in 648 ms there, Ultra in 581 ms
  on a Radeon RX 7800 XT, Low (software) in ~0.5 s on llvmpipe.
- Hosts: the start order, the file, the baked blob, Steam Deck and mobile detection;
  CI smoke-runs the PC at each preset, then `auto`, `quality check` and `quality bench`.
- Web: the self-test's `quality bench` step in Chromium (WebGL2, WebGPU), Firefox and
  WebKit, and the hosted path of the `quality` step.
- Consoles: `TRINO_STEP:quality` on ares (N64, with and without the Expansion Pak) and
  Azahar (3DS, New 3DS).
- Editor: `ui_tests::quality_tab_writes_the_presets_and_the_game_view_previews_them`, the
  layout audit with the Quality tab and the PC Game view at 960×600 and 1600×940.
