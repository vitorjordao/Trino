---
name: run-emulator
description: Run or test Trino builds in emulators — ares (N64) and Azahar (3DS) — launch the game, run the automated test harness, or iterate with the watch loop. Use for "run on N64/3DS / test in the emulator / emulator loop" requests.
---

# run-emulator

```
cargo xtask run n64        # build + open the game in ares
cargo xtask test n64       # build test ROM + assert TRINO_TEST_PASS (60s timeout)
cargo xtask watch n64      # rebuild ROM + relaunch ares on every save

cargo xtask run 3ds        # build + open the game in Azahar
cargo xtask test 3ds       # build test app + assert TRINO_TEST_PASS (120s timeout)
cargo xtask watch 3ds      # rebuild .3dsx + relaunch Azahar on every save
```

Emulator locations:
- **ares** at `ares-v148/` in the repo root (gitignored; ask the user if missing).
- **Azahar** auto-detected (`$TRINO_AZAHAR`, the default install dir, or PATH).

## Test protocol (same on both consoles)

`test <console>` bakes a `test_mode` marker into the filesystem (DFS/RomFS).
The app detects it, runs a deterministic self-check (`apps/<console>/src/…::run_self_test`)
and prints `TRINO_TEST_PASS` / `TRINO_TEST_FAIL:<reason>` over the debug
channel. The harness matches the magic strings, kills the emulator and sets
the exit code. New console tests follow the same pattern: print the magic
strings via `runtime::log`.

- N64 channel: ISViewer — ares echoes it to stdout; the harness reads pipes.
- 3DS channel: `svcOutputDebugString` — Azahar logs it to
  `%APPDATA%/Azahar/log/azahar_log.txt`; the harness tails that file and
  idempotently widens Azahar's `log_filter` (`Debug.Emulated:Trace` in
  qt-config.ini) so the strings are not filtered out.

## Notes

- A timeout usually means the app crashed before the test — look for
  `TRINO_PANIC:` in the harness output.
- `watch` has no in-place hot reload (consoles!) — the loop is kill +
  relaunch, watching `examples/`, `assets/` and the console's own crates.
- Emulator tests run locally only for now; CI builds the ROM/.3dsx but does
  not boot emulators yet (see the job comments in `.github/workflows/ci.yml`).
