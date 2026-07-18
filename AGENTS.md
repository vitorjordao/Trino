# Trino — Agent Guide

Trino is a Rust game engine targeting **Nintendo 64, Nintendo 3DS and PC** from a single
codebase, with a visual editor, console-simulation modes, live reload and emulator-based
testing. This file is the canonical entry point for AI agents (and a good one for humans).

Roadmap and per-phase acceptance criteria: `PLANO_EXECUCAO_TRINO.md` (Portuguese).
Original architecture rationale: `PLANO_ENGINE_TRINO.md` (Portuguese).

## Current state

**All 9 roadmap phases (0–8) implemented.** Working today:

- PC 2D rendering (wgpu, console-sim resolutions, golden tests), audio, input.
- Asset pipeline: `assets/manifest.toml` + shared masters + per-platform overrides →
  `cargo xtask assets pc`. Handles = fnv1a(logical path), stable across reloads.
- Live reload: `cargo xtask watch pc` — saving a master rebakes + re-uploads it live;
  saving game source rebuilds the dylib and hot-lib-reloader swaps functions under the
  running state (`apps/pc` feature `reload`).
- Editor (`cargo xtask editor`): dockable Viewport (render-to-texture via the real
  `PcRenderer` on eframe's wgpu device) + Hierarchy/Inspector/Assets/Console, scene
  save/load, sim-profile switcher, Play = separate `trino-app-pc` process, live asset
  reload inside the editor. No gizmo yet — see `docs/adr/0001-defer-transform-gizmo.md`.
- Scene format: **versioned RON** in `scenes/*.scene.ron` (`trino-scene` crate).
  Removing/renaming a field bumps `SCENE_VERSION` and requires a migration in
  `Scene::from_ron`; adding an optional defaulted field does not.
- **N64**: `cargo xtask build n64` produces `target/n64/trino.z64` (Docker
  toolchain image + C shim + Rust staticlib — ABI story in
  `docs/adr/0002-n64-abi-o32-staticlib.md`); `run n64` opens it in ares
  (expected at `ares-v148/` in the repo root, gitignored); `test n64` boots a
  test ROM and asserts on ISViewer magic strings (`TRINO_TEST_PASS/FAIL`);
  `watch n64` rebuilds + relaunches on save. Skills: `build-n64`, `run-emulator`.
- **N64 look on PC**: the `SimProfile::N64` renderer emulates the console
  output — 3-point filtering + RGBA5551 quantization with the RDP magic-square
  dither (`TRINO_LOOK=off|n64` overrides). Strict mode (`TRINO_STRICT=1` or
  `PcRenderer::set_strict`) panics with an actionable message when content
  busts the profile's `Caps`. Deferred: mupen64plus golden screenshots (ares
  is the reference emulator for now) and the VI post stage (dedither/divot).
- **3DS**: `cargo xtask build 3ds` produces `target/3ds/trino.3dsx` — all
  local (devkitPro install auto-detected; built-in `armv6k-nintendo-3ds`
  Tier 3 target, C shim over libctru + citro2d in
  `crates/platform-3ds/shim/`); `run 3ds` opens it in Azahar (auto-detected
  or `$TRINO_AZAHAR`); `test 3ds` asserts the TRINO_TEST_* magic strings by
  tailing Azahar's log (`svcOutputDebugString` channel); `watch 3ds`
  rebuilds + relaunches on save. The `SimProfile::N3ds` PC renderer samples
  sprites bilinearly (the 3DS GPU default). Skills: `build-3ds`,
  `run-emulator`.

- **Showcase platformer** (`examples/platformer`, the default game in all
  three `apps/*`): tilemap + AABB physics from `trino_core::{tilemap,collide}`
  (ASCII levels, zero-alloc parse, substepped collision — deterministic
  across targets, verified by unit tests and the console self-tests), coins,
  goal, camera with bounds, chiptune music loop + SFX. Music flows through
  the whole pipeline (`[music.*]` in the manifest → TSND on PC, wav64
  `--wav-loop` on N64, looped ndsp wavebuf on 3DS). `hello-sprite` stays as
  the minimal example (the hot-reload E2E builds it standalone).
  README media: hero shot via `cargo test -p xtask --test hero_shot --
  --ignored`; gameplay GIF (a bot plays the level on the real renderer) via
  `cargo test -p xtask --test record_gif -- --ignored`.
