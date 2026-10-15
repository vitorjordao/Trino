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
| `TRINO_EDITOR_TAB=scene\|game\|level\|animation` | focused center tab |
| `TRINO_EDITOR_ANIM=<model>:<clip>@<time>` | the Animation tab previews that clip, paused at that time |
| `TRINO_EDITOR_ANIM_VIEW=ghosts=3,blend=wave:0.5,look=n64,palette` | its view options (onion skin, crossfade, look, palette) |
| `TRINO_EDITOR_OPEN=fake-object` / `array` / `material:<logical>` / `model:<logical>` / `prefab:<logical>` | open the fake-object or array dialog, an asset inspector, or a prefab in edit mode |
| `TRINO_EDITOR_PALETTE=<query>` | open the command palette (Ctrl+K) with that query |
| `TRINO_EDITOR_PLAY=here` | start **Play here** on the first frame: the scene runs inside the Game view (CI smoke) |
| `TRINO_EDITOR_ECHO_LOG=1` | mirror the Console to stderr (see reloads, errors, saves) |
| `TRINO_EDITOR_GAME=pc\|n64\|3ds` | the Game view's profile (default n64) — screenshots of each platform's variant |
| `TRINO_EDITOR_SIZE=960x600` | window size (default 1600x940; never below the 960x600 minimum) — screenshot small windows |
| `TRINO_EDITOR_INSPECT=platforms\|lod\|impostor` | the Inspector opens that section and scrolls to it |
| `TRINO_EDITOR_LOD_PREVIEW=<m>` | preview the selected entity's LOD at that distance (both views) |
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
`Caps`; under the N64 profile also **N64 3D time** — this frame's T&L work through the
calibrated cost model `trino_core::perf::N64_COST`, against one 30 fps frame) and culls interior cells like the game does. The Scene view draws collider,
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
| `anim_panel.rs` | the Animation tab: its own preview renderer, clips, transport, crossfade, measurements (`trino_asset_pipeline::anim`), onion skin, palette, looks, console pose export |
| `panels/lod.rs` | Inspector › Model › LOD: the chain (mesh / impostor / cull), per-platform starts, cost per level, the preview slider |
| `panels/impostor.rs` | Model asset › Impostor: settings, Bake (the same `impostor::bake` as the CLI), atlas preview, fresh/stale badge |
| `panels/platforms.rs` | Inspector › Platforms: per-platform variants (grid of PC/N64/3DS columns, stacked when narrow) |
| `layout_audit.rs` | test helper: no widget off the window, none overlapping (accesskit rects) |
| `ui_tests.rs` | headless UI flows with egui_kittest (real `App` + wgpu): drive panels by accessibility label |
| `widget_tests.rs` | widget tests with egui_kittest and no renderer (no GPU, not ignored): inspector editors, Console |

## Rules when changing the editor

- Keep everything text-backed: a UI action must end in a file an agent could also write.
- New structural edits go through `Cmd` (undo + echo guards stay correct).
- Watcher paths are compared without `./` (`Library::logical_for_file` normalizes) — use
  root `Path::new("")`, never `"."`.
- Views re-render only when their `RenderKey` changes; request a repaint for anything
  animated (toasts, flashes, camera easing).
- Hand-painted widgets must expose `widget_info` (accessibility), otherwise screen
  readers and the kittest flows cannot find them (see the Hierarchy rows).
- Extend `ui_tests.rs` for new panel flows (run them with `cargo test -p trino-editor
  ui_tests -- --ignored --test-threads=1`: they set the cwd and env). They open a temp
  copy of a scene, so they
  never write repository files. A flow that runs file-writing `Cmd`s (save, prefabs,
  materials, fake objects, new scene) copies `assets/`, `scenes/`, `prefabs/` and the
  platformer level into a temp dir and runs the `App` there
  (`commands_edit_save_and_export_in_a_temp_project`); push `Cmd`s into
  `h.state_mut().cmds` and step two frames.
- A widget that only needs `&mut egui::Ui` and data (no `App`) is tested without a GPU
  in `widget_tests.rs` (`Harness::new_ui_state`). Install the theme on the first frame
  (`theme::install`) before drawing anything that uses the bold font family, or epaint
  panics.
- Gizmo hit tests: `point_in_quad` treats an edge-on (zero-area) quad as empty. Before,
  in a side view the ground-plane handle took clicks meant for the X axis.
- After UI changes, take a screenshot with the hooks above and look at it before claiming
  it looks right — at the minimum size too (`TRINO_EDITOR_SIZE=960x600`).
- **Layout must hold at 960x600.** `ui_tests::editor_layout_is_clean_at_minimum_and_default_sizes`
  runs `layout_audit` over the whole window; widget tests run it on a panel at several
  widths. What broke before it existed: the Inspector's sliders and long check box
  labels widened the panel past the window (one wide row widens every row after it in an
  egui `Ui`), the Assets tabs ran under the Inspector, and a long scene path overlapped
  the toolbar. The rules that fixed them:
  - rows go through `theme::prop` (truncating text, slider width from the space left)
    and `theme::vec3_row` (fewer decimals when narrow);
  - a combo followed by an icon button is `available_width() - 38`;
  - long free text in a row is `Label::truncate()` (or `elide_start` for paths);
  - tab strips use `horizontal_wrapped`;
  - the dock gives the Hierarchy ≥ 200 px and the Inspector ≥ 300 px (`default_dock(width)`,
    built on the first frame).
- **The layout audit judges overlaps on what shows.** Accessibility rects are unclipped,
  so a scroll area's hidden rows "overlapped" the Assets panel under the Animation tab.
  `layout_audit::widgets(h, Some(&h.ctx))` matches each node to egui's own widget rect and
  uses its clipped `interact_rect` (sideways overflow still uses the full rect: text wider
  than its panel is a bug even when clipped).
- **Icon buttons need an accessible name** (`anim_panel::named_icon`): a tooltip is not a
  label, so kittest (and a screen reader) can't find "Pause" by its glyph.
- **Kittest clicks need the widget on screen.** A panel's content below the fold has rects
  past the window or its scroll area; `ui_tests::click` wheels the panel (over a visible
  part of it) until the widget fully shows. Items of an open combo list are clicked with
  `click_accesskit()` (the list scrolls on its own), which leaves the list open: close it
  on its combo before the next click.
- **A truncating `ComboBox` sizes its text to the space *available*, not to its
  `.width()`.** With a long selected name it grew to the panel's edge and pushed the
  button after it out of the window (`layout_audit` caught it). Put such a combo in
  `theme::fixed_width(ui, w, ..)`. Don't add `widget_info` to a `Slider`'s response: it is
  the union of the track and its value box, so the audit sees them overlap.
- Impostors in the views: `DrawItem::lod_from`/`lod_at` give a `LodPick`; an impostor
  draws through `Impostor::draw` (a billboard). The N64 Game view uploads the N64 atlas
  (`Project::upload_platform_sprites`) so its regions match the N64 table.
- **The Game view resolves its own platform** (`App::resolved_game`, `viewport::target_of`):
  the Scene view, picking and the Inspector use the PC resolve.
