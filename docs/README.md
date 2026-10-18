# Trino documentation

Start with the [README](../README.md) for the pitch, the feature matrix and the
quickstart. [AGENTS.md](../AGENTS.md) is the complete reference for contributors and AI
agents: every command, the repository map, the rules and the pitfalls. The pages below
explain the engine by topic and link the detailed material instead of repeating it.

## Guides

| Guide | What it covers |
|---|---|
| [ARCHITECTURE.md](ARCHITECTURE.md) | The dependency rule, the `no_std` core, the N64 as the design ceiling, the software T&L and the N64 RSP fast path, 2D in the 3D pipeline, authored content, hot reload |
| [ASSETS.md](ASSETS.md) | The manifest, masters and per-platform overrides, glTF to TMDL, fake objects, Blender, materials, impostors, particles, baked scenes, validation commands |
| [PLATFORMS.md](PLATFORMS.md) | PC (console looks, strict mode, live reload), Web (wasm, WebGPU/WebGL2, the browser self-test), N64 (Docker toolchain, ares), 3DS (devkitPro, Azahar), external projects |
| [HARDWARE.md](HARDWARE.md) | Deploying to a real N64 (SummerCart64, EverDrive 64) or 3DS (3dslink), the debug log in the terminal, Luma3DS gdb |
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
| [editor](../.agents/skills/editor/SKILL.md) | Running and driving the editor, its hooks, its code map |
| [build-n64](../.agents/skills/build-n64/SKILL.md), [build-3ds](../.agents/skills/build-3ds/SKILL.md) | How the console builds work and how to fix them |
| [run-emulator](../.agents/skills/run-emulator/SKILL.md) | ares and Azahar: runs, the self-test harness, goldens, watch loops |
| [hardware](../.agents/skills/hardware/SKILL.md) | Real-console deploys and how they are wired |
| [n64-performance](../.agents/skills/n64-performance/SKILL.md) | Measuring the N64 frame, the perf gates, the cost model |
| [engine-feature](../.agents/skills/engine-feature/SKILL.md) | Adding an engine service end to end on all three backends |
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
| [0017](adr/0017-texture-formats.md) | Texture formats: 3DS hardware formats, `auto`, PC BCn, the Web's KTX2 |
| [0018](adr/0018-n64-rsp-coverage-and-water.md) | N64: the RSP takes fog, colored light, textures, rigid skeletons and water (#69) |
| [0019](adr/0019-web-target.md) | The Web target: one renderer for PC and browsers, WebGPU with a WebGL2 fallback |

## Plans and history

- [plans/n64-throughput.md](plans/n64-throughput.md): the N64 throughput plan, with each
  item's measured result.
- [PLANO_ENGINE_TRINO.md](../PLANO_ENGINE_TRINO.md) and
  [PLANO_EXECUCAO_TRINO.md](../PLANO_EXECUCAO_TRINO.md) (Portuguese): the original
  architecture plan and the phase roadmap the engine was first built from. They are
  historical: the engine has since grown past them, and their 2D-first sequencing no
  longer describes the current design.
