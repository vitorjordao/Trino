# Architecture

This page explains how Trino is put together and why. The decisions behind each part are
in the [ADRs](adr/); the full rules and pitfalls are in [AGENTS.md](../AGENTS.md).

## Layers and the dependency rule

```
game ──► trino-core ◄── trino-platform-{pc,n64,3ds}          apps/* glue them together
```

| Crate | Role |
|---|---|
| `crates/core` (`trino-core`) | Traits (`Game`, `Renderer`, `Platform`, `Audio`, `Input`), math, the software T&L, physics, text, scenes as data. `no_std`, zero dependencies |
| `crates/game-api` | The stable boundary for hot-reloadable game dylibs on PC (`export_game!`) |
| `crates/platform-pc` | wgpu and winit backend, the console-simulation profiles, strict mode |
| `crates/platform-n64` | libdragon backend through a C shim, and the RSP overlay (`shim/rsp_trino.S`) |
| `crates/platform-3ds` | libctru, citro2d and citro3d backend through a C shim |
| `crates/asset-pipeline` | Bakes masters into each platform's formats; glTF and fake objects into TMDL |
| `crates/scene` | Scenes, prefabs and materials (RON): load, migrate, resolve per platform, export to code |
| `crates/editor` | The egui editor |
| `crates/runner-pc` | A ready-made PC host for standalone projects |
| `apps/{pc,n64,3ds}` | Thin binaries that join a game to a backend |
| `xtask` | The single entry point for building, running, testing and baking |

The rule that holds it together:

- `trino-core` has **zero dependencies** and stays `no_std`, so it compiles for MIPS bare
  metal (N64) and ARM (3DS) unchanged.
- Game crates depend only on `trino-core` (and `trino-game-api`). No external crates.
  A game that builds for PC therefore builds for both consoles.
- Platform crates may use external crates, but within the workspace only `trino-core`.
  libdragon, libctru and wgpu types never appear in the core or in game code.

`xtask/tests/dep_graph.rs` checks this on every `cargo test --workspace`. Game crates
start with `#![cfg_attr(any(target_os = "none", target_os = "horizon"), no_std)]`: std
exists on PC only for the dylib's panic handler, and game code never calls std APIs.

Console FFI goes only through the C shims (`crates/platform-{n64,3ds}/shim/` with the
matching `src/ffi.rs`), with at most four scalar or pointer arguments per call. On the N64
that is an ABI requirement: Rust compiles to o32 and libdragon to o64
([ADR 0002](adr/0002-n64-abi-o32-staticlib.md)).

## The N64 is the design ceiling

If a feature cannot work on the N64, the engine does not expose it. Two things follow:

- **No free shaders.** The pipeline is chosen by `Material::{Sprite, VertexLit}`, and a 3D
  surface is plain data, `ModelParams`: tint, emissive, `Shading::{Gouraud, Flat, Unlit}`,
  double-sided, texture, filter, detail layer, gloss, animation pose, a palette for color
  slots. The engine evaluates it the same way on every target.
