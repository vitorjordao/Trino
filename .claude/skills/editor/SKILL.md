---
name: editor
description: Run, drive and observe the Trino editor (Scene/Game views, hierarchy, inspector, assets, prefabs, materials, fake objects) — including headless-friendly hooks to screenshot the UI, preselect entities and mirror the console to stderr so an AI agent can see what a human sees. Use for "open the editor", "show me the scene", "screenshot the UI", or when changing editor code.
---

# editor

```
cargo xtask editor                    # the project launcher (= cargo run -p trino-editor)
cargo xtask editor <dir>              # straight into the project at <dir>
cargo xtask editor .                  # the engine's own content, as before the launcher
cargo trino editor                    # inside a standalone project: that project
```

A project opens on `scenes/showcase.scene.ron` (override:
`TRINO_EDITOR_SCENE=scenes/x.scene.ron`); the editor works relative to the project
folder, which becomes the working directory.

## Project launcher (`launcher.rs`, `recent.rs`)

- **When it shows:** with no project argument. A hook that acts on an open scene
  skips it and opens the working directory as before. The hooks are
  `TRINO_EDITOR_SCENE`, `SELECT`, `TAB`, `OPEN`, `PLAY`, `INSPECT`, `ANIM`,
  `ANIM_VIEW`, `PALETTE`, `GAME` and `LOD_PREVIEW` (`main.rs::EDITOR_HOOKS`).
  `--launcher` or `TRINO_EDITOR_LAUNCHER=1` always show it. The rule is
  `main.rs::start`, unit-tested.
- **Recent projects:** each row shows the name, the path (elided at the start), the
  time since it was last opened, **Open**, and **Forget** (×).
  - They are kept in `recent-projects.txt`, one `<unix secs>\t<path>` per line,
    newest first, 12 at most. Lines that don't parse are dropped.
  - The file lives in `$XDG_CONFIG_HOME/trino` or `~/.config/trino` (Linux),
    `~/Library/Application Support/trino` (macOS) or `%APPDATA%\trino` (Windows).
    It never lives in a repository.
  - `TRINO_EDITOR_RECENT=<file>` moves it. Tests and screenshots must set it, so
    they never touch the real one.
- **Open existing:** a path field, checked by `launcher::check_project`.
  - It needs a folder with a `trino.toml`, or an `assets/manifest.toml` (the
    engine).
  - **Engine demo scenes** opens the engine checkout.
- **Create new project:** name, folder and template (Blank, or based on the castle64
  or showcase demo).
  - It calls `trino_scaffold::scaffold`, the same code as
    `cargo xtask new --standalone --template`, then opens the result.
  - Templates come from `$TRINO_ENGINE` or the checkout the editor was built in.
    Without one the Create button is disabled and a warning says why.
- **Opening is in-process:** `enter_project` changes the working directory and
  records the recent entry, then `App::open(ctx, render_state)` builds the editor.
  `App::new(cc)` is a thin wrapper over it, which the kittest flows use. The
  launcher runs the screenshot and smoke hooks itself (`app::screenshot_hook`).
- **Glyphs:** the editor's fonts have no `→`. It drew a box in the launcher's first
  screenshot, so write words or use a Phosphor icon.
- **Screenshots after a change:** run `cargo build -p trino-editor` first.
  `cargo test` and `clippy` leave the old `target/debug/trino-editor` in place, and
  the hooks then capture the old UI.
- **No stray folders:** the watcher no longer creates `examples/platformer/src` in a
  project that lacks it. The Level tab then shows its "file not found" note.

Screenshot of the launcher:

```
TRINO_EDITOR_LAUNCHER=1 TRINO_EDITOR_RECENT=target/recent-demo.txt \
TRINO_EDITOR_SCREENSHOT=target/launcher.png TRINO_EDITOR_SCREENSHOT_FRAME=10 cargo run -p trino-editor
```

## Hooks for agents and tests

