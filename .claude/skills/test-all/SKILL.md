---
name: test-all
description: Run Trino's full verification suite (fmt, clippy, workspace tests — dep-graph, skill mirrors, scene codegen drift, goldens) exactly like CI, plus the optional console type-checks and the T&L benchmark. Use before committing, after refactors, or when asked to "run the tests".
---

# test-all

Run the three CI gates in order. Stop at the first failure and fix it before rerunning.

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

`cargo test --workspace` includes guard tests — fix the cause, never weaken the test:

| Test | Fails when | Fix |
|---|---|---|
| `xtask/tests/dep_graph.rs` | core gained a dependency / a game crate depends on more than core + game-api | revert the Cargo.toml change |
| `xtask/tests/agents_sync.rs` | `.claude/skills/` drifted from `.agents/skills/`, or a skill is missing from the AGENTS.md table | edit `.agents/skills/`, run `cargo xtask agents sync`, list the skill in AGENTS.md |
| `xtask/tests/scene_codegen.rs` | `examples/showcase/src/showcase_gen.rs` is stale vs. its scene | `cargo xtask scene export scenes/showcase.scene.ron examples/showcase/src/showcase_gen.rs` |
| `xtask/tests/showcase_render.rs` | runtime-resolved scene and exported code render differently | a codegen/resolve mismatch — fix the code, not the scene |
| `crates/platform-pc/tests/golden.rs` | renderer output changed | intended? `cargo xtask test --bless` and review the PNG diff; a pure refactor/perf change must NOT change goldens |
| `xtask/tests/engine_checks.rs` | a `castle64::selftest` step fails on the PC backend (the same steps run in ares/Azahar) | fix the service; never special-case PC |
| `xtask/tests/n64_parity.rs` | the PC N64 look drifted from the ares golden frame (mean diff / % within two 5-bit steps) | a quantizer/VI/T&L change; compare with `TRINO_PARITY_DUMP=1` (writes `tests/golden/n64/actual/pc_look.png`) |
| `xtask/tests/perf_budget.rs` | castle64's per-scene T&L work (verts, tris, splits, emitted, …) differs from `tests/perf/castle64_work.toml` — heavier content, a level drawing more, a culling or split regression | intended? `TRINO_BLESS=1 cargo test -p xtask --test perf_budget`, review the TOML diff (each number is N64 time), then `cargo xtask test n64 --bless`. The failure prints the hot models by name |
| `xtask/tests/n3ds_parity.rs` | the PC N3ds look drifted from the Azahar golden frame (`tests/golden/3ds/castle64_hub.png`: mean diff < 2.5, < 2.5% far pixels) | a backend difference or **renderer state leaking** (it found castle64 inheriting a fogged light); `TRINO_PARITY_DUMP=1` writes `tests/golden/3ds/actual/pc_look.png` |
| `xtask/tests/feature_matrix.rs` | a `docs/TESTING.md` test reference is stale, or a trait method / TMDL section / asset kind / xtask command has no row | add the row with its test (or `n/a: <reason>` when no host test can exist) |
| `xtask/tests/standalone.rs` | `cargo xtask new --standalone` output no longer builds/tests (windowed smoke with GPU + display) | fix `templates/standalone/` or `trino-runner-pc` |
| `crates/editor/src/ui_tests.rs` (`#[ignore]`: `cargo test -p trino-editor ui_tests -- --ignored --test-threads=1`, own CI step) | an editor panel flow broke (select → palette → duplicate → undo) | fix the UI; hand-painted widgets need `widget_info` |
| `xtask/tests/ui_goldens.rs` | a starter UI screen (`templates/standalone/ui/`) renders differently on the N64, 3DS or PC profile (`tests/golden/pc/ui_*.png`; the diff lands in `tests/golden/pc/actual/`) | a layout, theme or font change? intended: `TRINO_BLESS=1 cargo test -p xtask --test ui_goldens` and review the PNGs |
| `xtask/tests/ui_cli.rs` | a shipped screen fails `ui check`, or a committed `ui export` (Solid Eleven's `pause_gen.rs`) is stale | fix the screen; re-export with `cargo xtask ui export examples/solid-eleven/ui/pause.ui.ron examples/solid-eleven/src/ui/pause_gen.rs` |
| `xtask/tests/access_cli.rs` | a shipped project fails `access check` (palettes per color-vision mode, the theme, caption keys, a screen at 200 %) | fix the content the finding names (`accessibility` skill) |
| `xtask/tests/access_goldens.rs` | the palette swap or the simulation view renders differently (`tests/golden/pc/access_*.png`), or the GPU view drifted from the CPU simulation | intended? `TRINO_BLESS=1 cargo test -p xtask --test access_goldens`, review the PNGs |
| `xtask/tests/web_parity.rs` | the PC renderer drifted from the browser goldens (`tests/golden/web/`) | find what changed the picture; re-bless with `cargo xtask test web --bless` only on purpose |

GPU-dependent tests skip without an adapter unless `TRINO_REQUIRE_GPU=1`.

**The Web (issue #47)** builds for another target, which the workspace commands never
compile: `cargo clippy -p trino-render-wgpu -p trino-platform-web -p trino-app-web --target
wasm32-unknown-unknown -- -D warnings`, then `cargo xtask test web` (`build-web` skill). A
renderer change also runs `cargo test -p trino-render-wgpu webgl2` (the WebGL2 shaders
through naga's GLSL ES backend).

**No `libasound2-dev` on the host** (`alsa-sys` panics: "Package alsa was not found"):
its bindings are bundled, so a scratch `alsa.pc` is enough. Write `Name: alsa`,
`Version: 1.2.0`, `Libs: -L<dir> -lasound` into `<dir>/alsa.pc`, symlink
`<dir>/libasound.so` to `/usr/lib/x86_64-linux-gnu/libasound.so.2`, and export
`PKG_CONFIG_PATH=<dir>` (2026-10-17).

**CI's clippy is the newest stable** (`dtolnay/rust-toolchain@stable`: 1.99.1 on
2026-10-17), and a local stable a few releases behind disagrees both ways: 1.93 flagged
`overly_complex_bool_expr` in Solid Eleven's AI (CI didn't; simplified anyway, same
behaviour), and CI's 1.99 flagged a `manual_checked_div` 1.93 didn't. Read the version in
the CI log (`Run dtolnay/rust-toolchain@stable` step), install it beside your stable
(`rustup toolchain install 1.99.1 --profile minimal -c clippy,rustfmt`) and run
`CARGO_TARGET_DIR=target/clippy-ci cargo +1.99.1 clippy --workspace --all-targets -- -D warnings`
before pushing.

**Feature matrix and coverage.** `docs/TESTING.md` maps every engine feature to
its tests; `xtask/tests/feature_matrix.rs` fails when a listed test no longer
exists, or when a `Renderer`/`Platform`/`Audio` method, a TMDL section, a
manifest asset kind or an xtask command has no row. A new feature adds an
integration test (`crates/<crate>/tests/`, public API, end to end) and a row.
Coverage: `TRINO_REQUIRE_GPU=1 cargo llvm-cov --workspace --summary-only
--ignore-filename-regex '(crates/platform-(n64|3ds)|apps/(n64|3ds))/|crates/editor/src/main\.rs'`
(details and the editor's ignored flows in `docs/TESTING.md`).

## Also worth running

- Content: `cargo xtask scene check` (scenes/prefabs/materials resolve cleanly).
- Console type-check without the toolchains (catches `no_std`/target breakage in
  `crates/platform-{n64,3ds}` and game crates, which are cfg-gated on the host):
  ```
  rustup toolchain install nightly --component rust-src
  cargo +nightly check -Zbuild-std=core,alloc -Zjson-target-spec --target platforms/n64/mips-nintendo64-none.json -p trino-app-n64 --release
  cargo +nightly check -Zbuild-std=core,alloc --target armv6k-nintendo-3ds -p trino-platform-3ds -p castle64 -p showcase --release
  ```
- The PC `gamepad` feature (no default gate compiles it):
  `cargo clippy -p trino-platform-pc --features gamepad --all-targets -- -D warnings`
  and `cargo test -p trino-platform-pc --features gamepad --lib --test input_audio`.
  Without `libudev-dev`: write a scratch `libudev.pc` (`Libs: -L<dir> -ludev`), symlink
  `<dir>/libudev.so` to `/usr/lib/x86_64-linux-gnu/libudev.so.1`, set `PKG_CONFIG_PATH=<dir>`
  (verified 2026-10-16). With a pad connected, `platform_services::rebinding_keys_through_the_platform`
  fails (it asserts no pad).
- T&L cost (what the consoles' CPUs pay): `cargo test --release -p xtask --test bench_tnl -- --ignored --nocapture`.
  Compare against the base commit in a `git worktree` (never `git stash` a dirty tree for this).
- Full console suites: `cargo xtask test n64|3ds` (emulators required; see `run-emulator`).
  N64: golden frame (`tests/golden/n64/`) and the perf gates (work counters = PC,
  times ≤ `tests/perf/castle64_n64.toml`; `n64-performance` skill). 3DS: the
  castle64 hub golden frame within a tolerance (`tests/golden/3ds/`).
- `cargo xtask model inspect <glb>` prints an **N64 cost estimate** per draw
  (`trino_core::perf::N64_COST`) and its share of a 30 fps frame. PC strict mode
  splits a frame's estimated N64 3D time like the console (draws `rsp_check` accepts:
  90 µs of CPU each plus their triangles on the RSP) and panics when the CPU or the RSP
  side passes `Caps::max_3d_us_per_frame` (50 ms);
  `strict_mode_rejects_3d_over_the_n64_time_ceiling` covers it.
- Blender toolkit, on machines with Blender: `cargo xtask blender smoke`.
- Quality presets (#55), as CI's desktop job: `for q in low medium high ultra; do
  TRINO_SIM=native TRINO_QUALITY=$q TRINO_SMOKE_FRAMES=60 xvfb-run -a cargo run -p
  trino-app-pc | grep "TRINO_FRAME.*quality=$q "; done`, then `cargo xtask quality
  check` and `xvfb-run -a cargo xtask quality bench --json`. `TRINO_SIM=native` is
  required: `apps/pc` defaults to the N64 profile, which ignores quality. Evidence
  (castle64 at each preset, the Graphics page, a dynres CSV/chart, the 30/60/120 fps
  limiter): `cargo test -p xtask --test quality_shots -- --ignored` →
  `target/evidence/quality/`. On llvmpipe dynres pins its minimum scale (the frame is
  mostly fixed cost); measure it on a real GPU (Windows `cargo.exe`, as `pc_tnl_bench`).
- Skinning cost: `bench_tnl` includes `skinning_rigid_vs_smooth`.
- The PC native profile's old CPU path (#51): `TRINO_PC_TNL=cpu TRINO_REQUIRE_GPU=1
  cargo test -p trino-platform-pc` must pass too (CI runs it; `gpu_tnl.rs` turns the GPU
  path on itself). The PC frame time, CPU vs GPU T&L: `cargo test --release -p xtask --test pc_tnl_bench --
  --ignored --nocapture` (`TRINO_POWER=low|high` picks the iGPU or the discrete GPU;
  on WSL run it with the Windows `cargo.exe` on a copy of the tree under `C:\`: WSL has
  only llvmpipe).

On Linux the PC crates need `pkg-config` and ALSA headers (`apt install pkg-config libasound2-dev`).

## Solid Eleven

```sh
cargo test -p solid-eleven --release                       # rules, ball, league, e2e, body fit, balance, scenarios
cargo test -p xtask --test solid_eleven_budget --test solid_eleven_layers --test solid_eleven_render
TRINO_BLESS=1 cargo test -p xtask --test solid_eleven_budget   # re-pin the frame cost (review the TOML)
TRINO_BLESS=1 cargo test -p xtask --test solid_eleven_render   # re-bless the PC goldens (view the PNGs)
cargo test --release -p xtask --test solid_eleven_menus   # menu crawler (every state and club, EN/PT, 4 sizes, 100/200 % text, text in its box) + random play
cargo test --release -p xtask --test solid_eleven_shots -- --ignored --nocapture   # screenshots + pc_frame_time
cargo test --release -p xtask --test solid_eleven_poses -- --ignored               # pose sheets, target/solid_eleven_poses
cargo test -p solid-eleven --release tuning_table -- --ignored --nocapture        # the balance table, big sample
```

- **Body fit** (`render/fit.rs`): every clip at 49 samples, mirrored, and the game's
  real crossfades, on the PC body and the console body, must keep parts attached, plugs
  hidden, no holes, nothing in the turf. A failure names clip, time and part; look at the
  pose sheet before changing a tolerance. Most failures were real (floating head, arms
  out of sleeves, bodies through the turf in slides).
- **Balance** (`sim/balance.rs`, parallel threads): round robins over four seeds. When
  it fails, run `tuning_table` and `which_lever` before retuning; change one lever at a
  time and rerun everything (the pass completion check caught forwards pushing up the
  whole pitch).
- **Scenarios** (`game/scenarios.rs`): one test per rule staged in a real match, plus
  seven match styles with frames fit-checked and the gait flicker counted.

The console half is the football template's self-test (`new-project` › Overlays).
The sim hash must match the PC's (`selftest::EXPECTED_HASH`).

## GPU tests on RADV: crashes and frames from parallel devices

On AMD's RADV (this repo's dev machine) a test binary that created several `PcRenderer`s
from parallel test threads segfaulted inside `libvulkan.so.1`, in
`set_debug_utils_object_name` while another thread built its device (GitHub #94:
`engine_checks` 9 runs in 12 on 2026-10-19; `render_e2e` 1/40 on 2026-10-15). The debug
names were the trigger: `WGPU_DEBUG=1` brings the crash back (4 in 8), and without them
it is 0 in 20 and 0 in 5 loops of every `trino-platform-pc` binary.

- `PcRenderer::new_headless` builds its instance with `renderer::headless_flags()` (the
  build's flags minus `DEBUG`, then the `WGPU_*` variables). A test that makes its own
  `wgpu::Instance` (`render_e2e`'s shared-device check) passes the same flags.
- One named instance is enough: while `render_e2e`'s shared-device check still built
  its own instance with the default (debug) flags, a parallel test in that binary read
  back the previous frame's pixels (the tint check read red where it drew blue: 2 runs
  in 25). With the names gone everywhere: 0 in 85 parallel runs on RADV, 0 in 60 on
  lavapipe, no mutex.
- A SIGSEGV in `libvulkan` or a stale frame in a GPU binary: look for a
  `wgpu::Instance` built without `headless_flags()`, then rerun with
  `--test-threads=1` to tell it from a real failure.

The loop that measured it (counts SIGSEGVs per binary):

```sh
for b in $(cargo test -p trino-platform-pc --no-run 2>&1 | sed -n 's/.*(\(target[^)]*\)).*/\1/p'); do
  for i in 1 2 3 4 5; do TRINO_REQUIRE_GPU=1 $b >/dev/null 2>&1; [ $? -eq 139 ] && echo "$b crashed"; done
done
```

Before the fix, more pipelines per renderer made it much worse: building the four 4× MSAA pipelines in
every `PcRenderer::new` took `render_e2e` to 2 crashes in 3 runs (2026-10-16). A mutex
around `new_headless` alone did not help; building them only on `set_msaa(true)` did
(0 in 5 full `trino-platform-pc` runs). Keep optional GPU objects lazy.

## MSAA frames from two allocations differ by a resolve rounding on RADV

The dynamic-resolution tests compare a frame drawn in the corner of big targets with the
same scale in targets of its own size. With 4× MSAA RADV resolves edge pixels 1/255 apart
between the two sizes (and the CAS sharpening makes it 2–3/255); lavapipe is exact
(GitHub #93, ADR 0023 §3). Compare such frames with `common::corner_diff` and
`corner_tolerance(msaa, sharpen)` (0 at 1×, 1 unsharpened, the CAS bound sharpened).
To find which pass parts two frames, toggle one setting at a time (MSAA, AO, sharpen)
and count differences at tolerance 0 and 1, on both ICDs:
`VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/{radeon,lvp}_icd.json`.

