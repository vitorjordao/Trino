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
    `GAME_API_VERSION`. A new `Renderer`/`Platform` method **with a default**
    does not: the host and the dylib share `trino-core`, and a core change
    already needs a restart (rule written in `crates/game-api/src/lib.rs`;
    `second_screen` and the typed screen API followed it).
- `ModelParams` and similar structs grow. Use `..ModelParams::DEFAULT` everywhere.
  A new `ModelParams` field also needs three more things:
  - **Scene codegen:** `crates/scene/src/codegen.rs` writes every field, so add it to
    the template, then regenerate `examples/showcase/src/showcase_gen.rs` with
    `cargo xtask scene export` (the `scene_codegen` guard);
  - **The draw memo:** decide it in `trino_core::memo::MemoKey::new`, which has all 16
    key words in use, so a big field must be excluded;
  - **The RSP:** decide it in `rsp_eligible_mv`. The palette (2026-10-15) did all three.
- A per-vertex T&L input that is unused by most draws gets resolved once per draw into
  `Setup`, and the vertex reads it through a predictable `Option` branch. Don't add a
  const generic: it would double every `tessellate_joints` body on the N64's 16 KB
  I-cache. Prove "unchanged when unused" with a bit-identical comparison test
  (`crates/core/tests/color_slots.rs`).
- **A per-vertex feature with real code (lights, varyings) goes in `render3d/lit.rs`,
  not in `tessellate_joints`.** Local lights first lived there behind never-taken
  branches: the static body grew 15.6 → 18.0 KB, past the I-cache, and every unlit draw
  got 10–35 % slower in ares. `tessellate_with` dispatches a draw to the lit copy only
  when the feature applies. After a T&L change, compare `mips64-elf-nm -S` of
  `tessellate_joints::<false, false>` (0x3d04 today) and the `micro-*` scenes with the
  main branch's ROM.
- **A new surface or light feature must reach the N64's RSP path too.** Most N64 draws
  skip `tessellate` and run on `rsp_trino.S` (ADR 0007), which ignores whatever it
  doesn't implement. Either exclude the feature in
  `trino_core::render3d::rsp_eligible`, which is the safe default, or implement it in
  the overlay behind the oracle and parity checks (`n64-performance` › RSP T&L).

- **A feature the consoles can't run gets a tier, not a PC-only API.** Local lights
  (ADR 0010) and ambient occlusion (ADR 0014) set the pattern:
  - a `Caps` field names the tier;
  - the `Renderer` method has a default no-op and is cleared at `begin_frame`;
  - the console tier is data the asset bake writes (vertex colors), so the consoles
    and the PC's console profiles draw the same thing;
  - the sim profiles never run the PC-only pass, so `n64_parity`/`n3ds_parity` hold.
- **A PC post pass that must spare the HUD ends the main pass after the frame's last
  3D draw**, then draws the rest with `LoadOp::Load` (`draw_commands` › `split`).
  Sprites don't write depth, so a composite at `end_frame` would darken a HUD over
  geometry.

- **A hardware T&L path gets a rule in the core, a reason enum and a histogram before
  it gets a backend** (`rsp_check`/`RspReject`, `gpu_check`/`GpuReject`,
  `pc_gpu_check`/`PcGpuReject`, ADR 0016). The enum has `ALL`, `COUNT` (a const assert
  ties them), `index`, `label`; the renderer keeps a per-frame histogram;
  `trino_scene::budget` predicts the same split and a test holds the two equal
  (`xtask/tests/scene_budget.rs::pc_budget_reports_the_gpu_split`). Phase 1 of #51
  shipped exactly that, with the CPU path still drawing.
- **A new draw call with a default `Renderer` method needs no backend code** when it
  is a loop over an existing one (`draw_model_instances` over `draw_model_matrix`).
  Its self-test step compares one call with the separate draws (pixels on a probe
  grid, `models`/`tris`/`tnl`), and the mutations to try are the ones an override
  can get wrong: a copy dropped, a per-copy field ignored.
