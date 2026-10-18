---
name: build-web
description: Build, run and test Trino in a web browser (wasm32-unknown-unknown, WebGPU with a WebGL2 fallback) — `cargo xtask build|run|test web`, the Playwright harness, KTX2 textures, the browser self-test and goldens, and the WebGL2 traps. Use for any "web / wasm / browser / WebGPU / WebGL / itch.io" request.
---

# build-web

```
cargo xtask build web [--release]              # target/web/{index.html, trino.js, trino.wasm, snippets/, basis/, assets/*.pak}
cargo xtask build web --release --out dist/web # a folder to upload (the editor's Export Web Package)
cargo xtask run web [--port 8080] [--query "?game=solid-eleven&backend=webgl2"]
cargo xtask test web [--browsers chromium,firefox,webkit] [--headed] [--no-build] [--bless]
cargo xtask assets web                         # the Web bake alone (KTX2 beside the masters)
```

ADR 0019 is the design; `docs/PLATFORMS.md` › Web the user guide.

## What runs where

- `crates/render-wgpu`: the PC's renderer, shared. Anything you change there changes the
  Web too — run `xtask/tests/web_parity.rs` (PC vs `tests/golden/web/`) and a browser.
- `crates/platform-web`: browser-free modules (`keys.rs`, `pack.rs`, `b64.rs`,
  `sound.rs` — TSN2 decode whole or in stream blocks, the bus-settings text: host tests)
  and wasm-only ones (`platform.rs`, `input.rs`, `audio.rs`, `assets.rs`, `runtime.rs`,
  `js.rs` + `glue.js`). Off wasm32 only the first build.
- Web audio (#53, ADR 0021): pack TPAK v2 (sounds are TSN2, kind 7 the `TMIX` mix; v1
  packs load, `TS16` migrates to PCM16 TSN2). Effects decode into `AudioBuffer`s at
  load; streams are 8192-sample chunks scheduled ≤ 1 s ahead (`audio.rs`), so a hidden
  tab runs dry after 1 s and counts an underrun. Each bus is a gain node fed by
  `BusMixer` in `poll()`. The player's volumes: `localStorage` `trino-audio:<save>`.
- `apps/web`: `lib.rs` (game by `?game=`), `selftest.rs` (`?selftest`), `index.html`
  (templated by xtask: `{{TITLE}}`, `{{CANVAS_ID}}`), `vendor/basis/` (Binomial's
  transcoder, the KTX2 decoder in the browser).
- `xtask/src/web.rs`: toolchain checks, the packs, wasm-bindgen, the HTTP server, the
  harness and the golden comparison.
- `tools/web-test/run.mjs`: Playwright, one process, every browser; writes
  `target/web-test/<run>/{console.log, *.png}` and `results.json`.

## Toolchain

- `wasm32-unknown-unknown` (xtask runs `rustup target add` when missing).
- The wasm-bindgen CLI at the **exact** version of the `wasm-bindgen` crate in
  `Cargo.lock` (the workspace pins `=0.2.x`): `$TRINO_WASM_BINDGEN`, PATH, or installed
  into `target/tools` by xtask the first time (about a minute). Bump the crate and the CLI
  follows.
- `test web`: Node.js. xtask runs `npm ci` in `tools/web-test` and `npx playwright
  install <browsers>` (skip with `TRINO_SKIP_BROWSER_INSTALL=1`). The full Chromium is
  needed for WebGPU (`npx playwright install chromium` installs it with the headless shell).

## The browser self-test

`?selftest` (spread over animation frames): `castle64::selftest::engine_checks`, audio,
idle input, `ktx2 transcode` (both Basis modes decode exactly as the bake's native
transcoder: fixtures `web/ktx2_*` packed by xtask), the showcase demo + baked scene,
castle64 settle/walk/jump + the bot's Green Hills run, Solid Eleven's scripted match
(`TRINO_SE_HASH` must equal `solid_eleven::selftest::EXPECTED_HASH`), then the golden
frames `showcase`, `castle64_hub`, `texture_formats` at 640×480, single-sampled.

The harness then reloads (`localStorage` save still there), clicks (audio `running`) and
presses keys (`__trino.info.input` shows them). Runs: `chromium-webgl2`,
`chromium-webgpu` (skipped, not failed, when the browser gives no adapter),
`firefox-webgl2`, `webkit-webgl2`. Goldens: mean ≤ 3/255, ≤ 2% far pixels; `--bless`
rewrites them from the first passing run — review the PNGs, and run `web_parity`.

