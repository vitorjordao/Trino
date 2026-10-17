---
name: run-emulator
description: Run or test Trino builds in emulators — ares (N64) and Azahar (3DS) — launch the game, run the automated test harness (self-tests, the emulator golden frames, the N64 perf gates), provision headless emulators, or iterate with the watch loop. Use for "run on N64/3DS / test in the emulator / emulator loop / emulator CI" requests.
---

# run-emulator

```
cargo xtask run n64            # build + open the game in ares
cargo xtask test n64           # build test ROM, self-test + golden frame + perf gates (300s timeout)
cargo xtask test n64 --bless   # same, rewriting tests/golden/n64/showcase.png + tests/perf/castle64_n64.toml
cargo xtask watch n64          # rebuild ROM + relaunch ares on every save

cargo xtask run 3ds            # build + open the game in Azahar
cargo xtask test 3ds           # build test app + assert TRINO_TEST_PASS + golden frame (300s timeout)
cargo xtask test 3ds --bless   # same, rewriting tests/golden/3ds/castle64_hub.png + castle64_hub_bottom.png + showcase_bottom.png
cargo xtask build 3ds --selftest && cargo xtask test 3ds --no-build   # build, boot later
cargo xtask watch 3ds          # rebuild .3dsx + relaunch Azahar on every save
cargo xtask run|test|watch 3ds --accurate-timing   # Azahar's "Simulate 3DS GPU timings" on for this session
TRINO_AZAHAR_GPU_TIMING=1 cargo xtask test 3ds     # the same, from the environment
```

Real consoles instead of emulators (`run n64 --device sc64|everdrive`,
`run 3ds --device network --ip <IP>`): the `hardware` skill and `docs/HARDWARE.md`.

## Emulator locations

- **ares**, in order of preference:
  - `$TRINO_ARES`: a path to the binary, or `docker` for the headless image;
  - else `ares-v148/` in the repo root (gitignored).
- `cargo xtask emulators setup ares` builds the `trino-ares` image from
  `docker/ares/Dockerfile` (ares v148 n64 core, Xvfb, Mesa lavapipe). Then:
  ```
  TRINO_ARES=docker cargo xtask test n64
  ```
  This is what CI runs. The golden frame matches on lavapipe too, because
  parallel-RDP is bit-exact.
- **Azahar**, in order of preference:
  - `$TRINO_AZAHAR`;
  - `target/emulators/azahar/` (from `cargo xtask emulators setup azahar`, Linux
    x86_64: the pinned 2126.2 AppImage, SHA-256 checked and extracted);
  - the default install dir;
  - PATH.
- Headless Azahar: run the test under
  `xvfb-run -a -s "-screen 0 1280x720x24"`. Its runtime libraries are listed in
  `.github/workflows/ci.yml` (job `n3ds-emulator`).

## Test protocol (same on both consoles)

`test <console>` bakes a `test_mode` marker into the filesystem (DFS/RomFS). The app
detects it and runs `run_self_test` (`apps/<console>/src/…`):

1. `castle64::selftest::engine_checks`, one `TRINO_STEP:<name>` per engine service:
   text, blob, save, textured_3d, billboards, **occlusion**, smooth skin, portals.
   `occlusion` (boxes standing on a big floor, pixels read back) is the check that
   caught the 3DS drawing floors over characters — it fails on any backend that
   resolves depth per triangle instead of per pixel.
2. Audio checks.
3. The showcase demo and the baked scene.
4. On N64, the golden frame (showcase). The `water` step runs with the engine
   steps; the N64 then measures the `showcase-river` perf scene and dumps its frame
   as the third golden, `tests/golden/n64/showcase_river.png`. The 3DS dumps the
   same scene after the showcase panel as its fourth, `tests/golden/3ds/showcase_river.png`.
