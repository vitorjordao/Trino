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
     - **(c)** Overlay coverage: done for Flat, fog, colored light, emissive,
       double-sided, textures, rigid skeletons and water with its ripples and rings
       (GitHub #69, ADR 0018). Left: palettes, smooth skins.
     - ~~**(d)** An RSP term in `N64_COST`~~: **done** (#69, ADR 0018 §7): RSP triangles,
       water surfaces priced apart (`RspWork`), and the RSP draws' CPU side in
       `scene budget`.
     - **(e)** Merge the hub's static blocks (2b), deferred.
2. **Freeze `TSCN` v1 at the first release** (S). Until then the layout may still grow.
   Afterwards, every change bumps the version and adds a migration (ADR 0005 §3).

### P1 — the next quality and reach jump

- **Profiler follow-ups (#62, ADR 0030; #79)** (S–M each).
  - A trace from a real N64 (SummerCart64) and a real 3DS (3dslink), and the hardware's
    ISViewer/USB print cost per flush.
  - Zones on other threads (the PC's T&L workers, the audio stream, the 3DS worker core)
    with a track per thread instead of the shared ring's try-lock.
  - The rest of #79: named scopes inside the N64 renderer's hot paths with an A/B over
    layouts (`perf ab`), backend counters (memo hits, rspq words), the RSP cost report.
  - The trace flush on the N64 costs ~115 ms per 60 frames over ISViewer: a binary
    channel (USB on a flashcart) or a smaller flush per frame.

- **Shipping follow-ups (#61, ADR 0029)** (S–M each).
  - Minidumps behind a feature (`minidumper` + `crash-handler`: an out-of-process
    monitor) for crashes that aren't Rust panics (a driver, `abort`).
  - The input-replay tail in crash reports once #67 records input.
  - A first real Steam submission: the manual checklist (app 480: unlock, stats, Cloud
    round trip on two machines, rich presence, the Deck) on a machine with a client.
  - The `.cia` on a real 3DS (FBI); a `[3ds]` exheader option for apps that need more
    services or memory.
  - Epic/GOG backends behind `StoreApi`; itch.io/Newgrounds adapters for the Web.
  - A Flatpak beside the AppImage; a Windows installer (MSI/Inno) if a store asks.

- **Web target follow-ups (#47, ADR 0019)** (S–M each).
  - ~~Opus/Vorbis sounds in the packs (#53)~~: **done** (ADR 0021): Vorbis `TSN2` in
    pack v2; Solid Eleven's pack 3.07 → 2.18 MB (gzip 1.09 → 0.50 MB).
  - Web audio in an AudioWorklet (#53 follow-up): streams schedule chunks 1 s ahead on
    the main thread, so a hidden tab runs dry after 1 s. A worklet needs a second wasm
    instance and a `SharedArrayBuffer` ring (COOP/COEP).
  - ~~Lazy asset bundles (#56)~~: **done** (ADR 0022): a group's bundle downloads the
    first time the game loads it; the pack holds `boot` only.
  - `wasm-opt` on the release wasm (~4.5 MB before compression), and wasm threads
    (they need COOP/COEP from the host).
  - A real-device pass on Safari (macOS/iOS) and a phone GPU: CI runs Playwright's
    WebKit on WebGL2 only.
  - Pixel probes in the browser's `engine_checks`: no synchronous read-back, so they
    are `skipped` there (the goldens cover the pixels).
- **Quality presets follow-ups (#55, ADR 0023)** (S–M each).
  - A temporal upscaler (FSR 2-style) once the renderer has motion vectors and jitter.
  - A bench scene from the game (a project-chosen scene) for games much heavier than
    the engine's.
  - The post level takes effect with #64 (shadows read theirs since #57, ADR 0025).
  - A run on a real Steam Deck and a phone GPU (measured so far: llvmpipe, SwiftShader,
    an AMD Radeon iGPU and an RX 7800 XT on Windows).
  - Dynamic resolution's full-size clear and MSAA resolve (1–3 ms on llvmpipe with 4×
    MSAA at half scale): a partial resolve if a preset with MSAA ever turns it on.
- **Accessibility follow-ups (#60, ADR 0027)** (S–M each).
  - The PC/Web correction filter (daltonization) for content without semantic palettes:
    #64's post stack (the simulation view is review-only).
  - ~~Text scale for text a game draws itself (`draw_text` HUDs: castle64's counters,
    Solid Eleven's hand-drawn menus and match HUD)~~: **done** (#95, #86 item 8): all 18
    Solid Eleven menus and the 3DS touch screen are now UI-system screens (`examples/solid-eleven/ui/*.ui.ron`),
    and both Solid Eleven's match HUD and castle64's HUD counters scale up to 200 % text.
  - The Inspector's palette slots by semantic name (`ModelParams::palette` in authored
    content is P2 #13); the Accessibility tab edits the palettes today.
  - Plan & Party's status colors and Solid Eleven's menus on semantic palettes; speaker
    names and colors for dialogue captions; caption fonts per language with #59.
  - A ready-made Accessibility page in the core (like `GraphicsOptions`) for games that
    don't use the starter screen.
- **UI system follow-ups (#58, ADR 0024)** (S–M each).
  - ~~The rest of Solid Eleven's menus as screens~~: **done** (#95, #86 item 8). Plan &
    Party's menus remain in its own repository.
  - Text entry (a name, a save slot): an on-screen keyboard screen for the consoles.
  - Transitions and animated focus (a slide, a fade) — today a screen changes in one frame.
  - ~~Localization beyond plain strings (#59)~~: **done** (ADR 0026). ~~Text scale with
    high-contrast themes (#60)~~: **done** (ADR 0027).
  - The interface scale on the Graphics page (`DisplaySettings::ui_scale` is stored and
    applied, but only a game's own options page sets it).
  - A scissor for partly visible children in a `Scroll` (2D has none on the consoles, so
    a child is drawn whole or not at all).
- **Localization follow-ups (#59, ADR 0026)** (S–M each).
  - Plan & Party (another repository) on the engine's tables: its `pnp8` glyph set
    becomes `chars = "auto"` + `extra_chars`, its `i18n.rs` a `Localizer`.
  - Gender and grammatical case (Fluent-style selectors) as a `Select` entry: a new
    `TSTR` record flag.
  - Right-to-left scripts and shaping: the text layout is left to right.
  - Keep a table's comments when the editor or `i18n import` rewrites it (both rewrite
    the whole file today).
  - A language menu page in `trino_core::options` (the tables' `name`s), next to the
    Graphics page.
  - Solid Eleven picks its first language from `Platform::system_language()` (it starts
    in English and saves the player's choice).
- **PC GPU T&L for the draws it still rejects (#51 follow-up, ADR 0016 §11)** (M).
  Translucent draws (the painter order per triangle), water and per-pixel lit draws
  stay on the CPU path. A per-pixel lit GPU path would compute the lit pipeline's
  surface varyings in the vertex stage; translucency needs a sorted GPU path. The Web
  target (#47) reuses the GPU path as it is.

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
7. **3DS performance numbers** (M). The 3DS renderer's stages are traced since #62
   (`test 3ds --trace`), but Azahar is not cycle-accurate. The work counters are identical to the N64 anyway
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
    levels larger than RDRAM. The base is in (#56, ADR 0022 §5): cells name asset groups
    (scene v6) and `trino_core::CellGroups` streams them by visibility, verified on the
    PC's real groups. Left: TSCN chunks per cell (the scene's nodes still load whole), a
    distance trigger for exteriors, castle64's interior split into grouped rooms on the
    consoles.
12. **Audio**: Doppler and occlusion through portals, only if a game needs them. From
    #53 (ADR 0021 follow-ups): tracker music (XM/MOD; libdragon's `xm64`) on every target;
    Opus on the PC and the Web; stereo masters (mono today, stereo averaged);
    ~~captions (#60)~~ **done** per sound (ADR 0027); the 3DS audio thread's share of the second core in #56's CPU budget;
    a real-hardware listen on the N64 and the 3DS (ares and Azahar only so far); `auto`
    picking PCM16 on the PC/Web when Vorbis is bigger (Ogg's ~4 KB of headers make a
    0.05 s click 2× its PCM16 size — Solid Eleven's `se_click`).
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
15. **Scene-level ambient occlusion bake** (M). Today's baked AO is per model: each
    model occludes only itself, plus an optional ground plane. For consoles to get
    contact shadows between models, bake per static instance: a vertex-color copy per
    instance, under a RAM budget. Also put the scene's `ao` block in TSCN (a version
    bump) for PC games that load levels as data. (ADR 0014, Consequences.)
16. **Solid Eleven follow-ups** (S–M each, from issue #39).
    - Local two-player matches: the sim takes two pads already (`human: [bool; 2]`), but
      no menu offers it.
    - The N64 frame is 27.4 ms locally (eight code layouts) against its 33.3 ms gate,
      but CI's layout ran the same code ~3.6 ms slower: the sim update (7.2 ms) is the
      next thing to cut.
    - Engine: an RSP draw that misses the N64 memo spends ~110 µs before its T&L stage
      (key, mesh lookups, `rsp_check_mv`). A cheaper miss path would help every game
      with moving models.
    - The PC's SSAO over the multisampled depth costs 4.5 ms at 1080p on an AMD iGPU
      (2.7 ms single-sampled): a cheaper depth for the AO (a resolve in the scene pass)
      or a separate AO setting.
    - The console heads show the hair on the top and the back only (one 14-triangle
      wedge); the PC's styles have no console counterpart beyond dark, light and shaved.
    - A check that no pitch line runs through a player's screen box in the menus' 3D
      backdrops (that artifact came back twice).
    - Nothing has run on real hardware yet.

17. ~~**Water on the RSP: the rest**~~: **done** (#69's second part, ADR 0018 §7): the
    ripples and their rings on the overlay. `micro-river` 1.78 ms (the river's draws
    0.94 ms), `showcase-river` 26.2 ms in ares. Left: the number on a real N64 (P0 #1a).
18. **3DS water: fragment lighting and long surfaces** (S–M). A `C3D_LightEnv` with a
    Fresnel LUT for a per-pixel glint. Split long water surfaces into pieces so the
    GPU takes the ones wholly ahead of the camera; the ripple rings then need
    deduplicating (ADR 0015 §5).
19. **Shadows follow-ups** (S–M each, from #57, ADR 0025 §10 and Consequences).
    - Baked static shadows into static receivers' vertex colors (with item 15's
      per-instance bake): free at run time on the consoles.
    - Cascades for the PC map on Ultra, for long views past the shadow distance.
    - Receivers other than horizontal box tops in the editor (slopes, ramps); the API
      already takes any plane.
    - An RSP shadow command if an N64 game ever needs more than two projected shapes (a
      blob would save at most ~20 µs there).
    - Nothing has run on real hardware yet: blob cost on an N64, the stencil path on a
      3DS.

## Engineering hygiene (ongoing)

- Keep `docs/adr/` current: every format change gets a version bump, a migration and
  an ADR line.
- Console performance changes cite the `cargo xtask test n64` scene table (emulated N64
  time) and bless `tests/perf/` on purpose. Host benches stay as regression guards only.

---

## Done — stereoscopic 3D on the 3DS top screen: per-eye replay, floating HUD and UI, the Auto policy, previews on the PC and in the editor (2026-10-24, branch `feat/63-stereo`, GitHub #63, ADR 0032)

| Item | Evidence |
|---|---|
| `trino_core::stereo` (`no_std`): `StereoSettings`/`StereoPolicy`, `Eye`, `LayerDepth`, `StereoFrame::resolve`, `StereoGovernor`, `StereoStats`, `StereoCmd` + `replay`; `TnlContext::with_eye` (off-axis: zero parallax at the convergence, no vertical parallax, a zero shift is the mono context); `Renderer::set_stereo`/`stereo_eye`/`set_layer_depth`/`read_eye_pixel` and `Caps::stereo` as defaults (no `GAME_API_VERSION` bump) | 11 unit tests in `stereo.rs`, `crates/core/tests/stereo.rs` (4), `provided_renderer_methods_are_harmless_defaults` |
| 3DS: record the top screen, replay per eye; right-eye target, `gfxSet3D`, the slider, screen 2 read-back; the 2DS family has no stereo (`TRINO_HW … stereo=`) | the `stereo` self-test step in Azahar as a New 3DS and an old one (both eyes read back); `a_2ds_has_no_stereo_screen`; every 3DS golden unchanged |
| The Auto policy and per-eye cost | `the_governor_drops_after_three_slow_frames_and_retries`; Azahar with "Simulate 3DS GPU timings", every castle64 level at 60 fps in stereo, CPU per frame mono → stereo: hub 3.10 → 5.70 ms, green-hills 2.61 → 4.71, lava-land 2.77 → 5.01, sky-tower 2.55 → 4.58, throne 3.67 → 6.68 (`TRINO_FRAME:<level>-stereo`, New 3DS; Azahar times the old 3DS's CPU the same) |
| PC 3DS profile: the same replay, eye textures, `StereoView` previews (anaglyph, side by side, left, right, flip), `TRINO_STEREO`, warmed composites | `crates/platform-pc/tests/stereo.rs` (4 tests; goldens `stereo_left`, `stereo_right`, `stereo_anaglyph`; the slider down is the mono frame byte for byte), `the_stereo_step_reads_both_eyes_on_the_3ds_profile` (4 mutations fail it) |
| Scenes `environment.stereo` (codegen `STEREO`, no TSCN), UI `depth` (TUIL v3), `scene budget --stereo`, `ui shot --stereo`, `TRINO_FRAME stereo=` | `stereo_is_optional_defaulted_and_round_trips`, `codegen_exports_the_scene_stereo`, `node_depths_round_trip_through_ron_tuil_and_rust`, `tuil_v3_carries_depths_and_v2_still_loads`, `the_stereo_budget_is_two_eyes_of_the_3ds_frame`, `ui_shot_previews_node_depths_in_stereo`, `the_3ds_profile_reports_its_stereo_frames` |
| castle64 and Solid Eleven opt in (convergence on the player / the ball, HUD 6 px out) | their goldens unchanged; the football template's 3DS self-test times `match-stereo` |
| Editor: Game view stereo preview + 3D slider (3DS), Inspector › Stereo 3D, the Scene view's convergence plane | see the PR |

Deviations: scenes stay v7 (an optional field, not the issue's "v4"); `LayerDepth` is a
renderer state (`set_layer_depth`), not a parameter of every 2D call; the Auto policy
drops to mono but doesn't reduce the separation (a smaller separation costs the same).

Follow-ups: measure a stereo frame on a real New 3DS and an old 3DS (Azahar doesn't
time the clock); TSCN could carry `stereo` at its next version bump.

## Done — shader precompilation and the CPU thread budget (2026-10-24, branch `feat/101-shader-precompile`, GitHub #101, ADR 0031)

| Item | Evidence |
|---|---|
| PC/Web warm-up of every lazily built pass (`Renderer::precompile`, background workers or one pass a frame on the Web; frames wait only for a pass in flight) | `crates/platform-pc/tests/precompile.rs`: 0 pipelines in frames after `EVERYTHING` while MSAA, SSAO, the shadow map, CVD views and the render scale toggle; same pixels |
| Persistent Vulkan pipeline cache in the player's save folder | `the_pipeline_cache_round_trips_through_its_file`; heavy-shader bench: 1 110 → 14 ms on the next launch |
| Thread budget `auto` = all CPUs − 1 (≥ 1): `--threads`, `TRINO_THREADS`, `trino.toml [threads]`, the package marker | `trino_core::threads`, `xtask::threads`, `project.rs` tests |
| Parallel bakes, bundle decoding, RSP overlay and picasso assembly | `parallel_bake.rs` (byte-identical on 1/2/8 threads) |
| Consoles: nothing compiles at run time (documented, `shader precompile` self-test step answers 0 passes) | ares / Azahar self-test |

Follow-ups: `VK_EXT_graphics_pipeline_library` once wgpu exposes it; a measured
`precompile` on DX12/Metal (their own driver caches); a `TRINO_FRAME` gate on `pipes=0`
for the shipped games' smoke runs.

## Done — a unified profiler: one trace format from PC, ares, Azahar and the Web, Perfetto JSON, Tracy on the PC, the editor's Profiler (2026-10-24, branch `feat/62-unified-profiler`, GitHub #62, ADR 0030)

| Item | Evidence |
|---|---|
| `trino_core::trace` (`no_std`): `zone!`, `counter`, `frame_mark`, tracks, the ring (wraparound counted), `TTRC` packets, `TRINO_TRACE:` lines with sequence and check; compiled out without `trino-core/trace` (zero-sized `Zone`, no `Drop`); `Platform::trace_platform` (a default) | 13 unit tests in `trace.rs` (both feature states; CI lint runs the global ring's), `platform_defaults_report_no_services`, `the_trace_names_the_pc_whatever_profile_it_simulates` |
| Zones at every renderer's stage sites (N64, 3DS, wgpu) and the hosts' loops; counters per frame; the PC's GPU passes from timestamp queries | `a_traced_pc_frame_records_cpu_and_gpu_zones` (CPU + GPU on RADV), `passes_nest_in_their_frame_and_queued_frames_never_overlap`; real runs: ares (11 sessions, 1 707 events), Azahar (6 sessions, 20 185 events), the PC (240 frames, 10 560 events, cpu + gpu tracks), the N64 play loop (60 s, 1 740 frames) |
| The `trace` self-test step (nested zones around real work, a counter, frame marks, every line read back, a ring overflowing) | `the_trace_step_lines_collect_on_the_host` (a depth mutation in the encoder fails it); `TRINO_STEP:trace` + the `selftest` session decoded by `test n64`, `test 3ds` and `test web` (Chromium WebGL2/WebGPU, Firefox) |
| `trino-trace` and `cargo xtask trace collect\|report\|diff [--json]`; `test n64\|3ds\|web` write `target/traces/`; `--trace` on `run pc`, `build\|run\|test n64\|3ds\|web` | `trace_cli.rs` (5 tests, the golden Perfetto JSON of an ares log), `end_to_end.rs`, `trino-trace`'s unit tests |
| Tracy (`tracy` feature: spans, plots, frames, a GPU context) | `cargo clippy -p trino-app-pc --features tracy` in CI; captured with `tracy-capture` 0.14.1 from a 600-frame run (zones `game`, `render`, `end`, `present`, `encode`, `draw`, …, see the PR) |
| Web: the lines on the console, `performance.measure` per zone | wasm32 clippy with `trino-core/trace`; the browsers' `selftest` sessions |
| N64 hot-symbol guard (`tessellate_joints::<false, false>` ≤ 16 KB, no recorder without `--trace`) | `hot_symbols_fit_the_icache_and_zones_compile_out`; every `build n64` |
| Editor: the Profiler tab (timeline, frame flame, zones, diff, live reload), the measured N64 line in the Inspector | `profiler_tab_reads_compares_and_follows_traces`, the layout audit at 960×600 and 1600×940, 7 unit tests in `crates/editor/src/profiler.rs` |

Release builds unchanged: the 143 functions of the N64 renderer and `render3d` have the same
sizes as on `main`, `tessellate_joints::<false, false>` is 16 216 B on both, and the scene
table over the same 8 `TRINO_N64_TEXT_PAD` layouts (frame_us, mean (min–max), main → branch):
hub 12 201 (11 948–12 502) → 12 275 (11 950–12 998); green-hills 7 337 → 7 410;
lava-land 7 880 → 8 023; sky-tower 7 103 → 7 133; throne 18 244 → 18 070; micro-river
1 788 → 1 790; showcase-river 25 242 (24 866–25 854) → 25 878 (25 075–26 389): every range
overlaps (layout, the self-test's new step); no ceiling or golden moved.

Deviations (ADR 0030): the self-test ROM is not instrumented by default (the issue asked
for it): its perf scenes are the gate, so the zones go in a separate `--trace` ROM, and
every default boot checks the format through the `trace` step instead. Hardware traces are
`trace collect` on a saved terminal log, not verified on a console yet. The "estimated vs
measured" link is in the Inspector's platform budget, not the painted Game view HUD.
Zones are main-thread only (another thread's event is dropped and counted). Game zones
record nothing under hot reload (the dylib's own core).

## Done — shipping games: achievements, stats, presence and cloud on every target; PC packages, Steam, crash reports, the 3DS `.cia` (2026-10-23, branch `feat/61-shipping-pc`, GitHub #61, ADR 0029)

| Item | Evidence |
|---|---|
| `trino_core::services` (`no_std`): `AchievementId`/`StatId`, the `TACH` table (`services/achievements` from `assets/achievements.ron`, baked on every target), `AchievementState` (+ `pack`: 3 + ⌈n/8⌉ + 4 × stats bytes), `HostedServices`; `Platform::unlock`, `set_stat`, `achievements`, `restore_achievements`, `set_presence`, `cloud_sync_hint`, … as defaults (no `GAME_API_VERSION` bump) | `round_trip`, `refuses_broken_blobs`, `pack_round_trips_within_the_eeprom_budget`, `unlock_stats_thresholds_and_toasts`, `restore_merges_and_shows_no_popup`, `presence_keeps_arguments_as_text` |
| Every backend: PC `<save>.achievements` (by API name), Web `localStorage`, N64 and 3DS for the session (the game's save holds the packed state) | `services_text_round_trip_and_platform_defaults`; the `services` self-test step on PC, ares, Azahar, Chromium (WebGL2, WebGPU) and Firefox: the packed state through the real EEPROM / SD save |
| The overlay: `AchievementToast`, `AchievementList` (2D calls only); castle64 unlocks on coins, stars and stomps, Y opens the list | `toast_queues_slides_and_expires`, `list_rows_fit_and_focus_moves`, `a_coin_unlocks_an_achievement_and_y_opens_the_list`, goldens `services_{list,toast}_{n64,3ds,pc}.png`; the step's pixel probes (a mutation that drops the rows fails it) |
| `cargo xtask services check\|lock\|list\|export steam` (`--json`): ids, stats, icons, strings in every language, the shipped bit order; Steam's CSV, localization VDF, rich presence, Steam Input manifest, 64 px icons | `services_cli.rs` (4 tests, a fixture with one problem of each kind), `check_reports_each_problem`, `exports_every_language_steam_knows` |
| Steam backend (`trino-runner-pc` feature `steam`): `StoreSync` over `StoreApi` (merge both ways, forward, `StoreStats` pacing, overlay flag, Steam Input, labels); `steamworks` glue | `start_merges_both_ways_without_popups`, `pump_forwards_unlocks_stats_presence_reset_and_overlay`, `a_store_pad_folds_into_the_frame` (mock); compiled in CI (`--features steam`); a package with `--steam` starts without a client in CI |
| `cargo xtask package pc` (fat LTO, the version stamped, assets baked in, packaged mode in the runner: the user data folder): AppImage + zip (Linux), zip with the `.exe`'s icon and version info (Windows), `.app` + `.dmg` (macOS); SteamPipe and butler layouts; signing hooks | `package_cli.rs` (the E2E runs in CI's `package` job on all three OSes: smoke run outside the repo, its frame, a deliberate crash's report; Windows reads the version back), `res_holds_icons_group_and_version`, `round_trips_names_modes_and_bytes`, `icns_lists_png_entries_with_big_endian_lengths`, `marker_round_trips_and_defaults`, `user_data_dirs_per_os` |
| `trino_core::build_info!()`; the starter's About screen | `stamps_the_calling_crate`, `ui_about_*` goldens |
| Crash reports: panic hook (identity, OS, GPU, backtrace, the last 200 log lines), consent, upload | `a_panic_writes_a_report_with_the_log` (a child process), `uploads_to_a_local_collector`, `consent_round_trips_and_gates_the_upload`; the starter's crash question (`ui_crash_*` goldens, `the_crash_question_comes_first_and_answers`) |
| `cargo xtask deck check` (controller, glyphs, Steam Input, text ≥ 9 px at 1280 × 800, profile, defaults, launcher, suspend) | `deck_check_passes_on_castle64` |
| `cargo xtask package 3ds --cia` (+ the `.cxi`): bannertool, makerom, ctrtool pinned | the CI `n3ds` job builds it; `n3ds-emulator` boots the `.cxi` through `test 3ds` (`TRINO_3DS_APP`): the whole self-test and the 3DS goldens pass on the installable title (checked locally too) |
| `trino.toml` `[pc]` packaging keys, `[steam]`, `[itch]`, `[3ds]` cia keys; `cargo xtask package config [set]` | `shipping_keys_parse_with_defaults_and_checks`, `set_keeps_comments_and_reads_back`, `set_refuses_what_package_would` |
| Editor: the Project tab (Achievements: the table, fields, stats, presence, checks, lock, Steam export, the overlay preview with a test unlock; Packaging: `trino.toml` keys, Package and Deck check) | `project_tab_edits_achievements_and_packaging_as_text`, `edits_respect_the_lock_and_round_trip`, the layout audit at 960×600 and 1600×940 |

Deviations (ADR 0029): the issue's `[pc.package]` table would collide with `[pc]
package` (the crate's name), so the packaging keys are in `[pc]`; consoles keep
achievements in the game's save (a reserved EEPROM region would move the save layout);
the editor's two pages are one **Project** tab (the dock's bars have no room for two
more); the "test-unlock in Play mode" is a test unlock in the tab's overlay preview (Play
runs the game in its own process); Steam is verified against a mock in CI, and by hand
against app 480 only where a Steam client runs (not on the machine that built this);
macOS and Windows packages are built and run by CI only; no minidumps (follow-up), no
input-replay tail (#67 doesn't exist yet), no Flatpak.

## Done — accessibility on every target: color vision through palettes, captions, text scale and high contrast, motion and flash guards (2026-10-19, branch `feat/60-accessibility`, GitHub #60, ADR 0027)

| Item | Evidence |
|---|---|
| `trino_core::access` (`no_std`, zero deps): `AccessSettings` (+ `pack`/`unpack`, text), `HostedAccess`, `Platform::access`/`set_access` (defaults, no `GAME_API_VERSION` bump); PC `<save>.access` + `TRINO_ACCESS`, Web `localStorage` + `?access`, consoles for the session | `pack_round_trips_every_mode_and_refuses_blank_bytes`, `accessibility_settings_through_the_platform_and_the_file`, `access_settings_load_and_persist`, `platform_defaults_report_no_services`; the `accessibility` step's round trip on ares, Azahar and the browsers |
| Color-vision math (Machado 2009, sRGB tables, WCAG contrast, CIE76 ΔE) | `simulation_matches_the_reference_values` (an independent float64 implementation, ±1) and three more |
| Semantic palettes (`assets/palettes.ron`), icons (`cvd_icon` on sprites and UI nodes, TUIL v2), the high-contrast theme file, all in the `access/data` (`TACC`) blob on every target | `tacc_round_trips_and_answers_per_mode`, `damaged_tacc_blobs_are_refused`, `palettes_and_the_theme_bake_on_every_target_and_read_back`, `tuil_v2_carries_cvd_icons_and_v1_still_loads`, `cvd_icons_reach_the_bake_and_the_export` |
| The palette swap: the same draw with the mode's palette, every target | `palette_swap_and_simulation_match_their_goldens` (`access_palettes.png`, `access_cvd_deuteranopia.png`); the step's palette probe on ares and Azahar |
| Captions: manifest fields, the queue, `Captions::wrap` forwarding every `Audio` call, the box (3DS touch screen optional) | `the_caption_wrapper_forwards_every_call_and_captions_what_plays`, `captions_follow_the_mode_priority_and_time`, `captions_draw_a_readable_box_where_the_style_puts_it`; the step's caption box read back (on the touch screen on Azahar) |
| UI text scale (whole glyph pixels), high contrast, icons; every existing golden unchanged | `the_ui_scales_text_in_whole_pixels_and_keeps_the_default_draws`, `high_contrast_and_color_vision_icons_change_only_the_look`, the UI goldens |
| Motion, flashes, held input: `CameraFx`, `ScreenFlash`, `FlashMonitor` (strict-mode warning), `HoldToggle` | `the_flash_guard_holds_a_synthetic_strobe_to_three_a_second`, `strict_mode_finds_a_raw_strobe`, `camera_effects_damp_under_reduced_motion_and_fade_out`, `hold_to_toggle_latches_a_held_action` |
| High-contrast outlines on 3D objects: `Outline`, `draw_outlined` (a second, unlit draw just behind; no backend code); castle64's player and boars, Solid Eleven's ball | `an_outline_is_a_larger_unlit_copy_behind_the_object`, `the_copy_lies_behind_what_it_copies_and_rings_it_by_the_pixels`; the `accessibility` step's outline probe on the PC profiles, ares, Azahar and the browsers |
| Solid Eleven's texts checked against their boxes: the menu crawler finds each text's box (the innermost panel, button or row drawn before it) and fails on ink outside it, in every language, at four sizes and at 100 % and 200 % text, for every club; the overflows it found fixed (the Portuguese club buttons, the longest club names on the title and quick match cards, the Controls chips at 1080p); the pause menu takes the player's text scale | `every_menu_state_draws_cleanly_in_every_language_and_size` (three mutations fail it: a narrower button, a pause menu without the settings, two lines of a name on each other); `solid_eleven_menus_fit_their_boxes` (evidence) |
| `cargo xtask access check\|shot` (`--json`); every screen at 200 % (overflow, safe area, budgets) | `access_cli.rs` (4 tests, a fixture with one problem of each kind) |
| The PC simulation view (`set_cvd_view`, `TRINO_CVD`), both screens, WebGL2 | `the_simulation_view_matches_the_cpu_simulation`, `webgl2_shaders_translate_to_glsl_es_300` |
| Editor: the Accessibility tab (simulation in every view, palettes, checklist), View › Accessibility simulation, `TRINO_EDITOR_ACCESS`, the UI tab's preview at the simulated settings | `accessibility_tab_palettes_simulation_and_checklist`, the layout audit at 960×600 and 1600×940 |
| Adopters: castle64's doors and captions, Solid Eleven's kits for every color vision and its captions, the starter Accessibility screen; the starter screens and Solid Eleven's pause menu grow with their text | `kits_stay_apart_for_every_color_vision`, `the_accessibility_screen_sets_the_platforms_settings`, `access_check_passes_on_the_projects_the_repo_ships` |

Deviations (ADR 0027): the correction filter on the PC/Web stays with #64 (the view is a
simulation for review); text scale for a game's own `draw_text` is a follow-up (Solid
Eleven's hand-drawn menus are checked against their boxes at 100 % and 200 % but don't
grow: 57 % of their texts have no room for 125 %); the semantic colors are edited in the Accessibility tab,
not the Inspector; text scale is whole glyph pixels where the UI scales by whole pixels
(100 % or 200 % on the consoles).

## Done — localization: string tables, per-language glyph subsets, plurals and locale formats (2026-10-19, branch `feat/59-localization`, GitHub #59, ADR 0026)

| Item | Evidence |
|---|---|
| `trino_core::i18n` (`no_std`, no allocation): `Localizer` (fallback chain, `tr`, `tr_fmt`, `plural`, `pick`, `best_match`, `font`, `group`), `StrKey`/`LangId`/`LangTag`, CLDR plural rules, `Locale` defaults by tag, integer-only formatting (numbers, money, dates); `Platform::system_language` and `UiStrings::font` as defaults (no `GAME_API_VERSION` bump) | `crates/core/tests/i18n.rs` (11 tests, pt-BR/en-US/es-ES fixtures), `a_languages_font_remap_reaches_layout_and_draw` |
| `TSTR` v2 (locale, plural records, font remaps, group); v1 still parses | `tstr_v2_round_trips_its_locale_fonts_and_group`, `tstr_v1_still_parses` |
| `assets/i18n/<lang>.ron` with `Plural`/`Format` entries, `plural`, `locale` (bare optional fields), `[i18n]` in the manifest, `i18n/languages`, pseudo-locales on PC/Web only | `crates/scene/src/i18n.rs` (7 tests), `crates/asset-pipeline/tests/i18n_bake.rs` (4 tests) |
| Glyph subsetting: `chars = "auto"`, `per_language` atlases (in `i18n-<lang>` groups with `groups = true`), overrides per platform, the TMEM report | `auto_fonts_bake_exactly_the_glyphs_the_languages_use`, `each_languages_table_names_its_atlas_and_group`, `the_font_report_sees_every_atlas_and_its_tmem`, `a_languages_atlas_loads_and_unloads_with_its_group` (the PC loader; the consoles' group loading is #56's `asset groups` step); the engine's `fonts/i18n`: three 2 KB N64 I4 atlases (41–49 glyphs), half of TMEM each |
| `cargo xtask i18n check\|pseudo\|export\|import\|stats\|fonts\|list --json` (missing/extra/unused keys, placeholders, plural forms, missing glyphs per target, UI overflow per language and target, TMEM) | `xtask/tests/i18n_cli.rs` (5 tests, one fixture with every kind of problem) |
| System language: PC (`sys-locale`, `TRINO_LANG`), Web (`navigator.language`, `?lang=`), 3DS (`CFGU_GetSystemLanguage` through the shim, `TRINO_LANG` boot line); none on the N64 | `the_override_wins_then_the_os_locale`, `the_query_wins_then_the_browser`, `every_cfg_language_maps_to_its_tag`; the `i18n` step's `pick` on every target |
| Self-test step `i18n` on PC, ares, Azahar and the browsers: every language loaded, keys, fallback, plurals, prices (`$1,200.00`, `R$ 1.200,00`, `1200,00 €`), the per-language atlas, pixels where the price ends; fails when the fallback or the drawn text is broken (checked by mutation) | `engine_checks_pass_on_pc`, `engine_checks_pass_on_the_pc_3ds_profile`, `engine_checks_pass_on_the_simulated_expanded_tiers`; `TRINO_STEP:i18n` in ares and Azahar |
| Editor: Localization tab (keys × languages, fallback and missing cells, add key/language, pseudo-locale, CSV, issues, glyph coverage and N64 TMEM per font), preview language shared with the UI tab and Play | `localization_tab_edits_the_tables_and_follows_their_files`, the layout audit at 960×600 and 1600×940 |
| Solid Eleven: 187 strings (the ad hoc `P1: CASA`, `PORTUGUÊS` and `N MIN` among them) in `assets/i18n/{en-US,pt-BR}.ron`, read as static code (`strings_gen.rs`); fonts `chars = "auto"` | `every_string_fits_the_font`, `the_tables_say_what_the_code_said`, the fresh-export test; 78/78 menu, match and pause screenshots (N64, 3DS and PC looks, English and Portuguese) byte-identical to `main` |

Deviations (ADR 0026): locale rules live in each language's file (with defaults by tag),
not in the manifest; `Format("…")` is a marker (any string formats); Plan & Party is
not in this repository; the editor and `i18n import` drop a table's comments; the
editor's preview language reaches the UI tab and Play (`TRINO_LANG`), not Play here or
the Scene and Game views (they draw scenes, which have no text); per-language groups are
loaded at run time by the PC test, and on the consoles through #56's generic group
loader (the engine's own `fonts/i18n` stays in boot).

## Done — shadows per tier: blobs on the N64, planar on the 3DS, a shadow map on the PC and the Web (2026-10-18, branch `feat/57-shadows`, GitHub #57, ADR 0025)

| Item | Evidence |
|---|---|
| `trino_core::shadow` (`no_std`, zero deps): `ShadowTier`, `ShadowCast` on `ModelParams` (`None`/`Blob`/`Mesh`/`Auto`), `ShadowSettings`, `ShadowPlane` receivers, the planar matrix, blob building and cutting, the map fit; `Caps::shadow_tier`, `max_planar_shadows`, `depth_step`; two `Renderer` defaults, no `GAME_API_VERSION` bump | the core unit tests in `shadow.rs` and `caps.rs`, `provided_renderer_methods_are_harmless_defaults` |
| Depth-safe placement: 4 steps of the target's own depth buffer (the RDP's 18-bit compressed z on the N64), no hand-tuned lifts | `the_blob_offset_beats_every_targets_depth_precision`, `rdp_steps_follow_the_compressed_format`, `blobs_show_at_every_distance_on_a_large_floor`; ares's `shadows golden frame` (`shadows_far.png`, 4–120 m on one floor) |
| N64: blobs after the opaque batch (`trino_3d_shadow` + `trino_tri_tex`), two projected `Mesh` casters a frame; 111 µs a blob (`N64_COST::us_per_blob`) | the `shadows` and `shadow cost` steps on ares at both RAM tiers (103–113 µs; 22 blobs ≈ 14.7 ms, the step fails past 33.3 ms) |
| 3DS: the mesh flattened by the GPU T&L, drawn once per pixel through the stencil (`trino_3d_material` bits 9–13, `trino_shadow_blob`) | the `shadows` step on Azahar (old and New 3DS), `planar_shadows_draw_once_per_pixel_on_the_3ds_and_twice_on_the_n64` |
| PC and Web: a `Depth32Float` map over the first `distance` meters, a fullscreen composite with normal offset, bias and PCF; size and taps from the quality preset, `Off` keeps planar; MSAA, render scale and WebGL2's depth sampler | `each_tier_matches_its_golden` (`shadows_native/n3ds/n64.png`), `the_map_shadows_with_msaa_and_a_render_scale`, `the_quality_preset_caps_the_native_tier`, the browsers' castle64 golden (`tests/golden/web/castle64_hub.png`, re-blessed: the map's shadow) in Chromium (WebGPU, WebGL2), Firefox and WebKit |
| `ShadowView` game/off/debug, `TRINO_SHADOWS` | `shadows_are_cleared_at_begin_frame_and_follow_the_view`, `views_parse_and_resolve` |
| Scenes v7 (`environment.shadows`, the entity `shadow` component), prefabs v5, TSCN v5, codegen `SHADOWS`/`RECEIVERS`; migrations from v6 / v4 | `shadows_are_optional_defaulted_and_round_trip`, `v6_scenes_load_as_v7_without_shadows`, `shadows_reach_the_tscn_and_the_generated_code`, `baked_scene_blob_reads_every_field` |
| Budget per tier (`FrameBudget::shadows`, the blobs' N64 time), `scene budget` text and `--json` | `shadow_casters_are_counted_per_tier_like_the_renderer` (against the PC renderer's counters on the N64 profile) |
| Editor: Inspector › Shadows, the Shadow component, each platform's tier in the Game view, the Scene view's as tier / off / debug (receivers and casters outlined), `TRINO_EDITOR_SHADOWS`, `TRINO_EDITOR_INSPECT=shadows` | `shadows_panel_components_tiers_and_view_modes`, `platform_lines_list_each_tiers_shadows`, the layout audit at 960×600 and 1600×940 |
| castle64 and Solid Eleven on engine shadows; the `c64_shadow` disc, `player_shadow.glb` and `blob.glb` gone | `the_match_matches_its_goldens_on_every_look`, `the_match_frame_cost_is_pinned`, `castle64_tnl_work_matches_the_baseline`; ares `castle64_hub.png`, the football template's `match.png` |

Deviations (ADR 0025): receivers are horizontal box tops in the editor; no cascades;
the N64 projects at most two shapes a frame (the rest cast nothing); Phases 4 (an RSP
shadow command) and 5 (baked static shadows) were evaluated and left as follow-ups.

## Done — a declarative UI system for every target (2026-10-18, branch `feat/58-ui-system`, GitHub #58, ADR 0024)

| Item | Evidence |
|---|---|
| `trino_core::ui` (`no_std`, no allocation): containers and leaves, virtual units, whole-pixel scaling, the interface scale, safe areas and `bleed`, the 3DS bottom screen, `visible_on`/`visible_if`, lazy layout; two `Renderer` defaults (`safe_area`, `ui_stats`), no `GAME_API_VERSION` bump | `crates/core/tests/ui.rs` (35 tests), `provided_renderer_methods_are_harmless_defaults` |
| Focus by the D-pad, the sticks and the C buttons with overrides and wrap; A/B, Enter/Escape; mouse hover and click; touch taps; toggles, sliders, lists, scroll; `Ui::unreachable` | the same file's input tests; the `ui layout` self-test step (pad, A, a tap on the touch screen) |
| `ui/*.ui.ron` (unknown fields are errors) → `TUIL` blobs `ui/<name>` and `assets/i18n/*.ron` → `TSTR` blobs on every target; `ScreenBlob`/`StringsBlob` follow live edits; static Rust export | `crates/scene/tests/ui.rs` (12 tests), `crates/asset-pipeline/tests/ui_bake.rs` (2 tests), `baked_blobs_are_parsed_once_and_again_when_the_platform_swaps_them` |
| `cargo xtask ui check\|shot\|export\|list --json`, one fixture per problem | `xtask/tests/ui_cli.rs` (6 tests), `xtask/tests/ui_fixtures/` |
| `Caps::max_ui_nodes`/`max_ui_draws` (N64 128/384 since #95, 3DS 128/2048, PC and Web 512/16384), strict mode | `crates/platform-pc/tests/ui.rs` (3 tests), the audit's budget errors |
| Web safe areas (CSS `env(safe-area-inset-*)` past the letterbox); the PC's `DisplaySettings::ui_scale` as the interface scale | `window_insets_become_picture_insets_past_the_letterbox`, `the_display_ui_scale_is_the_interface_scale` |
| Starter `title`, `pause`, `options` and `controls` screens in every new project, the template game's menus (music and effects volumes, the Graphics page, key rebinding) | `ui_goldens` (N64, 3DS and PC goldens; 1080p, 4K, 21:9), `standalone_project_builds_tests_and_runs`, `scaffolds_blank_and_demo_projects` |
| Editor: the UI tab (targets, hierarchy, inspector, focus order, languages, overflow, two-way file sync) | see TESTING.md › Editor |
| Solid Eleven's pause menu as a screen: three rows drawn (two before, the second asking for a substitution) | `the_pause_menu_draws_and_walks_its_three_rows`, the menu crawler |
| Self-test step `ui layout` on the PC profiles, ares, Azahar and the browsers | `the_ui_layout_step_passes_at_pc_resolutions`, `engine_checks_pass_on_pc`, `engine_checks_pass_on_the_pc_3ds_profile` |

Deviations (ADR 0024): no text entry, transitions or partial clipping yet; the templates
ship English only (their font is ASCII).

## Done — quality presets for the PC and the Web: auto-detect, dynamic resolution, frame pacing, a Graphics page (2026-10-18, branch `feat/55-quality-presets`, GitHub #55, ADR 0023)

| Item | Evidence |
|---|---|
| `QualitySettings`, Low/Medium/High/Ultra/Custom, `PresetTable`, `classify`, the console presets; five `Platform` defaults and `Renderer::lod_bias` (no `GAME_API_VERSION` bump) | `crates/core/src/quality.rs` (7 tests), `platform_defaults_report_no_services` |
| Render scale with bilinear upscale + CAS, the HUD at output resolution; High = the PC's old frame; Low and High goldens | `platform-pc/tests/quality.rs` (7 tests), `tests/golden/pc/quality_{low,high}.png` |
| Dynamic resolution (PI on log area; GPU timestamps or frame intervals; one allocation for its range, drawn in its corner) | `converges_on_the_target_and_never_oscillates_beyond_5_percent` (16.6 ms ±1 ms, scale ±5%), `dynamic_resolution_follows_the_frame_time_within_its_range`, `dynamic_resolution_steps_reuse_one_allocation_and_draw_what_a_fixed_scale_draws`; measured on an AMD Radeon iGPU at 1440p: GPU median 16.71 ms, frame median 17.28 ms at 0.59× (`quality_shots`) |
| Frame pacing and 1% lows (`TRINO_FRAME low1_us/low1_fps quality=…`) | `caps_hold_within_two_percent` (30/60/120 fps), `the_wait_learns_how_long_the_oss_sleeps_run` (a fake clock whose sleeps run up to 5× what they ask, as CI's macOS VM's do), `one_percent_lows_and_the_quality_note` |
| Auto-detect bench (≤ 12 frames per preset, 2 s budget) on the PC and, one rAF at a time, in the browser | `the_bench_picks_a_preset_within_three_seconds_and_restores_the_renderer`, `a_budget_spent_in_the_warm_up_still_times_low`; ~0.5 s on llvmpipe (Low), 648 ms on an AMD Radeon iGPU (Medium), 581 ms on an RX 7800 XT (Ultra); the browser's `quality bench` step (~0.5 s) |
| Start order (forced, saved `<save>.quality` / `localStorage`, Steam Deck → Medium @ 60, phones → Low, project default) | `the_start_follows_forced_saved_the_machine_then_the_project`, `the_steam_deck_is_recognized_by_steam_or_its_board`, `phones_and_tablets_are_mobile_and_desktops_are_not` |
| LOD bias and particle scale in the engine; the AO cap | `crates/core/tests/quality.rs` (3 tests), `the_preset_ao_caps_the_games` |
| `assets/quality.ron` + `[pc.quality]` → the `quality/presets` blob | `crates/asset-pipeline/src/quality.rs` (4 tests), `the_pc_section_takes_a_quality_table` |
| `cargo xtask quality show\|set\|reset\|check\|bench`, `run pc --quality` | `xtask/tests/quality_cli.rs` (3 tests), `bench_lines_parse`; CI smoke runs at every preset + `auto` + `quality bench --json` |
| Editor: Quality tab, the Game view's preset dropdown and dynres toggle, the budget HUD's quality line | `ui_tests::quality_tab_writes_the_presets_and_the_game_view_previews_them`, the layout audit |
| `trino_core::GraphicsOptions` (the options page), castle64 on Select | `crates/core/tests/options.rs` (4 tests), `select_opens_the_graphics_page_where_the_host_offers_presets` |
| Consoles stay fixed (Low; Medium on the New 3DS) | the `quality` self-test step on ares (both tiers) and Azahar (both models), `the_quality_step_reports_each_consoles_fixed_preset` |

Deviations (ADR 0023): `post` is a stored level only until #64 (`shadows` is read since #57); the
bench draws the engine's scene, not the game's; WebGL2 never gets MSAA; WebGPU hides the
adapter's name, so the Web reports `gpu=unknown` there.

## Done — asset groups, bundles, compression and a memory budget (2026-10-18, branch `feat/56-memory-loading`, GitHub #56, ADR 0022)

| Item | Evidence |
|---|---|
| Groups in the manifest (`groups = [...]`, `[groups.<name>]`, `boot` = every earlier build), `TBND` bundles per group and platform, deduplicated | `membership_and_defaults`, `bundles_round_trip_and_dedup`, `bundle.rs` round trip and corrupt-bundle tests |
| Compression: N64 `mkasset` levels 1–3 (+ `mksprite --compress`), LZ4 on the 3DS/PC/Web; `[groups.boot]` on the consoles (the 3DS packs `boot.tbnd`) | `a_compressed_boot_is_packed_on_the_3ds_and_compressed_in_place_on_the_n64`, `lz4.rs` tests; ares and Azahar boot castle64 and Solid Eleven compressed |
| `Platform::load_group`/`unload_group`/`group_state`/`group_blob`/`memory`, time-sliced (`Caps::load_budget_us`) on every backend; 3DS decoding on the worker core | `group.rs` (9 tests), `asset_groups_load_draw_and_unload_on_pc`; the `asset groups` self-test step in ares (both paks), Azahar (both models), three browsers |
| RAM plans per console tier (`[memory]`), strict mode, `TRINO_MEM`/`TRINO_HEAP` lines and the N64 heap watermark, `heap=` in `TRINO_FRAME` | `memory.rs` (4 tests), `strict_mode_stops_a_group_past_the_consoles_ram` |
| Arenas (`FrameArena`, `Arena`, `LevelArena`), the N64 loader's locators off the heap; load/unload soaks | `mem.rs` (4 tests); soaks of 1 000 (N64), 600 (3DS), 10 000 (PC) frames, heap back within 8 KB on the consoles |
| Scene v6: `requires`, cell groups, `CellGroups` streaming (BACKLOG P2 #11's base) | `crates/scene/tests/asset_groups.rs` (4 tests), `cell_groups_follow_the_view_and_linger`, `cells_stream_their_groups_on_pc` |
| `cargo xtask assets report\|check\|group` (`--json`) | `xtask/tests/assets_cli.rs` (3 tests) |
| Editor: group chips and drops in Assets, scene `Requires` with problems and budget, a cell's `Groups`, the Memory tab | `ui_tests::asset_groups_chips_requires_cells_and_memory`, the layout audit |

Sizes with boot compressed (N64 level 1, 3DS LZ4):

| | Before | After | |
|---|---|---|---|
| castle64 N64 ROM / DFS | 1 589 248 / 474 365 B | 1 441 792 / 326 296 B | −9.3% / −31.2% |
| castle64 3DS `.3dsx` / RomFS | 1 299 988 / 494 863 B | 1 141 400 / 342 837 B | −12.2% / −30.7% |
| Solid Eleven N64 ROM / DFS | 1 638 400 / 522 988 B | 1 523 712 / 411 759 B | −7.0% / −21.3% |
| Solid Eleven 3DS `.3dsx` / RomFS | 1 474 356 / 531 202 B | 1 343 416 / 411 921 B | −8.9% / −22.5% |

Boot RAM and the heap's peak are unchanged (castle64: 534 112 B of boot assets, peak
2 144 584 B with the Expansion Pak).

Deviations (ADR 0022): no zstd (LZ4 only off the N64); the PC and the Web keep `boot`
uncompressed; PC/Web decoding stays on the main thread; live reload of a grouped asset
waits for its group's next load; the Game view's budget overlay has no RAM line yet (the
Memory tab has the numbers).

## Done — audio pipeline: a codec per platform, streaming, buses and ducking (2026-10-18, branch `feat/53-audio-pipeline`, GitHub #53, ADR 0021)

| Item | Evidence |
|---|---|
| Codecs per platform from one master, `auto` + manifest overrides (`codec`, `quality`, `stream`, `sample_rate`, `bus`): N64 VADPCM (Trino's encoder) / Opus, 3DS DSP-ADPCM, PC/Web Vorbis, PCM16 everywhere | `audio_codecs` (SNR floors on four fixtures), `vadpcm_decodes_like_libdragon`, `lewton_matches_libvorbis`, `n64::audio_bakes_to_wav64_per_codec_with_whole_block_loops`, `n3ds::sounds_bake_to_t3au_with_the_manifests_codec` |
| Formats: `TSN2` (PC/Web, `TSND` migrates), `T3AU` (3DS), `wav64` written by the engine (N64), `TMIX` (the mix), Web `TPAK` v2 | `sound_blobs_round_trip_and_reject_damage`, `mixer_config_round_trips_and_rejects_damage`, `version_1_sounds_migrate_to_tsn2`, `written_wav64s_round_trip` |
| Streaming on every target, sample-exact loops; the 3DS refill thread on #54's second core (core 2 New, core 1 old) above the worker's jobs | `a_ten_minute_stream_plays_with_bounded_ram` (317 KB heap growth vs 115 MB), `streamed_vorbis_loops_without_a_gap`, Web `vorbis_decodes_like_the_pc_whole_and_in_stream_chunks`; Azahar `stream_core=` equal to `worker::core()` on both models; ares/Azahar/browsers: `TRINO_AUDIO: … underruns=0` |
| Buses, mutes, duck rules in the core (`BusMixer`), mixed by every backend; the project mix (`assets/audio.toml`); the player's volumes persisted (PC file, Web `localStorage`, 4-byte `BusSettings` for console saves) | `audio_buses.rs` (4 tests), `bus_volumes_persist_per_game`, `settings_pack_into_four_bytes`; the `audio codecs` self-test step on PC, ares, Azahar (New and Old 3DS) and four browser runs |
| Measured N64 cost: `N64_COST.audio_us` (504 µs/frame idle, +260 µs per 44.1 kHz VADPCM voice, ~4.6 ms Opus; 8 layouts) | `audio_estimates_match_the_measured_cases`; ares `TRINO_AUDIO_COST:` lines |
| `cargo xtask audio inspect\|report\|preview\|mix` (`--json`) | `xtask/tests/audio_cli.rs` (5 tests) |
| Editor: Assets › Audio inspector (per-platform codec/rate, SNR, memory, waveform + noise, preview player), Mixer tab, audio budget lines + HUD | `ui_tests::audio_inspector_and_mixer_write_the_text_files`, the layout audit, `editor/src/audio.rs` tests |

Sizes (castle64's 9 original sounds; Solid Eleven's 10):

| | Before (main) | After | |
|---|---|---|---|
| PC, castle64 | 1 355 740 B (f32) | 153 893 B (Vorbis) | −88.6% |
| PC, Solid Eleven | 2 288 904 B | 252 619 B | −89.0% |
| Web pack audio, castle64 / Solid Eleven | 677 924 / 1 144 512 B | 153 893 / 252 619 B | −77% / −78% |
| 3DS RomFS, castle64 / Solid Eleven | 677 924 / 1 144 512 B | 194 316 / 327 856 B | −71% each |
| 3DS linear memory at boot, castle64 / Solid Eleven | 677 816 / 1 144 392 B | ~96 478 / 62 588 B (+24 576 per playing stream) | −86% / −95% |
| N64 ROM audio, castle64 | 192 708 B (VADPCM, 7.6 dB on the theme) | 192 708 B (VADPCM, 30.8 dB) | same bytes, +23 dB |

Found and fixed on the way: 3DS audio never played in Azahar (ndsp had no DSP firmware;
the old `audio_checks` only checked a buffer was queued), and the N64 ignored `looped`.

Deviations (ADR 0021): Vorbis q6 measures 27–41 dB of waveform SNR on the fixtures (the
test floor is 26 dB at q6, 29.5 at q8); the Web plays Vorbis, not Opus; N64 Opus is
opt-in and only decodes on the console; mono only; `BusSettings` is 4 bytes, not 2; the
N64 castle64 ROM grows 1 376 256 → 1 507 328 B (libopus for the self-test's Opus track
and three self-test sounds).

## Done — console hardware tiers: the Expansion Pak and the New 3DS (2026-10-18, branch `feat/54-hw-tiers`, GitHub #54, ADR 0020)

| Item | Evidence |
|---|---|
| `HwTier { Base, Expanded }`; `Caps::N64_EXPANDED` (8 MB) and `Caps::N3DS_NEW` (2 cores, 804 MHz, crowds and particles ×2); `Caps` gains `ram_bytes`, `cpu_cores`, `cpu_mhz`, `hw_tier`, the two scales, `with_tier`/`base`; `Platform::hw_tier` (default `Base`, no API bump) | core unit tests (the tables, `with_tier` both ways, the scales); `platform_save` (the default is `Base`) |
| Detection at boot, logged first: N64 `get_memory_size()` (the Pak at 8 MB); 3DS `APT_CheckNew3DS` + `osSetSpeedupEnable`, opted out by `[3ds] speedup = false` / `--speedup off` (a RomFS `no_speedup`) | host tests of both `hw.rs` decoders; every ares and Azahar boot checks `TRINO_HW` against the switch (`hwtier::the_n64_line_must_match_the_expansion_pak`, `the_3ds_line_must_match_the_console_and_the_speedup`) |
| `trino_platform_3ds::worker`: one worker thread on core 2 (New 3DS) or core 1 at 30% (old 3DS), a 32-slot job ring, jobs inline when there is no worker | `worker` host tests; the app's `worker` step (8 jobs checked against the main thread): core 2 in 6.0 ms, core 1 in 19.9 ms (Azahar) |
| Content keys `n64_expanded` / `n3ds_new` in scenes (v5), prefabs (v4), particles (v3), LOD starts and manifest `platforms`, falling back to their console; tier twins in the console index (`<id>.x.blob`, a 4th `base`/`expanded` column), loaded by `asset::loads_on` | `platform_variants::tier_variants_round_trip_migrate_and_fall_back_to_their_console`, `lod_chain::a_tier_starts_a_level_at_its_own_distance_or_its_consoles`, `asset-pipeline/tests/tiers.rs`, `tier` and `asset` unit tests; every existing ROM and app bakes byte-identical blobs |
| Self-test `hw tier` step: the probe scene's card, the tier sparks' cap and a tier-only model must all be the booted tier's | ares with and without the Pak, Azahar as a New and an old 3DS (CI boots all four); the PC suite on `n64x` / `3dsnew` (`engine_checks_pass_on_the_simulated_expanded_tiers`) |
| PC `TRINO_SIM=n64x` / `3dsnew`: the console's look with the tier's `Caps` and `hw_tier` | `sim::tier_values_give_their_consoles_look_and_their_own_caps`, `hw_tier_follows_the_simulated_caps`, the runner's `tier_from_env` |
| xtask: `test n64 --expansion-pak` and `test 3ds --new3ds` (`on`, `off`, `both`), `--speedup on` / `off`; `scene budget` and `export --platform n64_expanded` / `n3ds_new` (RAM, cores, scales); `particles check` and `inspect` per tier | `cli::hardware_tier_flags_go_with_their_console`, `scene_budget_cli_estimates_the_hardware_tiers`, `scene_export_applies_the_tier_variants_over_the_consoles`, `particles_cli` |
| Editor: **+Pak** and **New** in the Game view (`TRINO_EDITOR_GAME=n64x` / `3dsnew`), a base-vs-expanded HUD with the RAM and hardware lines, the shown console's platform line on its tier; tier columns in Inspector › Platforms, LOD and particle caps (shown under a Tiers switch, starting from their console) | `ui_tests::new_3ds_tier_variant_reaches_the_game_view_the_file_and_the_budget`, the layout audit at 960×600 and 1600×940 with the tier columns and views, 4 widget tests, `budget::tier_tables_put_a_consoles_two_tiers_side_by_side`, `budget::platform_lines_follow_the_game_views_tier` |
| PC strict mode: the loaded models against the tier's `ram_bytes` | `render_e2e::n64_look_and_strict_mode_budgets` (5 MB of models panic on the N64, pass with the Pak) |

Measured (ADR 0020 §7):
- N64, castle64 in ares, one ROM, emulated µs per frame with → without the Pak: hub
  12 332 → 12 211, green-hills 7 449 → 7 690, lava-land 8 076 → 8 262, sky-tower
  7 366 → 7 379, throne 18 573 → 18 438, showcase-river 25 000 → 25 470. Layout noise:
  the Pak adds RAM, not speed.
- 3DS, Azahar as a New 3DS, `--accurate-timing`, CPU µs per frame with the speedup on →
  off: castle64 hub 1 703 → 1 703, throne 2 262 → 2 262; Solid Eleven's match
  5 424 → 5 356 (same hash `d51e91bf`). Azahar ignores the speedup's clock.

Left: the speedup and the Pak on real hardware (`--device`, P0 #1a's session).

## Done — the Web target: WebGPU with a WebGL2 fallback, KTX2 textures, browser CI (2026-10-18, branch `feat/47-web-target`, GitHub #47 and #52 phase 5, ADR 0019)

| Item | Evidence |
|---|---|
| `trino-render-wgpu`: the PC renderer (GPU T&L of #51 included) shared by the PC and the browser; `platform-pc` re-exports it | `dep_graph`, every PC golden unchanged, `web_parity` (PC frame vs browser golden: mean 0.15 / 0.06 / 0.14) |
| `trino-platform-web` + `apps/web`: canvas, input (keyboard, pointer lock, gamepad), Web Audio, `localStorage` saves, packs (`TPAK`), fullscreen; castle64, showcase and Solid Eleven by `?game=` | the browser self-test (`?selftest`): engine checks, audio, castle64 bot star, `TRINO_SE_HASH` equal to the native one |
| WebGL2: no MSAA, depth through `depth_as_float` + a nearest sampler, `depth_blit` (GL can't copy depth) | `webgl2_shaders_translate_to_glsl_es_300`, `depth_reads_go_through_the_nearest_sampler`, the WebGL2 goldens |
| KTX2 (ETC1S/UASTC) baked with Basis Universal, `auto` by download size, transcoded in the browser to BC/ETC2/ASTC/RGBA8 (#52 phase 5) | `ktx2` unit tests, `cli::assets_web_bakes_ktx2_textures`, the self-test's `ktx2 transcode` (bit-exact against the native transcoder) |
| `cargo xtask build/run/test/assets web`, Playwright harness: Chromium WebGL2 + WebGPU, Firefox, WebKit; goldens `tests/golden/web/` | the CI `web` job; locally all four runs pass, goldens within 0.12/255 |
| Data: `Target::Web` (scene v4, prefab v3, particles v2) falling back to the PC; `Caps::WEB`; the Web budget (calls, pipelines, switches) in `scene budget` and the editor | `platform_variants::the_web_resolves_like_the_pc_unless_it_has_its_own_variant`, `scene_budget::the_web_warns_past_a_phones_budget` |
| Editor: Web profile and canvas presets in the Game view, Web column in Platforms/LOD/texture inspector, Run in Browser / Export Web Package | `ui_tests::platform_variants_reach_the_game_view_and_the_file`, `texture_inspector_formats_quality_and_errors` |
| `trino.toml` `targets = [..., "web"]` and `[web]`; the standalone template's `web/` host | `project::the_web_section_names_the_host_canvas_and_title`; a standalone castle64 project passes `test web` (CI) |

Packs: castle64 + showcase 944 KB (33 KTX2 textures), Solid Eleven 3.07 MB. Release wasm
5.2 MB with the self-test (4.5 MB standalone), before compression. 600 frames headless,
960×540: Chromium WebGPU (real GPU) 57 fps on all three games (rAF-paced); Firefox 41 fps
(its headless rAF pacing); WebKit 65 fps; Chromium WebGL2 on SwiftShader (a CPU
rasterizer) 59 fps showcase, 22 castle64, 18 Solid Eleven.

## Done — N64 water at budget: ripples and rings on the RSP, the river at 1.2 ms, the showcase at 30 fps (2026-10-18, GitHub #69 second part, ADR 0018 §7)

| Item | Evidence |
|---|---|
| The ripple sum per vertex and the ripple rings on `rsp_trino_water`; two vertices per pass; the block split into a static part (per memoized draw) and a frame part (per material) | the water oracles (`micro::water_checks`: swell, ripple), `showcase_river` N64 golden re-blessed, `perf_budget` counters (ROM = PC: `micro-river` 0 CPU triangles, 92 on the RSP) |
| Streaming starts the RSP (`trino_rsp_flush`); the water texture stays in TMEM; arena lines claimed without a fill; the water memo evicts stale entries | `micro-river` 10.48 → **1.78 ms** (T&L 6.39 → **0.94**, 0.88–0.98), `showcase-river` 75.17 → **26.21 ms** (8 paddings, ares) |
| Gates at the issue's budgets: `perf::BUDGETS` caps `micro-river` `tnl_us` at 1 200 and `showcase-river` `frame_us` at 33 300 | `tests/perf/castle64_n64.toml`, `perf::tests::budgets_cap_the_ceilings` |
| Phase 4: `N64_COST` prices water on the RSP (`us_per_rsp_water_tri`, `us_per_rsp_water_draw`, `RspWork`); `scene budget` counts the RSP draws' CPU side | showcase estimate 25.8 ms vs ares 26.2 (was 25.4 vs 75); `scene_budget` tests |
| Left | a measurement on a real N64 (ares doesn't time the RDP; P0 #1a) |

## Done — N64 RSP coverage: fog, colored light, textures, rigid skeletons and water (2026-10-18, branch `perf/69-n64-water`, GitHub #69 phases 0–3, ADR 0018)

- **Phase 0:** the `micro-river` perf scene (the river alone), gated on the PC (work)
  and in ares (time).
- **Phase 2:** `rsp_trino_color` (colored light, emissive, double-sided, per-vertex fog),
  flat copies for Flat shading, rigid skeletons as a command per joint, and
  `rsp_trino_tex` (textured triangles). `rsp_plan` is the one rule every predictor uses.
- **Phase 1:** the showcase terrain in 52 camera-following tiles, the river in pieces,
  one ring per ripple, lighter props.
- **Phase 3:** `rsp_trino_water` (swell, displacement, normal, Fresnel, glint, light,
  fog per vertex), with an oracle and a CPU-vs-RSP pixel check.
- **Results (ares, 8 paddings):** `micro-river` 19.6 → 10.5 ms, `showcase-river`
  286.6 → 75.2 ms (0/46 → 85/113 draws on the RSP, 738 → 0 splits); castle64
  unchanged. The 3DS showcase also moved 11 → 51 draws to the PICA200 (the tiles).
- **Not met:** #69's 1.2 ms river and 33.3 ms showcase. Left: ripples and rings on the
  overlay (P2 #17), RSP triangle throughput, per-draw CPU, the RSP cost term and real
  hardware (P0 #1a, #1d).

## Done — texture formats, phases 2–4: `auto`, PC BCn, the editor's texture inspector (2026-10-18, branch `feat/52-texture-formats-phases-2-5`, GitHub #52, ADR 0017)

| Item | Evidence |
|---|---|
| `auto` per platform (default on PC and 3DS), `quality` (30/35/40 dB) and `min_psnr`; an explicit format below `min_psnr` fails the bake with what passes; one `texture::choose` for the PC bake, the 3DS and N64 stagers, the CLI and the editor | `texture/auto.rs` unit tests, `texture_formats::auto_decides_the_same_on_every_bake_and_in_inspect`, `min_psnr_fails_the_bake_with_what_would_pass` |
| `cargo xtask texture report [--platform] [--json]` | `texture_cli::report_lists_every_texture_biggest_first_with_totals` |
| PC BC1/BC3/BC7 (texpresso, an own BC7 mode-6 encoder, bcdec_rs), `<hash>.bc`, uploaded as blocks where the GPU has BC, decoded otherwise (`TRINO_BC=0`), `TRINO_TEXTURES:` | `bc_textures::bc_uploads_draw_like_their_decoded_fallback` (GPU vs fallback ≤ 4 levels); castle64 VRAM 147 712 → 19 904 B |
| N64 I4/I8 masks staged by coverage (`texture::n64::stage`): Solid Eleven's glow was a solid white disc on the N64 | `n64::masks_are_staged_with_their_coverage_as_intensity`; `se_glow` 2.3 → 19.7 dB; the football template's N64 golden unchanged |
| The editor's texture inspector and texture memory in the Platform budget | `ui_tests::texture_inspector_formats_quality_and_errors`, the layout audit |

3DS linear memory (engine assets) 113 920 → 58 336 B (game textures 60 928 → 13 504 B);
Solid Eleven 169 216 → 42 304 B. Every N64 golden byte-identical; the five 3DS goldens
within tolerance without re-blessing (castle64 hub mean 0.52). Phase 5 (Web KTX2) moved
to #47.

## Done — texture formats, phase 1: 3DS hardware formats and `texture inspect` (2026-10-17, branch `feat/52-texture-formats-3ds`, GitHub #52, ADR 0017 draft)

| Item | Evidence |
|---|---|
| 3DS formats `ETC1`, `ETC1A4`, `RGBA4`, `RGBA5551`, `LA8`, `L8`, `A8` (besides `RGBA8`/`RGB565`), mapped to `tex3ds -f` by `N3dsFormat`; an unknown one fails the bake with the list (the PC bake checks it too) | `crates/asset-pipeline/tests/texture_formats.rs` (PSNR per format on a gradient/disc/alpha-ramp master, each 0.5–1 dB under the measured value; `a_bad_3ds_format_fails_the_pc_bake_with_the_list`) |
| PC preview of the decoded 3DS result (`N3dsFormat::preview`, an ETC1 encoder, `<id>.3ds.sprite` / `preview_3ds`, `LoadedSprite::pixels_for`), drawn by `trino-runner-pc`, the editor's 3DS Game view and the 3DS-profile tests | `preview_matches_tex3ds` with devkitPro's `tex3ds` v2.3.0: every uncompressed format texel for texel (luminance ±1), ETC1 28.75 dB vs `tex3ds` 28.77, ETC1A4 identical alpha; `pc_bake_writes_and_loads_the_3ds_preview` |
| The shim's A8 as a white mask (TexEnv `1 − color` operand on base/detail/water, `C2D_TintSolid` blits; `C3D_Tex::fmt` read as 4 unsigned bits) | the `texture formats` step in Azahar: `probed 9 formats`, `TEST PASS`; fails with the 3D operand removed (`A8 3D q0 got 0,0,0`) and, on the PC, with A8 decoded black or ETC1A4's alpha dropped |
| `cargo xtask texture inspect <sprites/x> [--platform] [--json]` | `xtask/tests/texture_cli.rs`; `inspect_reports_each_platform` |
| The `texture formats` self-test step (9 cards as sprites and 3D cards) and the 3DS golden `texture_formats.png` | `engine_checks` on the N64 and 3DS profiles; `n3ds_parity::pc_3ds_texture_formats_track_the_azahar_frame` (mean 0.19, 0 % far); ares `TEST PASS` |

The four older 3DS goldens pass unchanged (not re-blessed). ares: `texture formats probed 9 formats`,
`TEST PASS`, `showcase.png`, `castle64_hub.png` and `showcase_river.png` match byte for byte,
every ceiling held without re-blessing (hub 11.5 ms, throne 17.1 ms, showcase-river
272.7 ms), stack 25 408 bytes.
No game texture changed format in this phase, so RomFS and linear memory are unchanged
(the self-test adds 12 small sprites).

Deviations: ETC1 is encoded by the pipeline for the PC preview and by `tex3ds` for the
console, so the blocks are alike, not byte-identical (the parity test bounds it). A8 is
defined as white + alpha on every target, while the PICA200 samples it black; the shim
enforces the engine's meaning.

## Done — PC hardware T&L, phases 2–6: the GPU path, skeletons, instancing, the parallel CPU fallback, tooling, measurements (2026-10-18, branch `feat/51-pc-gpu-tnl-phases-2-6`, GitHub #51, ADR 0016 accepted)

| Item | Evidence |
|---|---|
| WGSL vertex stage + resident meshes (`gpu_tnl.rs`, `shaders_tnl.wgsl`): Gouraud/Flat/Unlit, fog per vertex, tint, emissive, palette, detail layer; the CPU path's depth and near plane | `crates/platform-pc/tests/gpu_tnl.rs::static_draws_match_the_cpu_path` (oracle 0.474/255, 0% far); `gpu_tnl::tests::layouts_match_the_shader` |
| The oracle on every native PC golden (`tnl_oracle::compare`, ≤ 1/255 mean, ≤ 0.5% over 8/255) | `ao_native` 0.003, `ao_mask` 0.006, `lights_native*` 0.000, `texture_filter_native` 0.000, `water_native` 0.000, `showcase_river` 0.252 (0.002% far), `solid_eleven_match_native` 0.000, `solid_eleven_teams_native` 0.070 (0.174% far); every golden unchanged |
| Rigid and `SKW2` skeletons on the GPU (`FlatSkinned` the only skeleton reason) | `gpu_tnl.rs::animated_and_smooth_skinned_meshes_draw_on_the_gpu` (walk, crossfade, the SKW2 strip: 0.000/255, no re-upload) |
| Automatic instancing (draws sharing mesh, texture, context, pose merged at each flush) | `gpu_tnl.rs::instanced_copies_are_a_few_draw_calls_and_the_same_pixels` (400 copies in ≤ 4 calls, one call = separate draws pixel for pixel) |
| Rejects with reasons, `TextureAlpha` at upload | `gpu_tnl.rs::rejected_draws_stay_on_the_cpu_with_their_reason`, `gpu_tnl::tests::compressed_alpha_is_found` |
| Parallel CPU fallback (`cpu_tnl.rs`, `TRINO_PC_TNL_THREADS`) | `cpu_tnl::tests::parallel_runs_emit_the_serial_stream` (2/3/8/64 workers = serial), `gpu_tnl.rs::the_parallel_cpu_fallback_draws_the_same_frame_on_any_worker_count` |
| `TRINO_PC_TNL=cpu`, `set_hw_tnl`, `TRINO_POWER` | `gpu_tnl.rs::turning_the_gpu_path_off_restores_the_cpu_path`; the whole `trino-platform-pc` suite with `TRINO_PC_TNL=cpu` (a CI step) |
| `FrameBudget::paths`, `scene budget` entity lists and `--json` paths, the editor's Render path row, clickable reasons, the HUD opening the budget | `xtask/tests/scene_budget.rs`; the kittest `platform_budget_shows_rsp_reasons_and_unseen_draws` with the layout audit at 960×600 and 1600×1000 |
| Frame times, CPU vs GPU path | `xtask/tests/pc_tnl_bench.rs`, ADR 0016 §10: 2000 instanced robots 28.1 → 0.46 ms (RX 7800 XT, 1080p) and 40.5 → 6.0 ms (Radeon iGPU); Solid Eleven match 1.78 → 0.76 ms (RX 7800 XT) |

Consoles: the core changed only in `render3d/pc.rs` (reasons the consoles never call) and
docs; the console goldens and N64 perf gates run in CI's `n64` and `n3ds-emulator` jobs.

## Done — PC hardware T&L, phase 1: instancing API, `pc_gpu_check` and reporting (2026-10-17, branch `feat/51-pc-gpu-tnl`, GitHub #51, ADR 0016 draft)

| Item | Evidence |
|---|---|
| `Instance`, `Renderer::draw_model_instances` / `draw_model_instances_with` (default: a loop over `draw_model_matrix`; the PC counts `TnlStats::instanced`) | `crates/core/tests/instancing.rs` (the default loop = separate draws: screen triangles bit for bit, `FrameStats`; tint and palette rules); the `instancing` self-test step on PC (`engine_checks`, the N64, 3DS and native profiles), in Azahar (`instancing probed`, `TEST PASS`) and in ares (`instancing probed`, `TEST PASS`); three mutations of the PC override (a copy dropped, the tint ignored, the palette table ignored) each fail the step |
| `render3d::pc_gpu_check` / `pc_gpu_mesh_check` / `PcGpuReject`, `TnlStats::pc_gpu_draws` / `pc_gpu_tris`, `PcRenderer::pc_gpu_rejects` (native profile only; the CPU path still draws) | `crates/core/tests/instancing.rs::pc_gpu_rules_reject_only_what_the_gpu_path_does_not_port` (`Offscreen` = the CPU path's cull); `xtask/tests/engine_checks.rs::instancing_matches_separate_draws_and_counts_the_pc_gpu_split`; every PC golden and `castle64_work*.toml` unchanged |
| `TRINO_FRAME` on the PC (`FrameMeter`, `apps/pc`, `trino-runner-pc`): `pc_gpu_draws= pc_gpu_tris= instanced= off_pc_gpu=` | `frame_report::tests::the_line_averages_per_frame_and_sorts_the_reasons`; castle64 native 26/26 draws, showcase native 42/46 (`skinned:3,water:1`), the N64 profile zeros |
| `scene budget` PC line and `--json` fields, the editor's Game-view HUD and Inspector `PC GPU` line | `xtask/tests/scene_budget.rs::pc_budget_reports_the_gpu_split` (estimate = the renderer's counters and reasons, CLI JSON); `budget::tests`; the editor kittest `platform_budget_shows_rsp_reasons_and_unseen_draws` with the layout audit at 960×600 and 1600×1000 |

N64: the three ares goldens byte-identical and every ceiling unchanged (not re-blessed);
`tessellate_joints` sizes identical to `main` and no `pc_gpu_*` symbol in the ROM. Frame,
emulated ms, mean (min–max) over 8 `TRINO_N64_TEXT_PAD` layouts (0/352/1024/2048/3072/
4096/6144/8192), `main` → this branch: hub 11.60 (11.26–12.01) → 11.51 (11.19–11.79),
green-hills 6.73 → 6.80, lava-land 7.46 → 7.39, sky-tower 6.87 → 6.79, throne 17.22 →
17.06, showcase-river 283.3 (275.5–293.1) → 274.1 (258.9–292.4): every change inside the
layout spread.

Deviations from the issue's Phase 1 wording: `palette_index` needed a table to index,
so `draw_model_instances_with` takes `&[Palette]` (`draw_model_instances` passes none);
an instance's tint multiplies the call's rather than replacing it. `PcGpuReject` also
names `PerPixelLit` (reported by the renderer; the budget predicts it for glossy draws).

## Done — water surfaces per tier and the showcase river (2026-10-17, branch `feat/water-surfaces`, GitHub #49, ADR 0015)

| Item | Evidence |
|---|---|
| `trino_core::water` (`Water`, `Ripple`, `RippleBuffer`, `WaterState`, `WaveSet`, `sin_turns`, Fresnel, `WaterTier`), `ModelParams::water`, `Renderer::set_water` (default no-op, cleared at `begin_frame`), `Caps::{water_tier, max_ripples}` | core unit tests (sine, gradients by finite differences, ripple rings and decay, the buffer, Fresnel and glint, scroll); `renderer_defaults` |
| Water T&L in the core (a `WATER` copy of `lit.rs`'s loop): displacement, normals, UV flow, per-vertex light + Fresnel + glint + fog, ripple rings (`tessellate_ripples`); `RspReject::Water`, `GpuReject::Water`, no memo; symmetric clipping; fog-band depth splits | `crates/core/tests/water.rs` (6 tests: unchanged without water, swell, flow, rings drawn last, surface normals, off the RSP/GPU/memo); every castle64 work counter and every PC golden unchanged |
| PC tiers: `fs_water` (per-pixel normals, Fresnel, glint, ripple crests, depth foam from a depth copy), `fs_water_vertex` (the consoles' 50/50 layer blend), `TriKind::Water` drawn last without depth writes, water sets, strict ripple cap | `crates/platform-pc/tests/water.rs` (7 tests: goldens `water_native`/`water_n64`/`water_n3ds`, the floor through the water, rings and swell per tier, foam per pixel only, state cleared per frame, drawn after the batch under the 2D, strict mode); `xtask/tests/showcase_river.rs` (golden `showcase_river.png`) |
| N64: a water bucket after the RSP commands; `trino_3d_water` (2-cycle `(TEX1 − TEX0) · ENV_A + TEX0` then × SHADE, TILE1 translated by the layer's flow, `RDPQ_BLENDER_MULTIPLY`, z compare without update) | the `water` self-test step in ares (2026-10-17: probed, `TRINO_TEST_PASS`); `n64_parity` (PC N64 look vs ares, mean 2.79); goldens `showcase.png` (re-blessed) and `showcase_river.png` (new), `castle64_hub.png` unchanged |
| 3DS: `water.v.pica` (the PICA200 displaces, lights, Fresnel, glint, fog, four ripples in uniform lanes) + TexEnv blend, `gpu_water` constants, CPU fallback with the same TexEnv, `detailoff` on texcoord1 | `crates/core/tests/gpu_water.rs` (the shader's math in f32 = the CPU path within 2/255); Azahar: the `water` step probed, `gpu tnl water` 0 of 960 samples differ (mean 0.08), goldens `showcase_river.png` (new) and `showcase_bottom.png` (re-blessed metrics), castle64's unchanged |
| Materials (`WaterDef`, ranges in `scene check`), TSCN v4, codegen | `material.rs` unit test, `crates/scene/tests/export.rs` (a pond through TSCN and codegen), `scene_nodes.rs` (v3 refused) |
| Showcase river: `showcase::river` (course, floor), generated terrain/ribbon/bridge/layers/sounds (`river_assets.rs`), wading, ripples, splashes, a patrolling robot, a shoved crate, `showcase-river` perf scene | showcase tests (wading, patrol, crate, the scripted run splashes), `river_assets` tests (the terrain matches the floor the game walks), `perf_budget` (new `showcase-river` baselines), the console self-tests check the demo splashed |
| Editor: material › Water (toggle and 7 sliders), Drop a ripple, the views' water clock, budget lines (triangles, combiner cycles, TMEM), `TRINO_EDITOR_RIPPLE`; `scene budget` prints the same | `widget_tests::water_editor_turns_a_surface_on_and_edits_it`, `ui_tests::water_budget_ripples_and_material_edits`, `budget::tmem_counts_padded_rows_and_refuses_palettes` |

N64: the self-test's stack peak fell from 58 672 to 25 072 B. The showcase games used
to be built on the stack; now one is copied from a static into a box and reset in place.

Deviations from #49:
- No WGSL vertex shader: the core displaces on every target (ADR 0015 §3). The PC
  adds its detail per pixel.
- **The river's N64 budget (≤ 1.2 ms) is not met.** It costs 12.5 ms of CPU T&L in
  ares. An RSP water command is follow-up P2 #17.
- **The showcase is not 30 fps on the N64.** It wasn't before either: about 150 ms
  estimated, with every draw on the CPU because of its colored light and fog. The
  `showcase-river` scene measures 284 ms per frame and is now gated.
- The 3DS's Fresnel and glint are per vertex in `water.v.pica`, not a
  `C3D_LightEnvLut` (follow-up P2 #18).
- Ripples on the per-vertex tiers are drawn as rings over the displaced surface: a
  16 × 4 ribbon can't show a 0.55 m ripple in its vertices.
- One river ribbon of 128 triangles. Under the showcase's own camera it stays on the
  3DS's CPU water path: part of it is behind the camera.

---

## Done — Solid Eleven, a football demo and the `football` template (2026-10-16, branch `feat/solid-eleven`, GitHub #39)

| Item | Evidence |
|---|---|
| `examples/solid-eleven`: a fixed-point (Q16.16, 60 Hz) match sim with the referee (restarts by last touch, offside, fouls, cards, penalties, substitutions), ball physics with curl, posts and net, a tactical AI, ten clubs balanced by point budgets, a career with transfers and a 1.8 KB save; menus after the issue's concept art in EN/PT-BR; HUD with the active player, the switch target and pass lanes; a 3DS touch screen (radar, formation, substitution and stamina tiles); PC-only detail (11-part players, rain, wet look, glows) | 67 crate tests (rules matrix, ball, league, e2e through the buttons with a bot, a career that survives the save on a 2 KB cart); `xtask/tests/solid_eleven_{budget,layers,render}.rs` (every draw on the RSP, pinned frame cost, layer rules, PC goldens); `docs/TESTING.md` › Solid Eleven |
| Models and clips from `tools/blender/solid_eleven_{models,anim}.py` (`cargo xtask blender solid-eleven`); its own `assets/` | `solid_eleven_layers::the_pose_tables_match_the_clip_source` |
| The `football` template (`--template football`, the editor's launcher): the game, its assets and an overlay with console hosts that self-test a scripted match | ares: 30.9 ms avg over four code layouts (30.5–31.3, gate 33.3 ms), all 83 draws on the RSP, hash `d51e91bf` = PC; Azahar: 16.8 ms, all 99 draws on the PICA200, same hash; goldens in `templates/football/tests/golden/`; CI `n64` and `n3ds-emulator` jobs run it |
| Engine: TTF atlases with black transparent texels, N64 I4/I8 font atlases with intensity = coverage | `font.rs::ttf_font_bakes_ascii`, `n64.rs::intensity_atlases_carry_coverage_in_their_gray_level` |

Not done: the items under P2 #16.

---

## Done — ambient occlusion per tier (2026-10-16, branch `feat/ambient-occlusion`, GitHub #42, ADR 0014)

| Item | Evidence |
|---|---|
| `trino_core::ao` (`AmbientOcclusion`, `AoQuality` 8/16/32 samples, `AoTier`), `Caps::ao_tier` (PC `ScreenSpace`, consoles `Baked`), `Renderer::set_ambient_occlusion` (default no-op, cleared at `begin_frame`) | core unit tests; `renderer_defaults`; `console_profiles_never_run_it` (the N64/3DS sim profiles ignore it, even forced on) |
| PC SSAO (`renderer/ao.rs`, `ao.wgsl`): depth read-back, normals from the nearer neighbor, a Bayer-rotated cosine kernel, a 5-tap bilateral blur, a multiply composite after the frame's last 3D draw and before the 2D; a fog fade; half resolution above 540 lines with a depth-aware upsample; `AoView` (game/off/on/mask), `TRINO_AO` | `crates/platform-pc/tests/ambient_occlusion.rs`: contacts and creases ≥ 18/25 luma darker, open floor and box tops within 3, background exact, a HUD patch over a contact untouched, no AO = identical frames (and cleared next frame), quality and intensity, fog, half resolution; goldens `ao_native.png`, `ao_mask.png`; every other PC golden unchanged |
| Baked vertex AO (`ao = { strength, distance, rays, ground }` on `[models.*]`, `asset-pipeline/src/ao.rs`: BVH ray casts, deterministic), `manifest::edit::set_model_ao` | `crates/asset-pipeline/tests/vertex_ao.rs`, unit tests (corner, ground, BVH = brute force, manifest forms, edit helper); the `ambient occlusion` self-test step (`models/selftest_ao` vs `selftest_ao_raw`) on PC, in ares and in Azahar (2026-10-16: probed, `TRINO_TEST_PASS`); disabling the bake makes the step fail |
| Scenes: `Environment::ao` (`AmbientOcclusionDef`, all fields defaulted, `enabled` keeps the tuning), codegen `AO`, `showcase` and `apps/pc`'s scene player apply it | `crates/scene` unit tests, `export.rs::codegen_exports_the_scene_ambient_occlusion` (TSCN bytes unchanged); `showcase_gen.rs` regenerated (`AO = None`), `scene_codegen` and `showcase_render` pass |
| Editor: Inspector › Ambient Occlusion (checkbox, radius, intensity, bias, quality; undoable, saved), the Scene view's AO menu (toolbar and View: as the scene, always on, off, mask), model › Ambient occlusion (baked) with Bake / Re-bake / Remove, Play here; hooks `TRINO_EDITOR_AO`, `TRINO_EDITOR_INSPECT=ao` | `ui_tests::ambient_occlusion_panel_view_modes_and_baking` (3/3 runs), the layout audit |
| castle64 sets SSAO on the PC (radius 0.6, Medium) | evidence `cargo test -p xtask --test ao_shots -- --ignored` (every example and scene, off / on / mask; the baked panel on the native, N64 and 3DS profiles); PC cost (Renoir iGPU, whole hub frame) 360p +1.2 ms, 720p +1.3, 1080p +3.0, 4K +9.4 |

Consoles: no N64 or 3DS backend code changed. Both N64 goldens are identical, and so are
the three 3DS goldens (means 0.58, 0.02 and 0.00, as before). Every N64 perf scene is
under its ceiling, and the stack watermark is 58 672 B (gate 61 440).

Deviations from #42:
- Consoles get AO from vertex colors baked per model: self-occlusion plus an optional
  ground contact. Occlusion *between* models is PC-only (SSAO).
- TSCN carries no AO: PC games that load baked scenes set it in code.
- The bake trigger is in the model inspector, as the issue asked. It writes the
  manifest, and the watcher rebakes. No vertex colors are edited by hand.

Follow-ups: scene-level AO baking per instance, so static models occlude each other on
consoles; AO in TSCN.

## Done — the showcase uses both 3DS screens (2026-10-16, branch `examples/showcase-two-screens`, GitHub #20)

| Item | Evidence |
|---|---|
| `examples/showcase` keeps the scene on the top screen and draws a touch panel on the bottom (`showcase::panel`): live metrics (frame time from `dt`, `Renderer::stats`: triangles, models, sprites, the `tnl` culled/verts/clipped and the hardware T&L draws and triangles, RSP or PICA200 (`HW DRAWS`, since 2026-10-16; `rsp_draws`/`rsp_tris` before), zero-padded so the panel's own glyph count is stable) and five look toggles that exist in the engine (camera preset follow/high/scene, scene vs dusk light, fog on/off, `set_texture_filter` default/point/bilinear, the animation clock). Every toggle is also on a pad button (X, Y, B, Select, Start), so it works on PC native and the N64, where the panel is skipped (`on_screen(Screen::Bottom)` says no) and the top draws the same calls as before | showcase unit tests (hit-testing inside and just outside every button, toggles cycling, taps on press only, top-screen clicks ignored, pad buttons, the panel only with a second screen and the same top draws, the default look = the scene's light and the target's filter); `xtask/tests/showcase_panel.rs` on the PC's 3DS profile (`check_panel`: button fills 2 px inside every corner, the background 2 px outside every edge, no panel on the top, a FOG tap through `update`), with three mutations that must fail (a stale panel, a layout shifted by 3 px each way, the panel drawn without `on_screen`), and the N64 profile collapsing; the `showcase panel` step and the golden `showcase_bottom.png` in Azahar (written by `--bless`, viewed before commit; mean 0.00 on the next run, castle64's two goldens byte-identical); the N64 golden `showcase.png` pixel-identical; the N64 stack watermark after the showcase steps 57 224 → 58 040 bytes (gate 61 440; the showcase frames grew ≤ 90 bytes, `panel::draw` is out of line and never runs there) |

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
