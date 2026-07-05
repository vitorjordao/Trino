---
name: run-emulator
description: Run or test Trino ROMs in the ares emulator (N64) — launch the game, run the ISViewer test harness, or iterate with the watch loop. Use for "run on N64 / test in the emulator / emulator loop" requests.
---

# run-emulator

```
cargo xtask run n64        # build + open the game in ares
cargo xtask test n64       # build test ROM + assert TRINO_TEST_PASS (60s timeout)
cargo xtask watch n64      # rebuild ROM + relaunch ares on every save
```

ares lives at `ares-v148/` in the repo root (gitignored; not fetched by the
build — if missing, ask the user to place it there).

## Test protocol

`test n64` bakes a `test_mode` marker into the DFS. The ROM detects it, runs a
deterministic self-check (`apps/n64/src/lib.rs::run_self_test`) and prints
`TRINO_TEST_PASS` or `TRINO_TEST_FAIL:<reason>` over the ISViewer debug
channel. ares echoes ISViewer to stdout by default; the xtask harness matches
the magic strings, kills the emulator and sets the exit code. New console
tests follow the same pattern: print the magic strings via `runtime::log`.

## Notes

- The harness times out after 60s — a hang usually means the ROM crashed
  before reaching the test (look for `TRINO_PANIC:` in the `[ares]` lines).
- `watch n64` has no in-place hot reload (console!) — the loop is kill +
  relaunch, watching `examples/`, `assets/`, `apps/n64/`, `crates/platform-n64/`.
- Emulator tests run locally only for now; CI builds the ROM but does not run
  ares yet (see the n64 job comment in `.github/workflows/ci.yml`).
