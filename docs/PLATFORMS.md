# Platform guides

Every command goes through `cargo xtask` (an alias in `.cargo/config.toml`), so the steps
are the same on Linux, Windows and macOS. Setup notes are also in
[CONTRIBUTING.md](../CONTRIBUTING.md); [HARDWARE.md](HARDWARE.md) covers real consoles.

## PC

Needs only Rust (stable, picked by `rust-toolchain.toml`). On Linux, install `pkg-config`
and the ALSA headers (`libasound2-dev`).

```sh
cargo xtask run pc            # build and launch castle64
cargo xtask editor            # the editor
cargo xtask watch pc          # live reload: game code as a dylib, assets rebaked on save
                              #   --game <crate> picks the game to rebuild (default castle64)
cargo xtask test              # the workspace suite (same as CI)
```

Run modes and switches for `trino-app-pc`:

| Variable | Effect |
|---|---|
| `TRINO_SIM=native\|n64\|3ds\|web` | Console simulation profile (default `n64`): internal resolution, filtering, color depth, the 3DS's two screens; `web` is the native picture with the browser's budget (`Caps::WEB`) |
| `TRINO_LOOK=off\|n64` | Turns the N64 output emulation off or on, independently of the profile |
| `TRINO_STRICT=1` | Strict mode: panic when content exceeds the profile's `Caps` |
| `TRINO_SCENE=scenes/x.scene.ron` | Play an editor scene instead of the game; it reloads when the scene, its prefabs or materials change |
| `TRINO_GAME=showcase` | Run `examples/showcase`, the demo scene exported to code |
| `TRINO_SMOKE_FRAMES=N` | Exit after N frames (CI smoke runs) |
| `TRINO_BC=0` | Upload BC1/BC3/BC7 textures decoded instead of as blocks (the path of a GPU without BC); a smoke run prints `TRINO_TEXTURES:` with the VRAM either way |

Keyboard mapping: A/B = Z/X, X/Y = C/V, L/R = Q/E, Start = Enter, Select = Right Shift,
D-pad = arrows, stick = WASD. The mouse feeds `InputState::pointer`. Gamepads go through
gilrs behind the `gamepad` feature of `trino-platform-pc` and `trino-runner-pc`; on Linux
it links libudev (`libudev-dev`), so it is opt-in there. Keys and pad buttons can be
rebound from the game (`Platform::rebind`).

The editor has its own guide in the [editor](../.agents/skills/editor/SKILL.md) skill,
including hooks that let a script or an agent screenshot the window.

## Nintendo 64

**Toolchain:** Docker. `cargo xtask build n64` builds the pinned `trino-n64` image the first
time (libdragon pinned by commit, about ten minutes), then the C shim, the RSP overlay,
the assets and the ROM. Only rustc runs on the host, on nightly with `-Zbuild-std`
(`rustup toolchain install nightly --component rust-src`) against
`platforms/n64/mips-nintendo64-none.json`. On Windows, Docker must be reachable from WSL2;
a plain dockerd inside WSL is enough.

