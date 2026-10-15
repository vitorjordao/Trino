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
- T&L cost (what the consoles' CPUs pay): `cargo test --release -p xtask --test bench_tnl -- --ignored --nocapture`.
  Compare against the base commit in a `git worktree` (never `git stash` a dirty tree for this).
- Full console suites: `cargo xtask test n64|3ds` (emulators required; see `run-emulator`).
  N64: golden frame (`tests/golden/n64/`) and the perf gates (work counters = PC,
  times ≤ `tests/perf/castle64_n64.toml`; `n64-performance` skill). 3DS: the
  castle64 hub golden frame within a tolerance (`tests/golden/3ds/`).
- `cargo xtask model inspect <glb>` prints an **N64 cost estimate** per draw
  (`trino_core::perf::N64_COST`) and its share of a 30 fps frame. PC strict mode
  panics when a frame's estimated N64 3D time passes `Caps::max_3d_us_per_frame`
  (50 ms); `strict_mode_rejects_3d_over_the_n64_time_ceiling` covers it.
- Blender toolkit, on machines with Blender: `cargo xtask blender smoke`.
- Skinning cost: `bench_tnl` includes `skinning_rigid_vs_smooth`.

On Linux the PC crates need `pkg-config` and ALSA headers (`apt install pkg-config libasound2-dev`).
