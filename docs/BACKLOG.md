# Trino — ranked backlog

The October 2026 backlog (18 items plus hygiene) is **done**: see the record below. The
format and ABI decisions are in `docs/adr/0005-backlog-pass-formats-and-services.md`.

The ranked list after it is what the pass itself surfaced. Ranking is by **what blocks
shipping a 3D game first**, then by cost:
- S: up to 1 day
- M: up to 1 week
- L: more than 1 week

## Next — ranked

### P0 — blocks a real N64 release

1. **N64 render throughput: grow the RSP path** (M). castle64 is CPU-light now.
   - castle64 frame, emulated N64 ms (update + render − begin, queue drained):

     | Scene | Before the plan | CPU T&L, same ROM | **RSP T&L (ADR 0007)** |
     |---|---|---|---|
     | hub | 76.1 | 38.0 | **13.1** |
     | green-hills | 37.3 | 19.4 | **7.3** |
     | lava-land | 49.1 | 21.1 | **7.3** |
     | sky-tower | 31.2 | 18.5 | **7.2** |
     | throne | 43.4 | 15.4 | **8.4** |

     The CPU T&L column comes from the same ROM as the RSP column (`cpu-*` scenes,
     with RSP draws switched off). Both are measured with the queue drained at the end
     of each frame. "Before" predates the drain, which only adds time, so the
     76.1 → 13.1 comparison is conservative.
   - Next, best first:
     - **(a)** Measure on real hardware. ares doesn't model RDP fill time, so the RDP
       may now bind first.
     - **(b)** Near/guard-band clipping on the RSP. The draws that still clip (big
       floors and walls) are the hub's remaining 4.6 ms of CPU T&L.
     - **(c)** Overlay coverage: Flat, fog, colored light, emissive, textures, rigid
       skeletons. Each lands behind the oracle and parity checks.
     - **(d)** An RSP term in `N64_COST`. The HUD, strict mode and `model inspect`
       still assume CPU T&L for every draw.
     - **(e)** Merge the hub's static blocks (2b), deferred.
2. **Freeze `TSCN` v1 at the first release** (S). Until then the layout may still grow.
   Afterwards, every change bumps the version and adds a migration (ADR 0005 §3).

### P1 — the next quality and reach jump

3. ~~**Console packaging for standalone projects**~~: **done** (2026-10-14, ADR 0008, the
   "Done — external console projects" table below).
4. **Publish the engine crates** (S–M). Publish `trino-core`, `trino-game-api` and
   `trino-runner-pc` to crates.io, with a semver policy tied to `GAME_API_VERSION`
   and the format versions. Standalone projects then default to registry deps; they
   use path or git deps today.
5. **N64 depth precision** (S–M).
   - The 16-bit z with `NEAR = 0.05` z-fights coplanar decals; the showcase pond
     needed 10 cm.
   - Workaround that works today: draw the world at a smaller scale (1/128 gives about 1 cm per depth step at 40–60 m). A per-camera depth near would
     make it unnecessary.
   - Either raise the near plane per scene, or add a material `decal` flag mapping to
     `Z_MODE_DECAL` on N64 (and a depth bias on PC/3DS).
6. **RDP antialiasing + the VI AA/divot stage** (M).
   - With rdpq AA enabled, partial coverage reaches the VI. The `AA_DEDITHER` filter
     already set in the shim would then antialias edges and run the divot filter.
   - The PC look would need coverage, for example from MSAA resolve.
   - Then extend the ares golden and the `n64_parity` bound to cover it.
