# Evidence for PR #89: shadows per tier (issue #57)

Captured on the PR's branch (`feat/57-shadows`), October 2026. Before/after pairs
compare against `main` at the merge base.

| Folder | What |
|---|---|
| `pc/` | The three PC goldens of the shadows test scene, one per tier (native: map; `3ds` profile: planar; `n64` profile: blobs + 2 projected) |
| `n64/` | ares: `shadows_far` (blobs from 4 m to 100 m on one floor, on the RDP's 18-bit z, no z-fighting) and castle64 before/after |
| `3ds/` | Azahar: castle64 before/after (hand-made disc vs the body's planar shadow) |
| `web/` | castle64 in Chromium (WebGL2, WebGPU), Firefox and WebKit; before/after |
| `games/` | castle64 on every platform, Solid Eleven on the PC, the football template on ares |
| `editor/` | Inspector › Shadows, the Shadow component, the Game view per platform, the Scene view's debug mode, the budget |
| `logs/` | ares (both RAM tiers), Azahar (both models), the browsers, the football template, `scene budget` (text and `--json`), tests, lints, CI smokes |
