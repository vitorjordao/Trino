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

1. **N64 render throughput** (L). Measured per stage and per model since the
   2026-10-13 pass (`n64-performance` skill):
   - castle64 frame, emulated N64 ms (update + render − begin):

     | Scene | Frame (ms) | ≈ fps |
     |---|---|---|
     | hub | 76.2 | 13 |
     | green-hills | 37.3 | 27 |
     | lava-land | 49.1 | 20 |
     | sky-tower | 31.2 | 32 |
     | throne | 43.4 | 23 |

     The hub was 640 ms of render before these passes, and it is still ~2.3× from
     30 fps.
   - Calibrated cost: T&L ≈ 16.6 µs/vertex + 26.1 µs/triangle + 0.28 ms/draw +
     22.3 µs/split, plus ~30 µs per emitted triangle (`trino_core::perf::N64_COST`).
     A 30 fps frame affords roughly 300–400 triangles of CPU T&L.
   - **Execution plan: `docs/plans/n64-throughput.md`.** It orders the work: measure
     fixed costs, output-identical CPU wins, fewer draws, golden-moving changes, then an
     RSP spike. It also records the disassembly evidence. Start there.
   - Options, best first:
     - **(a)** Move vertex transform and lighting to the RSP. A small ucode, or
       tiny3d's, behind the same `draw_model` contract (ADR 0003 allows it). Of the
       levers below, only this one is the size of the gap.
     - **(b)** Fewer draws (S–M). Each costs 0.28 ms before any vertex, and the hub
       makes 34:
       - merge each level's static blocks into one baked mesh;
       - draw coins as billboards;
       - make the player one rigid-skeleton model (today it is 5 draws).
     - **(c)** DEPTH_SPLIT policy (S–M): every target z-buffers now (the 3DS too, since
       this pass), so splits mostly buy painter-order quality. They cost ~22 µs each
       and inflate emitted triangles (hub grass 12 → 198, throne castle 60 → 273).
       Relaxing them moves goldens: a deliberate renderer change.
     - **(d)** Replace `Caps::N64.max_tris_per_frame = 4000` (an RDP-era number) with
       a cost budget from `N64_COST`. Strict mode and `model inspect` then warn
       honestly.
     - **(e)** Cache transformed static geometry while the camera is still.
     - **(f)** Fixed-point T&L paths for the VR4300.
   - Judge every change on the `test n64` scene table, on identical code apart from
     the change: code layout alone moves a stage ~20%. Bless the baselines afterward.
2. **Freeze `TSCN` v1 at the first release** (S). Until then the layout may still grow.
   Afterwards, every change bumps the version and adds a migration (ADR 0005 §3).

### P1 — the next quality and reach jump

3. **Console packaging for standalone projects** (M).
   - `cargo xtask new --standalone` gives a PC host plus a portable game crate.
   - `.z64` / `.3dsx` builds still live in the engine repo, because `apps/n64` and
     `apps/3ds` link castle64.
   - Make the console apps generic over a game crate (a `trino_game!` entry macro,
     like `export_game!`), and let `xtask build n64|3ds --project <dir>` build an
     external project.
4. **Publish the engine crates** (S–M). Publish `trino-core`, `trino-game-api` and
   `trino-runner-pc` to crates.io, with a semver policy tied to `GAME_API_VERSION`
   and the format versions. Standalone projects then default to registry deps; they
   use path or git deps today.
5. **N64 depth precision** (S–M).
   - The 16-bit z with `NEAR = 0.05` z-fights coplanar decals; the showcase pond
     needed 10 cm.
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

## Engineering hygiene (ongoing)

- Keep `docs/adr/` current: every format change gets a version bump, a migration and
  an ADR line.
- Console performance changes cite the `cargo xtask test n64` scene table (emulated N64
  time) and bless `tests/perf/` on purpose. Host benches stay as regression guards only.

---

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