- **Budgets are explicit.** `Caps` in `crates/core/src/caps.rs` holds each console's limits
  (the N64's 4 KB TMEM, its 2 KB EEPROM save, the local-light tier). The PC's strict mode
  (`TRINO_STRICT=1` or `PcRenderer::set_strict`) panics with an actionable message when
  content exceeds the simulated console's `Caps`, including the estimated N64 3D time.

Where a platform can do more, it does it behind the same API, chosen by its tier. Local
lights are the example: per pixel on PC, per vertex on the 3DS and the N64, with fewer
lights on the N64 ([ADR 0010](adr/0010-local-lights-per-tier.md)).

## Rendering

### Software T&L in the core

`trino_core::render3d` transforms, lights, culls and clips every 3D draw on the CPU with
deterministic `f32` math and its own `sin`/`cos`/`sqrt`
([ADR 0003](adr/0003-software-tnl-3d.md)). Backends only rasterize screen-space
triangles: a wgpu vertex pipeline on PC, the RDP on the N64, citro3d on the 3DS.

- Meshes come from TMDL, the engine's portable mesh format (see [ASSETS.md](ASSETS.md)).
- Whole meshes outside the frustum are culled from a baked bounding sphere; back faces
  are rejected before clipping.
- Triangles are clipped against the near plane and a 1.5x guard band, so coordinates
  stay inside the RDP's fixed-point range.
- Occlusion is z-buffered on every target, with per-vertex depth. Consecutive model draws
  also form a depth-sorted batch.
- Because the same code runs everywhere, the T&L work counters on PC equal the ones in
  the N64 ROM, and a test pins them (`tests/perf/castle64_work.toml`).

### The N64 RSP fast path

On the N64 most draws skip the CPU T&L and run on Trino's own RSP microcode, an rspq
overlay in `crates/platform-n64/shim/rsp_trino.S` ([ADR 0007](adr/0007-n64-rsp-tnl.md)).
It takes static, opaque, untextured, unclipped Gouraud or Unlit draws. On castle64's hub
that cut the emulated frame from 38.0 ms to 13.1 ms in the same ROM.

`trino_core::render3d::rsp_check` decides per draw and returns an `RspReject` reason when
a draw must stay on the CPU (fog, translucency, textures, a colored light, a local light,
a palette, skinning, a draw that needs clipping, and so on). The editor's budget HUD and
`cargo xtask scene budget` report those reasons per draw. The CPU path stays the reference:
an oracle and CPU-vs-RSP pixel checks gate the overlay.

A new surface or light feature must either reach the overlay behind those checks or be
excluded in `rsp_check`, because the overlay ignores what it does not implement.

### The 3DS GPU path

On the 3DS, static untextured meshes skip the CPU T&L too: the PICA200's vertex shader
(`crates/platform-3ds/shim/tnl3d.v.pica`) transforms, lights and projects them from a
copy in linear memory ([ADR 0012](adr/0012-console-hardware-paths-and-honest-n64-timing.md)).
`trino_core::render3d::gpu_check` decides per draw, with its own rules: the PICA clips in
hardware and lights in floats, so it takes colored lights, emissive and double-sided
draws. It returns a `GpuReject` reason for the draws that keep the CPU path (fog,
translucency, textures, skeletons, local lights, anything crossing the near plane). The
`gpu tnl` self-test step compares the frame with the CPU path's, and
`tests/perf/castle64_work_3ds.toml` pins the split per scene.

### 2D in the same pipeline

2D is not a separate engine path. The same `Renderer` draws it in the same frame as the
3D:

- **Screen space:** `draw_sprite`, `fill_rect` and `draw_text` (bitmap fonts built on
  sprite regions) draw HUDs and interfaces over the world. A 2D draw flushes the current
  model batch, so draw order is submission order.
- **World space:** `draw_billboard` draws a sprite as a camera-facing quad, z-tested and
  fogged like the 3D around it. Particle systems and impostors (a model pre-rendered into
  an atlas and drawn as one billboard, chosen by the camera's angle) build on it.
- **Two screens:** `Renderer::on_screen(Screen)` sends the next draws, 2D and 3D, to the
  3DS top or bottom screen. Single-screen targets return `false` for the bottom so the
  game can skip those draws.

The 2D-only path the engine started with (the `hello-sprite` and `platformer` examples)
is legacy. A redesigned 2D sprite and tile system is planned for a future release
([BACKLOG.md](BACKLOG.md)).

### Console looks on PC

`SimProfile::N64` emulates the console's output on PC: 3-point filtering, RGBA5551
quantization with the RDP's dither rule, and the Video Interface's dedither pass. The N64
shim enables the same dither and filter, and `xtask/tests/n64_parity.rs` compares the PC
frame with the ares golden. `SimProfile::N3ds` uses the 3DS resolution, bilinear sampling
and both screens, held to the Azahar golden by `n3ds_parity`.

## Authored content

Scenes, prefabs, materials, particle effects and fake objects are versioned RON files
([ADR 0004](adr/0004-authored-content.md)). The editor reads and writes them, any tool or
agent can edit them, and the editor and the PC scene player reload them live. An entity
can swap its model, material or scale per platform, or be left out on one
([ADR 0009](adr/0009-platform-aware-authoring.md)).

Games reach that content two ways, both `no_std` on every target:

- `cargo xtask scene export` writes a scene as Rust data (`trino_core::SceneNode`).
  `examples/showcase` is built this way, and `xtask/tests/showcase_render.rs` proves it
  renders like the scene file.
- A `[scenes.*]` manifest entry bakes a scene into a `TSCN` blob that `BakedScene` parses
  at run time.

## Hot reload and determinism

- **PC:** with the `reload` feature, `cargo xtask watch pc` rebuilds the game dylib on a
  source change and swaps it under the running state; saving a master rebakes and
  re-uploads that asset under the same handle. The boundary rules (no generics in exports,
  no statics in the dylib, no layout change between reloads) are in
  `crates/game-api/src/lib.rs` and AGENTS.md.
- **Consoles:** `watch n64` and `watch 3ds` rebuild and relaunch the emulator on save.
- **Handles** are the FNV-1a hash of a logical asset path, so they stay stable across
  reloads and platforms.
- **Determinism:** the math, physics and T&L give the same results on the three targets.
  The console self-tests and the bot that plays castle64 in ares and Azahar depend on it.

## Testing

Every feature has a test through the public API and a row in [TESTING.md](TESTING.md).
The console self-test (`castle64::selftest`) has a step for every engine service and runs
the same code on PC, in ares and in Azahar. The commands are in
[PLATFORMS.md](PLATFORMS.md) and the `test-all` and `run-emulator` skills.
