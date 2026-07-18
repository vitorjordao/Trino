# {{name}} — Agent Guide

A Trino game crate. The engine-wide guide (repository root `AGENTS.md`) is
the canonical reference; this file covers what is specific to game code.

## Rules for game code

- Depend **only** on `trino-core` (+ `trino-game-api`). No external crates,
  no std APIs — `xtask/tests/dep_graph.rs` enforces it. That is what makes
  this game run on PC, N64 and 3DS by construction.
- Keep `update` deterministic: same inputs + same state = same result.
  Audio calls are fire-and-forget and excluded from determinism.
- Assets are referenced by logical path
  (`SpriteId::from_path("sprites/hero")`) and declared in the repo's
  `assets/manifest.toml`.
- Screen space is X-right, **Y-down**, origin top-left; stick input is Y-up.
- Hot-reload boundary: no statics (they reset on reload), no `TypeId`
  across the boundary, and state layout must not change between reloads.

## Commands

```
cargo test -p {{name}}          # this game's unit tests
cargo xtask run pc              # run the workspace's default game on PC
cargo xtask watch pc --game {{name}}   # hot-reload session for THIS game
cargo xtask test n64|3ds        # console emulator suites
```

To make this game the one the apps launch, point `apps/*` at
`{{name_snake}}::{{name_camel}}Game` (see how `examples/platformer` is wired) —
including the `hot_module` dylib name in `apps/pc/src/main.rs`, which must be
this crate's name for `watch pc --game {{name}}` to hot-swap it.
