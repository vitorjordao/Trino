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
| `TRINO_EDITOR_SELECT=<name>[,<name>…]` | start with those entities selected (the first is active; gizmo + inspector visible) |
| `TRINO_EDITOR_TAB=scene\|game\|level` | focused center tab |
| `TRINO_EDITOR_OPEN=fake-object` / `array` / `material:<logical>` / `model:<logical>` / `prefab:<logical>` | open the fake-object or array dialog, an asset inspector, or a prefab in edit mode |
| `TRINO_EDITOR_PALETTE=<query>` | open the command palette (Ctrl+K) with that query |
| `TRINO_EDITOR_PLAY=here` | start **Play here** on the first frame: the scene runs inside the Game view (CI smoke) |
| `TRINO_EDITOR_ECHO_LOG=1` | mirror the Console to stderr (see reloads, errors, saves) |
| `TRINO_EDITOR_SMOKE_FRAMES=N` | close after N frames (CI smoke test) |

Example — look at an entity in the Game view, then read the PNG:

```
TRINO_EDITOR_TAB=game TRINO_EDITOR_SELECT=robot-walker \
TRINO_EDITOR_SCREENSHOT=/tmp/shot.png TRINO_EDITOR_SCREENSHOT_FRAME=60 cargo run -p trino-editor
```

Keys worth knowing (F1 lists all):
- Ctrl+K: the command palette, with every menu action searchable.
- Ctrl+Shift+P / Esc: start / stop Play here.
- Ctrl/Shift+click and Shift+drag: multi-select and box select.
- The **Arrange** menu: align, distribute, array, drop to surface.
- Dropping a `.glb`, `.gltf` or `.png` on the window imports it and writes the
  manifest entry.

The Game view shows a **budget HUD** (tris, sprites, texture bytes vs the console's
`Caps`) and culls interior cells like the game does. The Scene view draws collider,
cell and portal wireframes.

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
| `play.rs` | Play here: the scene as a game inside the Game view (keys → `InputState`) |
| `ops.rs` | arrange operations (align, distribute, array, drop to surface, group delete) — pure, unit-tested |
| `palette.rs` | command-palette matcher (the app builds the item list from its menus) |
| `budget.rs` | Game-view budget HUD rows from `FrameStats` vs `Caps` |
| `ui_tests.rs` | headless UI flows with egui_kittest (real `App` + wgpu): drive panels by accessibility label |

## Rules when changing the editor

- Keep everything text-backed: a UI action must end in a file an agent could also write.
- New structural edits go through `Cmd` (undo + echo guards stay correct).
- Watcher paths are compared without `./` (`Library::logical_for_file` normalizes) — use
  root `Path::new("")`, never `"."`.
- Views re-render only when their `RenderKey` changes; request a repaint for anything
  animated (toasts, flashes, camera easing).
- Hand-painted widgets must expose `widget_info` (accessibility), otherwise screen
  readers and the kittest flows cannot find them (see the Hierarchy rows).
- Extend `ui_tests.rs` for new panel flows. It opens a temp copy of a scene, so it
  never writes repository files.
- After UI changes, take a screenshot with the hooks above and look at it before claiming
  it looks right.