7. **3DS performance numbers** (M). The 3DS renderer has no stage profile, and
   Azahar is not cycle-accurate. The work counters are identical to the N64 anyway
   (`tests/perf/castle64_work.toml`). Add a profile only once there is a timing
   source worth trusting (hardware, or Azahar's CPU clock scaled).
8. **Run the new CI jobs on GitHub** (S). The emulator jobs were verified in local
   containers that replicate each job, not on Actions. Watch the first runs:
   - ares image build time and cache;
   - lavapipe speed under the 300 s harness timeout;
   - Azahar under Xvfb (the 3DS golden matched exactly with llvmpipe in the local
     replica);
   - the N64 perf ceilings with CI's nightly (layout shifts should fit the headroom).
9. **A second N64 emulator cross-check** (M). mupen64plus with angrylion-plus, as an
   independent check of the ares golden. Skipped in this pass: ares's parallel-RDP is
   bit-exact and already the reference.

8. **RSP streaming per draw** (S). The "streaming hang" was a lost RSP wake-up
   (the RSP halted with SIG_MORE set; see AGENTS.md › Pitfalls), fixed by
   `docker/n64/rsp-lost-wakeup.patch`. Chunks of 8 stay until single-draw streaming
   is re-measured with the patch.
9. ~~**3DS bottom screen**~~ Done (2026-10-14): `Renderer::set_screen`, touch in the
   bottom screen's own pixels (`pointer_screen`), simulated on PC. Next: 3D on the
   bottom screen (a map), if a game asks.
10. **N64 Mouse** (S). Fill `InputState::pointer` from the N64 mouse peripheral
    (libdragon's joypad styles).
11. **Gamepads on Linux without libudev at build time** (S). An evdev backend, or a
    dlopen'd udev, so `gamepad` can be on by default.
12. **Palette draws on the N64 RSP** (M). A palette draw on a slotted mesh runs the
    CPU T&L (`rsp_eligible` refuses it), so a crowd of recolored characters costs
    several times what the same draws cost on the overlay (ADR 0007). Upload the
    four entries with the draw command, keep the slot per vertex in the RSP mesh copy,
    and multiply in the overlay. Gate it with the oracle and a CPU-vs-RSP pixel check.
    Measure a crowd scene in ares before and after.

### P2 — when a game asks for it

10. **Editor UI test coverage** (incremental). The kittest harness exists
    (`crates/editor/src/ui_tests.rs`). Grow it alongside panel changes:
    - inspector edits;
    - drag-and-drop;
    - the Level tab;
    - Play here.
11. **Scene streaming** (M). Load and unload TSCN chunks by portal cell or distance, for
    levels larger than RDRAM.
12. **Audio**: Doppler and occlusion through portals, only if a game needs them.
13. **Palette in authored content** (S). `ModelParams::palette` is code-only today.
    Expose it in materials (`*.material.ron`), scene entities, TSCN (version bump),
    scene codegen and the editor inspector.

## Engineering hygiene (ongoing)

- Keep `docs/adr/` current: every format change gets a version bump, a migration and
  an ADR line.
- Console performance changes cite the `cargo xtask test n64` scene table (emulated N64
  time) and bless `tests/perf/` on purpose. Host benches stay as regression guards only.

---

## Done — LOD chains with impostors, baked by the engine (2026-10-15, branch `editor/lod-impostors`, ADR 0009 §2)

| Item | Evidence |
|---|---|
| `trino_core::Impostor` (`TIMP` blob, `view_for` by yaw, `draw` = one billboard), `LodLevel::impostor`, `LodPick`, `SceneNode::lod_from`; `draw_nodes` and `EntityStore::draw_in` draw impostor levels; TSCN v2 (32-byte LOD records); scene `LodDef::impostor` + per-platform `LodAt`; `N64_COST.us_per_billboard` | `crates/core/tests/impostors.rs` (view per 45°, half-way switches, rotated objects; exactly 2 triangles; deterministic chain switches; packed records; TIMP round trip), `crates/scene/tests/lod_chain.rs`, castle64 `impostor` self-test step (table parses on the target, side view picked, 1 billboard of 2 triangles, pixel probe) |
| Impostor bake in the engine (`asset-pipeline::impostor`: own T&L + CPU raster, 4x4 supersampling, clear margins, color bleed, CI4 median cut for the N64 within TMEM, manifest entries, model hash) behind `cargo xtask impostor new/bake/check/list` and the editor's Model › Impostor | `crates/asset-pipeline/tests/impostors.rs` (K frames, margins, light-side orientation, CI4 PNG, determinism, staleness), `xtask/tests/impostor_cli.rs`, kittest `impostors_bake_from_the_inspector_and_drive_a_lod_level` (editor bake == CLI bake byte for byte) |
| Editor LOD panel (chain, per-platform starts, cost per level, preview slider), budget HUD LOD counts and impostor cost, N64 atlas in the N64 Game view | widget tests, the kittest flow, the layout audit at 960x600 and 1600x940 with both panels open |

Deviation: the bake renders vertex colors only (textured models lose their texture in the
atlas) and a model, not a prefab. Follow-ups: textured and prefab impostors; an N64
impostor as an RDP rectangle with depth (cheaper than CPU-submitted triangles).

## Done — platform variants in scenes and a layout-safe editor (2026-10-15, branch `editor/platform-variants`, ADR 0009)

| Item | Evidence |
|---|---|
| Scene v3 / prefab v2: `Entity::platforms` (`pc`, `n64`, `n3ds`: model, material, scale, visible, disabled); v2 scenes and v1 prefabs migrate; `resolve_for`; `bake_scene_for` per platform in the PC, N64 and 3DS bakes; `scene export --platform`; `scene check` flags models a platform does not bake; Inspector › Platforms (grid of three columns with the N64 cost per draw, stacked when narrow); the Game view, Play here and `TRINO_SIM` scene player show their platform's variant | `crates/scene/tests/platform_variants.rs` (RON both ways, migrations, resolve per platform, prefab parts, TSCN + codegen per platform), `asset-pipeline` per-platform scene bake, `scene_cli` export test, editor widget tests and the kittest flow `platform_variants_reach_the_game_view_and_the_file` (pick a variant in the Inspector → the N64 Game view draws the other model; save → the file has it; an external edit → the editor reloads it). Colliders stay the PC ones by design (deterministic games) |
| Editor layout audit (`layout_audit.rs`) and the fixes it found at 960x600: Inspector rows truncate and size their sliders from the space left, the Assets tabs wrap, long scene paths elide at the start (toolbar) or truncate (status bar), the dock keeps the Hierarchy ≥ 200 px and the Inspector ≥ 300 px; hooks `TRINO_EDITOR_SIZE` and `TRINO_EDITOR_INSPECT` | `ui_tests::editor_layout_is_clean_at_minimum_and_default_sizes` (whole window, 960x600 and 1600x940, scene settings / entity / Game view), screenshots at both sizes |

## Done — color slots and a per-draw palette (2026-10-15, branch `vertex-color-slots`)

| Item | Evidence |
|---|---|
| glTF `_SLOT` → TMDL `SLOT` (one `u8` per vertex, 0..=4, only when used); `Mesh::slot`/`has_slots`; `ModelParams::palette` (`Palette = [Color; 4]`) multiplies slotted vertex colors before tint and lighting on every T&L path; palette draws on slotted meshes stay off the N64 RSP and out of the draw memo; `model inspect` counts vertices per slot; Blender `paint_slots` + `export_attributes` | an oracle test (palette draw == the same mesh with pre-multiplied colors) under Gouraud, Unlit and Flat, static, rigid and smooth; no-palette draws bit-identical; every PC asset bakes byte-identical; castle64 `palette` self-test step probes three slots on PC (run in ares and Azahar on 2026-10-15: the step passes, both console goldens and the N64 perf ceilings hold); `cargo xtask blender smoke` exports `_SLOT` from Blender 5.2.2. Deviation: palette draws skip the N64 RSP (follow-up P1 #12); no material/scene support yet (P2 #13) |

## Done — external console projects (2026-10-14, branch `engine/standalone-consoles`, ADR 0008)

| Item | Evidence |
|---|---|
| `trino.toml` + `--project` for `build/run/test/watch n64|3ds`, `n64_main!`/`n3ds_main!`, `N64Host`/`N3dsHost`, project goldens + `n64_max_frame_us` | a fresh standalone scaffold passes in ares and Azahar; an external project ships through it |
| Pointer input (PC mouse + wheel, 3DS touch), PC gamepads (gilrs, feature `gamepad`) | `PcInput` tests, the window → game mapping test; gamepads type-checked for Windows/macOS |
| `Renderer::fill_rect` (all backends), N64 resident text atlas, 2D blit cache, I4/I8 sprites, font `chars`/`advance`, black transparent texels | castle64 `fill rect` self-test step on PC, ares and Azahar; goldens unchanged |
| `RSP_BAND` 4×; opt-in `set_rsp_stream` (chunks of 8); `FrameReport` with stages, top models, vblank fps | castle64 hub 13.4 → 10.6 ms; a large isometric scene 107 → 24 ms emulated (30 fps locked) |

## Done — N64 RSP T&L (2026-10-13, branch `n64-rsp-tnl`, ADR 0007)

| Item | Evidence |
|---|---|
| Milestone 1: an rspq overlay built by xtask (n64.mk's RSP recipe), drawing a triangle through `RDPQ_Triangle_Send_Async` | the self-test reads the triangle back |
| Milestone 2: `TrinoCmd_Draw` (fixed-point transform, light, projection), `rsp_eligible`/`rsp_mesh_supported`, an oracle, and the micro scenes twice in one ROM | oracle xy 0/4 px, z 2, color 1; pixels identical; micro-gouraud 23.7 → 2.7 ms |
| Milestone 3: on by default; z from 1/depth with CPU rounding; RSP draws after the batch's CPU triangles; `rsp_split` in the core; a work split in the gates; the `castle64_hub` N64 golden; hub CPU-vs-RSP parity | hub 38.0 → 13.1 ms in the same ROM; 0 of 1200 samples differ; golden 0.05/255 from the CPU path; Docker ares identical |

## Done — N64 throughput plan (2026-10-13, branch `plan/n64-throughput`)

Plan: `docs/plans/n64-throughput.md` (its checklist has each item's numbers). One
commit per item, each A/B'd on the `test n64` scene table against its parent.
Emulated N64 frame, ms, before → after: hub 76.1 → 39.4, green-hills 37.3 → 19.8,
lava-land 49.1 → 21.7, sky-tower 31.2 → 18.9, throne 43.4 → 16.0 (−39…−63%).

| Item | Evidence (frame, A/B on the parent commit) |
|---|---|
| Phase 0: `micro-*` perf scenes measure fixed costs (ungated; their work counters checked) | visible-draw setup 280 µs vs a 33 µs culled draw: per-draw cost first |
| 1a `TnlContext`: camera/light constants once per `set_camera`/`set_light` | culled draw 33 → 16 µs; frame neutral (layout) |
| 1b `TnlScratch`: T&L scratch owned by the backend, not cleared per draw | **−12…−15%** in every scene |
| 1c `MeshLayout`: meshes parsed once at register | `game` −13…−20%; frame neutral |
| 1d vertex fetch through `as_chunks` records (one bounds check per attribute) | **−5…−14%** |
| 1e tint quotients per draw, not per vertex (15 → 3 `div.s` per vertex) | **−8…−14%** |
| 1f batched submit + `/255` table | +3…+6% submit: **reverted**. Submit is 26 uncached rspq words per triangle |
| 1h no sin/cos for zero angles in `Transform3::matrix` | `game` −15…−18% |
| `N64_COST` recalibrated (twice) | scene totals within −4…+9% |
| 2a coins as billboards (output-moving: 3DS golden, work baseline) | coin T&L gone; throne −8.9%, two stages −4.9% |
| 1i `tessellate_with` was 60 KB: per-path functions + a `#[cold]` clip | **−4.5…−7.9%** |
| 2b block merge priced and deferred, 2c player merge skipped | ~0.1–0.5 ms per stage |
| 3a `split_policy`: depth splits only for fog, textures, translucency (output-moving) | **hub −17%, lava-land −30%, throne −45%** |
| 3b one reciprocal per projected vertex | −2.5…−3.5%; goldens byte-identical |
| 3c model-space lighting for rotation + scale draws (boxes under any per-axis scale) | T&L −11…−14% in four scenes; goldens byte-identical |
| 3d `Caps::max_3d_us_per_frame` (N64 50 ms) in strict mode; time instead of a triangle share in `model inspect` and the editor | tools; golden test for the strict panic |
| Phase 4 spike (desk): pin capabilities, preview divergence, ADR 0007 proposed | no microcode yet |

Every output-identical item kept the PC, N64 (pixel-exact) and 3DS goldens
byte-identical, with work counters equal. The two output-moving items (2a, 3a)
re-blessed the work baseline and the 3DS castle64 golden. Both were reviewed, and the
N64 hub, lava-land and throne frames were checked in one-off framebuffer dumps.

## Done — performance & stability pass (2026-10-13, branch `backlog/full-pass`)

It was triggered by "N64 runs at a deplorable fps, maybe the enemy" and "on 3DS the
models' vertices overlap the floor".

| Item | Evidence |
|---|---|
| Per-stage / per-model N64 profiling (`trino_core::perf`, `TRINO_PERF_SCENE/MODEL`, 5 deterministic `castle64::perf` scenes, VI-count clock check, exact µs conversion) | ares report every `test n64` |
| T&L scratch buffers initialized per draw, not per triangle | hub render 640 → 259 ms; goldens byte-identical |
| Batch sort by 8-byte keys with an explicit tie-break (all backends) | 259 → 190 ms; same order everywhere |
| newlib `mem*` instead of Rust's weak generic ones (link order, `check_mem_symbols`) | update −10%, T&L −1% (A/B on identical code) |
| **3DS depth bug**: all 3D through citro3d with per-vertex depth (`trino_3d_draw`, vertex buffer) instead of citro2d's one depth per triangle | new `occlusion` step **failed** in Azahar before, passes after (and on PC, ares) |
| castle64 declares its light every frame (it inherited the showcase's fogged light on consoles) | 3DS parity mean diff 24.5 → 0.6; hub 190 → 175 ms |
| Work gate: `perf_budget` pins every scene's T&L counters; `test n64` requires the ROM's counters to equal them | all 5 scenes equal on ares |
| Time gate: `tests/perf/castle64_n64.toml` ceilings (+20% frame, +35% stages — code layout moves a stage ~20%) | passes un-blessed |
| Calibrated `N64_COST` in `model inspect`, the editor budget HUD and the `test n64` table | ±11% per model, ±2% on 4 of 5 scenes |
| 3DS golden frame (castle64 hub, tolerance) + `n3ds_parity` (PC N3ds look vs Azahar) | local + CI replica (llvmpipe): mean diff 0.00 |
| castle64 characters and doors rebuilt in Blender (`tools/blender/castle64_models.py`, only visible faces; KayKit doorway 1068 → 75 tris) + bake fixes (COLOR_0 linear, vertex-cache order) | hub frame 174 → 88.5 ms, stages −3…−14% (same ROM code); goldens re-blessed and reviewed |
| Back faces rejected before clipping/splitting in `tessellate` | frames another −8…−17% (hub 76.2 ms, sky-tower 31.2 ms ≈ 32 fps); `emitted` and every golden identical |
| **N64 stack overflow** (present since the baked-scene step): the self-test stacked two ~22 KB `ShowcaseGame` frames past libdragon's 64 KB stack and hung at random. Each showcase step now runs out of line; `check_stack` fails the self-test above 60 KB | peak >64 KB → 47.6 KB; same ROM passed 4/4 runs (it had hung 4 times in a row before) |

Tried and reverted, because it was neutral or worse on the scenes: a per-vertex
projection cache with a no-copy fast path (±2%), compacting the hot loop with
`#[inline(never)]` (+3…+17%), and `opt-level = "s"` (worse). The enemy was not the
cause: a boar costs ~8 ms, and all game logic 0.3 ms.

## Done — October 2026 pass (branch `backlog/full-pass`, one commit per item)

| # | Item | Commit | Verified |
|---|---|---|---|
| 1 | Collision from scenes (`collider` component, `core::physics`) | 2cf6f36 | PC, ares, Azahar (showcase demo plays through) |
| 2 | Runtime entities (`EntityStore<N>`) | 2cf6f36 | PC, ares, Azahar |
| 3 | Bitmap text (`TFNT`, `draw_text`, sprite regions) | 84e1e75 | PC, ares, Azahar (`selftest::text`) |
| 4 | Blender toolkit validated (Blender 4.5 LTS, `cargo xtask blender smoke`) | 711e2bb | local Blender (not in CI) |
| 5 | Clip crossfade (`AnimRef::blend_to`) | 2124ba3 | PC unit tests, consoles type-check |
| 6 | Baked scenes (`TSCN`) + blob API + save data | a218bf7, 84e1e75 | PC, ares, Azahar (`check_baked`, `blob`, `save`) |
| 7 | Textured 3D (TXUV/TXID, all backends) | 94b2c66 | PC, ares, Azahar (quadrant pixel probes) |
| 8 | Budget HUD in the Game view | 314015c | PC unit tests + editor screenshot |
| 9 | Distance LOD (fake objects as far levels) | 31507ba | PC, consoles (TSCN round trip) |
| 10 | Billboards + particles | 9de9845 | PC, ares, Azahar (`selftest::billboards`) |
| 11 | Editor productivity (multi-select, arrange, palette, import, tabs, prefab editing) | bb4e5a7 | PC unit tests + kittest flow |
| 12 | Play here (scene runs inside the Game view) | 92d92a5 | PC + CI smoke (`TRINO_EDITOR_PLAY=here`) |
| 13 | Emulators in CI, VI dedither, emulator goldens | 32c8a3b, 063e0f2 | ares (native + Docker/lavapipe, golden pixel-exact), Azahar (native + container); GitHub run pending |
| 14 | Save data (`Platform::save_*`, framed) | 84e1e75 | PC, ares (EEPROM), Azahar (SD) |
| 15 | Positional audio, voices, stingers | 3bdb4cb | PC, ares, Azahar (`selftest::audio_checks`) |
| 16 | Smooth skinning (`SKW2`, opt-in) | 4530b41 | PC, ares, Azahar (`selftest::smooth_skin`); +3% host CPU worst case |
| 17 | Portals / cells (`core::vis`, `vis` component) | c386eb3, 3b2dd6d | PC, ares, Azahar (`selftest::portals`) |
| 18 | Standalone projects (`xtask new --standalone`, `trino-runner-pc`) | 2dd29fe, 3b2dd6d | temp-dir build/test/run (path + git source); console packaging → P1 #3 |
| — | Hygiene: N64 frame timing (`TRINO_PERF`) | 429c246 | ares |
| — | Hygiene: editor UI harness (egui_kittest 0.35) | 429c246 | PC (GPU) |
| — | Hygiene: `rust-version` workspace warning | 2dd29fe | gone |

Deviations from the original wording, on purpose:
- **Item 13.** The emulator golden uses **ares** framebuffer dumps, not mupen64plus.
  ares is the reference emulator, and its parallel-RDP is bit-exact.
- **VI stage.** Only dedither changes pixels, because coverage is always full without
  RDP AA (see P1 #6).
- **Item 18.** crates.io publishing was not done (P1 #4).