Locally, Chromium's WebGL2 is SwiftShader (as in CI) and its WebGPU, Firefox and WebKit
use the real GPU; all four stayed within 0.12/255 of each other on the goldens.

Debug a run: `target/web-test/<run>/console.log` has every `TRINO_STEP:` and the
browser's own warnings; `--headed --browsers chromium` shows the page; `?log` adds a log
panel; `globalThis.__trino` in devtools holds the state (`steps`, `info`, `frames`).

## Traps (all hit while building it)

- **WebGL2 depth:** naga's GLSL can't `textureLoad` a `texture_depth_2d`, and wgpu's GL
  backend leaves a depth texture's filter incomplete (a fetch reads 0). Shaders that read
  depth go through `depth_as_float` (unfilterable float binding + a nearest sampler at
  `NEAREST_DEPTH_BINDING`); check them with
  `cargo test -p trino-render-wgpu webgl2_shaders_translate_to_glsl_es_300`.
- **WebGL2 can't copy depth** (`copy_texture_to_texture` → `GL_INVALID_FRAMEBUFFER_
  OPERATION`): `renderer/depth_blit.rs` draws it.
- **No MSAA on WebGL2** (no `sampler2DMS` to read the multisampled depth): `set_msaa` keeps
  one sample there; the goldens are single-sampled anyway.
- **No synchronous read-back in a browser:** `read_pixel` is `None` (probes `skipped`);
  use `start_read_offscreen` + `poll_read` across frames.
- **`std::time::Instant` panics on wasm**: shared code uses `web_time::Instant`.
- **wgpu 29 needs a display handle for canvas surfaces** (`new_canvas` passes a
  `RawDisplayHandle::Web`).
- **A browser without Web Audio** (Playwright's WebKit on Windows) runs `WebAudio`
  silent, voices ending by the page clock.
- **WebGPU needs a secure context** (`127.0.0.1` yes, `about:blank` no) and an adapter;
  the headless shell has none.
- **Closures handed to JS must outlive their use:** a fire-and-forget sound clears its
  `onended` before its `Closure` drops (`audio.rs`).
- **wgpu objects aren't freed on drop in a browser** (the page's GC frees them): never
  allocate per frame or per probe on WebGPU. `try_read_texture` returns `None` there
  before allocating; mesh uploads go through `queue.write_buffer`, not
  `mapped_at_creation` (Chromium on SwiftShader refused one partway through Solid
  Eleven's 197 meshes).

## GPU-less Linux (CI)

What the `web` job needs, all found the hard way (reproduce with Docker:
`mcr.microsoft.com/playwright:v1.63.0-noble`, `docker run --init --ipc=host`):
- Chromium WebGPU: `--use-angle=swiftshader --use-vulkan=swiftshader
  --use-webgpu-adapter=swiftshader` (the harness adds them on Linux). With
  `--disable-vulkan-surface` the canvas swap chain had no shared-image backing and
  Chromium destroyed the device (`device.lost` reason `destroyed`) mid-run.
- Firefox: headless has no WebGL there ("Exhausted GL driver options"); headed under
  `xvfb-run` it gets llvmpipe. `TRINO_WEB_FIREFOX_HEADED=1` runs it headed.
- Audio: Firefox's `AudioContext` stays `suspended` without an output device; CI starts
  PulseAudio with a null sink. Playwright's Chromium runs an `AudioContext` with no
  gesture whatever the autoplay flag, and WebKit starts `running`: only Firefox proves
  the suspended → running unlock.
- Firefox and WebKit on WSL: run them in the Playwright Docker image
  (`mcr.microsoft.com/playwright:v1.63.0-noble`) with `--init` (xvfb-run as PID 1 hangs)
  and a PulseAudio null sink, and serve the build from inside the container (Docker
  Desktop's host network is not WSL's localhost).
- Self-test checks that count frames need steady frames: the first frames after the
  GPU-heavy steps are slow (the `audio codecs` step waits for 10 steady frames).
- Never `pkill -f` a pattern that also appears in your own shell command: it kills that
  shell.

## Sizes and speed (2026-10-18)

castle64 + showcase pack 456 KB (33 KTX2 textures, Vorbis sounds; 944 KB before #53), Solid
Eleven's 2.18 MB (3.07 MB with 16-bit PCM). The release wasm is ~5.4 MB before compression
(lewton added 72 KB gzipped). The self-test takes
8–15 s per browser locally.
