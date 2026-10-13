# Plan — N64 render throughput (BACKLOG P0 #1)

Written 2026-10-13 for a **fresh agent session**. Read all of it before writing code.
It turns BACKLOG P0 #1 into ordered, gated work and records what was already
measured, so nobody has to re-derive it.

## Goal

castle64 at **≥ 30 fps (frame ≤ 33.3 ms emulated) in all four stages**, plus a
measured, decided path for the hub. Today, from `cargo xtask test n64`, frame =
update + render − begin:

| Scene | Frame (ms) | ≈ fps | T&L | sort | submit | game¹ | Draws | Emitted |
|---|---|---|---|---|---|---|---|---|
| hub | 76.2 | 13 | 55.0 | 3.8 | 12.2 | 4.1 | 34 | 533 |
| green-hills | 37.3 | 27 | 28.1 | | | | 23 | |
| lava-land | 49.1 | 20 | 33.7 | | | | 22 | |
| sky-tower | 31.2 | 32 | 23.2 | | | | 26 | |
| throne | 43.4 | 23 | 28.7 | | | | 19 | |

¹ `game` is render time outside the timed stages. It covers castle64's own render code
plus the `draw_model` overhead outside `tessellate`: `Mesh::from_tmdl`, the `BTreeMap`
lookup and `Transform3::matrix`'s software sin/cos. For the hub that is ~120 µs per draw
on top of the ~278 µs fit inside the T&L.

Fill in the empty cells from your own Phase 0 baseline run; they are in
`tests/perf/castle64_n64.toml` and in the `TRINO_PERF_SCENE` lines.

## Start here

1. **Branch.** This plan exists only on `plan/n64-throughput` until its PR merges.
   - Work on `plan/n64-throughput`. Run `git fetch` first.
   - If PR #1 (`backlog/full-pass` → `main`) has been merged, run `git merge origin/main`
     into it first.
   - Make one commit per item.
   - Don't push or merge without asking the user.
2. **Environment.** These shells don't inherit rustup, and `/usr/bin/cargo` shadows it:
   ```sh
   source ~/.cargo/env
   export DEVKITPRO=$HOME/.local/opt/devkitpro TRINO_AZAHAR=$HOME/.local/opt/azahar/azahar DISPLAY=:0
   ```
   ares runs from `ares-v148/` at the repo root, or set `TRINO_ARES=docker`. The N64
   toolchain is the `trino-n64` Docker image.
3. **Read first:**
   - the `n64-performance` skill: gates, cost model, how to read numbers, what worked
     and what didn't;
   - ADR 0003 (software T&L);
   - ADR 0006 (perf and visual gates; §9 content, §10 back faces);
   - `crates/core/src/render3d.rs` (`tessellate`, `tessellate_joints`);
   - `crates/platform-n64/src/renderer.rs` and `crates/platform-n64/shim/trino_shim.c`
     (`trino_tri`).
4. **Gates.** Run them before every commit:
   ```sh
   cargo fmt --all --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace                 # includes perf_budget (exact T&L work) + PC goldens
   cargo xtask test n64                   # ROM self-test, pixel-exact golden, work == PC, time ceilings
   cargo xtask build 3ds --selftest && cargo xtask test 3ds --no-build   # 3DS golden (tolerance)
   ```

## Ground rules

- **Measure on the scene table, A/B on identical code.**
  - Code layout alone moves a stage by ~20% and the frame by ~5%: a 16 KB I-cache and an
    8 KB D-cache, both direct-mapped.
  - Build the baseline in a `git worktree` of the parent commit, never `git stash`.
  - Judge on all five scenes' `frame` values; never claim a stage win under 10% from one
    build.
  - Synthetic benches lie. The micro scenes below measure *fixed costs*, but a change is
    kept or reverted on `TRINO_PERF_SCENE`.
- **Two kinds of change. Label each commit with one.**
  - **Identical:** goldens byte-identical (PC, N64) and the 3DS golden at mean 0.00;
    work counters equal. Most of Phase 1 must be this. If a "should be identical" change
    moves a golden, it is a bug in the change.
  - **Output-moving:** content, DEPTH_SPLIT, math changes.
    1. Re-bless deliberately: `TRINO_BLESS=1 cargo test -p xtask --test perf_budget`,
       then `cargo xtask test n64 --bless`, then `cargo xtask test 3ds --bless` if needed.
    2. Look at each PNG diff.
    3. Explain the change in the commit.
