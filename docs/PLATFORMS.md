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

The system language (`Platform::system_language()`, [ADR 0026](adr/0026-localization.md))
is the OS locale (`sys-locale`); `TRINO_LANG=pt-BR` overrides it for a run (the editor's
Play passes its preview language that way).

Run modes and switches for `trino-app-pc`:

| Variable | Effect |
|---|---|
| `TRINO_SIM=native\|n64\|3ds\|web` | Console simulation profile (default `n64`): internal resolution, filtering, color depth, the 3DS's two screens; `web` is the native picture with the browser's budget (`Caps::WEB`) |
| `TRINO_SIM=n64x\|3dsnew` | A console's expanded hardware tier ([ADR 0020](adr/0020-console-hardware-tiers.md)): the N64 with the Expansion Pak or the New 3DS. The console's look, the tier's `Caps` (`renderer.caps().hw_tier`, RAM, cores, crowd/particle scales) and `Platform::hw_tier()`; `TRINO_SCENE` resolves the tier's variants |
| `TRINO_LOOK=off\|n64` | Turns the N64 output emulation off or on, independently of the profile |
| `TRINO_STRICT=1` | Strict mode: panic when content exceeds the profile's `Caps` (also the loaded models against the tier's `ram_bytes`: 4 MB N64, 8 MB with the Pak), and when boot plus the loaded asset groups pass the console's RAM left for assets (`cargo xtask assets report`'s numbers, [ADR 0022](adr/0022-asset-groups-and-memory.md)): the panic names the groups, the overrun and the biggest assets |
| `TRINO_SCENE=scenes/x.scene.ron` | Play an editor scene instead of the game; it reloads when the scene, its prefabs or materials change |
| `TRINO_GAME=showcase` | Run `examples/showcase`, the demo scene exported to code |
| `TRINO_SMOKE_FRAMES=N` | Exit after N frames (CI smoke runs) |
| `TRINO_BC=0` | Upload BC1/BC3/BC7 textures decoded instead of as blocks (the path of a GPU without BC); a smoke run prints `TRINO_TEXTURES:` with the VRAM either way |
| `TRINO_QUALITY=low\|medium\|high\|ultra\|auto` | The quality preset for this run (`cargo xtask run pc --quality low`), never saved; `auto` runs the calibration. Native profile only (below) |
| `TRINO_QUALITY_BENCH=1` | Run the calibration, print its `TRINO_QUALITY: bench` line and exit (`cargo xtask quality bench`) |
| `TRINO_MSAA=0` | No MSAA, whatever the preset says |
| `TRINO_SHADOWS=off\|debug` | No shadows, or every shadow in magenta at full strength (where each caster's lands); [ADR 0025](adr/0025-shadows-per-tier.md) |

Keyboard mapping: A/B = Z/X, X/Y = C/V, L/R = Q/E, Start = Enter, Select = Right Shift,
D-pad = arrows, stick = WASD. The mouse feeds `InputState::pointer`. Gamepads go through
gilrs behind the `gamepad` feature of `trino-platform-pc` and `trino-runner-pc`; on Linux
it links libudev (`libudev-dev`), so it is opt-in there. Keys and pad buttons can be
rebound from the game (`Platform::rebind`).

Audio goes out through cpal. Sounds are `TSN2` (Ogg Vorbis by default, decoded by lewton
in Rust): effects decode to f32 at load, music streams — a worker thread decodes into a
half-second lock-free ring per playback, so a 10-minute track costs its Ogg bytes plus
~320 KB, and loops are sample-exact. Six buses (master, music, sfx, voice, ambience, ui)
with mutes and duck rules start from the project's `assets/audio.toml`; the player's
volumes persist per game in `<save_name>.audio` next to the save
([ADR 0021](adr/0021-audio-pipeline.md), [ASSETS.md](ASSETS.md#audio-a-codec-per-platform-streaming-buses-adr-0021)).

UI screens ([ADR 0024](adr/0024-ui-system.md), [ASSETS.md](ASSETS.md#ui-screens)) scale by
whole pixels against their reference size — ×4 at 1080p, ×9 at 4K, ×4 at 2560×1080 with
the anchors at the monitor's real edges — times the player's interface scale
(`DisplaySettings::ui_scale`, in quarters: 6 is 150 %). The mouse focuses what it hovers
and clicks it; Enter and Escape work like A and B. A saved screen or string table reloads
in the running game. With `TRINO_STRICT=1` on a console profile, a screen over that
console's UI budget panics.

### Accessibility

The player's accessibility settings ([ADR 0027](adr/0027-accessibility.md)) live in
`target/save/<save_name>.access` (`key=value` lines, written when the game changes them,
never on smoke runs); castle64 keeps `target/save/castle64.access`.
`TRINO_ACCESS=cvd=deuteranopia,text_scale=200,high_contrast=on,captions=all` forces
settings for one run (nothing is written then), and `TRINO_CVD=protanopia|deuteranopia|
tritanopia|grayscale` shows the window as a viewer with that color vision sees it (the
renderer's simulation view, a review tool: the game's colors are unchanged). In strict
mode the renderer warns when full-screen fills strobe more than three times a second.
In a browser the settings live in `localStorage` (`trino-access:<name>`), `?access=...`
forces them and `?cvd=deuteranopia` turns the simulation view on. The consoles keep them for the session: a game stores `AccessSettings::pack`
(3 bytes) in its save to keep them longer; everything else (palettes, captions, text
scale, high contrast, icons, motion, flashes) runs the same on the N64 and the 3DS.

### Shadows

A draw's `ModelParams::shadow` says what it casts once, and each target draws the best it
affords ([ADR 0025](adr/0025-shadows-per-tier.md)). The native profile and the Web draw a
shadow map: every surface in the light's view within `distance` meters, PCF-filtered.
The quality preset sizes it (1024 texels on Low, 2048 above) and sets the filter (2×2,
3×3, 4×4 taps); `shadows: Off` in a preset keeps the planar shadows and runs no map pass.
The console profiles draw the console's tier, blobs on `n64` and planar shadows on `3ds`,
so a PC run or the editor's Game view shows what the console shows. Blob and planar
shadows land only on the receivers the game sets (`set_shadow_receivers`); the map
ignores them.

### Quality presets

On the native profile (`TRINO_SIM=native`, a standalone project's PC build) the game runs
at one of four presets — **Low**, **Medium**, **High**, **Ultra** — each a render scale,
dynamic resolution, MSAA, AO, LOD bias, particle scale, anisotropy, sharpening and a
frame limit ([ADR 0023](adr/0023-quality-presets.md)). High is the PC's look before
presets; Ultra is High supersampled at 1.25×. The consoles' presets are fixed (N64 and
3DS Low, New 3DS Medium) and change nothing there.

- **First launch:** `trino.toml`'s `[pc.quality] default` — `auto` (the default) draws a
  built-in scene at each preset for at most 2 s (about 0.5 s on CI's software adapter)
  and keeps the highest that holds 90% of the display's frame; a software adapter gets
  Low. A **Steam Deck** (`SteamDeck=1` or its DMI board) starts at Medium capped at
  60 fps. The result is saved to `<save_name>.quality` next to the save: `key=value`
  text, safe to edit or delete (every value is clamped on read).
- **Dynamic resolution** (Low, Medium) moves the 3D's scale within its range to hold the
  frame time — GPU timestamps where the adapter has them, else the frame intervals. The
  3D is upscaled bilinearly with contrast-adaptive sharpening; the HUD always draws at
  the picture's resolution.
- **Frame limit:** VSync, 30/40/60/120/144 fps or unlimited; a cap under the display's
  rate presents FIFO and sleeps, over it presents mailbox or immediate.
- **`TRINO_FRAME`** carries the 1% lows and the quality:
  `low1_us=… low1_fps=… quality=low scale=0.82 quality_from=detected detected=low gpu_us=…`.
- **The project's presets** live in `assets/quality.ron` (each preset names only what it
  changes); `cargo xtask quality show|check|set|reset|bench` and the editor's Quality tab
  edit it:

```sh
cargo xtask quality show                      # the table (--json: every value)
cargo xtask quality set low render_scale=0.6 frame_limit=30
cargo xtask quality reset low                 # back to the engine's values
cargo xtask quality bench --json              # calibrate on this machine
cargo xtask run pc --quality ultra            # one run at a preset
```

A game reads `Platform::quality()` (and `lod_bias` / `particle_scale` are applied for
it), changes it with `set_quality` (saved as the player's choice) and can show the
engine's ready-made **Graphics page**, `trino_core::GraphicsOptions` (castle64: Select).

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
- **No system language:** `Platform::system_language()` is `None` ([ADR
  0026](adr/0026-localization.md)). A game starts in its default language and saves the
  player's choice (a `LangId` is 4 bytes of EEPROM).
- **Expansion Pak** ([ADR 0020](adr/0020-console-hardware-tiers.md)): the ROM checks the
  RAM at boot and logs `TRINO_HW n64 ram=8M tier=expanded` (or `ram=4M tier=base`).
  `Platform::hw_tier()` and `renderer.caps()` (`Caps::N64_EXPANDED`: 8 MB) follow; content
  takes its `n64_expanded` variants there. The ROM must still run in 4 MB. `run|test|watch
  n64 --expansion-pak on|off` sets ares's switch (`test`: also `both`, what CI runs).
- Audio: every `wav64` streams from the cartridge (VADPCM by default, written by Trino's
  encoder; `opus` links libopus, +99 KB, only when an asset asks for it). Call
  `N64Audio::poll` once per frame: it mixes on the RSP while the CPU waits — ~0.5 ms per
  60 Hz frame with nothing playing, +0.26 ms per 44.1 kHz VADPCM voice (+0.16 ms at
  22.05 kHz), ~4.6 ms more for an Opus track (`N64_COST.audio_us`, measured in ares over
  eight code layouts). The self-test prints them as `TRINO_AUDIO_COST:` lines, and
  `TRINO_AUDIO:` with resident and streaming bytes and underruns (a poll that finds all
  four 40 ms output buffers empty: the game went ~160 ms without polling).
- **Asset groups and RAM** ([ADR 0022](adr/0022-asset-groups-and-memory.md)): boot assets
  load at boot, a group's from its `groups/<name>.tbnd` file list for 4 ms a frame
  (`N64Assets::pump`). DFS files are compressed with libdragon's `mkasset` at each group's
  level (`[groups.boot]` too; castle64 and Solid Eleven ship boot at level 1, 21–31 % off
  the DFS), sprites by `mksprite --compress`; `asset_load` decodes as it reads. Boot logs
  `TRINO_MEM: ram= heap_total= before_assets= boot_assets=`, and the self-test fails when
  the heap's peak leaves less than 128 KB of libdragon's heap (`TRINO_HEAP:` lines).
- **Shadows** ([ADR 0025](adr/0025-shadows-per-tier.md)): a blob under each caster on
  the receiver below it (~111 µs of CPU each, after the opaque batch; 22 blobs ≈
  14.7 ms in ares), and the first two `ShadowCast::Mesh` casters of a frame projected
  flat instead (the rest cast nothing: `ShadowStats::over_budget`). Blobs sit four steps
  of the RDP's 18-bit compressed z in front of their receiver, so they never z-fight.
  The `shadows`, `shadows golden frame` and `shadow cost` self-test steps hold them.
- **UI** ([ADR 0024](adr/0024-ui-system.md)): a screen's root keeps 16×12 pixels inside
  the 320×240 picture, the margin a TV's overscan hides (`Caps::safe_area`; `bleed` nodes
  fill it). The budget is 64 laid-out nodes and 384 2D draws a frame; `cargo xtask ui
  check` fails a screen past it, or one with a control the pad cannot reach (the D-pad, the
  stick and the C buttons move the focus).
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
- **System language** ([ADR 0026](adr/0026-localization.md)): the boot reads the
  console's language (`CFGU_GetSystemLanguage`) and logs `TRINO_LANG <tag>` after the
  `TRINO_HW` line. It is a bare language (`pt`, `es`, `zh-CN`): `Localizer::pick` matches
  it to `pt-BR` by its primary subtag.
- **New 3DS** ([ADR 0020](adr/0020-console-hardware-tiers.md)): at boot the app asks
  `APT_CheckNew3DS` and turns the 804 MHz speedup and the L2 cache on
  (`osSetSpeedupEnable`), logging `TRINO_HW 3ds new=1 speedup=1 app_ram=…M tier=expanded`.
  `Platform::hw_tier()` and `renderer.caps()` (`Caps::N3DS_NEW`: 2 cores, crowds and
  particles ×2) follow; content takes its `n3ds_new` variants. Opt out with
  `[3ds] speedup = false` in `trino.toml` (or `--speedup off` for one command): the New 3DS
  then runs at the old clock on the base tier. `run|test|watch 3ds --new3ds on|off` sets
  Azahar's console (`test`: also `both`, what CI runs).
- **The second core** (`trino_platform_3ds::worker`): `worker::start()` gets core 2 on a
  New 3DS (the app's whole) or core 1 on an old one (30% of it), `submit(job, arg)` queues
  an `extern "C" fn(u32)` (or runs it inline without a worker), `wait()` joins. Jobs
  compute into memory; the renderer, audio and platform stay on the main thread.
- Audio: sounds bake to `T3AU` (DSP-ADPCM by default, decoded by the DSP at no CPU cost,
  ~3.5× smaller than PCM16). Whole sounds load into linear memory at boot; music streams
  from the RomFS through three 1024-frame wave buffers per playback (24 KB for ADPCM),
  refilled by an audio thread woken by ndsp. It runs on the worker's second core (core 2
  on a New 3DS, core 1 at 30% on an old one; one grant for both, `stream_core=` in the
  `TRINO_AUDIO` line) one priority above the jobs, so a long job never holds a refill
  back; without a second core, on the app core. Every buffer carries its ADPCM context, so loops and
  chunk seams play exactly what the bake previews. On hardware, audio needs the DSP
  firmware (`sdmc:/3ds/dspfirm.cdc`, or launch through the Homebrew Launcher); without it
  ndsp does nothing and the log says `TRINO_AUDIO: ndsp unavailable`. Azahar runs without
  it.
- **Asset groups and RAM** ([ADR 0022](adr/0022-asset-groups-and-memory.md)): a group's
  textures, meshes, fonts and blobs are LZ4 entries in `groups/<name>.tbnd`, decoded on
  the worker core four ahead while the main thread uploads for 6 ms a frame (its sounds stay
  `T3AU` files beside it). `[groups.boot] compression = { 3ds = "lz4" }` packs boot the same
  way into `groups/boot.tbnd`, decoded at boot. GPU memory of an unloaded asset is freed two
  frames after its last draw. Boot logs the same `TRINO_MEM:` line as the N64.
- **Shadows** ([ADR 0025](adr/0025-shadows-per-tier.md)): each caster's mesh flattened
  onto the receiver under it along the light by the GPU T&L, and drawn through the
  stencil so overlapping shadows darken once. A `Blob` cast draws a blob.
- **UI** ([ADR 0024](adr/0024-ui-system.md)): nodes with `screen: Bottom` go to the
  320×240 touch screen and a tap clicks them (on a one-screen target they join the main
  screen); the pad reaches both screens. The budget is 128 nodes and 2048 2D draws.
- Troubleshooting: the [build-3ds](../.agents/skills/build-3ds/SKILL.md) and
  [run-emulator](../.agents/skills/run-emulator/SKILL.md) skills.

## Web (WebGPU / WebGL2)

A Trino game in a browser tab, from one `wasm32-unknown-unknown` build ([ADR
0019](adr/0019-web-target.md), issue #47). It runs the PC's own renderer
(`crates/render-wgpu`, shared by `trino-platform-pc` and `trino-platform-web`): the same
shaders, the GPU T&L path, per-pixel lights, SSAO and water.

The system language is `navigator.language`; `?lang=pt-BR` overrides it ([ADR
0026](adr/0026-localization.md)).

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
| `?quality=low\|medium\|high\|ultra\|auto` | The quality preset for this visit, never stored ([ADR 0023](adr/0023-quality-presets.md)) |
| `?frames=N` | Stop after N frames and print a `TRINO_FRAME:` line (smoke runs) |
| `?selftest` | The browser self-test (below) |
| `?log` | A log panel under the canvas |

What a browser changes:

- **Audio** starts suspended (every browser's autoplay rule, Safari's strictest). The
  first click, key or touch resumes it; a banner says so until then. A browser without
  Web Audio runs silent, with voices and stingers still keeping time. Sounds are the PC's
  `TSN2` (Ogg Vorbis by default) decoded by lewton in the wasm — the same samples as the
  PC. Effects decode once into `AudioBuffer`s; music keeps only its Ogg bytes and plays
  as 8192-sample chunks scheduled up to 1 s ahead, seamless at loops, so a stream holds
  about 1 s of decoded audio. Each bus is a gain node into a master gain, driven per
  frame by `BusMixer`; the player's volumes persist in `localStorage`
  (`trino-audio:<save>`). Not an AudioWorklet (that needs a second wasm instance and a
  `SharedArrayBuffer`): a hidden tab runs dry after 1 s, and the stats count it.
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
- **Quality:** the PC's presets. A phone's or a tablet's browser starts at the project's
  Low; a desktop's at the project's default — `auto` calibrates over the first animation
  frames (one presented bench frame each, timed by their intervals: a frame that holds
  60 fps comes one refresh after the last), at most 2 s. The choice persists in
  `localStorage` (`trino-quality:<game>`). A frame cap skips animation frames; dynamic
  resolution follows the frame intervals. The self-test and `?frames=` runs use High.
- **Saves** go to `localStorage` (`trino-save:<game>`, base64, at most
  `Caps::WEB.save_bytes` = 1 MiB). A private window without storage reports
  `save_capacity() == 0`.
- **Assets** come in one pack per game (`assets/trino.pak`, `assets/solid-eleven.pak`),
  fetched at boot. Textures are KTX2 (Basis Universal), transcoded at load to BC on
  desktops, ETC2 or ASTC on phones, RGBA8 elsewhere; sounds are `TSN2` (Vorbis by default)
  and the project's mix rides along (pack TPAK v3: v3 names asset groups, whose bundles
  sit beside the pack as `trino.<group>.tbnd` and download the first time the game loads
  the group, ADR 0022; v1 and v2 packs still load).
- **WebGL2's limits:** no MSAA, and wgpu's GL backend can't copy or `textureLoad` a
  depth texture, so the SSAO and the water read the depth through a nearest sampler and
  the water's depth copy is drawn (`renderer/depth_blit.rs`). The picture is the same:
  `tests/golden/web/` holds every browser and the PC to it.
- **Shadows:** the PC's shadow map ([ADR 0025](adr/0025-shadows-per-tier.md)). On WebGL2
  the composite reads the scene's depth through the same nearest sampler as the SSAO.
- **No threads:** the parallel CPU T&L runs on one worker (wasm without
  `SharedArrayBuffer`); the GPU T&L path takes most draws anyway.

`test web` serves the build and drives `?selftest` in each browser: the engine checks
(blobs, a save to `localStorage`, every renderer step), audio and idle input, the KTX2
transcode (both Basis modes decode exactly as the bake's native transcoder did), the
quality calibration as a page runs it (done in 3 s, the renderer restored), the
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

UI screens ([ADR 0024](adr/0024-ui-system.md)) keep clear of a phone's notch and home
bar: the page sets `viewport-fit=cover`, and the backend reads CSS
`env(safe-area-inset-*)` on every resize (an orientation change is one) and hands the
renderer the part inside the canvas, in device pixels; the renderer keeps what the
letterbox's bars do not already cover. Touch taps controls like the mouse clicks them.

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
