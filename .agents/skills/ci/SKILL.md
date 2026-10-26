---
name: ci
description: Understand, change, debug or locally replicate Trino's GitHub Actions CI (.github/workflows/ci.yml, release.yml) — the lint/desktop/hot-reload/n64/n3ds/n3ds-emulator jobs, cached Docker images, emulator timeouts, headless displays. Use for "CI is red", "add a CI step", "why does this pass locally but not in CI", or before pushing CI changes.
---

# ci

## Jobs (`.github/workflows/ci.yml`)

| Job | Runs on | What it proves |
|---|---|---|
| `lint` | ubuntu | fmt, clippy `-D warnings`, the dependency graph; clippy of `trino-runner-pc` with `steam,crash-upload,gamepad` and its `store` tests with `steam` (links `libsteam_api` from cargo's path); the whole workspace's clippy with `trino-core/trace` (the zones are compiled out by default, #62), the core's global-ring test with `trace`, and `trino-app-pc` with `tracy` (Tracy's C++ client); `services check`, `deck check` |
| `package` | Linux, Windows, macOS | scaffolds the football template with `[steam] app_id = 480`, then `package pc --steam --itch --smoke --json`: the fat-LTO build, the OS's package, run outside the repo for 30 frames and crashed on purpose (the report must exist); Windows reads the `.exe`'s version info back (PowerShell); uploads the AppImage/zip/dmg and the smoke frame (#61) |
| `desktop` | Linux, Windows, macOS | `cargo test --workspace`, with GPU goldens required on Linux. Linux also runs `cargo test -p trino-platform-pc` with `TRINO_PC_TNL=cpu` (the PC's old CPU T&L path, #51) and headless smokes: game, a traced game (`trace_zones` test with `trino-core/trace`, then a 60-frame `TRINO_TRACE` smoke collected and reported by `xtask trace`, #62), the game at each quality preset + `auto` (`TRINO_SIM=native`, then `quality check` and `quality bench --json`, #55), scene player, exported scene, editor, editor UI tests (`--ignored --test-threads=1`), Play here, standalone project |
| `hot-reload` | ubuntu | the dylib swap end to end |
| `n64` | ubuntu-24.04 | builds the ROM in `trino-n64`, then runs `test n64` in the `trino-ares` image (self-test, the pixel-exact golden, and the perf gates: T&L counters equal `tests/perf/castle64_work.toml`, emulated times under `tests/perf/castle64_n64.toml`). On failure it uploads `tests/golden/n64/actual/` |
| `n3ds` | `devkitpro/devkitarm` container | builds the `.3dsx`, then `build 3ds --selftest`, and uploads the result as an artifact, plus the self-test's ELF, RomFS and SMDH (`trino-3ds-cia-inputs`: makerom and ctrtool need glibc 2.38, the Debian image has 2.36) |
| `n3ds-emulator` | ubuntu-24.04 | downloads that artifact, runs `emulators setup azahar`, then `xvfb-run … test 3ds --no-build` (self-test + the castle64 hub golden within a tolerance; on failure uploads `tests/golden/3ds/actual/` as `n3ds-golden-actual`); then `package 3ds --cia --no-build` from those inputs (tools fetched pinned) and the same harness on the `.cia`'s NCCH (`TRINO_3DS_APP=…/trino.cxi`); uploads `trino-3ds-cia` |
| `web` | ubuntu-24.04 | wasm32 clippy of the Web crates, `build web --release` once, then `test web --no-build`: the browser self-test in Chromium (WebGL2, and WebGPU on SwiftShader's Vulkan), Firefox and WebKit through Playwright, each against `tests/golden/web/` (and a save surviving a reload, audio unlocking on a click, keys reaching the game); then a scaffolded castle64 project built for the Web and smoke-run in Chromium. Uploads `target/web-test/` (every run's console log and frames) and `tests/golden/web/actual/` |

The `n64` job also scaffolds the football template (`ci-football`) and runs
`test n64 --project` on it, using the template's own goldens and frame gate. The `n3ds`
job builds that project's self-test app (artifact `football-3ds-selftest`), and
`n3ds-emulator` boots it with `test 3ds --project … --no-build`. The project needs the
same scaffold in the booting job: the goldens and the `trino.toml` come from it. Each job runs
`rm -rf target/test-projects/ci-football` before `xtask new`: the cargo cache restores
`target/` with the last green run's project in it, and `new` refuses a non-empty folder
(the second CI run failed that way).

`robustness.yml` (issue #68, the `robustness` skill) runs nightly at 03:17 UTC, by hand
(`workflow_dispatch`, `fuzz_seconds`) and on a PR that touches it, `fuzz/` or
`crates/fuzz/` (45 s per target there, 600 nightly): `miri` (trino-core's unit tests,
`-Zmiri-strict-provenance -Zmiri-ignore-leaks`), `asan` (`-Zsanitizer=address
-Zbuild-std` over trino-core and the asset pipeline) and six `fuzz` shards
(target `i` runs in shard `i % 6`, `cargo xtask fuzz run`, the corpus cached per shard
with `actions/cache`, crash inputs uploaded as `fuzz-artifacts-<shard>`). `schedule` only
fires on the default branch: the PR trigger is how a change to the workflow is proven
before it lands. The per-PR half is `cargo test -p trino-fuzz` inside `cargo test
--workspace` (seeds, regressions, mutations).

`release.yml` tags new workspace versions on main and publishes editor binaries, demo
ROMs and SHA256SUMS. A version with a pre-release part (`0.2.0-dev.1`) publishes a GitHub
pre-release; `docs/releases/v<version>.md`, when present, is the release body (GitHub's
generated changelog is appended). Merging a version bump into main *is* the release:
write the notes file in the same PR.

## Why it is shaped like this

- **The Web is one job, not a browser matrix** (issue #47): the wasm build and the cargo
  cache are shared, the page is served once, and one Playwright process opens each
  browser in turn. A browser that gives no WebGPU adapter on the runner reports that run
  skipped; WebGL2 covers it. Replicate it with `cargo xtask build web --release && cargo
  xtask test web --no-build` (Windows and macOS run it too: there WebGPU and Firefox use
  the real GPU).

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

- **An N64 self-test failure that passes locally** (PR #92: the RSP water ripple
  oracle failed in CI on every rerun, never locally): the ROM's only absolute paths
  are the toolchain's `library/` sources in panic messages, so CI's
  `/home/runner/.rustup/toolchains/nightly-x86_64-unknown-linux-gnu` shifts the data
  layout against yours. Build CI's layout locally with the same nightly and a remap:

  ```sh
  rustup toolchain install nightly-YYYY-MM-DD --profile minimal -c rust-src  # the job's `rustc -V` date + 1
  L=$HOME/.rustup/toolchains/nightly-YYYY-MM-DD-x86_64-unknown-linux-gnu
  RUSTFLAGS="--remap-path-prefix=$L=/home/runner/.rustup/toolchains/nightly-x86_64-unknown-linux-gnu" \
    TRINO_NIGHTLY=nightly-YYYY-MM-DD TRINO_ARES=docker cargo xtask test n64 --expansion-pak on
  ```

  `strings target/n64/trino.elf | grep /home/runner` confirms the remap. A failure that
  follows the layout is a real bug (there: a freed buffer the RSP still read).
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
- **A test binary that exits with an odd code on Windows** (`exit code: 2173`, no
  test named): a panic outside a test's own thread, usually in a drop. Find it with a
  throwaway draft PR whose `ci.yml` is cut to one job looping the binary with
  `--nocapture` and `RUST_BACKTRACE=1` (PR #113 did: each test alone passed, all five
  together failed 3 runs in 6). There it was wgpu-core's queue drop timing out on WARP.
- **The editor launcher test's "the demo's assets baked"** was a rebake from the
  previous test's watcher, on relative paths, landing in the new project after the
  cwd moved (a corrupt `.3ds.sprite`, so nothing loaded). The assertion now prints the
  editor log; read it first.

## Status to keep current

The `package` job (issue #61) was first green on all three OSes on 2026-10-23 (run 39951765032,
PR #98): about 17 min on Ubuntu, 8 on Windows, 7 on macOS, most of it the football
template's fat-LTO build. The `.cia` step and its `.cxi` boot in `n3ds-emulator` were
green in the same run. A step's `ls | head` fails under the runner's `pipefail`
("Broken pipe", exit 2): list without truncating.

The emulator jobs (`n64` ares step, `n3ds-emulator`) were first verified in local
replica containers on 2026-10-12. The perf gates and the 3DS golden (2026-10-13)
were verified the same way: `TRINO_ARES=docker` gives µs identical to native ares,
and in the `trino-ci3ds-boot` replica (ubuntu 24.04, Xvfb, llvmpipe, `--cpus 4`) the 3DS
golden matched with mean diff 0.00 (the Azahar boot took ~90 s). Until a green GitHub
run is recorded here, treat their first runs as BACKLOG P1 #8. Check image build time, cache hits and run time
against the timeout.
