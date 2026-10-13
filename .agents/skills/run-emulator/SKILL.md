---
name: run-emulator
description: Run or test Trino builds in emulators — ares (N64) and Azahar (3DS) — launch the game, run the automated test harness (self-tests, the N64 golden frame, TRINO_PERF), provision headless emulators, or iterate with the watch loop. Use for "run on N64/3DS / test in the emulator / emulator loop / emulator CI" requests.
---

# run-emulator

```
cargo xtask run n64            # build + open the game in ares
cargo xtask test n64           # build test ROM, self-test + golden frame (300s timeout)
cargo xtask test n64 --bless   # same, rewriting tests/golden/n64/showcase.png
cargo xtask watch n64          # rebuild ROM + relaunch ares on every save

cargo xtask run 3ds            # build + open the game in Azahar
cargo xtask test 3ds           # build test app + assert TRINO_TEST_PASS (300s timeout)
cargo xtask build 3ds --selftest && cargo xtask test 3ds --no-build   # build, boot later
cargo xtask watch 3ds          # rebuild .3dsx + relaunch Azahar on every save
```

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
   text, blob, save, textured_3d, billboards, smooth skin, portals.
2. Audio checks.
3. The showcase demo and the baked scene.
4. On N64 only, the golden frame.
5. castle64 physics and a bot that plays a full stage.

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
  - **`TRINO_PERF`:** castle64 hub frame cost in **emulated N64 time** (ares emulates
    the count register). Quote it for T&L changes; `bench_tnl` is host CPU only.
- **3DS channel: `svcOutputDebugString`.** Azahar logs it to `azahar_log.txt`: under
  `%APPDATA%/Azahar` on Windows, `~/.local/share/azahar-emu` on Linux.
  - The harness seeds a fresh `qt-config.ini`, or patches an existing one, with
    `log_filter=*:Info Debug.Emulated:Trace` and `instant_debug_log=true`.
  - Without the second setting, the file log only flushes on Error entries, and
    the magic strings can sit in the buffer.

## Notes

- **Timeout length:** 300 s by default; `TRINO_EMU_TIMEOUT=<secs>` overrides it (CI uses
  900). On 4 CPUs, ares/lavapipe takes ~95 s and Azahar ~15 s.
- **A timeout** usually means one of these:
  - the app crashed before the test: look for `TRINO_PANIC:` in the harness output;
  - a modal dialog blocked the emulator. A read-only ares save path pops one; the
    Docker mode passes `Paths/Saves=/tmp/`.
  - In a container, screenshot the Xvfb display with ImageMagick's
    `import -window root`.
- **N64 stack:** `TRINO_STACK:<bytes>` lines report the self-test's stack high-water
  mark (64 KB available).
- **No in-place hot reload on consoles:** `watch` kills and relaunches, watching
  `examples/`, `assets/` and the console's own crates.
- **CI:**
  - the `n64` job boots the test ROM in the cached `trino-ares` image;
  - `n3ds` builds `--selftest` and uploads it;
  - `n3ds-emulator` (ubuntu-24.04) boots it, because the devkitARM image's glibc is
    too old for the AppImage.
