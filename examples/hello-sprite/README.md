# hello-sprite (legacy)

The minimal example from the engine's first, 2D-only phase: a sprite that moves with the
stick or D-pad and beeps on A.

It is **legacy** and not the model for new games. Trino is a low-poly 3D engine whose
renderer also draws 2D (sprites, text, HUDs, billboards) in the same frame; see
[`examples/castle64`](../castle64) and start a new game with `cargo xtask new <name>`.

The crate stays in the workspace because the hot-reload end-to-end test
(`xtask/tests/hot_reload.rs`) builds a patched copy of it. Do not rename it or change its
`SPEED` constant without updating that test.

A redesigned 2D sprite and tile system is planned for a future release
([docs/BACKLOG.md](../../docs/BACKLOG.md)).
