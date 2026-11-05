# Trino documentation

Start with the [README](../README.md) for the pitch, pictures of every target, the
quickstart and the feature tables. [AGENTS.md](../AGENTS.md) is the complete reference for contributors and AI
agents: every command, the repository map, the rules and the pitfalls. The pages below
explain the engine by topic and link the detailed material instead of repeating it.

## Guides

| Guide | What it covers |
|---|---|
| [ARCHITECTURE.md](ARCHITECTURE.md) | The dependency rule, the `no_std` core, the N64 as the design ceiling, the software T&L and the N64 RSP fast path, 2D in the 3D pipeline, authored content, hot reload |
| [ASSETS.md](ASSETS.md) | The manifest, masters and per-platform overrides, glTF to TMDL, fake objects, Blender, materials, impostors, particles, baked scenes, UI screens and string tables, validation commands |
| [PLATFORMS.md](PLATFORMS.md) | PC (console looks, strict mode, live reload), Web (wasm, WebGPU/WebGL2, the browser self-test), N64 (Docker toolchain, ares), 3DS (devkitPro, Azahar), external projects |
| [HARDWARE.md](HARDWARE.md) | Deploying to a real N64 (SummerCart64, EverDrive 64) or 3DS (3dslink), the debug log in the terminal, Luma3DS gdb |
| [SHIPPING.md](SHIPPING.md) | Shipping a game: achievements, stats, presence and cloud saves on every target; PC packages (AppImage, .exe, .app), Steam and itch.io, signing, crash reports, the version stamp, `deck check`, the 3DS `.cia` |
| [ACCESSIBILITY.md](ACCESSIBILITY.md) | The player's accessibility settings on every target: color vision through semantic palettes and icons, text scale and high contrast, captions, reduced motion, the flash guard, hold-to-toggle; `access check` and the editor's simulation |
| [TESTING.md](TESTING.md) | The kinds of tests, coverage, and the feature-to-test matrix that a guard test keeps current |
| [BACKLOG.md](BACKLOG.md) | The ranked roadmap and the record of what each pass delivered |
| [SOLID_ELEVEN.md](SOLID_ELEVEN.md) | Solid Eleven, the football demo and `football` project template: playing it, its rules, AI and career, its art pipeline, performance on each console, tests, and customizing it |

## Skills (step-by-step workflows)

The skills in [`.agents/skills/`](../.agents/skills/) are written for AI agents, and
they read just as well for people. Each one is the most detailed source for its topic.

| Skill | Topic |
|---|---|
| [add-asset](../.agents/skills/add-asset/SKILL.md) | Every asset kind: manifest entries, formats, baking, live reload |
| [blender-asset](../.agents/skills/blender-asset/SKILL.md) | Modeling, coloring, rigging and exporting within Trino's limits |
| [scene-authoring](../.agents/skills/scene-authoring/SKILL.md) | Scenes, prefabs and materials as text; `scene check`, `scene export` |
| [animation](../.agents/skills/animation/SKILL.md) | Animation graphs, blend spaces, layers with joint masks, clip events, root motion, foot IK, crowds; `anim check`, `anim simulate` |
| [ui-authoring](../.agents/skills/ui-authoring/SKILL.md) | Menus, HUDs and dialogs as `ui/*.ui.ron` screens; `ui check`, `ui shot`, driving a screen from game code |
| [localization](../.agents/skills/localization/SKILL.md) | String tables, plurals and locale formats, fonts with each language's glyphs; `i18n check`, `i18n pseudo`, CSV for translators |
| [accessibility](../.agents/skills/accessibility/SKILL.md) | Semantic palettes, captions, text scale and high contrast; `access check`, `access shot`, the editor's simulation |
| [netplay](../.agents/skills/netplay/SKILL.md) | Local players, `TREC` recordings and replays, rollback and lockstep netplay, the lobby and relay |
| [editor](../.agents/skills/editor/SKILL.md) | Running and driving the editor, its hooks, its code map |
| [build-n64](../.agents/skills/build-n64/SKILL.md), [build-3ds](../.agents/skills/build-3ds/SKILL.md), [build-web](../.agents/skills/build-web/SKILL.md) | How the console and browser builds work and how to fix them |
| [run-emulator](../.agents/skills/run-emulator/SKILL.md) | ares and Azahar: runs, the self-test harness, goldens, watch loops |
| [hardware](../.agents/skills/hardware/SKILL.md) | Real-console deploys and how they are wired |
| [n64-performance](../.agents/skills/n64-performance/SKILL.md) | Measuring the N64 frame, the perf gates, the cost model |
| [profiling](../.agents/skills/profiling/SKILL.md) | Timeline traces from every target: zones, `trace collect\|report\|diff`, Perfetto, Tracy, the editor's Profiler |
| [robustness](../.agents/skills/robustness/SKILL.md) | The fuzz targets, adding one for a new format, triaging a crash, the A/B save slots |
| [engine-feature](../.agents/skills/engine-feature/SKILL.md) | Adding an engine service end to end on every backend |
| [shipping](../.agents/skills/shipping/SKILL.md) | Achievements and stats, `package pc`, Steam, crash reports, `deck check`, the `.cia` |
| [new-project](../.agents/skills/new-project/SKILL.md) | `cargo xtask new`, standalone projects, `trino.toml` |
| [test-all](../.agents/skills/test-all/SKILL.md), [ci](../.agents/skills/ci/SKILL.md) | The local gates and the GitHub Actions jobs |

