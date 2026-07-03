# Trino — Agent Guide

Trino is a Rust game engine targeting **Nintendo 64, Nintendo 3DS and PC** from a single
codebase, with a visual editor, console-simulation modes, live reload and emulator-based
testing. This file is the canonical entry point for AI agents (and a good one for humans).

Roadmap and per-phase acceptance criteria: `PLANO_EXECUCAO_TRINO.md` (Portuguese).
Original architecture rationale: `PLANO_ENGINE_TRINO.md` (Portuguese).

## Current state

**Fase 2 (asset pipeline + live reload) complete.** Working today:

- PC 2D rendering (wgpu, console-sim resolutions, golden tests), audio, input.
- Asset pipeline: `assets/manifest.toml` + shared masters + per-platform overrides →
  `cargo xtask assets pc`. Handles = fnv1a(logical path), stable across reloads.
- Live reload: `cargo xtask watch pc` — saving a master rebakes + re-uploads it live;
  saving game source rebuilds the dylib and hot-lib-reloader swaps functions under the
  running state (`apps/pc` feature `reload`).

Consoles and the editor are still ahead — check `PLANO_EXECUCAO_TRINO.md`.

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
cargo xtask gen-assets    # regenerate sample masters (dev utility)
cargo xtask build n64     # Fase 4 (Docker + libdragon)
cargo xtask build 3ds     # Fase 5 (devkitARM + cargo-3ds)
cargo xtask new <name>    # Fase 8 (scaffold a game)
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
- Console tests (Fase 4/5) report through debug channels (ISViewer magic strings on N64,
  GDB-stub exit codes on 3DS) and run in emulators in CI.

## Toolchain notes

- Workspace builds on **stable** (see `rust-toolchain.toml`). Console targets pin their
  own nightly in later phases — never change pins casually; they are verified
  combinations.
- Windows: N64 builds (Fase 4) require Docker Desktop. PC development needs nothing
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
- Game crates use `#![cfg_attr(target_os = "none", no_std)]` — std exists on PC only for
  the dylib's panic handler; game code must never call std APIs.