- **After every kept change:**
  - bless the time ceilings so the gain is locked in;
  - quote the before/after scene table in the commit message;
  - update the `n64-performance` skill: the "Where the time goes" table and a row in
    "What worked, what didn't", reverted attempts included.
- **Don't retry what failed** (skill › What worked, what didn't):
  - a per-vertex cached projection plus a no-copy fast path was neutral and was reverted;
  - `#[inline(never)]` splitting of the hot loop made it 3–17% worse;
  - `opt-level = "s"` was worse everywhere.
- **Recalibrate `N64_COST`** after Phase 1 and again after any T&L change. The
  procedure is in the skill: least squares over the `TRINO_PERF_MODEL` lines.
- **Knowledge loop at the end.** Update the skills, AGENTS.md Pitfalls if a trap
  appeared, BACKLOG (P0 #1 table plus a Done row) and ADR 0006 (or a new ADR for Phase 4).
  Then run `cargo xtask agents sync`.

## Evidence already gathered (2026-10-13, ROM built from `backlog/full-pass`)

Disassembly from `target/n64/trino.elf`:
```sh
docker run --rm -v "$PWD/target/n64":/w -w /w trino-n64:latest \
  sh -c 'mips64-elf-objdump -d --no-show-raw-insn trino.elf' > /tmp/rom.S
```

- **The float path is hardware FPU.** There are no `__*sf3` libcalls in `tessellate_joints`.
  The target spec is `mips3` o32 hard-float.
- **Vertex fetch is slow.**
  - `render3d::rd_vec3` is an out-of-line call: 12 bounds-checked byte loads, then
    little-endian assembly on a big-endian CPU (172 instruction lines, 12
    `panic_bounds_check` branches).
  - Each transformed vertex calls it twice (position and normal).
  - `Mesh::color` and `Mesh::index` are bounds-checked byte loads too.
- **The vertex closure is heavy.**
  - The static-path vertex closure (`tessellate_joints<false,false>::{closure}`) has 1210
    lines, with **15 `div.s`** and **72 `cvt`/`trunc`**. A VR4300 `div.s` costs ~29
    cycles.
  - The divisions come from:
    - `Setup::finish`'s `t as f32 / 255.0` per channel per vertex (4 per vertex);
    - `normalized()`'s `1.0 / len`;
    - fog, when on.
  - The fit's 16.6 µs per vertex is ~1550 cycles.
- **Per draw, before any vertex:**
  - `camera.view()`, focal (software `sin`/`cos`) and `light.dir.normalized()`
    (software `sqrt` plus a division) are recomputed every draw;
  - `VertexCache` (640 B), `poly_a`/`poly_b` (384 B) and `stack` (3456 B) are
    initialized every draw, about 4.5 KB of stores into an 8 KB D-cache;
  - skinned draws also initialize three `[Mat34; 32]` (4.6 KB);
  - outside the T&L timer, `draw_model_matrix` runs `Mesh::from_tmdl` every call. For
    skinned meshes that re-validates `joint_of_vertex`, every ANIM clip and SKW2.
- **Submit, ~23 µs per emitted triangle** (hub: 12.2 ms / 533):
  - Rust repacks each `ScreenTri`, then calls `trino_tri` once per triangle;
  - the shim divides **12 color bytes by 255.0f** (12 `div.s`, ~350 cycles);
  - libdragon's `rdpq_triangle` then multiplies each one back by `255.0`, in double.
  - At the pinned commit `rdpq_triangle` is `rdpq_triangle_rsp`: edge setup runs on the
    RSP, and the CPU only converts and does 4 `rspq_write`s. This is already corrected in
    the skill.
  - `RDPQ_OVL_ID` and `RDPQ_CMD_TRIANGLE_DATA` are public in `rdpq.h`.
  - `__rdpq_autosync_use` is internal; don't depend on it.
- **libdragon pin.** `docker/n64/Dockerfile` pins `2c3d5db` on **trunk** (2026-06-26).
  tiny3d requires the **preview** branch, which has ~4,600 commits the pin lacks (see
  Phase 4).

## Phase 0 — measure the fixed costs (S, no output change)

The cost-model fit can't separate its terms (ADR 0006 §3). Phases 1–2 depend on the
real per-draw and per-triangle numbers.

1. **Micro perf scenes, N64 only and ungated.**
   - Draw sets:
     - **A:** 64 draws of a 1-triangle mesh on screen;
     - **B:** 64 draws culled by the bounding sphere (behind the camera);
     - **C:** one draw of a dense mesh (a grid of ≥ 200 vertices) in `Gouraud`, `Unlit`
       and `Flat`;
     - **D:** a batch of N emitted triangles (sort + submit only).
   - Report them like the scenes, e.g. `TRINO_PERF_SCENE:micro-draws`. Give them their
     own names so `xtask/src/perf.rs` doesn't gate them; check how it parses scene names.
   - Use existing small meshes (`M_SHADOW`, a block) or a tiny generated TMDL. The
     synthetic `cube_tmdl` is `cfg(test)`.
   - Read the results:
     - (A − B)/64 is the visible per-draw cost;
     - B/64 is the floor;
     - C per vertex is the per-vertex cost by shading mode;
     - D per triangle is the sort and submit cost.
2. **Write the numbers into the skill's cost-model section** as measured fixed costs.
   Keep the fit for scene totals.
3. **Lock a new baseline.** The micro scenes add ROM code, and code layout moves every
   scene. Once Phase 0 lands:
   - run `cargo xtask test n64 --bless` (time ceilings);
   - use that commit as the A/B base for Phase 1.
4. **Decide.**
   - If the per-draw floor is ≥ 150 µs, Phase 1a–1c pays first.
   - If per-vertex is ≥ 1000 cycles after 1d–1e, keep going on the CPU before Phase 4.
   - Record the decision in the plan's checklist (bottom).

## Phase 1 — CPU constant factors, output-identical (S–M each)

Each item is its own A/B commit, ordered by expected value. Bit-identity comes from
computing the same f32 expressions in the same order, only fewer times.

- **1a. Per-frame T&L setup.**
  - Compute `view`, `focal`, `lim_x`/`lim_y`, `ldir` and the light factors once per
    `set_camera`/`set_light`, not per draw.
  - One option: a `FrameSetup` (or `TnlContext`) in `trino_core::render3d` that backends
    keep and that `tessellate` takes by reference.
  - The PC and 3DS backends call `tessellate` too; change all three. The work counters
    prove identity.
- **1b. Per-draw scratch without initialization.**
  - Keep the `poly_a`/`poly_b`/`stack` scratch and the vertex-cache arrays in a struct the
    backend owns and passes in as `&mut`, or use `MaybeUninit`.
  - Only the cache tags need resetting per draw (32 words).
  - The skinned `[Mat34; 32]` ×3 should fill only `joints` entries.
  - The comment at `render3d.rs:1019` says why scratch matters: initializing it per
    triangle was once the single largest cost.
- **1c. Parse meshes once.**
  - `N64Renderer::meshes` stores bytes and calls `Mesh::from_tmdl` per draw. Store the
    parsed layout at `register_mesh`, e.g. a `MeshLayout` of offsets and counts, so
    `Mesh` can be rebuilt without validation.
  - Do the same on PC/3DS if they re-parse.
- **1d. Vertex fetch.**
  - Make `rd_vec3`/`position`/`normal`/`color`/`index` free of bounds checks on the hot
    path: validate lengths once in `from_tmdl`, then index with `get_unchecked` behind a
    debug assert, or read through `as_chunks`.
  - Make sure `rd_vec3` inlines. Check the disassembly: it must disappear as a symbol.
  - The TMDL is little-endian and the N64 is big-endian. Byte-swapping at load into an
    owned, aligned native-endian buffer (one copy per mesh at register) would make the
    reads plain `lwc1` loads. Only do that if the measurements show the fetch matters
    after the bounds checks are gone; it costs RAM (8 MB on N64).
- **1e. Shading divisions.**
  - Hoist `tint.{r,g,b,a} as f32 / 255.0` into `Setup`. `finish` computes
    `v * (t / 255) * k`, so the per-draw quotient is the same value: bit-identical.
  - Check every remaining `div.s` in the vertex closure. Only hoist divisions whose
    operands are per-draw constants. Rewriting `x / y` as `x * (1/y)` is **not**
    identical; that belongs in Phase 3.
- **1f. Submit.**
  - Replace the per-triangle FFI with one `trino_tris(ptr, n)` per batch. Rust writes a
    packed array in painter order: 6 floats xy, 3 floats z, 12 color bytes.
  - In the shim, replace `c / 255.0f` with a 256-entry `float` table built at init. It
    holds the same values, with no divisions.
  - Keep calling `rdpq_triangle`.
  - Optionally, a later output-identical step: pack `RDPQ_CMD_TRIANGLE_DATA` directly
    with `rspq_write`, replicating libdragon's exact conversions (`floorf(x*4)`,
    `z*0x7FFF`, `(c/255.0f)*255.0` as double). It needs `__rdpq_autosync_use`, which is
    internal. Do it only if the micro scene D says submit still dominates, and record the
    libdragon coupling in ADR 0002/0006.
  - The same applies to `trino_tri_tex` and the 3DS/PC paths. Only the N64 is measured.
