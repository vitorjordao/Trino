# ADR 0018: The Web target — one renderer for PC and browsers, WebGPU with a WebGL2 fallback

Status: accepted (October 2026, GitHub #47; #52's phase 5, Web KTX2, is
[ADR 0017 §9](0017-texture-formats.md)).

## Context

Trino ran on the PC (wgpu), the N64 and the 3DS. A browser build lets a game be played
from a URL (itch.io, GitHub Pages, a studio's site) with no install. Issue #47 asked for
it end to end: a `wasm32-unknown-unknown` backend on WebGPU and WebGL2 for Chrome,
Firefox and Safari, audio that survives the browsers' autoplay rules, keyboard, mouse,
Pointer Lock, touch and gamepads, the editor simulating a browser, `xtask` commands, and
one CI job that runs all three browser engines.

The PC renderer had just gained a GPU T&L path (#51, ADR 0016) and BC textures (#52):
everything a browser's WebGPU can run. The question was how to put it in a browser
without a second renderer.

## Decisions

### 1. One renderer, two backends: `trino-render-wgpu`

- The PC's renderer (`renderer/`, `sim.rs`, `frame_report.rs`) moved out of
  `trino-platform-pc` into **`crates/render-wgpu`**, unchanged in behavior.
  `trino-platform-pc` re-exports it at its old paths (`trino_platform_pc::renderer::…`,
  `::sim`, `::PcRenderer`), so no caller changed. `trino-platform-web` uses the same
  crate: the browser draws with the PC's shaders, its T&L paths, its per-pixel lights,
  SSAO and water. `xtask/tests/web_parity.rs` holds the PC renderer to the browsers'
  golden frames (mean 0.15/255 measured).
- **Dependency rule** (`xtask/tests/dep_graph.rs`): platform crates may depend, within
  the workspace, on `trino-core` and `trino-render-wgpu`; the renderer only on
  `trino-core`. It is not a platform: it never sees a window, an audio device or the
  clock of a host.
- What the browser needs from it: a canvas constructor (`PcRenderer::new_canvas`,
  `WebBackend::{Auto, WebGpu, WebGl2}` — `Auto` is wgpu's WebGPU detection, which falls
  back to WebGL2 when `navigator.gpu` gives no adapter), `web_time::Instant` instead of
  `std::time::Instant` (which panics on wasm32), one T&L worker (no threads), a
  read-back that doesn't block (`try_read_offscreen`, `start_read_offscreen` +
  `poll_read`), and ETC2/ASTC uploads (`upload_sprite_compressed`) for phones.

### 2. WebGPU first; WebGL2 is the same picture with three workarounds

wgpu's GL backend in a browser has WebGL2's limits. Where they bit:

| Limit | Workaround |
|---|---|
| GLSL ES 3.0 has no `sampler2DMS`: the SSAO and the water read a multisampled depth under MSAA | `set_msaa` keeps one sample on WebGL2 |
| naga's GLSL can't `textureLoad` a `texture_depth_2d`; and wgpu leaves a depth texture's own GL filter at the mipmapped-linear default, so even read as a float it is *incomplete* and reads 0 (the water foamed everywhere) | `depth_as_float`: on WebGL2 the depth binding is an unfilterable float texture, read through a nearest sampler added to the bind group, at the texel's center |
| `glCopyTexSubImage2D` can't read a depth attachment: the water's depth copy stayed empty | `renderer/depth_blit.rs`: a full-screen triangle writes each texel as `frag_depth` |

Downlevel WebGL2 limits are requested on that backend (`BackendLimits`); WebGPU keeps the
PC's. The browser goldens hold both APIs and all three engines to one picture.

### 3. The page's services (`crates/platform-web`)

- **Platform:** `performance.now()`; logs to the console with a styled `trino` tag
  (errors and `TRINO_TEST_FAIL` as `console.error`) and to `globalThis.__trino.log`;
  blobs from the asset pack; the save in `localStorage` (`trino-save:<game>`, base64,
  one `setItem`: the browser replaces it atomically), capped at `Caps::WEB.save_bytes`.
  A private window without storage reports `save_capacity() == 0`, never a crash.
- **Input** (`keys.rs`, pure and host-tested): `KeyboardEvent.code` (the physical key,
  any layout) → `trino_core::Key` bits and the PC's default bindings; game keys don't
  scroll the page (`preventDefault`), Ctrl/Cmd shortcuts stay the browser's; focus loss
  releases every key. The mouse is `InputState::pointer` through the renderer's
  letterboxed upscale; under **Pointer Lock** a virtual pointer moves by the mouse's
  deltas inside the canvas. Touch: the first finger is the pointer and the primary
  button (`preventDefault`: the page doesn't scroll or zoom under the game). Gamepads:
  `navigator.getGamepads()` once a frame in the W3C standard mapping, folded like the
  PC's gilrs pads. Rebinding is the PC's only (`binding_label` reports the fixed
  bindings).
- **Audio:** Web Audio, each playback a source → gain → stereo panner → bus → master,
  music ducking under stingers like the PC mixer. The context starts suspended in every
  browser until a gesture; the first click, key or touch resumes it, a "click to enable
  sound" banner shows until then, and a refused `resume()` is logged (no unhandled
  rejection). Without Web Audio (Playwright's WebKit on Windows has none) the backend runs
  silent and keeps time: voices and stingers end by the page clock, so games and the
  self-test behave the same.
- **The canvas:** a `ResizeObserver` gives its size in device pixels
  (`devicePixelContentBoxSize`, else the CSS box × `devicePixelRatio` — Safari), so Retina
  screens are sharp; the picture is the native profile's (`sim::native_internal`: the
  output over an integer pixel scale). Fullscreen (Fullscreen API) and Pointer Lock need
  a gesture, so the page's toolbar offers them; a game asks through
  `Platform::set_display`.
- **The loop:** `requestAnimationFrame`, `dt` clamped to 0.1 s like the PC. No threads:
  wasm threads need `SharedArrayBuffer`, so the page is cross-origin isolated only when
  served with COOP/COEP (`xtask run web` sends them, for later).

### 4. Assets: one pack per game, KTX2 textures

`cargo xtask build web` bakes `Platform::Web` (which follows the PC where it has no
choice of its own: `assets/pc/` overrides, a model's `platforms = ["pc"]`, a scene's `pc`
variant) and writes `assets/<game>.pak` (`TPAK`: kind, id, length, bytes per asset).
Sprites go as KTX2 when their Web format is one (ADR 0017 §9), sounds as 16-bit PCM. The
page fetches the pack once at boot — a browser can't list a directory, and one request
beats a hundred. KTX2 textures are transcoded by Binomial's transcoder (WebAssembly,
vendored in `apps/web/vendor/basis`) to BC, ETC2, ASTC or RGBA8, whichever the device
samples.