5. castle64 physics and a bot that plays a full stage.
6. N64: the perf scenes (`TRINO_PERF_SCENE:` / `TRINO_PERF_MODEL:` lines, gated —
   the `n64-performance` skill). 3DS: the golden frames (castle64 hub perf scene:
   the top screen, then the same frame's touch screen, `castle64_hub_bottom.png`).
7. 3DS: the `showcase panel` step (`showcase::panel::check_panel`, the same code
   `xtask/tests/showcase_panel.rs` runs on the PC's 3DS profile): the showcase's touch
   panel read back (button fills by state 2 px inside every corner, background 2 px
   outside every edge, no panel on the top screen), a top-screen click ignored, two
   FOG taps through `update`. Its last frame is the third golden,
   `showcase_bottom.png`. It runs last so its toggles cannot leak renderer state
   (the texture filter persists) into castle64's goldens.

The `dual screen` step probes both 3DS screens through `read_screen_pixel(Screen, ..)`
(`trino_debug_pixel(x, y, bottom)` in the shim), drawing the probed frame three
times because both LCDs are double-buffered. ares logs `dual screen single screen
(collapsed)`: `on_screen(Bottom)` must say no there.

It prints `TRINO_TEST_PASS` or `TRINO_TEST_FAIL:<reason>` on the debug channel. The
harness matches the strings, kills the emulator and sets the exit code. New console
tests follow the same pattern: add a step to `castle64::selftest` so the PC suite
(`xtask/tests/engine_checks.rs`) runs it too.

- **N64 channel: ISViewer.** ares echoes it to stdout and the harness reads the pipes.
  - **Golden frame:** the ROM renders the showcase's first frame and dumps the
    framebuffer as `TRINO_FBR:` hex rows plus `TRINO_FB_END:<w>x<h>:<fnv>`. xtask
    rebuilds the PNG and compares it pixel-exactly with
    `tests/golden/n64/showcase.png`. A mismatch is written to
    `tests/golden/n64/actual/`. Review the diff before re-blessing.
  - **Perf gates:** per-scene, per-stage, per-model cost in **emulated N64 time**
    (ares emulates the count register). The harness requires the T&L work
    counters to equal the PC baseline (`tests/perf/castle64_work.toml`) and the
    times to stay under `tests/perf/castle64_n64.toml`, then prints a table with
    the cost-model estimate. Workflow, layout caveats, history: the
    `n64-performance` skill. `bench_tnl` is host CPU only.
- **3DS channel: `svcOutputDebugString`.** Azahar logs it to `azahar_log.txt`: under
  `%APPDATA%/Azahar` on Windows, `~/.local/share/azahar-emu` on Linux.
  - The harness seeds a fresh `qt-config.ini`, or patches an existing one, with
    `log_filter=*:Info Debug.Emulated:Trace` and `instant_debug_log=true`.
  - Without the second setting, the file log only flushes on Error entries, and
    the magic strings can sit in the buffer.
  - The harness re-reads the log every 500 ms and only consumes **whole lines**
    (Azahar may be mid-write).
  - **Golden frame:** the app renders the castle64 hub perf scene (fresh game) and
    `N3dsRenderer::dump_framebuffer` prints the top screen in the N64's
    `TRINO_FBR` format (RGBA5551, so `fbgolden::FbCapture` rebuilds it). Compared
    with `tests/golden/3ds/castle64_hub.png` **within a tolerance**
    (`fbgolden::check_close`: mean ≤ 3, ≤ 2% far pixels) because Azahar's GPU output
    depends on the host OpenGL (desktop GPU vs CI's llvmpipe — unverified in CI so
    far). Locally it reproduces exactly (mean 0.00). `xtask/tests/n3ds_parity.rs`
    holds the PC's N3ds look to the same image (mean 0.60, 1.0% far when written).
  - Visual debugging without a window: `renderer.read_pixel` in a TEMP step and
    `runtime::log` the values (that is how the light leak below was found).

## Notes

- **Azahar GPU timing (`--accurate-timing`, `docs/HARDWARE.md` › Before the 3DS):**
  - **What it is.** Off by default since Azahar #2774. With it off (per issue #43)
    the emulated GPU runs at host speed, decoupled from the PICA200's limits, so a
    GPU-bound frame can look cheap. The flag forces `[Renderer]`
    `simulate_3ds_gpu_timings=true` *and* `simulate_3ds_gpu_timings\default=false` in
    `qt-config.ini`. With `\default=true` Azahar ignores the value.
  - **The key name.** Issue #43's `simulate_gpu_timing` is not a key Azahar reads. Get a
    name from the binary (`strings usr/bin/azahar | grep -i simulate`) and from the
    boot log's `Renderer_<Name>:` lines.
  - **No flag, no change.** The user's setting stands, and CI runs at the default
    (off).
  - **Restore after the emulator exits.** Azahar writes qt-config.ini while it runs
    (`Paths\recentFiles` changes on every boot), and maybe again on a normal close
    (not checked). So xtask restores after it exits, and only our two lines (`with_gpu_timing`, line-based, keeps CRLF).
    `test` kills and waits, `run` blocks until Azahar closes, `watch` restores when
    the loop ends.
  - **Interrupted sessions.** `qt-config.ini.trino-gpu-timing` holds the user's lines
    until then. The next 3DS emulator command restores from it, which covers Ctrl+C.
  - **Proof it applied.** `test` echoes the boot log's
    `Renderer_Simulate3DSGPUTimings:` and fails if it says false while the flag is set.
  - **Cost.** On 2026-10-16 the self-test took 7.7–8.2 s from boot to pass with it
    on or off (three runs, noise-level), and the goldens were unchanged.
  - **Scripting it.** SIGTERM doesn't close Azahar (use SIGKILL, or close the window).
    Never `pkill -f AppRun.wrapped`: the pattern matches the shell running it. Use
    `pgrep -x AppRun.wrapped`.
  - **Mixing commands.** `run 3ds` rebuilds `target/3ds/trino.3dsx` as the game, so a
    following `test 3ds --no-build` boots castle64 and "stalls before the first step".
- **Timeout length:** 300 s by default; `TRINO_EMU_TIMEOUT=<secs>` overrides it (CI uses
  900). On 4 CPUs, ares/lavapipe takes ~95 s and Azahar ~15 s.
- **A timeout** usually means one of these:
  - the app crashed before the test: look for `TRINO_PANIC:` in the harness output;
  - a modal dialog blocked the emulator. A read-only ares save path pops one; the
    Docker mode passes `Paths/Saves=/tmp/`.
  - In a container, screenshot the Xvfb display with ImageMagick's
    `import -window root`.
- **Renderer state carries over between games in one boot.** The self-test runs
  the showcase before castle64 on the same renderer; castle64 never set a light and
  rendered with the showcase's colored, fogged one (found by the 3DS parity test:
  24.5 mean diff → 0.6 after castle64 declared `set_light` every frame). A game
  must set camera and light itself.
- **N64 stack (64 KB, libdragon's main stack, nothing stops an overflow):**
  `TRINO_STACK:<bytes>` lines report the high-water mark (a painted pattern,
  `trino_stack_used`), and `check_stack` in `apps/n64` **fails the self-test above
  60 KB**. Until 2026-10-13 the baked-scene step overflowed it (main +
  `run_self_test` + `check_baked` + `ShowcaseGame::from_baked`, ~22 KB values built
  by value) and the ROM hung at random points, timing-dependent because
  interrupts push onto the same stack. Fixed by running each showcase step in its own
  `#[inline(never)]` function (peak 47.6 KB then; 57 224 bytes on 2026-10-16 main and
  58 040 with the showcase's touch panel, against the 61 440 gate: ~3.4 KB left). **`Box::new(big())` does not help**:
  LLVM still builds the temporary in the caller's frame. Find big frames with
  `mips64-elf-objdump -d -C target/n64/trino.elf` and grep `addiu sp,sp,-<N>`
  (`run_self_test` was 21 KB, `tessellate` is 10 KB).
- **The ares output is buffered:** after a hang, the last line printed is not where
  the ROM stopped. Bracket with `TRINO_STACK`/`TRINO_STEP` lines and compare runs.
- **No in-place hot reload on consoles:** `watch` kills and relaunches, watching
  `examples/`, `assets/` and the console's own crates.
- **CI** (details, local replicas and red-run debugging: the `ci` skill):
  - the `n64` job boots the test ROM in the cached `trino-ares` image;
  - `n3ds` builds `--selftest` and uploads it;
  - `n3ds-emulator` (ubuntu-24.04) boots it, because the devkitARM image's glibc is
    too old for the AppImage.

## A project's self-test: the football template (2026-10-16)

```sh
cargo xtask new se-demo --standalone target/test-projects/se-demo --template football
TRINO_ARES=docker cargo xtask test n64 --project target/test-projects/se-demo
cargo xtask build 3ds --project target/test-projects/se-demo --selftest
cargo xtask test 3ds --project target/test-projects/se-demo --no-build
```

- **Output:** the hosts print `TRINO_FRAME:match …`, `TRINO_SE_HASH:<hex>` (the
  sim's state after the script, which `check()` holds to the PC's) and
  `TRINO_SE_RENDER_CPU`, then dump the `match` golden (and `match_bottom` on the
  3DS).
- **Measured results:** ares 30.9 ms avg over four code layouts (first iteration:
  26.6 ms, with lighter bodies) against a 33.3 ms gate; Azahar 16.8 ms.
- **No local devkitPro?** Build the project's `.3dsx` in a devkitPro image with Rust
  nightly (the `n3ds` CI job's container plus rustup), then boot it on the host with
  `test 3ds --project … --no-build`.
- **The script must press a kick at each kick-off.** A side the human controls waits
  up to 8 s for one, and the first script idled three seconds of its 300 frames that
  way.
- **Keep the build cache between re-scaffolds:** move the project's `target/` aside,
  scaffold again, and move it back. A cold N64 build of the project takes minutes.