- **1g. Sort.**
  - Untextured batches all have group 0, so sort the `u64` painter keys alone.
  - Painter keys are unique (index tie-break), so any correct sort gives the same order.
    A radix sort on the 32-bit depth plus a stable index is an option if Phase 0 shows
    sort ≥ 3 ms.
- **1h. Game-side draw overhead.**
  - `Transform3::matrix` (`Mat34::from_rotation_scale_translation`) runs software
    `sin`/`cos` for all three axes every draw. Blocks have zero rotation, and coins and
    boars only yaw.
  - A fast path for zero angles (`sin(0) = 0`, `cos(0) = 1` exactly) is identical; check
    that `math3d::sin(0.0) == 0.0` and `cos(0.0) == 1.0` bit-exactly first.
  - castle64's per-frame `visible` closure and shadow search are small. Look at the
    `game` stage after 1c.

**Exit.**
- Every item is kept or reverted with numbers.
- `N64_COST` is recalibrated.
- The skill tables are updated.
- Re-check against the Phase 0 micro scenes.

## Phase 2 — fewer and cheaper draws (M, output-moving)

Do this after Phase 1, because cheaper draws shrink this phase's payoff. Re-measure
first. Target: **green-hills and lava-land ≤ 33 ms**.

- **2a. Coins as billboards.**
  - Hub coins cost 6.3 ms for 8 draws.
  - `draw_billboard` exists on all targets. It goes through the textured batch path with
    one TMEM upload per texture group.
  - Fake the spin by scaling the billboard's x by `|cos(spin)|`. The coin sprite already
    exists (`S_COIN`, used by the HUD).
  - Watch the bot and selftest: coin pickup is physics, not rendering, so they should
    not care.
