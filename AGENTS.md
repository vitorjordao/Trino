# Trino — Agent Guide

Trino is a Rust game engine targeting **Nintendo 64, Nintendo 3DS and PC** from a single
codebase, with a visual editor, console-simulation modes, live reload and emulator-based
testing. This file is the canonical entry point for AI agents (and a good one for humans).

Roadmap and per-phase acceptance criteria: `PLANO_EXECUCAO_TRINO.md` (Portuguese).
Original architecture rationale: `PLANO_ENGINE_TRINO.md` (Portuguese).

## Current state

**Fase 1 (PC 2D) complete.** The PC path renders sprites through wgpu with
console-simulation resolutions (offscreen 320x240 + nearest integer upscale), plays
audio through cpal, reads keyboard input, and has golden-image tests. Consoles, asset
pipeline, live reload and editor are still ahead — check `PLANO_EXECUCAO_TRINO.md` for
what exists vs. planned.

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
cargo xtask test          # full workspace test suite (same as CI)
cargo xtask build n64     # Fase 4 (Docker + libdragon)
cargo xtask build 3ds     # Fase 5 (devkitARM + cargo-3ds)
cargo xtask watch pc      # Fase 2 (live reload)
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
- Hot-reload boundary (Fase 2+): everything `#[repr(C)]`, no generics, no statics in the
  game dylib, no `TypeId` across the boundary. See `crates/game-api/src/lib.rs`.
