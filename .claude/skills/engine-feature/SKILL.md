---
name: engine-feature
description: Add or change an engine service, data format or backend capability in Trino end to end — core API, all three backends, the console self-test step, tests, format versioning, docs and the closing knowledge update. Use for "implement backlog item X", "add a feature to the engine", "change TMDL/TSCN/TFNT", or any change that touches trino-core plus a platform crate.
---

# engine-feature: the definition of done

A feature is done only when **PC, N64 and 3DS** all run it and an emulator has said
`TRINO_TEST_PASS`. Do these steps in order.

## 1. Design against the N64 first

- If the N64 cannot do it, the engine does not expose it (AGENTS.md › Design ceiling).
  Check `Caps` (`crates/core/src/caps.rs`) for the budget it spends.
- Logic goes in `trino-core` (no_std, zero deps, deterministic f32 through `math3d`).
  Backends only do what needs hardware: rasterizing, audio channels, storage.
  When a backend needs no new code, the feature is in the right place
  (`render3d`, `anim`, `vis` and `physics` are all core-only).
- Data that grows goes in a **new section or field**, never in a changed layout:
  - TMDL: add a tag; unknown tags are skipped.
  - Scene RON: add an optional, defaulted field (no version bump). Removing or
    renaming a field needs a version bump plus a migration in `Scene::from_ron`.
  - TSCN: frozen at the first release (BACKLOG P0 #2). After that, every change
    bumps the version and adds a migration in `BakedScene::parse`.
  - Game API: a new `Platform`/`Game` method that changes the dylib ABI bumps
    `GAME_API_VERSION`.
- `ModelParams` and similar structs grow. Use `..ModelParams::DEFAULT` everywhere.

## 2. Backends, in matched pairs

- The FFI goes through the shims only: `crates/platform-n64/shim/trino_shim.c` with
  `src/ffi.rs`, and `crates/platform-3ds/shim/trino_shim_3ds.c` with `src/ffi.rs`.
  - At most 4 scalar or pointer arguments; no by-value structs; no variadics.
  - **No scalar float after an integer argument.** Pass 16.16 fixed point as `u32`,
    or a pointer to floats. On the N64 a float in that position arrives as garbage,
    with no error from the compiler or the linker. The 3DS follows the same rule so
    the backends stay symmetrical.
- The PC backend (`crates/platform-pc`) is the reference. Anything visual must match
  the N64 look (`build-n64` › Console output conventions).

## 3. Tests: every layer, one feature

| Layer | Where | Notes |
|---|---|---|
| Unit tests | next to the code (`#[cfg(test)]`) | core uses std in tests |
| Format round trip | bake ↔ parse, byte for byte | hand-built blobs in core tests too |
| **Console step** | `examples/castle64/src/selftest.rs` | one `TRINO_STEP:<name>`; the same code runs on PC (`xtask/tests/engine_checks.rs`), in ares and in Azahar |
| Pixels | PC goldens (`--bless` only if intended); N64 golden through `test n64` | perf-only changes must leave goldens byte-identical |
| Editor | `crates/editor/src/ui_tests.rs` for new panel flows | `widget_info` on hand-painted widgets |

- A self-test step must **assert** what the feature does: probe pixels, read back
  bytes, compare counts. A step that only calls the API and prints proves nothing.
- Tests that change the cwd or the environment (`set_current_dir`, `set_var`) race
  with other tests in the same binary. Mark them `#[ignore]` and run them alone
  with `--test-threads=1`, as the editor UI tests do, or don't touch global state.
- Tests that spawn `cargo` should reuse this workspace's `target/`
  (`CARGO_TARGET_DIR`), or they rebuild the world.

## 4. Verify on every platform

1. The `test-all` gates: fmt, clippy `-D warnings`, `cargo test --workspace` (with a GPU).
2. Console type-checks (the nightly commands in `test-all`). Host `cargo check`
   proves nothing for cfg-gated console code.
3. `cargo xtask test n64` and `cargo xtask test 3ds` (`run-emulator`). Quote the
   `TRINO_PERF` line when render or T&L code changed (`n64-performance`).
4. `cargo xtask scene check` when content changed.

The emulators catch what PC tests can't:
- **ABI bugs**: the float rule above was found by the audio self-test.
- **16-bit z-fighting**: the ares golden caught the showcase pond 2 cm above the ground.
- **Output mismatches**: PC rounded where the RDP truncates; the ares golden and
  `n64_parity` caught it.

## 5. Record it

- **ADR:** a format, ABI or convention decision gets a section in an ADR. Extend the
  current one, or write `docs/adr/000N-*.md`.
- **`docs/BACKLOG.md`:** move the item to Done, with its commit and how it was
  verified. Write any **deviation** from the item's wording plainly, and add the
  follow-ups the work surfaced to the ranked list.
- **AGENTS.md:** the Current state section and the Commands block; add a Pitfall when
  something cost real time.
- **README:** only when the feature is user-visible.
- **Skills:** the skill a future agent would open for this kind of work. See the
  closing step.

## 6. Commit

- One commit per backlog item, so a reviewer can revert one feature cleanly. If two
  items had to share a commit, say so in the BACKLOG Done table.
- To compare against the base commit, use `git worktree`. Never `git stash` a dirty tree.
- Commit locally. Push or open a PR only when the user asks.

## Closing: the knowledge loop (mandatory)

Before you report done, ask: *what did this cost that the next agent should not pay
again?* Then write it down:
- **A trap with an obvious-in-hindsight fix:** a Pitfall line in AGENTS.md, plus a
  line in the closest skill.
- **A procedure:** the skill that owns it (`build-*`, `run-emulator`, `ci`,
  `test-all`, …). Create a new skill in `.agents/skills/<name>/SKILL.md` when no
  skill fits.
- **Then:** `cargo xtask agents sync`, add any new skill to the AGENTS.md table
  (`agents_sync` fails CI otherwise), and rerun `cargo test -p xtask --test agents_sync`.

Each skill states facts verified in this repository, with the command that proves
them. If a skill turns out wrong, fix it in the same session.