- **2b. Merge static blocks per level, in spatial chunks.**
  - Each block is a 24-vertex, 12-triangle cube draw with non-uniform scale.
  - Bake each level's static, untinted blocks into one TMDL **per chunk**, e.g. an XZ
    grid. One mesh per level would lose per-block culling, and green-hills draws only part
    of its blocks.
  - The generator is `xtask/src/castle64_assets.rs` (`cargo xtask gen-assets`) reading
    `castle64::levels`. Check that the xtask → castle64 dependency is allowed
    (`dep_graph`).
  - While merging, drop faces shared between touching blocks. They are buried, and
    ADR 0006 §9 says those pay full cost.
  - Keep lava (per-draw tint) and movers as single draws.
  - Physics keeps its own AABBs. Only the draw list changes.
- **2c. The player as one draw (optional, last).**
  - Today it is 5 draws: torso, two arms, two legs. A rigid-skeleton model needs the pose
    to come from code, not a clip.
  - Either add a per-draw joint-pose override to `ModelParams`, which is an engine API
    change (ADR 0003/0005, all three backends and a selftest step), or bake walk and air
    clips in Blender and drive them with `AnimRef::blend_to`.
  - It saves ~4 × (per-draw floor). Decide with the Phase 0 numbers; it may not pay.

**Exit.**
- Stage frames ≤ 33 ms, or the reason they aren't.
- Goldens re-blessed and reviewed.
- Work baseline blessed.

## Phase 3 — renderer changes that move goldens (S–M)

- **3a. DEPTH_SPLIT for z-buffered opaque draws.**
  - Every target z-buffers, so splits now mostly buy painter order, which matters for
    blending and coplanar tie-breaks.
  - They cost ~22 µs each and inflate `emitted`. Hub grass goes 12 → 198 triangles;
    lava and throne splits are on front faces.
  - Option: no splits for opaque draws, or a much larger `DEPTH_SPLIT`; keep them for
    alpha/blended or textured draws if the PNGs show trouble.
  - Fog is off in castle64 (`DEFAULT_LIGHT`), but per-vertex fog does need splits.
    Keep them when `light.fog.enabled()`.
  - Check the 3DS: untextured triangles there pass w = 1.
- **3b. Divisions to reciprocals.** One `1/z` per projected vertex instead of three
  divisions, and `x * inv` in `finish`/fog.
