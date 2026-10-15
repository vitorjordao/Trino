---
name: hardware
description: Run Trino games on real consoles — N64 through a SummerCart64 (sc64deployer) or EverDrive 64 (UNFLoader), 3DS over the network (3dslink) — with the game's debug log forwarded to the terminal, and debug a 3DS app with gdb through Luma3DS's GDB stub. Use for "run on real hardware / flashcart / SC64 / EverDrive / 3dslink / deploy to the console / Luma gdb" requests, or when touching xtask/src/hardware.rs or the shims' log channels.
---

# hardware

```
cargo xtask run n64 --device sc64                  # sc64deployer upload <rom>; sc64deployer debug
cargo xtask run n64 --device everdrive             # UNFLoader -b -f 3 -r <rom> -d
cargo xtask run 3ds --device network --ip <IP>     # 3dslink -a <IP> -s <app.3dsx>
cargo xtask run 3ds --device network --ip <IP> --gdb [--gdb-port 4000]   # + target/3ds/<name>.gdb
cargo xtask run --target n64 --device sc64         # the same; --target is moved to the positional slot
  ... --no-build                                   # deploy the ROM/.3dsx already in target/
  ... --selftest                                   # deploy the self-test build (TRINO_STEP/PERF/TEST lines)
  ... --project <dir>                              # an external project's ROM/app (ADR 0008)
```

The user guide is `docs/HARDWARE.md`. It covers prerequisites, device detection,
gdb and troubleshooting, and links the upstream docs for every flag. Code:
`xtask/src/hardware.rs`, wired into `main.rs`'s `run n64` / `run 3ds` arms. Without
`--device`, `n64::run` / `n3ds::run` run unchanged.

## How it works

- **Parsing** (`main.rs`, before project selection):
  - `normalize_target` turns `--target n64|3ds|pc` back into the positional
    platform. A cargo triple after `run pc` passes through.
  - `take_device_args` removes `--device/--ip/--gdb/--gdb-port`.
  - `check_invocation` rejects them anywhere but `run n64` / `run 3ds`, and also
    rejects a device of the other console. Without it, `test n64 --device sc64`
    would silently boot ares.
- **Tools** (`find_tool`):
  1. `$TRINO_SC64DEPLOYER` / `$TRINO_UNFLOADER` / `$TRINO_3DSLINK`. When set, the
     path must exist; there is no silent fallback.
  2. Else PATH.
  3. Else, for 3dslink, `$DEVKITPRO/tools/bin`, or the default devkitPro dirs when
     `DEVKITPRO` is unset.

  Missing tools fail with install links. The tool is looked up **before** the
  build, so nothing is built for a loader that isn't there.
- **Forwarding:** the loader inherits stdin/stdout/stderr. **Never pipe it:**
  - UNFLoader `vprintf`s and 3dslink `fwrite`s without flushing, so piped output
    arrives in 4 KB blocks;
  - `sc64deployer debug` reads stdin until EOF.
- **N64 log channel:** the shim's `trino_init` calls `debug_init_isviewer()` and
  `debug_init_usblog()`.
  - `debugf` is `fprintf(stderr)` to every channel. It is compiled out by `NDEBUG`,
    which xtask never defines.
  - libdragon's `debug.h` (in the `trino-n64` image, `/opt/libdragon/mips64-elf/include`)
    lists 64drive/EverDrive64/SC64 for USB.
  - So no ROM change was needed. Don't add `--isv` to sc64deployer: lines would
    print twice.
- **3DS log channel:**
  - libctru's `__system_initArgv` sets `__3dslink_host` when the last argument is
    3dslink's `XXXXXXXX_3DSLINK_` marker.
  - The shim's `trino_link_init` (first thing in `trino_init`) then runs
    `memalign(0x1000, 0x100000)` + `socInit` + `link3dsStdio()`, and registers
    `close` + `socExit` with `atexit`.
  - `trino_log` and `trino_panic` then also `fwrite` + `fflush` stdout.
  - Framebuffer dumps (`TRINO_FBR`) stay on `svcOutputDebugString` only.
  - Under Azahar `__3dslink_host` is 0. The Azahar golden stayed at mean 0.00.
- **SC64 order:** `upload` never boots the console (`--reboot` needs game support),
  so xtask prints "power on (or reset) the N64 now" before `debug`.

## Tests

`xtask/tests/hardware.rs` runs the real binary against a temp external project with
fake `target/` files (`--no-build`). `#!/bin/sh` stub loaders record `$*` to
`$STUB_LOG` and echo `TRINO_FRAME` lines.
- The deploy tests are `#[cfg(unix)]`. Flag errors and missing tools are tested
  everywhere.
- The test empties `PATH` and points `DEVKITPRO` at a missing dir, so a real
  3dslink on the machine is never run by accident.
- Unit tests in `hardware.rs` pin each command line and the gdb scripts.

## Not verified

Nothing here has run on a physical SummerCart64, EverDrive or 3DS (BACKLOG). Open
questions for the first hardware session:
- Does Luma's break-on-start (port 4003, "Force-debug next application at launch")
  catch a `.3dsx` the Homebrew Launcher starts?
- Is the USB log slow enough to change the self-test's frame times?
