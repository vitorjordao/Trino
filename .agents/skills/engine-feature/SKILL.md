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
- The ceiling is the **base** N64 (4 MB, no Pak). An expanded tier (`Caps::N64_EXPANDED`,
  `Caps::N3DS_NEW`, ADR 0020) may add memory, cores, crowd or particle scale, never a
  feature the base console lacks. Per-tier differences are content (`n64_expanded` /
  `n3ds_new` variants) or a branch on `platform.hw_tier()` at update time.
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
  Shadows (ADR 0025) are the run-time variant: each console tier draws at run time
  (blobs, planar), and the PC renderer draws the same tier on the console profiles
  (`Caps::shadow_tier` of the profile), so the editor's Game view and the goldens show
  what the console shows. Place coplanar geometry by the target's depth step
  (`Caps::depth_step`, `shadow::shadow_z`), never a fixed lift.
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
- **A GPU path is held to the CPU path by an oracle on every golden scene, not by
  eye.** The PC's `tnl_oracle::compare` draws one frame on both paths with the same
  renderer (≤ 1/255 mean, ≤ 0.5% of pixels over 8/255) and the native goldens call
  it next to their golden check. Reproduce the CPU path's approximations (per-vertex
  fog, affine color on w = 1 triangles, its truncations and operation order), even
  where per pixel would look better: the river missed by 2.7/255 with per-pixel fog.
  Make the counters exclusive when the work moves, and update every test that summed
  them (`render_e2e`, `scene_budget`, the editor's budget test).
- **A parallel CPU path must emit the serial stream.** Split the draws into
  contiguous chunks, give each worker its own scratch, concatenate in draw order, and
  test the triangles (`cpu_tnl::parallel_runs_emit_the_serial_stream`) and a whole
  frame's pixels and counters for several worker counts. Keep the console profiles on
  the serial path.
- **Per-entity paths make a split actionable.** `FrameBudget::paths` (the entity's draw
  name, `Scene::draw_name`, and its `RenderPath`) feeds `scene budget`'s entity lists and
  the editor's Render path row; a new hardware path adds its name to
  `RenderPath::hardware_name`.
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
- **Content the bake generates without a manifest entry** (the `audio/mixer`, the
  `quality/presets`, the UI's `ui/<name>` and `i18n/<lang>` blobs) touches five places:
  `asset-pipeline/src/bake.rs` (PC/Web), `xtask/src/n64.rs` and `xtask/src/n3ds.rs`
  (after the mixer: write `{id:08x}.blob` and an index line), `memory.rs` (the plan),
  `watch.rs` and `xtask/src/project.rs`'s `watch` lists (live reload). The Web pack
  takes every baked blob already.
- **Generated data a game reads can ride the UI bake:** `asset_pipeline::ui::bake` returns
  every generated blob (screens, string tables and, since #60, `access/data`), and the
  PC/Web bake, the N64 and 3DS images and the memory plan all iterate it — one function,
  no five-place edit. Count it in `xtask/tests/cli.rs::assets_pc_bakes_the_repo_manifest`.
- **A player setting with a default must take the old code path at the default**
  (ADR 0027): multiply by exactly 1.0, keep the old theme, and put the setting in any
  cache key the old path has (the UI's layout key). Every golden then stays byte-identical
  and the change can be proven by running the existing goldens unchanged.
- **A setting the player changes at run time on every platform** (accessibility): a
  `Hosted*` struct in the core held by every backend (apply now, record a change), the
  host persisting after the frame (`take_*_change`), a forced env/URL value that is never
  written back, and nothing persisted on smoke or self-test runs.
- **Content the bake generates for a language** (the `i18n/<lang>` tables and
  `i18n/languages`, issue #59) goes through `asset_pipeline::ui::bake`, which every
  platform already calls; a manifest option resolved at load (`chars = "auto"`,
  `per_language` atlases) belongs in `Manifest::load` (`asset_pipeline::i18n::resolve`)
  so the bakes, the memory plan, the reports and the editor all see the same fonts.
- **When a runtime value changes what the layout measures** (the language's font, #59),
  route the layout and the draw through one function (`ui::draw::font_of`) and test
  that both use it, or overflow checks and pixels disagree.
- **A RON format an editor round-trips denies unknown fields**
  (`#[serde(default, deny_unknown_fields)]`): with only `default`, a misspelt field
  parses as the default and the next save drops it.
- **A new `ModelParams` field costs `EntityStore` capacity times its size** in every
  game struct. Measure the N64 self-test's stack (`check_stack`) after one.

- **An upgrade of an existing look ships off by default, behind plain-`f32` settings
  whose 0 runs the old arithmetic** (contact shadows, #86: `soft_growth`,
  `fade_distance`). Guard the new shader work with a uniform branch and make the old
  path's operations identical (same tap count, same spacing, a `× 1.0`), then prove it:
  write the frames (`TRINO_SHADOW_SHOTS`) from this branch and from the base commit in
  a separate worktree with its own `target/`, on the same GPU, and compare bytes. Not
  against the goldens: they were blessed elsewhere and differ by driver noise. And not
  with PIL's `getbbox` of an RGBA difference, which reads only alpha. Opt the one game
  that wants it in, and re-bless only that game's native golden.
- **Something the host identifies (a pad's family, #86) is a pure core function over
  what the host copies out** (`PadFamily::detect(name, usb)`), fed by the PC's glue
  (`PcInput::note_pad`) and a non-wasm parser on the Web (`keys::parse_pad_id`), exposed
  as a defaulted `Platform` method (`gamepad_family`, `None` on consoles). The tests then
  need no device: a table of real id strings per browser and per OS.

- **A service a store may back (achievements, #61, ADR 0029):** the core holds the
  logic in a `Hosted*` struct every backend owns, the `Platform` methods are defaults
  over two accessors (`services()`/`services_mut()`), and the store lives in the PC
  runner behind a trait with a mock (`StoreApi`/`MockStore`). On the consoles, state the
  player earns goes into **the game's** save (`pack_for`/`restore_*`), never a region the
  engine carves out of the EEPROM (it would move the save layout of every game).

- **A per-frame effect that redraws the frame (stereo, issue #63, ADR 0032):** record
  the `Renderer` calls, not their triangles, and replay them (`trino_core::stereo::
  StereoCmd` + `replay`). The core owns the command type and the replay, with slices as
  `Span`s into arenas; the backends own the `Vec`s, because the core has no `alloc`.
  - **Every top-screen method records at its top**, and replays start from a snapshot
    of the state the recording began with.
  - **A projection change belongs in `TnlContext`** (`with_eye`: out of line, no new
    field, a zero shift returns the context unchanged). Then the CPU T&L, the PICA200's
    `gpu_screen_matrix`, water, billboards and shadows all follow with no shader edit.
  - **Prove the default path byte for byte:** compare the frame with the call against
    the frame without it (`the_slider_down_is_the_mono_frame_byte_for_byte`). Prove the
    effect by mutation: a flipped center sign, no eye move, no layer shift and a right
    eye equal to the left each fail the `stereo` step.
- **Instrument a new stage with the trace** (issue #62, the `profiling` skill):
  `trino_core::zone!("name")` at its boundary (never inside the T&L), a counter per frame
  for its work. Out-of-line code that only a `--trace` build needs goes behind
  `if trino_core::trace::ENABLED` into an `#[inline(never)]` function, so the release
  build's code doesn't change (`build n64` checks the hot symbols).

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
- **The Web gets the PC renderer for free** (`crates/render-wgpu`, issue #47): a renderer
  feature is a Web feature. Check it on WebGL2 too: a shader that reads depth goes through
  `depth_as_float`, a new pipeline in the scene pass needs no `sampler2DMS` (no MSAA
  there), and `cargo test -p trino-render-wgpu webgl2` translates every shader to GLSL ES
  3.00. A new `Platform`/`Audio`/`Input` method needs its `crates/platform-web` side (the
  self-test runs in browsers: `cargo xtask test web`).
- **A shader that reads a depth texture raw breaks only on WebGL2**, at translation (naga
  has no GLSL for a depth `textureLoad`) or silently (a sampler-less read returns 0). Bind
  the view again as unfilterable float with a nearest sampler (`depth_as_float`,
  `shadow_map::map_read_as_float`), add the shader to
  `webgl2_shaders_translate_to_glsl_es_300`, and when the feature is off in every web
  golden, opt castle64 in for one local `cargo xtask test web --browsers chromium` and
  compare the WebGL2 and WebGPU frames in `target/web-test/` (then revert).
- **A setting only the PC and the Web can change** (quality presets, #55, ADR 0023): a
  default `Platform` method that returns the console's fixed value (no-op setters,
  `None` info), and one host-side struct in `crates/render-wgpu` that both `PcPlatform`
  and `WebPlatform` hold (`quality::HostedQuality`), so the two hosts can't drift. Smoke
  runs, `?frames` and the self-test pin a deterministic value (High), never the
  machine's: goldens must not depend on a bench.

- **A new render pipeline on the PC/Web** (issue #101, ADR 0031):
  - **Create it through `renderer::pipes::render(device, desc)`**, never
    `device.create_render_pipeline`. That hands it the persistent pipeline cache and
    counts it (`PcFrameTimes::pipelines`, `TRINO_FRAME`'s `pipes=`). Build it under the
    renderer's `pipes::scope(&self.pipe_entry)`; an unscoped build counts as
    `unscoped`, which the precompile test holds at 0. Never key renderer state on
    `wgpu::Device ==`: the id repeats across instances.
  - **If the pass is built lazily** (on first use inside a frame), give it a
    `precompile::JobKind`, a line in `precompile_plan` and `warm_install`, and a
    `self.warm_wait(kind)` before its lazy site. Otherwise
    `precompile.rs::a_full_warm_up_leaves_nothing_to_compile_in_frames` catches the
    hitch: add the new toggle to its `toggle_everything`.
  - **Never build it eagerly in `PcRenderer::new`**: eager creation crashed parallel
    headless tests on RADV (#94).
  - **A job builds from cloned handles (`Inputs`)**, and the render thread installs the
    result. A pass whose bind groups must match a layout creates that layout on the
    render thread first (the CVD pass).
- **Work spread over CPUs** (bakes, loads, console shader assembly; #101):
  - use the one budget (`trino_core::ThreadSetting`, `auto` = all − 1, at least 1);
  - use `asset_pipeline::par::map_ordered` (results in input order), so N threads give
    the bytes, report and errors of 1. The tests compare 1 against N.
  - GPU and audio uploads stay on the render thread, in order (`BundleLoad::step_many`
    decodes; the loader installs).
  - The per-frame CPU T&L pool is a separate knob (`TRINO_PC_TNL_THREADS`).
- **Measure shader compile time with the driver's disk cache off**
  (`MESA_SHADER_CACHE_DISABLE=true`, `__GL_SHADER_DISK_CACHE=0`), or a second run
  measures Mesa's or NVIDIA's cache (`precompile_bench`). Report naga's time apart from
  the driver's: a pipeline cache only removes the driver's.

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
- A step that probes pixels must still run its logic where `read_screen_pixel` is
  `None` (the browsers): return `Ok(false)` ("probe skipped") after the non-pixel checks
  instead of skipping the whole step (`ui_layout` taps the touch screen either way).
- Tests that change the cwd or the environment (`set_current_dir`, `set_var`) race
  with other tests in the same binary. Mark them `#[ignore]` and run them alone
  with `--test-threads=1`, as the editor UI tests do, or don't touch global state.
- Tests that spawn `cargo` should reuse this workspace's `target/`
  (`CARGO_TARGET_DIR`), or they rebuild the world.

### Tuning a deterministic game sim (Solid Eleven, issue #86)

- Fixed-seed statistical tests (`sim/balance.rs`, the AI sample) sit near their
  limits: any sim change reshuffles every seed. Measure on bigger tuning samples
  first (`cargo test -p solid-eleven --release print_big_metrics print_press_duel
  print_side_bias -- --ignored --nocapture`, 60 to 400 matches, seconds in release),
  then pick constants that pass *every* fixed-seed test at once.
- Measure a one-sided behavior (pressing, a keeper's rush) as a duel: one side
  with it, the same clubs without, sides swapped every other match
  (`press_duel`, the test-only `Match::urgency_test`). Shares that alternate by
  construction (possessions won) stay at 50 %; use territory, possession, fouls.
- Prove a "no teleport" change with a whole-match per-tick step bound and run that
  test on the old code once (a scratch `git worktree` with its own `target/`): it
  must fail there.
- Evidence for motion: a top-down trace (one dot per tick per player) shows a
  jump as a gap; `xtask/tests/solid_eleven_gameplay_shots.rs` writes them.

## 4. Verify on every platform

1. The `test-all` gates: fmt, clippy `-D warnings`, `cargo test --workspace` (with a GPU).
2. Console type-checks (the nightly commands in `test-all`). Host `cargo check`
   proves nothing for cfg-gated console code.
3. `cargo xtask test n64` and `cargo xtask test 3ds` (`run-emulator`). When render
   or T&L code changed, quote the `test n64` perf table before/after and bless the
   baselines (`n64-performance`); a change in `perf_budget`'s work counters is a
   cost decision, say so in the commit. A change that touches memory, boot or the
   asset index boots both tiers: `--expansion-pak both`, `--new3ds both` (CI does).
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
  Give the worktree its own `CARGO_TARGET_DIR`: sharing this one makes the two
  checkouts' path crates overwrite each other's rlibs (same metadata hash), and the
  branch then fails to compile until its sources are touched (2026-10-19).
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