### 5. The Web in content and tools

- **`Caps::WEB`:** the PC's tiers (per-pixel lights, SSAO, per-pixel water), 4096 texture
  sides (what every WebGL2 device samples), 1 GiB of texture memory (a mid-range phone's
  tab), 1 MiB of save, a 1 M-triangle strict-mode cap. **`SimProfile::Web`** renders like
  the native profile (`is_native`) with those budgets: the Web app uses it, the editor's
  Game view simulates a browser with it, `TRINO_SIM=web` on the PC app.
- **Formats:** scenes v4, prefabs v3 and particle effects v2 carry a `web` variant, LOD
  start and cap, each falling back to the PC's (`PlatformVariants::get`); an empty `web`
  variant beside a `pc` one means "as written" and survives pruning. Older files load
  unchanged; an engine that ignored `web` would drop it on save, hence the bumps.
- **Budget** (`trino_scene::budget`): the Web gets the PC's GPU-T&L split plus `WebCalls`
  — calls (GPU T&L draws sharing a model and a texture merge into one, CPU triangles one
  per texture, each water surface its own), pipelines and switches — with warnings past a
  phone's budget (`WEB_CALLS_WARN` 300 calls, `WEB_TRIS_WARN` 250 k triangles). `scene
  budget --platform web --json` and the editor print the same numbers; the Game view adds
  what its renderer just drew (`PcFrameTimes::draw_calls` and the new `pipeline_switches`).
- **Editor:** a Web toggle in the Game view with a canvas preset (responsive, 1280×720,
  1920×1080), a Web column in Inspector › Platforms, the LOD panel and the texture
  inspector, a fourth "Render path" pill, the Web lines in the platform budget, and
  **Run in Browser** / **Export Web Package** (the Play menu and the command palette) —
  which run `cargo xtask run web` / `build web --release --out dist/web`, the same commands
  an agent types.
- **`trino.toml`:** `"web"` in `[project] platforms` and an optional `[web]` (`package`,
  `canvas_id`, `page_title`). A file without `platforms` keeps targeting pc, n64 and 3ds
  (the Web needs a host crate); new projects list `web`, and the standalone template has a
  `web/` host (`trino_platform_web::run_game`, the Web twin of `trino-runner-pc`).

### 6. Tests: the browser self-test, in one CI job

- `apps/web`'s `?selftest` page runs the consoles' self-test step for step (spread over
  animation frames): `castle64::selftest::engine_checks`, audio and idle input, a KTX2
  transcode check, the showcase demo and its baked scene, castle64's settle/walk/jump and
  the bot's Green Hills run, Solid Eleven's scripted match (its sim hash must equal the
  PC's), then three golden frames read back at 640×480 (`tests/golden/web/`).
- **No synchronous read-back in a browser:** WebGPU maps buffers between frames and
  WebGL2's fences only move between tasks, so the engine checks' pixel probes report
  `skipped (no read_pixel)` there. The goldens, read back asynchronously, hold the
  picture instead, and the same probes run on the PC renderer
  (`xtask/tests/engine_checks.rs`).
- `cargo xtask test web` serves the build (a small HTTP server in xtask with the right
  MIME types) and drives Playwright (`tools/web-test/run.mjs`) through Chromium (WebGL2
  and WebGPU), Firefox and WebKit in one process; after the self-test it reloads (the save
  is still in `localStorage`), clicks (audio runs) and presses keys (they reach the game,
  `__trino.info.input`). A browser that gives no WebGPU adapter reports that run skipped.
  CI's `web` job does all of it on one runner, with one build.

## Consequences

- The Web costs the consoles nothing: `trino-core` gained `Caps::WEB` (a constant), the
  formats gained optional fields, and no console code changed. The N64 and 3DS goldens and
  perf gates are untouched.
- Anything the PC renderer gains, the Web gains, and the browser goldens and
  `web_parity` say when a change breaks one of them.
- Open (BACKLOG): Opus/Vorbis instead of PCM (#53, the download), lazy asset bundles
  (#56), wasm threads for the CPU T&L, a rebinding screen's storage, WebGPU in CI only
  where SwiftShader's Vulkan gives an adapter, and Safari on real Apple hardware (CI runs
  the WebKit engine).