**Emulator:** [ares](https://ares-emu.net) v148, found at `$TRINO_ARES` or `ares-v148/` in
the repo root (gitignored). `cargo xtask emulators setup ares` builds a headless ares image
(Xvfb, Mesa lavapipe); `TRINO_ARES=docker` uses it, which is what CI does.

```sh
cargo xtask build n64         # target/n64/trino.z64
cargo xtask run n64           # build and open in ares
cargo xtask test n64          # self-test ROM: every step, the golden frames, the perf gates
cargo xtask test n64 --bless  # rewrite the goldens and the N64 perf ceilings (review them)
cargo xtask watch n64         # rebuild and relaunch on save
```

`test n64` reads the ROM's ISViewer output (`TRINO_TEST_PASS`/`TRINO_TEST_FAIL`), compares
framebuffer dumps with `tests/golden/n64/` pixel for pixel, and checks the emulated frame
cost per scene and stage against `tests/perf/castle64_n64.toml`. Measuring and improving
that cost is the [n64-performance](../.agents/skills/n64-performance/SKILL.md) skill.

Things to know:

- Rust is compiled as o32 and libdragon as o64. Every C call goes through the shim with at
  most four scalar or pointer arguments, and no scalar float after an integer argument
  ([ADR 0002](adr/0002-n64-abi-o32-staticlib.md)).
- The depth buffer is 16-bit. Draw wide worlds at a reduced scale (1/128 works) and keep
  coplanar surfaces apart.
- The main stack is 64 KB, and an overflow is silent. Build large structs in place on the
  heap; the self-test fails above 60 KB.
- Troubleshooting: the [build-n64](../.agents/skills/build-n64/SKILL.md) and
  [run-emulator](../.agents/skills/run-emulator/SKILL.md) skills.

## Nintendo 3DS

**Toolchain:** a local [devkitPro](https://devkitpro.org/wiki/Getting_Started) install with
the `3ds-dev` packages, found at `$DEVKITPRO`, `C:\devkitPro` or `/opt/devkitpro`. No
Docker. Rust uses the built-in `armv6k-nintendo-3ds` target on nightly with
`-Zbuild-std`. The shim covers libctru, citro2d (sprites) and citro3d (all 3D, with
per-vertex depth).

**Emulator:** [Azahar](https://azahar-emu.org), found at `$TRINO_AZAHAR`, in
`target/emulators/azahar/` (`cargo xtask emulators setup azahar` fetches the pinned
AppImage on Linux x86_64), in the default install directory or on `PATH`. Headless runs
use Xvfb.

```sh
cargo xtask build 3ds         # target/3ds/trino.3dsx
cargo xtask run 3ds           # build and open in Azahar
cargo xtask test 3ds          # self-test app: every step and the golden frames
cargo xtask test 3ds --bless  # rewrite the 3DS goldens (review the PNGs)
cargo xtask build 3ds --selftest && cargo xtask test 3ds --no-build   # build now, boot later (CI's split)
cargo xtask watch 3ds         # rebuild and relaunch on save
```

`test 3ds` reads the `TRINO_TEST_*` lines from Azahar's log (`svcOutputDebugString`) and
compares the top and bottom screens with `tests/golden/3ds/` within a tolerance, because
Azahar's pixels depend on the host's OpenGL.

Things to know:

- The bottom screen is 320x240 and touch-enabled. Draw to it after
  `renderer.on_screen(Screen::Bottom)`, and set a camera for it every frame.
  `InputState::pointer_on(Screen::Bottom)` gives touches in its own pixels.
- Never draw 3D with `C2D_DrawTriangle`: it has one depth per triangle. All 3D goes
  through the shim's citro3d path.
- Troubleshooting: the [build-3ds](../.agents/skills/build-3ds/SKILL.md) and
  [run-emulator](../.agents/skills/run-emulator/SKILL.md) skills.

## Web (WebGPU / WebGL2)

A Trino game in a browser tab, from one `wasm32-unknown-unknown` build ([ADR
0018](adr/0018-web-target.md), issue #47). It runs the PC's own renderer
(`crates/render-wgpu`, shared by `trino-platform-pc` and `trino-platform-web`): the same
shaders, the GPU T&L path, per-pixel lights, SSAO and water.

**Toolchain:** Rust's `wasm32-unknown-unknown` target (`rustup target add
wasm32-unknown-unknown`, added for you when missing) and the `wasm-bindgen` CLI at the
exact version of the `wasm-bindgen` crate in `Cargo.lock` (installed into `target/tools`
the first time, or `$TRINO_WASM_BINDGEN`). `cargo xtask test web` also needs Node.js: it
drives the browsers with Playwright (`tools/web-test`, `npm ci` and `npx playwright
install` run for you).

```sh
cargo xtask build web [--release]       # target/web/{index.html, trino.js, trino.wasm, assets/, basis/}
cargo xtask run web [--port 8080]       # build, serve on 127.0.0.1, open the browser
cargo xtask run web --query "?game=solid-eleven"
cargo xtask build web --release --out dist/web   # a folder to upload (itch.io, GitHub Pages)
cargo xtask test web                    # the self-test in Chromium (WebGL2 + WebGPU), Firefox, WebKit
cargo xtask test web --browsers chromium --headed --no-build
cargo xtask test web --bless            # rewrite tests/golden/web/ (review the PNGs)
```

The page picks the game and the look by its URL:

| Query | Effect |
|---|---|
| `?game=castle64\|showcase\|solid-eleven` | The game (castle64 by default); Solid Eleven loads its own pack |
| `?backend=auto\|webgpu\|webgl2` | WebGPU where the browser gives an adapter, else WebGL2 (`auto`), or one of them |
| `?sim=web\|native\|n64\|3ds` | The profile (default `web`); the console looks run in a browser too |
| `?scale=N` | Integer pixel scale of the picture (default auto: about 300 lines) |
| `?size=WxH` | A fixed canvas in CSS pixels (tests, screenshots); otherwise it fills the page |
| `?msaa=0` | No 4× MSAA (WebGL2 never has it: it can't sample a multisampled depth) |
| `?frames=N` | Stop after N frames and print a `TRINO_FRAME:` line (smoke runs) |
| `?selftest` | The browser self-test (below) |
| `?log` | A log panel under the canvas |

What a browser changes:

- **Audio** starts suspended (every browser's autoplay rule, Safari's strictest). The
  first click, key or touch resumes it; a banner says so until then. A browser without
  Web Audio runs silent, with voices and stingers still keeping time.
- **Input:** the keyboard by `KeyboardEvent.code` (layout-independent) with the PC's
  bindings; game keys (arrows, Space, Tab, WASD, the bound keys) don't scroll the page,
  Ctrl/Cmd shortcuts stay the browser's. The mouse is `InputState::pointer`; **Lock
  mouse** (or `canvas.requestPointerLock()`) moves a virtual pointer by the mouse's deltas
  for 3D navigation. Touch is the pointer and its primary button. Gamepads in the W3C
  standard mapping act like the PC's (A/B/X/Y, LB+LT = L, RB+RT = R, sticks, analog
  triggers).
- **The canvas** follows its element through a `ResizeObserver`, in device pixels
  (`devicePixelContentBoxSize`, or the CSS size × `devicePixelRatio` on Safari), so Retina
  screens stay sharp; the picture is the native profile's (the output over the pixel
  scale). **Fullscreen** is the toolbar's button (or `Platform::set_display`).
- **Saves** go to `localStorage` (`trino-save:<game>`, base64, at most
  `Caps::WEB.save_bytes` = 1 MiB). A private window without storage reports
  `save_capacity() == 0`.
- **Assets** come in one pack per game (`assets/trino.pak`, `assets/solid-eleven.pak`),
  fetched at boot. Textures are KTX2 (Basis Universal), transcoded at load to BC on
  desktops, ETC2 or ASTC on phones, RGBA8 elsewhere; sounds are 16-bit PCM (Opus is #53).
- **WebGL2's limits:** no MSAA, and wgpu's GL backend can't copy or `textureLoad` a
  depth texture, so the SSAO and the water read the depth through a nearest sampler and
  the water's depth copy is drawn (`renderer/depth_blit.rs`). The picture is the same:
  `tests/golden/web/` holds every browser and the PC to it.
- **No threads:** the parallel CPU T&L runs on one worker (wasm without
  `SharedArrayBuffer`); the GPU T&L path takes most draws anyway.

`test web` serves the build and drives `?selftest` in each browser: the engine checks
(blobs, a save to `localStorage`, every renderer step), audio and idle input, the KTX2
transcode (both Basis modes decode exactly as the bake's native transcoder did), the
showcase demo and its baked scene, castle64's settle/walk/jump and the bot's whole Green
Hills run, Solid Eleven's scripted match (its sim hash must equal the PC's and the
consoles'), then three golden frames read back at 640×480 and compared with
`tests/golden/web/` (mean ≤ 3/255, ≤ 2 % far pixels). After the self-test it reloads the
page (the save is still there), clicks (audio unlocks) and presses keys (they reach the
game). A browser without WebGPU here reports its WebGPU run as skipped; the WebGL2 runs
cover it. `xtask/tests/web_parity.rs` holds the PC renderer to the same goldens.

The pixel probes of the engine checks need a synchronous read-back, which no browser
gives (WebGPU maps buffers between frames; WebGL2's fences only move between tasks), so
in a browser they report `skipped (no read_pixel)` and the golden frames, read back
asynchronously, hold the picture. The same probes run on the PC renderer in
`xtask/tests/engine_checks.rs`.

Troubleshooting: the [build-web](../.agents/skills/build-web/SKILL.md) skill.

## External projects

A game in its own repository builds for the consoles with the same commands
([ADR 0008](adr/0008-external-console-projects.md)). It needs a `trino.toml` and a local
engine checkout, usually an `engine/` git submodule:

```sh
cargo xtask build|run|test|watch n64|3ds --project <dir>   # from the engine checkout
cargo trino build n64                                       # from a standalone project (alias)
```

xtask also picks the project from `$TRINO_PROJECT` or a `trino.toml` in the current
directory. `cargo xtask new <name> --standalone <dir>` scaffolds such a project; the
[new-project](../.agents/skills/new-project/SKILL.md) skill describes the `trino.toml` keys
and the `n64_main!`/`n3ds_main!` hosts.

## Real hardware

`cargo xtask run n64 --device sc64|everdrive` and `cargo xtask run 3ds --device network
--ip <IP>` send the build to a console and print its debug log in the terminal. They have
been tested with stub loaders, not yet on physical devices. See
[HARDWARE.md](HARDWARE.md).
