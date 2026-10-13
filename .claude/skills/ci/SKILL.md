---
name: ci
description: Understand, change, debug or locally replicate Trino's GitHub Actions CI (.github/workflows/ci.yml, release.yml) — the lint/desktop/hot-reload/n64/n3ds/n3ds-emulator jobs, cached Docker images, emulator timeouts, headless displays. Use for "CI is red", "add a CI step", "why does this pass locally but not in CI", or before pushing CI changes.
---

# ci

## Jobs (`.github/workflows/ci.yml`)

| Job | Runs on | What it proves |
|---|---|---|
| `lint` | ubuntu | fmt, clippy `-D warnings`, the dependency graph |
| `desktop` | Linux, Windows, macOS | `cargo test --workspace`, with GPU goldens required on Linux. Linux also runs headless smokes: game, scene player, exported scene, editor, editor UI tests (`--ignored --test-threads=1`), Play here, standalone project |
| `hot-reload` | ubuntu | the dylib swap end to end |
| `n64` | ubuntu-24.04 | builds the ROM in `trino-n64`, then runs `test n64` in the `trino-ares` image (self-test, the pixel-exact golden, and the perf gates: T&L counters equal `tests/perf/castle64_work.toml`, emulated times under `tests/perf/castle64_n64.toml`). On failure it uploads `tests/golden/n64/actual/` |
| `n3ds` | `devkitpro/devkitarm` container | builds the `.3dsx`, then `build 3ds --selftest`, and uploads the result as an artifact |
| `n3ds-emulator` | ubuntu-24.04 | downloads that artifact, runs `emulators setup azahar`, then `xvfb-run … test 3ds --no-build` (self-test + the castle64 hub golden within a tolerance; on failure uploads `tests/golden/3ds/actual/` as `n3ds-golden-actual`) |

`release.yml` tags new workspace versions on main and publishes editor binaries, demo
ROMs and SHA256SUMS.

## Why it is shaped like this

- **3DS: build and boot are separate jobs.** The devkitARM image is Debian, and its
  glibc is too old for the Azahar AppImage. Keep `build 3ds --selftest` and
  `test 3ds --no-build` as the seam.
- **Docker images are cached as tarballs**, keyed on the hash of their Dockerfile:
  `trino-n64` (libdragon, ~10 min cold) and `trino-ares` (ares built from source).
  Changing a Dockerfile means one slow cold run.
- **Emulator timeout:** `TRINO_EMU_TIMEOUT=900` in CI (300 s is the local default).
  With `--cpus 4`, ares on lavapipe measured ~95 s and Azahar ~15 s.
- **Display-dependent tests** run under `xvfb-run`. A test that opens a window must
  skip without `DISPLAY`/`WAYLAND_DISPLAY`, unless `TRINO_REQUIRE_GPU=1` says the
  job guarantees one.
- **Azahar's runtime libraries** are listed in the `n3ds-emulator` job. Missing
  `libharfbuzz0b` or `libfribidi0` makes the AppImage exit at once, with no log.

## Replicate a job locally before pushing

A CI change is unverified until the job ran somewhere like CI. Build a throwaway
container that mirrors the job's image and steps. Keep these out of the repo:

```
# n64 emulator step: the real image, the CI env
cargo xtask emulators setup ares
TRINO_ARES=docker TRINO_EMU_TIMEOUT=900 cargo xtask test n64

# 3DS: build in devkitpro/devkitarm, then boot on a plain ubuntu:24.04 with the
# job's apt list, `emulators setup azahar` and `xvfb-run`; add `--cpus 4` to
# approximate a runner.
```

Local replica images from earlier sessions (`trino-ci3ds`, `trino-ci3ds-boot`) are
not in the repo. Rebuild them from the job definitions when needed.

## Debugging a red job

- **The golden mismatch artifact** (`n64-golden-actual`): diff it against
  `tests/golden/n64/showcase.png`. Re-bless only when the change was intended.
- **A perf gate failure** (`n64` job, `perf gate:` table): a `ROM X, PC baseline Y`
  line is a determinism or stale-baseline problem, never noise. A time over its
  ceiling: check whether the CI nightly differs from yours first — code layout
  alone moves a stage ~20% (`n64-performance`), which the stage headroom (35%)
  should absorb; a frame-total breach (20%) is worth a real look.
- **`n3ds-golden-actual`:** Azahar under Xvfb renders with llvmpipe; the 3DS golden
  was blessed on a desktop GPU (mean 0.00 locally). If CI's first run lands just
  over the tolerance, look at the image before widening `check_close`'s limits.
- **An emulator timeout:** look for `TRINO_PANIC:` or the last `TRINO_STEP:`. Then
  suspect a modal dialog. ares with a read-only save path pops one, which is why the
  Docker mode passes `Paths/Saves=/tmp/`. Screenshot the Xvfb display with
  `import -window root shot.png`.
- **A 3DS run with no magic strings:** check `instant_debug_log` in the seeded
  config (`run-emulator`).
- **Paths that only fail on Windows:** `canonicalize` returns `\\?\C:\…`, which
  breaks cargo path dependencies and string comparisons. Use `std::path::absolute`
  plus normalization (see `xtask/src/standalone.rs`).
- **Flaky tests:** look for tests that mutate the cwd or the environment, running
  in parallel (`engine-feature` › Tests).

## Status to keep current

The emulator jobs (`n64` ares step, `n3ds-emulator`) were first verified in local
replica containers on 2026-10-12. The perf gates and the 3DS golden (2026-10-13)
were verified the same way: `TRINO_ARES=docker` gives µs identical to native ares,
and in the `trino-ci3ds-boot` replica (ubuntu 24.04, Xvfb, llvmpipe, `--cpus 4`) the 3DS
golden matched with mean diff 0.00 (the Azahar boot took ~90 s). Until a green GitHub
run is recorded here, treat their first runs as BACKLOG P1 #8. Check image build time, cache hits and run time
against the timeout.
