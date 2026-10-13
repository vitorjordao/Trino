# {{name}} — Agent Guide

A standalone [Trino](https://github.com/vitorjordao/Trino) game: one
codebase for PC, Nintendo 64 and Nintendo 3DS. This file is the entry point
for AI agents and humans.

## Layout

| Path | What it is |
|---|---|
| `game/` | The game crate (`{{name_snake}}`). Portable: depends only on `trino-core` + `trino-game-api`. |
| `pc/` | Desktop host (`trino-runner-pc`): window, renderer, audio, input, asset bake. |
| `assets/manifest.toml` | Every asset by logical path; masters in `assets/shared/`. |
| `Cargo.toml` | Workspace; the engine dependencies (path or git) are declared once here. |

## Rules for game code

- `game/` depends **only** on `trino-core` (+ `trino-game-api`): no external
  crates, no std APIs. That is what lets it run on the consoles unchanged.
- Keep `update` deterministic: same inputs + same state = same result.
- Assets are referenced by logical path and declared in `assets/manifest.toml`.
- Screen space is X-right, **Y-down**, origin top-left; stick input is Y-up.
- If a feature cannot work on the N64, the engine does not expose it —
  budgets live in `trino_core::Caps`.

## Commands

```
cargo run -p {{name}}-pc                 # play (bakes assets on start; saving a master reloads it)
TRINO_SIM=3ds cargo run -p {{name}}-pc   # 3DS look (native = full-resolution PC)
cargo test                              # the game's unit tests
cargo clippy --all-targets -- -D warnings
```

Console type-check of the game crate (needs nightly + `rust-src`):

```
cargo +nightly check -Zbuild-std=core,alloc --target armv6k-nintendo-3ds -p {{name}} --release
```

Console **packaging** (`.z64` / `.3dsx`), the editor, scenes and the AI
skills live in the engine repository: add the game there as
`examples/{{name}}` (`cargo xtask new {{name}}` in the engine) or point its
`apps/*` at this crate. See the engine's `AGENTS.md` and `.agents/skills/`.