- **3c. Model-space lighting.**
  - For rotation plus uniform-scale model matrices, transform `ldir` into model space
    once per draw: `diffuse = n_local · l_local / s`. That skips the per-vertex normal
    transform, `sqrt` and division.
  - Blocks use non-uniform scale with axis-aligned normals. Either special-case them or
    fall back to the current path.
- **3d. BACKLOG (d).** Replace `Caps::N64.max_tris_per_frame` with a cost budget from
  `N64_COST`, for strict mode, `model inspect` and the editor HUD.

Each of these is its own commit with re-blessed goldens and a reviewed PNG diff.

## Phase 4 — RSP T&L: spike, then ADR, then code (L)

Only the hub should still need this after Phases 1–3. Re-measure before starting.

1. **Spike, about a day, on a throwaway branch.** Answer:
   - Can the pin's `rspq` plus a custom overlay transform, light and clip Trino's vertex
     format? The guide is `rsp_rdpq_tri.inc` in the image's include dir.
   - Alternatively, what does moving the pin to libdragon **preview** cost, for tiny3d?
     Rebuild `trino-n64`, then rerun every N64 gate; preview's rdpq/display/audio may
     differ.
   - What is the hub frame with the hub's draws on the RSP?
2. **ADR before code.** The N64 path would stop running `trino_core::tessellate`. The
   ADR must redefine:
   - "ROM work counters == PC counters": they would be produced by the core on PC only;
   - the pixel-exact N64 golden: the RSP uses fixed point, so it becomes a new bless with
     `n64_parity` as the tolerance check;
   - the determinism promise of ADR 0003.
3. **Feature coverage.** Each feature is covered on the RSP or falls back to the CPU path
   per draw:
   - near and guard-band clipping;
   - DEPTH_SPLIT, if still alive;
   - back faces and double-sided;
   - `Gouraud`/`Flat`/`Unlit`;
   - tint, emissive, fog, colored light;
   - ≤ 32-joint rigid skeletons, SKW2 smooth skinning;
   - textured draws;
   - per-draw `TnlStats`.
4. Keep the CPU path as the reference and the fallback; the PC and 3DS keep it.

## Checklist

Edit this file as you go.

- [x] Phase 0 micro scenes, numbers in the skill, decision written here
  - Visible-draw setup (fit `d`) = **280 µs**; culled floor = 33 µs; outside the T&L
    timer 35–51 µs per draw. The setup is ≥ 150 µs, so **1a–1c pay first** (the
    decision used `d`, not B/64: B returns before `Setup` and the scratch init).
  - Per vertex 28 µs Gouraud / 33 µs Unlit (2 600–3 100 cycles): far above 1000
    cycles, so 1d–1e are worth doing before Phase 4.
  - Sort 2.4 µs and submit 22–24 µs per emitted triangle: 1f matters, 1g doesn't
    (sort is ≤ 4 ms in every scene).
  - `Transform3::matrix` 12.5 µs per draw, zero rotation or yaw: 1h is worth at
    most ~0.4 ms per scene.
  - New baseline (Phase 0 ROM, blessed): hub 79.6, green-hills 39.1, lava-land 51.4,
    sky-tower 32.8, throne 44.5 ms. That is +2…+5% from layout alone: the micro code
    adds ROM, and the castle64 code is unchanged.
- Phase 1:
  - [x] 1a per-frame setup — kept: culled draw 33 → 16 µs, frame neutral (±2%, layout)
  - [x] 1b scratch — kept: frame −12…−15% in all five scenes, visible setup 248 → 58 µs
  - [x] 1c parse once — kept: `game` −13…−20% in all five scenes; T&L +10% from layout
    (unchanged code), frame neutral
  - [x] 1d fetch — kept: frame −5…−14%, T&L −10…−20%; Gouraud vertex 21.7 → 15.8 µs.
    No byte swap at register: the byte assembly is ~100 of ~1 500 cycles per vertex
  - [x] 1e divisions — kept: frame −8…−14%, T&L −12…−20%; `div.s` in the vertex closure
    15 → 3
  - [x] 1f submit — reverted: one FFI call per batch plus the `/255` table made submit
    +3…+6%. Submit is bound by the 26 uncached rspq words per triangle, not by the
    conversion math: a throwaway shim that skipped `rdpq_triangle` measured 22.4 →
    10.0 µs per triangle, and the remaining conversion work overlaps the store drain. Packing `RDPQ_CMD_TRIANGLE_DATA` ourselves would write the same
    words, so the optional step is dropped too. Fewer words per triangle means a custom
    RSP command or RSP T&L with shared vertices (Phase 4).
  - [x] 1g sort — skipped: sort ≤ 4 ms per scene, 2.4 µs per emitted triangle (Phase 0)
  - [x] 1h transform fast path — kept: `game` −15…−18% in all five scenes, frame neutral