- **3D (`draw_model`)**: engine-side software T&L (`trino_core::render3d` —
  see `docs/adr/0003-software-tnl-3d.md`): glTF masters bake to the portable
  TMDL format (`[models.*]` in the manifest); the core transforms, lights
  (gouraud, directional + ambient), culls and depth-sorts on the CPU with
  deterministic f32 (own no_std sin/cos/sqrt); backends only rasterize
  screen-space colored triangles (`rdpq_triangle` shade / `C2D_DrawTriangle`
  / a wgpu vertex-color pipeline interleaved with sprites). `set_camera` +
  `draw_model(Material::VertexLit, &ModelParams)` on all three targets —
  `ModelParams::tint` multiplies vertex colors per draw (locked doors, glow
  pulses) so color variants don't need separate baked meshes; strict mode
  enforces `max_tris_per_frame`. The platformer shows a spinning cube.
  Triangles are clipped against the near plane + a 1.5x guard-band frustum
  (bounded coordinates — RDP fixed-point safe); edges spanning > 3 units of
  view depth are bisected and the painter key is the farthest vertex, so
  floors/walls layer correctly under objects standing on them; `math3d`
  ships deterministic `sin/cos/sqrt/atan2`. Consecutive `draw_model` calls
  form a batch whose triangles depth-sort together (painter across meshes;
  flushed on sprite draws, camera changes and `end_frame`). v1 limits (in
  the ADR): no z-buffer (interpenetrating triangles still mis-sort), vertex
  colors only. Deferred: editor 3D viewport/gizmo.

- **Scaffolding + release**: `cargo xtask new <name>` renders
  `templates/new-game/` into `examples/<name>` (game crate + its own
  AGENTS.md; the workspace glob picks it up). `release.yml` tags new
  workspace versions on main and publishes editor binaries + demo
  `.z64`/`.3dsx` + SHA256SUMS.
- **Editor two-way file sync** (`crates/editor/src/level_editor.rs` + the
  file watcher in `main.rs`): the **Level** tab paints the platformer's
  ASCII tilemap; every stroke saves immediately to
  `examples/platformer/src/level1.txt`, and external writes to that file or
  to `scenes/*.scene.ron` (AI agents, other tools) reload live in the UI —
  the file is the source of truth, with an echo guard so the editor's own
  saves don't bounce. A dirty scene is protected: external scene changes are
  logged, not force-loaded, until saved. Dev hook: `TRINO_EDITOR_TAB=level`
  opens with the Level tab focused (used for screenshots).

Deferred backlog (each with a note where it lives): transform gizmo
(ADR-0001), mupen64plus goldens + VI stage (AGENTS Fase 4 notes),
z-buffer/textured 3D/near clipping + editor 3D viewport (ADR-0003),
emulators in CI, standalone (out-of-repo) game scaffolding, entity
placement in the Level tab (spawn/flag paint as tiles today).

PC keyboard mapping: A/B = Z/X, X/Y = C/V, L/R = Q/E, Start = Enter,
Select = Right Shift, D-pad = arrows, stick = WASD (see `crates/platform-pc/src/input.rs`).

## Repository map

| Path | What it is |
|---|---|
| `crates/core` | Platform-agnostic traits + math. `no_std`, **zero dependencies** |
| `crates/game-api` | Stable ABI boundary for hot-reloadable game dylibs (Fase 2) |
| `crates/platform-*` | One backend per target: wgpu (pc), libdragon FFI (n64), ctru/citro FFI (3ds) |
| `crates/editor` | egui editor: viewport, inspector, asset browser (Fase 3) |
| `crates/asset-pipeline` | Asset baking + watching, shared by xtask and editor (Fase 2) |
| `apps/{pc,n64,3ds}` | Thin glue binaries: platform backend + game + main loop |
| `examples/` | Example games; also serve as integration tests |
| `assets/` | Master assets (`shared/`) + per-platform overrides (`n64/`, `3ds/`, `pc/`) |
| `platforms/*.toml` | Per-target config: resolution, material presets, budgets |
| `xtask/` | Build orchestrator — the only entry point you need |
| `templates/new-game/` | Scaffold for `trino new` (Fase 8) |
| `.github/workflows/` | `ci.yml` (lint + desktop matrix + consoles later), `release.yml` (Fase 8) |

## The one inviolable rule: dependency direction

```
game ──► core ◄── platform-{pc,n64,3ds}          apps glue them together
```

- `trino-core` must have **zero dependencies** and stay `no_std`.
- Game crates depend only on `trino-core` (+ `trino-game-api`). No external crates —
  that is what makes games portable to all three consoles by construction.
- Platform crates may use external crates but, within the workspace, only `trino-core`.
- libdragon/ctru/wgpu types must **never** appear in `core` or game code.

This is enforced by `xtask/tests/dep_graph.rs`, which runs in `cargo test --workspace`
and fails CI. If you add a crate, that test decides what it may depend on by name:
`trino-platform-*` and `trino-game*` prefixes carry rules.

## Design ceiling: the N64

If a feature cannot work on the N64, the engine does not expose it. Materials are enum
presets (`Material::{Sprite, VertexLit, Named}`), never free shaders. `Caps` in
`crates/core/src/caps.rs` encodes per-console budgets (N64 TMEM = 4 KB); PC strict mode
validates content against them at development time.

