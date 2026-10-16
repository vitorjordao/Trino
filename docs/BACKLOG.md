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
   bottom screen's own pixels (`pointer_screen`), simulated on PC. The typed API and
   3D on the bottom screen followed (2026-10-15, see Done below).
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
13. **Hardware deploys, first run on real devices** (S). `run n64 --device sc64|everdrive`
    and `run 3ds --device network` (`docs/HARDWARE.md`) are tested with stub loaders
    only. Run each on a physical SummerCart64, EverDrive 64 X7 and 3DS. Confirm three
    things: the USB/3dslink log reaches the terminal; Luma3DS break-on-start (port
    4003) catches a Homebrew-Launcher `.3dsx`; the USB log doesn't skew the
    self-test's frame times. Then take P0 1(a) (RDP time on real hardware) from those
    runs. A gdb path for the N64 (UNFLoader `-g` with libdragon's GDB support) is not
    wired yet.

13. **Point lights on the N64 RSP** (M). A draw a local light reaches runs the CPU T&L
    (ADR 0010 §4): the throne room's star went from 0.14 ms to 1.3 ms. Add one or two
    point lights to `rsp_trino.S` (per-vertex distance, `vrsq`, falloff), behind the
    oracle and a CPU-vs-RSP pixel check, then drop `RspReject::LocalLight` for them.
14. **Lights in scenes and the editor** (S–M). A `light` component (point/spot, color,
    radius) in scene files, codegen and TSCN, gizmos and a preview in the Scene view
    (the showcase's lamp posts would light up).
15. **Grow the 3DS GPU path** (M, ADR 0012). Textured meshes (alpha-tested first, then
    the blended ones in painter order), fog in `tnl3d.v.pica`, rigid skeletons (joint
    matrices as uniforms), local lights per vertex in the shader. Then measure on a
    real 3DS: Azahar doesn't time the GPU.
16. **PICA200 fragment lighting on the 3DS** (M). The 3DS evaluates local lights per
    vertex today; its GPU could do it per fragment with normals passed through the
    citro3d shim (the PC's lit pipeline needs the same data).

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
14. **A redesigned 2D sprite and tile system** (L), for a future release. The 2D-only
    examples from the first phases (`examples/hello-sprite`, `examples/platformer`)
    are legacy: the engine's model is low-poly 3D with 2D drawn in the same pipeline
    (sprites, text, `fill_rect`, billboards, impostors). The new system is designed on
    top of that pipeline, under the same per-platform budgets. The legacy examples stay
    until it replaces them: the hot-reload test builds `hello-sprite`, and a PC golden
    and the editor's Level tab use `platformer`.

## Engineering hygiene (ongoing)

- Keep `docs/adr/` current: every format change gets a version bump, a migration and
  an ADR line.
- Console performance changes cite the `cargo xtask test n64` scene table (emulated N64
  time) and bless `tests/perf/` on purpose. Host benches stay as regression guards only.

---

## Done — the showcase uses both 3DS screens (2026-10-16, branch `examples/showcase-two-screens`, GitHub #20)

| Item | Evidence |
|---|---|
| `examples/showcase` keeps the scene on the top screen and draws a touch panel on the bottom (`showcase::panel`): live metrics (frame time from `dt`, `Renderer::stats`: triangles, models, sprites, the `tnl` culled/verts/clipped and `rsp_draws`/`rsp_tris` counters, zero-padded so the panel's own glyph count is stable) and five look toggles that exist in the engine (camera preset follow/high/scene, scene vs dusk light, fog on/off, `set_texture_filter` default/point/bilinear, the animation clock). Every toggle is also on a pad button (X, Y, B, Select, Start), so it works on PC native and the N64, where the panel is skipped (`on_screen(Screen::Bottom)` says no) and the top draws the same calls as before | showcase unit tests (hit-testing inside and just outside every button, toggles cycling, taps on press only, top-screen clicks ignored, pad buttons, the panel only with a second screen and the same top draws, the default look = the scene's light and the target's filter); `xtask/tests/showcase_panel.rs` on the PC's 3DS profile (`check_panel`: button fills 2 px inside every corner, the background 2 px outside every edge, no panel on the top, a FOG tap through `update`), with three mutations that must fail (a stale panel, a layout shifted by 3 px each way, the panel drawn without `on_screen`), and the N64 profile collapsing; the `showcase panel` step and the golden `showcase_bottom.png` in Azahar (blessed after review; mean 0.00 on rerun, castle64's two goldens byte-identical); the N64 golden `showcase.png` pixel-identical; the N64 stack watermark after the showcase steps 57 224 → 58 040 bytes (gate 61 440; the showcase frames grew ≤ 90 bytes, `panel::draw` is out of line and never runs there) |

Not done: an `n3ds_parity` check for this panel. The PC's 3DS profile draws scale-2 text bilinear-filtered and dimmer than Azahar's crisp glyphs (mean max-channel diff 13.9, 11.8% far pixels on this text-heavy panel, against 2.5 / 2.5% allowed); castle64's bottom screen passes only because it has little text. Fixing the PC text sampling is a backend change with its own goldens.

## Done — 3DS GPU vertex T&L, N64 text in runs, layout-robust N64 timing, hang diagnostics (2026-10-16, branch `perf/hardware-tnl`, part of GitHub #31, ADR 0012)

| Item | Evidence |
|---|---|
| `render3d::gpu_check`/`gpu_mesh_check`/`gpu_screen_matrix`/`GpuReject`, `TnlStats::gpu_draws/gpu_tris`; 3DS static meshes in linear memory and `tnl3d.v.pica` (`trino_gpu_mesh`/`trino_gpu_draw`), `N3dsRenderer::set_gpu_tnl` | `crates/core/tests/gpu_tnl.rs` (shader arithmetic = `tessellate` per vertex; refusals); `gpu tnl` self-test step in Azahar (castle64 hub: 26/26 draws, 603 triangles on the GPU, 6/960 samples differ, mean 0.56); an external project's 3DS scenes, ARM11 time per frame 19.6 → 5.3 ms and 30 → 60 fps (its largest scene 20 → 60 fps), same build with the path off/on; `occlusion` step; `perf_budget::castle64_3ds_gpu_split_matches_the_baseline` (`tests/perf/castle64_work_3ds.toml`); 3DS golden within tolerance (mean 0.58) |
| `text::GlyphIndex` (O(1) lookup below 256, all backends), no line measuring for left-aligned text; N64 `trino_text_run` (32 glyphs per call, raw rectangles at integer scales) | `text::tests::the_index_finds_the_same_glyphs_as_the_search`; N64 goldens identical; an external project over 4 paddings: UI scenes −1.7 ms, its 30 fps scenes −0.6…−1.7 ms, spread 2.6 → 0.5 ms |
| `TRINO_N64_TEXT_PAD` (layout sweep), `draws2d=` in `TRINO_FRAME` | `n64::tests::the_text_pad_is_whole_instructions_before_the_rust_code`; the sweep in ADR 0012 §3 |
| Emulator idle limit (`TRINO_EMU_IDLE`, `emulators::Watchdog`) and the N64 stall watchdog (`TRINO_STALL`) | `emulators::tests::*` (hang vs slow run vs idle off) |

Deviations from #31: the PC native profile keeps CPU T&L this round (no `draw_mesh` /
instancing API yet), and the 3DS GPU path covers static untextured meshes only
(textures, fog, rigid skeletons and local lights stay on the CPU; ADR 0012 Consequences).

## Done — comprehensive input mapping (2026-10-16, branch `input/full-coverage`, GitHub #32, ADR 0005 §8)

| Item | Evidence |
|---|---|
| `InputState::keys` is a 128-bit `KeySet` and `Key` covers the whole keyboard (79 keys; the first 30 keep their bits); `Button::L2/R2/L3/R3`; `InputState::right_stick` and `triggers`; PC: every winit key, gilrs right stick, analog triggers, LT/RT → L2/R2, stick clicks → L3/R3; N64: the C buttons also drive `right_stick` (X/Y and Z unchanged); New 3DS: ZL/ZR → L2/R2, C-stick → `right_stick`; `.input` files gain four lines and old ones still parse; `GAME_API_VERSION` 4 | core tests (every key its own bit, the old bit positions, `u32`-era expressions, new buttons); PC tests (every key code, a device-free `PadReading` through the default bindings, trigger values, keys bound to L2/R2, old 12-line `.input` files); host tests of the N64 and 3DS shim decoders; the `input` self-test step (idle shims) on PC, in Azahar and in ares (2026-10-16: the step passes, both N64 goldens and the perf ceilings hold); `--features gamepad` clippy + tests with a scratch `libudev.pc` |

Deviations: L, R and Select keep their trigger and stick-click pad bindings, so by
default LT presses L and L2. Keys bound to L2/R2 read as fully pulled triggers.
Numpad 0 and 4–9 do not alias the digits, because they are bindable. Only 1–3 alias
them, as before. Super cannot be bound, because the OS takes it.

## Done — a typed two-screen API, 3D on the 3DS bottom screen, touch routing (2026-10-15, branch `n3ds/screen-api`, GitHub #17, ADR 0005 §6)

| Item | Evidence |
|---|---|
| `trino_core::Screen` (`Top`/`Bottom`); `Renderer::on_screen` (bool: `false` = skip, on single-screen targets or back to the top after the bottom), `screen`, `screen_size`, `read_screen_pixel`; `InputState::pointer_on`; 3D on the bottom screen (per-screen T&L size on PC and 3DS, per-screen citro3d projection in the shim); the 3DS backend now refuses going back to the top like the PC; PC strict budgets and `FrameStats::tris` count both screens; castle64's bottom-screen panel (map, counters with a 3D star, touch buttons) | core tests (single-screen collapse, top-then-bottom order, `pointer_on`); the PC N3ds test (typed API, read-back of both screens, the N64 profile collapses); the `dual screen` self-test step on PC (N3ds and N64 profiles), in ares (collapses) and in Azahar (both screens probed, the bottom card through the 320×240 projection — a leaked 400-wide projection fails it, checked by mutation on PC); the new Azahar golden `castle64_hub_bottom.png` and `n3ds_parity` for the bottom screen; castle64 tests (touch buttons, top clicks ignored, the panel only with a second screen); the N64 golden pixel-identical |

Deviations: `set_screen(u8)` stays as the low-level form (not deprecated: a
deprecation would fail `-D warnings` builds of existing users). Draws after a refused
`on_screen` are not discarded by the backends: they go to the current screen, and the
result tells the caller to skip them. That keeps the N64 renderer untouched.
`GAME_API_VERSION` is not bumped for the new `Renderer` methods, like the earlier
trait additions: the host and the game dylib build against the same `trino-core`, and
a core change already needs a restart.

## Done — texture filtering per draw and a detail layer (2026-10-15, branch `textures/filter-detail`, ADR 0011)

- `ModelParams::filter` + `set_texture_filter`, `ModelParams::detail` (`DetailTexture`),
  materials, the editor's material inspector, TSCN v3 (the surface: gloss, filter,
  detail).
- N64: `trino_3d_texture_ex` (`FILTER_POINT`/`FILTER_BILINEAR`, `TILE1` detail with a
  2-cycle combiner, dropped when the pair doesn't fit TMEM). 3DS: `C3D_TexSetFilter`,
  texture unit 1 + TexEnv stage 1 (`texcoord1` from the vertex shader). PC: per-draw
  samplers, a detail binding, the N64 look's point fetch.
- Verified: `crates/platform-pc/tests/texture_filter.rs` (goldens per profile), the
  `texture filter and detail` self-test step in ares and Azahar, the showcase's pillars.
- Deviation: the PC native profile has no separate "high-resolution detail map pass"; it
  uses the same multiply as the consoles, at its own resolution.

## Done — local lights with a quality tier per platform (2026-10-15, branch `lights/local-lights`, ADR 0010)

- `LocalLight` (point, spot), `Renderer::set_local_lights` / `set_wetness`,
  `ModelParams::gloss` + material `gloss`, `Caps::light_tier` / `max_local_lights`,
  `ground_pool`. PC native per pixel (specular, wetness), 3DS and N64 per vertex
  (`render3d/lit.rs`), N64 lit draws off the RSP (`RspReject::LocalLight`).
- Verified: `crates/core/tests/local_lights.rs`, `crates/platform-pc/tests/local_lights.rs`
  (goldens per tier), the `local lights` self-test step in ares and Azahar, castle64's
  throne torches (`xtask/tests/lights_shots.rs` evidence).
- No lights: every golden and the T&L work counters are unchanged; the static T&L body
  is the size it was (0x3d04) and the micro scenes match main.
- Deviations from the issue: the N64 does not bake lights into vertex colors at stage
  prep (a mesh copy per instance would not fit RAM); it lights per vertex on the CPU or
  draws `ground_pool` decals. The 3DS uses the core's per-vertex lights, not PICA
  fragment lighting (P1 #15). Wet surfaces are specular only (no reflections of the
  scene).

## Done — each platform's frame budget in the editor and the CLI (2026-10-15, branch `editor/platform-budget`, ADR 0009 §5)

| Item | Evidence |
|---|---|
| `rsp_check`/`RspReject` (the rule a draw fails; `rsp_eligible` defined by it); `trino_scene::budget` (per-platform draws, T&L work, N64 CPU vs RSP with `N64_COST.us_per_rsp_tri`, reasons, impostors, LOD, unseen draws); `cargo xtask scene budget`; editor Game-view HUD lines, "N64 3D time" with the RSP split, Inspector scene-settings section with every reason and clickable unseen draws | the estimate's T&L work equals `PcRenderer`'s N64-profile counters on the showcase and interior scenes; a mesh behind the camera is reported on every platform; each RSP rule (translucent, fog) and PC/3DS without an RSP; variants, LOD and impostors per platform; the CLI's JSON equals the library; `rsp_check` per rule; HUD lines and the N64 row; kittest: reasons, a new rock behind the camera, its line selects it, layout at 960x600 and 1600x1000 |

## Done — particle effects as assets with caps per platform (2026-10-15, branch `editor/particles`, ADR 0009 §4)

| Item | Evidence |
|---|---|
| `*.particles.ron` + `[particles.*]` → `TPFX` per platform (`trino_core::Effect`, `ParticleSystem::from_effect`, `limit`); bakes on PC, N64 and 3DS; `cargo xtask particles new/check/inspect`; editor Assets › Particles with an inspector (fields, caps per platform, cost, live preview with Burst on each platform's cap); castle64 `particles` self-test step | RON both ways and every validation error, caps per platform through the real bake and an override file, deterministic simulation (same seed same hash, another seed another, peaks at the cap), core cap/off/determinism tests, CLI tests, the editor widget and kittest flows (file both ways), the self-test on PC, ares and Azahar (`TRINO_STEP:particles cap 24/8/16`) |

## Done — the Animation tab, clip measurements and console poses (2026-10-15, branch `editor/animation-panel`, ADR 0009 §3)

| Item | Evidence |
|---|---|
| Editor Animation tab (own preview, clips, transport, speed, crossfade + weight, measurements with warnings, grid, onion skin, palette, PC/N64/3DS looks, Use on the entity, Console pose); `Mesh::posed_position/posed_normal/joint_of_vertex`; `trino_asset_pipeline::anim` (clip metrics, pose mesh, glTF writer, `export_pose`); `model inspect --anim`, `model pose` | clips built in the tests with known answers (`animation_metrics.rs`: stride 0.4 m/s and 0.4 m per cycle, a 10 cm skate, a 3 cm dip, crossfade at 0.5 = the mean pose, pose → glTF → static TMDL with the same triangles and colors), `posed_vertices_follow_their_joint`, panel unit tests, kittest `animation_tab_previews_measures_and_exports` (GUI pose == CLI pose byte for byte, clip written into the entity, layout at both sizes), xtask CLI tests. Found: the sample robot's walk skates 15 cm and dips 3.6 cm |

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