- [x] N64_COST recalibrated, skill tables updated — 8.1 µs/vertex, 27.5/triangle,
  67.7/draw, 25.5/split, 29.5/emitted (was 16.6 / 26.1 / 278 / 22.3 / 30); mean error 6%.
  Phase 1 exit vs the Phase 0 base, frame ms: hub 79.6 → 56.8, green-hills 39.1 → 25.8,
  lava-land 51.4 → 37.5, sky-tower 32.8 → 21.8, throne 44.5 → 33.8 (−24…−34%, every
  golden identical). Micro re-check: visible-draw setup 280 → 51 µs, Gouraud vertex
  28 → 9 µs, front emitted triangle 29 µs (unchanged: now the largest T&L term), submit
  22 µs per triangle (store-bound, 1f).
- [ ] Candidate after Phase 1: group the T&L hot code in the link (a `.text` section
  ahead of the rest via a wrapped `n64.ld`). The hot set (`tessellate_with` with the
  static path inlined, the vertex closure, the emit closure, `Vec::push`, `memcpy`)
  is close to the 16 KB direct-mapped I-cache, and unchanged code moves ±10–20% per
  stage between builds (1a: `Transform3::matrix` 12.5 → 36 µs; 1c: T&L +10%).
  Decide after Phase 1.
  - Decision (Phase 1 exit): deferred to just before the Phase 4 spike. 2a and 3a are
    large enough to read through the ±5% frame noise.
  - [x] 1i, done after 2a instead: `nm` showed `tessellate_with` at 60 KB (every path and
    five clip planes inlined). Each `tessellate_joints` instance out of line plus a
    `#[cold]` `clip_frustum`: static path 13.9 KB, frame −4.5…−7.9% in all five scenes,
    goldens identical. The link-order idea stays deferred.
- Phase 2:
  - [x] 2a coins — kept: coin T&L gone (1.6–3.4 ms per scene); frame throne −8.9%,
    green-hills/lava-land −4.9%, hub/sky-tower ±1% on an unlucky layout (micro scenes,
    identical code: T&L +17…+34%). 3DS golden re-blessed (round coins); the N64 hub
    was checked by eye in a one-off framebuffer dump.
  - [x] 2b chunks — deferred, priced first. A draw's fixed cost is now ~87 µs (setup 51,
    outside the timer ~29, matrix ~7). Static non-lava blocks per level: hub 14 (17
    touching face pairs), green-hills 7, lava-land 5, sky-tower 9, throne 5 (4 pairs); the
    stages have no buried faces. Merging saves ~0.1–0.5 ms per stage and ~1–2 ms in the
    hub. No stage crosses 30 fps with it.
  - [x] 2c player — skipped: 5 → 1 draws saves ~0.35 ms. The player's ~6 ms is
    per-vertex and per-triangle work, and the merge needs an engine API change on three
    backends.
  - Phase 2 exit (after 2a and 1i): green-hills 23.3, sky-tower 20.3, throne 29.4 ms are
    under 33.3. lava-land is 34.0 ms because of its lava block: 191 depth splits
    (≈ 5 ms of T&L) and ~190 extra emitted triangles (≈ 5.6 ms of sort + submit). That
    is 3a.
- Phase 3:
  - [x] 3a split policy — kept: splits only for fogged, textured or translucent draws.
    Frame hub 53.0 → 44.0, lava-land 34.0 → 23.7, throne 29.4 → 16.3 ms. 3DS golden
    re-blessed (a few pixels); the N64 showcase golden is identical (fogged).
    `N64_COST` estimates stay within ±6%, so no refit.
  - [x] 3b reciprocals — kept: frame −2.5…−3.5% in all five scenes; every golden stayed
    byte-identical (the normalize and fog divisions are left alone)
  - [ ] 3c model-space light
  - [ ] 3d Caps cost budget
- [ ] Phase 4 spike result + ADR (or a written "not needed")
- [ ] BACKLOG P0 #1 table, ADR 0006, AGENTS.md "Current state", README, `agents sync`
