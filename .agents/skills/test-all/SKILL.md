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

GPU-dependent tests skip without an adapter unless `TRINO_REQUIRE_GPU=1`.

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
- Skinning cost: `bench_tnl` includes `skinning_rigid_vs_smooth`.

On Linux the PC crates need `pkg-config` and ALSA headers (`apt install pkg-config libasound2-dev`).

## Solid Eleven

```sh
cargo test -p solid-eleven --release                       # rules, ball, league, e2e, body fit, balance, scenarios
cargo test -p xtask --test solid_eleven_budget --test solid_eleven_layers --test solid_eleven_render
TRINO_BLESS=1 cargo test -p xtask --test solid_eleven_budget   # re-pin the frame cost (review the TOML)
TRINO_BLESS=1 cargo test -p xtask --test solid_eleven_render   # re-bless the PC goldens (view the PNGs)
cargo test --release -p xtask --test solid_eleven_menus   # menu crawler (every state, EN/PT, 3 sizes) + random play
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

## GPU tests crash now and then on RADV

On AMD's RADV (this repo's dev machine, 2026-10-15) a test binary that creates and drops
several `PcRenderer`s from parallel test threads segfaults about once in 10–40 runs
(`render_e2e` on main: 1/40; a five-test binary: 3/30). Serial runs never crash. The
newer GPU test files hold a binary-wide mutex (`gpu()` in
`crates/platform-pc/tests/local_lights.rs`); a SIGSEGV in another GPU binary is this
flake, so rerun it before suspecting the code.

More pipelines per renderer make it much worse: building the four 4× MSAA pipelines in
every `PcRenderer::new` took `render_e2e` to 2 crashes in 3 runs (2026-10-16). A mutex
around `new_headless` alone did not help; building them only on `set_msaa(true)` did
(0 in 5 full `trino-platform-pc` runs). Keep optional GPU objects lazy.