- **A tiered surface (water, ADR 0015) keeps the geometry in the core** and lets each
  tier add what its hardware can: the PC per pixel, the 3DS a vertex program
  (`gpu_water` builds its uniforms and `crates/core/tests/gpu_water.rs` emulates the
  shader against the CPU path), the N64 the combiner. A per-vertex effect smaller than
  the vertex spacing (a ripple on a 16 × 4 ribbon) needs geometry of its own on the
  per-vertex tiers (ring decals), or it doesn't show.
- **Blended 3D must come after a batch's RSP commands on the N64** and write no depth
  on any target: the water bucket in `platform-n64/src/renderer.rs` and the 3DS's,
  `TriKind::Water` on PC.
- **A new `ModelParams` field costs `EntityStore` capacity times its size** in every
  game struct. Measure the N64 self-test's stack (`check_stack`) after one.

## 2. Backends, in matched pairs

- The FFI goes through the shims only: `crates/platform-n64/shim/trino_shim.c` with
  `src/ffi.rs`, and `crates/platform-3ds/shim/trino_shim_3ds.c` with `src/ffi.rs`.
  - At most 4 scalar or pointer arguments; no by-value structs; no variadics.
  - **No scalar float after an integer argument.** Pass 16.16 fixed point as `u32`,
    or a pointer to floats. On the N64 a float in that position arrives as garbage,
    with no error from the compiler or the linker. The 3DS follows the same rule so
    the backends stay symmetrical.
- Decode what a shim returns (packed bits, fixed point) in a pure module gated
  `#[cfg(any(target_os = "none", test))]` (N64) or `"horizon"` (3DS), so host
  `cargo test` covers the mapping: `crates/platform-n64/src/pad.rs` and
  `crates/platform-3ds/src/pad.rs` do it for the joypads. New data in an existing
  return value goes in unused bits (the N64's C buttons sit at bits 16..19, clear
  of the `Button` discriminants), which needs no new FFI function.
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
  Then break the feature on purpose (one line in the PC backend) and make sure the
  step fails. The first `dual screen` probes passed with a wrong projection.
- Tests that change the cwd or the environment (`set_current_dir`, `set_var`) race
  with other tests in the same binary. Mark them `#[ignore]` and run them alone
  with `--test-threads=1`, as the editor UI tests do, or don't touch global state.
- Tests that spawn `cargo` should reuse this workspace's `target/`
  (`CARGO_TARGET_DIR`), or they rebuild the world.

## 4. Verify on every platform

1. The `test-all` gates: fmt, clippy `-D warnings`, `cargo test --workspace` (with a GPU).
2. Console type-checks (the nightly commands in `test-all`). Host `cargo check`
   proves nothing for cfg-gated console code.
3. `cargo xtask test n64` and `cargo xtask test 3ds` (`run-emulator`). When render
   or T&L code changed, quote the `test n64` perf table before/after and bless the
   baselines (`n64-performance`); a change in `perf_budget`'s work counters is a
   cost decision, say so in the commit.
4. `cargo xtask scene check` when content changed.

The emulators catch what PC tests can't:
- **ABI bugs**: the float rule above was found by the audio self-test.
- **16-bit z-fighting**: the ares golden caught the showcase pond 2 cm above the ground.
- **Output mismatches**: PC rounded where the RDP truncates; the ares golden and
  `n64_parity` caught it.
- **Rasterizer semantics**: the 3DS drew 3D with citro2d's one-depth-per-triangle,
  so floors covered characters. PC/N64 z-buffers hid it; the `occlusion` step
  (pixels read back on every target) fails on it. A new backend path that changes
  depth, blending or interpolation needs a read-back step like that.
- **Stack overflows**: the N64 main stack is 64 KB. A ~22 KB game struct built by
  value inside the self-test overflowed it and hung the ROM at random; PC (8 MB
  stacks) and the 3DS (256 KB) never notice. `check_stack` fails the N64 self-test
  above 60 KB. Keep big values out of long-lived frames (`run-emulator` › N64 stack).
- **State leaking between users of a renderer**: castle64 rendered with the
  showcase's fogged light on consoles; `n3ds_parity` caught it. Games set their
  own camera and light every frame.

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
