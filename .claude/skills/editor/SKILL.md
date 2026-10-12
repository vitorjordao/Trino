---
name: editor
description: Run, drive and observe the Trino editor (Scene/Game views, hierarchy, inspector, assets, prefabs, materials, fake objects) — including headless-friendly hooks to screenshot the UI, preselect entities and mirror the console to stderr so an AI agent can see what a human sees. Use for "open the editor", "show me the scene", "screenshot the UI", or when changing editor code.
---

# editor

```
cargo xtask editor                    # = cargo run -p trino-editor
```

Opens `scenes/showcase.scene.ron` (override: `TRINO_EDITOR_SCENE=scenes/x.scene.ron`).

## Hooks for agents and tests

| Env var | Effect |
|---|---|
| `TRINO_EDITOR_SCREENSHOT=out.png` | save a PNG of the whole window, then exit |
| `TRINO_EDITOR_SCREENSHOT_FRAME=N` | …after N frames (default 30; use ~60 so thumbnails finish) |
| `TRINO_EDITOR_SELECT=<entity name>` | start with that entity selected (gizmo + inspector visible) |
| `TRINO_EDITOR_TAB=scene\|game\|level` | focused center tab |
| `TRINO_EDITOR_OPEN=fake-object` / `material:<logical>` / `model:<logical>` / `prefab:<logical>` | open the fake-object dialog, an asset inspector, or a prefab in edit mode |
| `TRINO_EDITOR_ECHO_LOG=1` | mirror the Console to stderr (see reloads, errors, saves) |
| `TRINO_EDITOR_SMOKE_FRAMES=N` | close after N frames (CI smoke test) |

Example — look at an entity in the Game view, then read the PNG:

```
TRINO_EDITOR_TAB=game TRINO_EDITOR_SELECT=robot-walker \
TRINO_EDITOR_SCREENSHOT=/tmp/shot.png TRINO_EDITOR_SCREENSHOT_FRAME=60 cargo run -p trino-editor
```

Live-edit check: start the editor with `TRINO_EDITOR_ECHO_LOG=1` in the background, edit a
`.material.ron` / `.prefab.ron` / scene, and watch for `… updated` / `… reloaded` lines.

## What's where (crates/editor/src)

| File | Role |
|---|---|
| `app.rs` | state, `Cmd` queue (panels never mutate structure directly), menus, toolbar, status bar, file sync, shortcuts, undo commit |
| `doc.rs` | the document (scene or prefab being edited) + snapshot undo/redo (auto-coalescing) |
| `viewport.rs` | Scene/Game views through the real `PcRenderer`, overlays, picking, drag-and-drop |
| `gizmo.rs` | orbit camera, projection (same math as `tessellate`), move/rotate/scale gizmo |
| `panels/{hierarchy,inspector,assets}.rs` | the panels |
| `dialogs.rs` | fake object (live preview), new material/scene, make prefab, export |
| `project.rs` | asset catalog, thumbnails, fake-object/manifest file operations |
| `theme.rs` | palette, fonts (Inter, JetBrains Mono, Phosphor icons), shared widgets |

## Rules when changing the editor

- Keep everything text-backed: a UI action must end in a file an agent could also write.
- New structural edits go through `Cmd` (undo + echo guards stay correct).
- Watcher paths are compared without `./` (`Library::logical_for_file` normalizes) — use
  root `Path::new("")`, never `"."`.
- Views re-render only when their `RenderKey` changes; request a repaint for anything
  animated (toasts, flashes, camera easing).
- After UI changes, take a screenshot with the hooks above and look at it before claiming
  it looks right.