## Commands

Everything goes through xtask (alias in `.cargo/config.toml`):

```
cargo xtask build pc      # compile the PC app
cargo xtask run pc        # build + launch (TRINO_SMOKE_FRAMES=60 auto-exits, for CI)
cargo xtask test          # full workspace test suite (same as CI); --bless regens goldens
cargo xtask assets pc     # bake assets into target/assets/pc
cargo xtask watch pc      # live-reload session (code dylib + assets)
                          #   --game <crate> picks the game dylib to rebuild
                          #   (default: platformer; must match apps/pc)
cargo xtask editor        # launch the visual editor
cargo xtask gen-assets    # regenerate sample masters (dev utility)
cargo xtask build n64     # ROM via Docker toolchain -> target/n64/trino.z64
cargo xtask run n64       # build + open in ares (ares-v148/ at repo root)
cargo xtask test n64      # build test ROM + assert ISViewer TRINO_TEST_PASS
cargo xtask watch n64     # rebuild ROM + relaunch ares on save
cargo xtask build 3ds     # .3dsx via local devkitPro -> target/3ds/trino.3dsx
cargo xtask run 3ds       # build + open in Azahar (auto-detected)
cargo xtask test 3ds      # build test app + assert magic strings (Azahar log)
cargo xtask watch 3ds     # rebuild .3dsx + relaunch Azahar on save
cargo xtask new <name>    # scaffold a game crate under examples/
```

Before committing, always run the CI-equivalent locally:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Testing conventions

- Every feature ships with tests in the same PR. No exceptions — the roadmap lists the
  expected tests per phase.
- Unit tests live next to the code (`#[cfg(test)]`). `core` uses
  `#![cfg_attr(not(test), no_std)]` so tests can use std.
- Golden-image tests (Fase 1+) live in `tests/golden/`; regenerate only via
  `cargo xtask test --bless` and review the diff in the PR.
- Console tests report through debug channels — ISViewer magic strings on
  N64 (`apps/n64/src/lib.rs::run_self_test`), `svcOutputDebugString` on 3DS
  (`apps/3ds/src/main.rs::run_self_test`, read from Azahar's log file).
  Emulator tests run locally via `cargo xtask test n64|3ds`; CI builds the
  ROM/.3dsx but does not boot emulators yet.

## Toolchain notes

- Workspace builds on **stable** (see `rust-toolchain.toml`). Console target
  builds run on **nightly** with `-Zbuild-std=core,alloc`: the N64 against
  `platforms/n64/mips-nintendo64-none.json` (plus `-Zjson-target-spec`), the
  3DS against the built-in `armv6k-nintendo-3ds` — never change pins or the
  target spec casually; they are verified combinations.
- Windows: N64 builds need Docker reachable from WSL2 (Docker Desktop optional —
  a plain in-WSL dockerd works; xtask calls `wsl docker ...`). 3DS builds need
  a local devkitPro install (`3ds-dev` packages). PC development needs nothing
  beyond Rust.

## Pitfalls

- Do not add dependencies to `trino-core` — the dep-graph test will fail, and the crate
  must compile for MIPS bare metal.
- Screen space is X-right, **Y-down**, origin top-left; stick input is Y-up.
- `SpriteId`/`SoundId` handles are stable across live reloads (derived from logical
  asset paths). Never cache anything keyed on their numeric value being contiguous.
- Hot-reload boundary: no generics in exports, no statics in the game dylib (they reset
  on reload), no `TypeId` across the boundary, and state layout must not change between
  reloads (that requires a restart). Exports are generated by
  `trino_game_api::export_game!` — see `crates/game-api/src/lib.rs`. `cargo xtask watch pc`
  deliberately watches only `examples/`: a change to `crates/core` can change type
  layouts, and swapping a dylib across a layout change is UB.
- Game crates use `#![cfg_attr(any(target_os = "none", target_os = "horizon"), no_std)]`
  (N64 is `none`, 3DS is `horizon`) — std exists on PC only for the dylib's
  panic handler; game code must never call std APIs.
- Console FFI goes **only** through the C shims (`crates/platform-{n64,3ds}/shim/`
  + the matching `src/ffi.rs`, kept in pairs), and every entry must stay
  within ≤4 scalar/pointer args, no by-value structs, no variadics, no 64-bit
  values across the boundary. On the N64 this is a hard ABI requirement
  (Rust is o32, libdragon is o64 — `docs/adr/0002-n64-abi-o32-staticlib.md`);
  on the 3DS it is convention (citro2d is static-inline C, and symmetry keeps
  the backends maintainable). Never call libdragon/libctru/citro2d from Rust.