| Env var | Effect |
|---|---|
| `TRINO_EDITOR_SCREENSHOT=out.png` | save a PNG of the whole window, then exit |
| `TRINO_EDITOR_SCREENSHOT_FRAME=N` | …after N frames (default 30; use ~60 so thumbnails finish) |
| `TRINO_EDITOR_SELECT=<name>[,<name>…]` | start with those entities selected (the first is active; gizmo + inspector visible) |
| `TRINO_EDITOR_TAB=scene\|game\|level\|animation` | focused center tab |
| `TRINO_EDITOR_ANIM=<model>:<clip>@<time>` | the Animation tab previews that clip, paused at that time |
| `TRINO_EDITOR_ANIM_VIEW=ghosts=3,blend=wave:0.5,look=n64,palette` | its view options (onion skin, crossfade, look, palette) |
| `TRINO_EDITOR_OPEN=fake-object` / `array` / `material:<logical>` / `model:<logical>` / `prefab:<logical>` / `particles:<name>[@pc\|n64\|3ds]` | open the fake-object or array dialog, an asset inspector, or a prefab in edit mode |
| `TRINO_EDITOR_PALETTE=<query>` | open the command palette (Ctrl+K) with that query |
| `TRINO_EDITOR_PLAY=here` | start **Play here** on the first frame: the scene runs inside the Game view (CI smoke) |
| `TRINO_EDITOR_ECHO_LOG=1` | mirror the Console to stderr (see reloads, errors, saves) |
| `TRINO_EDITOR_GAME=pc\|n64\|3ds` | the Game view's profile (default n64) — screenshots of each platform's variant |
| `TRINO_EDITOR_SIZE=960x600` | window size (default 1600x940; never below the 960x600 minimum) — screenshot small windows |
| `TRINO_EDITOR_INSPECT=platforms\|lod\|impostor\|budget` | the Inspector opens that section and scrolls to it (`budget`: the scene settings' platform budget, with nothing selected) |
| `TRINO_EDITOR_LOD_PREVIEW=<m>` | preview the selected entity's LOD at that distance (both views) |
| `TRINO_EDITOR_SMOKE_FRAMES=N` | close after N frames (CI smoke test; the launcher too) |
| `TRINO_EDITOR_LAUNCHER=1` | show the project launcher even when a scene hook is set (screenshots) |
| `TRINO_EDITOR_RECENT=<file>` | the launcher's recent-projects file (tests: never the user's own) |
| `TRINO_ENGINE=<checkout>` | the engine checkout the launcher creates projects from |

Example — look at an entity in the Game view, then read the PNG:

```
TRINO_EDITOR_TAB=game TRINO_EDITOR_SELECT=robot-walker \
TRINO_EDITOR_SCREENSHOT=/tmp/shot.png TRINO_EDITOR_SCREENSHOT_FRAME=60 cargo run -p trino-editor
```

Keys worth knowing (F1 lists all):
- Scene view navigation (Unity-style, `viewport.rs` › camera controls):
  - MMB drag pans, with or without Ctrl/Cmd or Shift; Shift+RMB pans too.
  - Alt+LMB orbits. Alt+RMB dollies (drag up = in). The wheel zooms.
  - Hold RMB to fly: mouselook, WASD along the view, Q/E down/up, Shift ×2.5,
    the wheel sets the speed. Flying switches an orthographic view back to
    perspective.
  - Dropping an asset in an orthographic view: the ground hit is used only near
    the target, else the plane through the target facing the camera
    (`gizmo::drop_point`). A ray times a constant would land kilometres back.
  - 1/3/7 (+Ctrl) give the axis views; 5 toggles orthographic.
- The **axis widget** (top right) is Blender-style:
  - Click a tip to look from that axis; click it again for the opposite side
    (`OrbitCam::snap_to_axis`, judged on the goal yaw/pitch).
  - Drag anywhere on it to orbit.
  - Click its centre or the Persp/Ortho label below it to switch projection.
    Looking down an axis, that axis's tips cover the centre and win the click.
- Gizmo:
  - Translate has three planar squares (`Handle::Plane(normal axis)`).
  - Rotate has an outer view ring (`Handle::ViewRing`, around the view axis).
  - While dragging, the gizmo shows the angle, the distance or the factor. With
    snapping on (the magnet, or Ctrl), it also shows the notches it snaps to.
- Ctrl+K: the command palette, with every menu action searchable.
- Ctrl+Shift+P / Esc: start / stop Play here.
- Ctrl/Shift+click and Shift+drag: multi-select and box select.
- The **Arrange** menu: align, distribute, array, drop to surface.
- Dropping a `.glb`, `.gltf` or `.png` on the window imports it and writes the
  manifest entry.

