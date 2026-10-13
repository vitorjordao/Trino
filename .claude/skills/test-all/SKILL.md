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
| `xtask/tests/standalone.rs` | `cargo xtask new --standalone` output no longer builds/tests (windowed smoke with GPU + display) | fix `templates/standalone/` or `trino-runner-pc` |
| `crates/editor/src/ui_tests.rs` (`#[ignore]`: `cargo test -p trino-editor ui_tests -- --ignored --test-threads=1`, own CI step) | an editor panel flow broke (select → palette → duplicate → undo) | fix the UI; hand-painted widgets need `widget_info` |

GPU-dependent tests skip without an adapter unless `TRINO_REQUIRE_GPU=1`.

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
  They also check the N64 golden frame (`tests/golden/n64/`) and print `TRINO_PERF`.
- Blender toolkit, on machines with Blender: `cargo xtask blender smoke`.
- Skinning cost: `bench_tnl` includes `skinning_rigid_vs_smooth`.

On Linux the PC crates need `pkg-config` and ALSA headers (`apt install pkg-config libasound2-dev`).
