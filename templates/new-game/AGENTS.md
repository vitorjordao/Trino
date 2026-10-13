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
- 3D levels: author them in the editor or as RON (`scenes/*.scene.ron`, skill
  `scene-authoring`), then `cargo xtask scene export scenes/<x>.scene.ron
  examples/{{name}}/src/<x>_gen.rs` and `include!` the result — the scene is
  plain `no_std` data (`trino_core::SceneNode`): `scene::draw_nodes`,
  `scene::find(NODES, "spawn")`, `scene::with_tag(NODES, "enemy")`. See
  `examples/showcase/src/lib.rs`; add a drift test like
  `xtask/tests/scene_codegen.rs`.
- Surfaces are data: `ModelParams::tinted(c)` or
  `ModelParams { shading: Shading::Flat, ..ModelParams::DEFAULT }`; animated
  models take `anim: Some(AnimRef { clip: ClipId::from_name("walk"), time })`.
- Hot-reload boundary: no statics (they reset on reload), no `TypeId`
  across the boundary, and state layout must not change between reloads.

## Engine services (all `no_std`, identical on PC / N64 / 3DS)

- **Platform services** through `Game::update_with(platform, …)`:
  - `platform.blob(id)` returns `'static` bytes from `[blobs.*]` / `[scenes.*]`;
  - `save_read` / `save_write` store a framed save (EEPROM on N64, SD on 3DS).
- **Text**: `renderer.draw_text(FontId::from_path("fonts/…"), pos, s, &TextParams::DEFAULT)`
  with `TextBuf<N>` for numbers.
- **Levels**:
  - from code: `scene export`;
  - from data: `BakedScene::parse(blob)`;
  - live objects: `EntityStore<N>` (spawn, query by tag, `draw`).
- **Collision**: `trino_core::physics` (`move_aabb`, `raycast`, triggers), using the
  colliders authored in the editor.
- **Rendering**:
  - animation crossfade: `AnimRef::new(clip, t).blend_to(other, t2, w)`;
  - billboards and particles: `draw_billboard`, `ParticleSystem`;
  - distance LOD comes from the scene and is applied by `draw_nodes(.., eye)`;
  - interior culling: `Cells::from_nodes` + `visible` + `draw_nodes_in`.
- **Audio**: `play_sound_with(id, &params)`, voices, `play_stinger`,
  `Listener` + `spatialize`, and `Ambience` for scene `sound` sources.

Every one of these is exercised on the consoles by `castle64::selftest`. Read it for
working examples.

## Skills

Workflows (adding assets, Blender models, scenes, editor, console builds,
tests) live as open-format skills in the repository's `.agents/skills/`.

## Commands

```
cargo test -p {{name}}          # this game's unit tests
cargo xtask run pc              # run the workspace's default game on PC
cargo xtask watch pc --game {{name}}   # hot-reload session for THIS game
cargo xtask test n64|3ds        # console emulator suites
cargo xtask scene check         # validate scenes / prefabs / materials
cargo xtask model inspect <glb> # check a model against the engine's limits
```

To make this game the one the apps launch, point `apps/*` at
`{{name_snake}}::{{name_camel}}Game` (see how `examples/platformer` is wired) —
including the `hot_module` dylib name in `apps/pc/src/main.rs`, which must be
this crate's name for `watch pc --game {{name}}` to hot-swap it.