The Game view shows a **budget HUD** (tris, sprites, texture bytes vs the console's
`Caps`; under the N64 profile also **N64 3D time** — this frame's T&L work through the
calibrated cost model `trino_core::perf::N64_COST`, against one 30 fps frame) and culls interior cells like the game does.
Below it, the **platform budget** of the scene camera (`trino_scene::budget`, the same
numbers as `cargo xtask scene budget`): the N64 frame as the slower of CPU and RSP, how
many draws take the RSP and the most common reason the rest don't, the 3DS triangles,
and draws sent but unseen. The "N64 3D time" row then uses that CPU/RSP split, so the
HUD's two N64 figures agree. The Inspector's scene settings (nothing selected) list every
reason (`off the RSP: n × reason`) and each unseen draw; clicking one selects it. The
budgets are cached on the scene text, the library revision and the project generation
(`App::budgets`), and hidden during Play here (the camera is the game's then). They use the
PC bakes except where a console has its own source (`assets/n64/...`) or does not bake a
model (`platforms = [..]`): `App::console_models` bakes those, as `scene budget` does.

The Scene view draws collider, cell and portal wireframes.

Live-edit check: start the editor with `TRINO_EDITOR_ECHO_LOG=1` in the background, edit a
`.material.ron` / `.prefab.ron` / scene, and watch for `… updated` / `… reloaded` lines.

## What's where (crates/editor/src)

| File | Role |
|---|---|
| `main.rs` | start rule (`start`: launcher, project argument or the old hooks), `Shell` (launcher, then editor) |
| `launcher.rs` | the project launcher: recent projects, open existing (validated), create from a template |
| `recent.rs` | the recent-projects file (location, parse, save, ages) |
| `app.rs` | state, `Cmd` queue (panels never mutate structure directly), menus, toolbar, status bar, file sync, shortcuts, undo commit |
| `doc.rs` | the document (scene or prefab being edited) + snapshot undo/redo (auto-coalescing) |
| `viewport.rs` | Scene/Game views through the real `PcRenderer`, overlays, picking, drag-and-drop |
| `gizmo.rs` | orbit camera (fly, dolly, axis snaps, near-orthographic `RenderCam`), projection (same math as `tessellate`), move/rotate/scale gizmo with planar handles, view ring and drag feedback, the axis widget |
| `panels/{hierarchy,inspector,assets}.rs` | the panels |
| `dialogs.rs` | fake object (live preview), new material/scene, make prefab, export |
| `project.rs` | asset catalog, thumbnails, fake-object/manifest file operations |
| `theme.rs` | palette, fonts (Inter, JetBrains Mono, Phosphor icons), shared widgets |
| `play.rs` | Play here: the scene as a game inside the Game view (keys → `InputState`) |
| `ops.rs` | arrange operations (align, distribute, array, drop to surface, group delete) — pure, unit-tested |
| `palette.rs` | command-palette matcher (the app builds the item list from its menus) |
| `budget.rs` | Game-view budget HUD rows from `FrameStats` vs `Caps` |
| `anim_panel.rs` | the Animation tab: its own preview renderer, clips, transport, crossfade, measurements (`trino_asset_pipeline::anim`), onion skin, palette, looks, console pose export |
| `panels/particles.rs` | Assets › Particles inspector: fields, caps per platform, the live preview (`ParticlesView`) with Burst |
| `preview.rs` | a small offscreen preview with its own `PcRenderer` (Animation tab, particle inspector) |
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
- **The orthographic view is a near-orthographic stand-in.** The engine has no
  orthographic projection, so `OrbitCam::camera` backs the eye off (≥ 2 km) with
  a field of view that keeps the target plane's size. The core's sin/cos are ~1%
  off near 0, so the angle is solved with the core's own cotangent. At that
  distance, depth `1 - NEAR/z` in Depth32Float would resolve only metres, so
  `Viewport::render` draws the world scaled around the target
  (`RenderCam::world`, 1 unit from the eye).
  - A new draw in `render` goes through its `place()`. Fog goes through
    `RenderCam::fog`, LOD is measured from `cam.eye()` (the orbit eye), and the
    render key includes the field of view (ortho zoom changes only that).
  - Overlays, picking and the gizmo use the unscaled `camera()`; a unit test
    holds the two to the same pixels.
- **Fly mode owns the keyboard.** While RMB is held on the Scene view,
  `Viewport::flying()` is true. `App::shortcuts` and the view's own key block
  then stand down (Shift+A would open the fake-object dialog mid-strafe). Gate any
  new single-key or Shift shortcut the same way. `App::shortcuts` also requires RMB
  to be down. If another tab hides the Scene view mid-flight, the view never sees
  the release, and its flag would mute Ctrl+S and undo.
- **A gizmo drag takes the handle under `pointer.press_origin()`.** egui reports
  `drag_started` only after the pointer passes its threshold. Hit-testing where
  the pointer is by then grabbed a neighbouring handle on a quick drag (the thin
  ground square became the X axis), and it lost the first pixels of motion.
  `ui_tests::gizmo_drags_grab_the_handle_under_the_press` guards it.
- **Kittest pointer details:**
  - `drag_at`/`drop_at` are Primary only. For MMB/RMB, send
    `Event::PointerButton` yourself.
  - Set held modifiers with `h.input_mut().modifiers`: they persist across
    steps, and each queued event is its own step.
  - Screen positions of world points: the Scene view's image has its top-right
    corner at the axis widget's centre plus (48, -52), and its size is
    `scene_view.internal_size()`. Build a `gizmo::Proj` from that, not from a
    guessed panel rect.
  - The env hooks can't hold a drag or switch the projection. To look at those,
    render mid-flow with `h.render()` from a scratch `#[ignore]` test (saving the
    PNG to the scratchpad), then delete the test.
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
- **Icon buttons need an accessible name** (`theme::named_icon_button`): a tooltip is not a
  label, so kittest (and a screen reader) can't find "Pause" by its glyph.
- **Kittest clicks need the widget on screen.** A panel's content below the fold has rects
  past the window or its scroll area; `ui_tests::click` first sends accesskit's
  `ScrollIntoView` (`Node::scroll_to_me`), then wheels the panel (over a visible part of
  it) until the widget fully shows. The wheel alone failed once a button sat above a
  3D preview: the wheel landed on the preview, which zooms instead of scrolling, and the
  click went nowhere (a rect above the window also counted as visible until the helper
  clipped it to the screen). Items of an open combo list are clicked with
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
