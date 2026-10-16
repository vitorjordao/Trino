# platformer (legacy)

A 2D tilemap platformer from the engine's first phases: run, jump, collect the coins,
reach the flag. The level is plain ASCII (`src/level1.txt`), parsed by
`trino_core::tilemap`, with collision from `trino_core::collide` and a music loop.

It is **legacy** and not the model for new games. Trino is a low-poly 3D engine whose
renderer also draws 2D (sprites, text, HUDs, billboards) in the same frame; see
[`examples/castle64`](../castle64) and start a new game with `cargo xtask new <name>`.

The crate stays in the workspace because other parts use it:

- the PC golden test `platformer_scene_matches_golden` (`crates/platform-pc/tests/golden.rs`);
- the editor's **Level** tab, which paints `src/level1.txt` and reloads it when another
  tool writes it;
- the media tools that record `docs/media/platformer.*` (`xtask/tests/hero_shot.rs`,
  `xtask/tests/record_gif.rs`).

A redesigned 2D sprite and tile system is planned for a future release
([docs/BACKLOG.md](../../docs/BACKLOG.md)).
