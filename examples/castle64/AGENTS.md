# castle64 — Agent Guide

Trino's **3D showcase**: an SM64-style game (castle hub + 3 platforming
stages + throne room) and the default game the three `apps/*` launch. The
engine-wide guide (repository root `AGENTS.md`) is the canonical reference;
this file covers what is specific to this crate.

## Map

- `lib.rs` — game state, inertial movement (accel/turn/variable jump),
  pickups, portals, stomp combat, articulated-player rendering, HUD.
- `physics.rs` — 3D AABB collision, axis-resolved with a contact **skin**
  (regression-tested: without it, f32 rounding made landed players snag on
  the side faces of the block they stand on) + a slab raycast for the
  camera.
- `levels.rs` — const level data (blocks/movers/boars/coins/stars/portals).
- `bot.rs` — deterministic waypoint bot that PLAYS the game: the PC test
  suite runs the full 4-star playthrough; the console self-tests run the
  first stage inside ares/Azahar.

## Rules for game code

- Depend **only** on `trino-core` (+ `trino-game-api`). No external crates,
  no std APIs — `xtask/tests/dep_graph.rs` enforces it.
- Keep `update` deterministic: same inputs + same state = same result.
  Audio calls are fire-and-forget and excluded from determinism.
- Assets are declared in the repo's `assets/manifest.toml` under `c64_*`
  logical names. Generated masters (blocks, player parts, boar, doors,
  HUD digits) regenerate via `cargo xtask gen-assets`
  (`xtask/src/castle64_assets.rs`). The castle entrance
  (`door_kaykit.glb`) is a REAL low-poly asset — KayKit Dungeon Remastered,
  CC0, license vendored next to the file — baked from textured glTF by the
  asset pipeline (texture sampled into vertex colors).
- N64 budget: the tri-budget unit test + PC strict mode keep every scene
  under `Caps::N64.max_tris_per_frame`; mind it when adding geometry.
- Hot-reload boundary: no statics (they reset on reload), no `TypeId`
  across the boundary, and state layout must not change between reloads.

## Commands

```
cargo test -p castle64            # unit tests incl. the bot playthroughs
cargo xtask run pc                # play it (N64 look by default)
cargo xtask watch pc              # hot-reload session (this is the default game)
cargo xtask test n64|3ds          # emulator suites (bot plays stage 1)
cargo test -p xtask --test c64_shots -- --ignored   # headless screenshots
cargo test -p xtask --test c64_gif -- --ignored     # gameplay GIF
```

Controls (PC keys): stick/d-pad = WASD/arrows, A (Z) jump — hold for
height, L/R (Q/E) orbit the camera, B (X) back to hub, Start (Enter)
restart the stage.