## Architecture decision records

| ADR | Decision |
|---|---|
| [0001](adr/0001-defer-transform-gizmo.md) | Defer the viewport transform gizmo (superseded) |
| [0002](adr/0002-n64-abi-o32-staticlib.md) | N64: Rust as an o32 staticlib, linked against o64 libdragon through a narrow C shim |
| [0003](adr/0003-software-tnl-3d.md) | 3D through software T&L in the core; backends only rasterize triangles |
| [0004](adr/0004-authored-content.md) | Scenes, prefabs, materials, fake objects, rigid animation |
| [0005](adr/0005-backlog-pass-formats-and-services.md) | Engine services, data formats and console rules |
| [0006](adr/0006-performance-and-visual-gates.md) | Performance and visual gates, 3DS per-vertex depth, renderer state |
| [0007](adr/0007-n64-rsp-tnl.md) | N64: vertex T&L on the RSP |
| [0008](adr/0008-external-console-projects.md) | Console packaging for external projects |
| [0009](adr/0009-platform-aware-authoring.md) | Platform-aware authoring in the editor |
| [0010](adr/0010-local-lights-per-tier.md) | Local lights, one API with a quality tier per platform |
| [0011](adr/0011-texture-filter-and-detail.md) | Texture filtering per draw and a detail layer |
| [0012](adr/0012-console-hardware-paths-and-honest-n64-timing.md) | GPU vertex T&L on the 3DS, N64 text in runs, N64 timings over code layouts, hang diagnostics |
| [0013](adr/0013-hardware-paths-round-two-and-frame-breakdowns.md) | Fog on the PICA200, why some draws stay on the CPU, the PC's first frame breakdown, an N64 change held back |
| [0014](adr/0014-ambient-occlusion-per-tier.md) | Ambient occlusion per tier: SSAO on the PC, baked into vertex colors on the consoles |
| [0015](adr/0015-water-surfaces-per-tier.md) | Water surfaces: one material with a tier per platform |
| [0016](adr/0016-pc-hardware-tnl.md) | PC hardware T&L: the GPU path, instancing on every target, the CPU oracle |
| [0017](adr/0017-texture-formats.md) | Texture formats: 3DS hardware formats, `auto`, PC BCn, the Web's KTX2 |
| [0018](adr/0018-n64-rsp-coverage-and-water.md) | N64: the RSP takes fog, colored light, textures, rigid skeletons, palettes and water |
| [0019](adr/0019-web-target.md) | The Web target: one renderer for PC and browsers, WebGPU with a WebGL2 fallback |
| [0020](adr/0020-console-hardware-tiers.md) | Console hardware tiers: the Expansion Pak and the New 3DS, one ROM per game, the 3DS's second core |
| [0021](adr/0021-audio-pipeline.md) | Audio: a codec per platform (VADPCM, DSP-ADPCM, Vorbis), streaming, buses and ducking |
| [0022](adr/0022-asset-groups-and-memory.md) | Asset groups, `TBND` bundles, compression per platform, loading in time slices, a RAM budget per console tier, cell streaming |
| [0023](adr/0023-quality-presets.md) | Quality presets for the PC and the Web: Low to Ultra, render scale, dynamic resolution, frame pacing, an auto-detect bench |
| [0024](adr/0024-ui-system.md) | A declarative UI system for every target: `ui/*.ui.ron` screens, safe areas and the 3DS touch screen, pad/mouse/touch focus, budgets, `ui check` |
| [0025](adr/0025-shadows-per-tier.md) | Shadows per tier: blobs on the N64, planar shadows on the 3DS, a shadow map on the PC and the Web |
| [0026](adr/0026-localization.md) | Localization: string tables, glyph subsets per language, plurals and locale formats |
| [0027](adr/0027-accessibility.md) | Accessibility on every target: semantic palettes per color-vision mode, icons, captions, text scale and high contrast, motion and flash guards |
| [0028](adr/0028-gamepad-families.md) | Gamepad families and button glyphs |
| [0029](adr/0029-shipping-services-packaging.md) | Shipping games: platform services, packages, crash reports |
| [0030](adr/0030-unified-profiler.md) | One trace format from every target: zones, export, Perfetto, Tracy, the editor's Profiler |
| [0031](adr/0031-shader-precompilation-and-thread-budget.md) | Shader precompilation and the CPU thread budget |
| [0032](adr/0032-stereoscopic-3d.md) | Stereoscopic 3D on the 3DS top screen |
| [0033](adr/0033-post-processing-stack.md) | Post-processing per tier (HDR on PC/Web, baked grading and glow halos on the consoles), and the daltonization filter |
| [0034](adr/0034-animation-graphs-and-clip-data.md) | Animation 2.0: graphs, clip events, root motion, compressed clips, foot IK |
| [0035](adr/0035-physics-2.md) | Physics 2.0: a broadphase, shape casts, triggers, a character controller, ramps and mesh colliders, a fixed-point mode; physics per cell (2.1) |
| [0036](adr/0036-players-replays-netplay.md) | Local players, `TREC` input recordings checked by state hashes on every target, rollback netplay over ggrs (UDP, WebRTC, a lobby) |
| [0037](adr/0037-robustness-fuzzing-and-save-recovery.md) | Fuzzing every parser, save slots and recovery, sanitizers in CI |
| [0038](adr/0038-performance-instrumentation.md) | Performance instrumentation: backend counters, the RSP cost per command, a CPU sampler, A/B over code layouts |
| [0039](adr/0039-animation-layers-2d-blend-inertialization.md) | Animation layers: joint masks, additive layers, 2D blend spaces, inertialized transitions, tiered per platform |
| [0040](adr/0040-graph-tables-and-crowd-instances.md) | Animation graphs for crowds: tables decoded once, instances sized by the game, synced layers, one graph per rig family |
| [0041](adr/0041-netplay-follow-ups.md) | Netplay follow-ups: local seats, spectators, NAT traversal and a relay, the consoles' lockstep over the 3DS's local wireless, snapshot recordings |

## Plans and history

- [plans/n64-throughput.md](plans/n64-throughput.md): the N64 throughput plan, with each
  item's measured result.
- [PLANO_ENGINE_TRINO.md](../PLANO_ENGINE_TRINO.md) and
  [PLANO_EXECUCAO_TRINO.md](../PLANO_EXECUCAO_TRINO.md) (Portuguese): the original
  architecture plan and the phase roadmap the engine was first built from. They are
  historical: the engine has since grown past them, and their 2D-first sequencing no
  longer describes the current design.
