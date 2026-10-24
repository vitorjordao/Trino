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
  `ANIM_VIEW`, `PALETTE`, `GAME`, `LOD_PREVIEW`, `UI`, `TRACE` and `TRACE_B`, among
  others (`main.rs::EDITOR_HOOKS` is the list).
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
| `TRINO_EDITOR_TAB=scene\|game\|level\|animation\|mixer\|quality\|memory\|accessibility\|ui\|localization\|project\|profiler` | focused center tab (`mixer`, `quality`, `memory`, `accessibility`: the bottom dock's Mixer, Quality or Memory tab; `project`: the Project tab; `profiler`: the Profiler tab) |
| `TRINO_EDITOR_TRACE=<file>[@session]` | the Profiler tab opens that trace (relative to the project, e.g. `target/traces/n64-trace.json@n64-hub`) as trace A on that session, and comes to front |
| `TRINO_EDITOR_TRACE_B=<file>[@session]` | …and that one as trace B (compare); without `@session` B pairs with A's session (`pair_sessions`) |
| `TRINO_EDITOR_PROJECT_PAGE=packaging` | the Project tab opens on Packaging (default Achievements) |
| `TRINO_EDITOR_LANG=<tag>` | the editor-wide preview language (issue #59): the Localization tab's, the UI tab's (unless `TRINO_EDITOR_UI` names one) and Play's `TRINO_LANG` |
| `TRINO_EDITOR_ACCESS=cvd=deuteranopia,text=200,contrast=on` | the Accessibility tab's simulation at start: the Scene, Game and UI views through that color vision, the UI tab at that text size and contrast (`AccessSettings::parse_text` keys) |
| `TRINO_EDITOR_UI=<screen>[,n64\|3ds\|pc\|4k\|wide\|web[,<lang>\|default[,<node id>]]]` | open the UI tab on `ui/<screen>.ui.ron` for that target and language with that node selected, e.g. `TRINO_EDITOR_UI=pause,3ds,pt-BR,quit` |
| `TRINO_EDITOR_ANIM=<model>:<clip>@<time>` | the Animation tab previews that clip, paused at that time |
| `TRINO_EDITOR_ANIM_VIEW=ghosts=3,blend=wave:0.5,look=n64,palette` | its view options (onion skin, crossfade, look, palette) |
| `TRINO_EDITOR_OPEN=fake-object` / `array` / `material:<logical>` / `model:<logical>` / `prefab:<logical>` / `particles:<name>[@pc\|n64\|3ds]` / `audio:<sounds/x\|music/x>[@pc\|n64\|3ds\|web]` | open the fake-object or array dialog, an asset inspector (`audio:`: the waveform overlays that platform's decode), or a prefab in edit mode |
| `TRINO_EDITOR_PALETTE=<query>` | open the command palette (Ctrl+K) with that query |
| `TRINO_EDITOR_PLAY=here` | start **Play here** on the first frame: the scene runs inside the Game view (CI smoke) |
| `TRINO_EDITOR_ECHO_LOG=1` | mirror the Console to stderr (see reloads, errors, saves) |
| `TRINO_EDITOR_GAME=pc\|n64\|3ds\|web\|n64x\|3dsnew` | the Game view's profile (default n64) — screenshots of each platform's variant; `n64x` / `3dsnew` are the hardware tiers (N64 + Expansion Pak, New 3DS) |
| `TRINO_EDITOR_QUALITY=low\|medium\|high\|ultra[,dynres]` | the Game view's quality preset on the PC and Web profiles (and the Play run's `TRINO_QUALITY`); `,dynres` turns dynamic resolution on |
| `TRINO_EDITOR_WEB_CANVAS=responsive\|720\|1080` | the Game view's browser canvas on the Web profile (the picture is the canvas over the auto pixel scale) |
| `TRINO_EDITOR_SIZE=960x600` | window size (default 1600x940; never below the 960x600 minimum) — screenshot small windows |
| `TRINO_EDITOR_INSPECT=platforms\|lod\|impostor\|budget\|ao\|shadows\|audio\|groups` | the Inspector opens that section and scrolls to it (`audio`: the audio inspector's Platforms grid; `budget`: the scene settings' platform budget, with nothing selected; `ao`: the scene's Ambient Occlusion, or a model's baked AO with `TRINO_EDITOR_OPEN=model:<logical>`; `shadows`: the scene's Shadows with each platform's line; `groups`: the scene settings' Asset groups) |
| `TRINO_EDITOR_ASSET_GROUP=<group>[@models\|sprites\|audio\|particles]` | the Assets panel opens filtered by that group's chip (on that tab) |
| `TRINO_EDITOR_TEXTURE=sprites/x\|fonts/x[@pc\|n64\|3ds\|web[,zoom]]` | open the texture inspector on that sprite or font, its preview tab on that platform at that zoom (1–8); with `TRINO_EDITOR_INSPECT=texture` its formats scroll into view |
| `TRINO_EDITOR_AO=game\|on\|off\|mask` | the Scene view's ambient occlusion mode (as the scene sets it, forced on, off, the occlusion alone) |
| `TRINO_EDITOR_SHADOWS=tier\|off\|debug` | the Scene view's shadow mode (as the tier draws them, none, debug: magenta shadows with every receiver and caster outlined) |
| `TRINO_EDITOR_RIPPLE=1` | keep a ripple ringing on the selected entity's water surface: one drops once the scene resolved, and another whenever it has faded (the inspector's "Drop a ripple"; screenshots of ripple rings) |
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
reason (`off the RSP: n × reason`, `off the 3DS GPU`, `off the PC GPU`) as a collapsed
header that opens on the entities it holds back (frameless buttons; clicking one selects
it), and each unseen draw. Clicking the HUD's platform lines selects nothing and scrolls the
Inspector there (`ViewAction::OpenBudget`, `App::scroll_to_budget`). A selected model's
**Render path** row shows PC / N64 / 3DS as GPU, RSP or CPU (the rule on hover and in the
line below), from `FrameBudget::paths` matched by `Scene::draw_name`. The
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
| `audio.rs` | audio measuring on a worker (`audio::encode`, like the bake), per-platform totals for the budgets, the preview player (an offline-capable `PcAudio`), the Mixer's `MixState` |
| `panels/audio.rs` | Assets › Audio inspector: waveform + codec noise, Play per platform, bake settings, the Platforms grid; the budget lines; `name_slider` |
| `panels/mixer.rs` | the Mixer tab: bus volumes and preview mutes, duck rules; writes `assets/audio.toml` |
| `ui_screens.rs`, `panels/ui_screens.rs` | the UI tab (issue #58): edits `ui/*.ui.ron` live — tree (add/duplicate/delete/drag), inspector, preview through the real renderer profile per target (3DS: both screens) and language, focus-order/selection/overflow overlays, issues from `validate` + `audit`; saves when the pointer and keyboard are idle, keeping the leading comments; outside edits reload. Tests: `ui_tests.rs::ui_tab_edits_screens_and_follows_their_files` (GPU, `--ignored --test-threads=1`) |
| `profiler.rs`, `panels/profiler.rs` | the Profiler tab (issue #62): traces A/B from `target/traces/` through `trino-trace`, timeline, frame flame graph, zone and diff tables, live reload, the budget's measured N64 line (see "Profiler tab") |
| `localization.rs`, `panels/localization.rs` | the Localization tab (issue #59): keys × languages from `assets/i18n/*.ron` (default first, pseudo-locales last), cells marked own / fallback (amber) / missing (red), edits written through `write_echo` (comments are not kept), add or remove keys and languages, a pseudo-locale, CSV at `assets/i18n/strings.csv`, `check()` issues with click-to-select, glyph coverage and N64 TMEM per checked font and language measured on a worker; the preview language is editor-wide (UI tab, Play's `TRINO_LANG`). Wide tables page sideways (‹ ›) so the layout audit stays clean. Tests: `ui_tests.rs::localization_tab_edits_the_tables_and_follows_their_files` (GPU, `--ignored --test-threads=1`) |

## Water (ADR 0015)

- The material inspector's **Water surface** section (`inspector::water_editor`): a
  checkbox, then flow, detail flow, swell height and frequency, ripple response.
  **Drop a ripple** (`Cmd::DropRipple`) starts one on the selected surface. The Scene
  and Game views run `set_water` with the editor's clock and ripples
  (`App::water_time`, `ripples`), on every profile.
- The budget's water lines (`trino_scene::budget::water_lines`): per surface,
  triangles, combiner cycles, and the N64 TMEM of its layers (`Project::tmem`).
- **Drop a ripple** lands on the surface's triangle nearest the scene camera's target
  (`app::ripple_point`). The middle of a curved river's bounds is on the bank, where
  the ring never showed.
- **The Scene view measures fog as if the orbit eye were at most `DEFAULT_DIST` away**
  (`viewport::scene_view_fog`): framing the 30 m river put the eye 45 m back, past
  the showcase's fog end, and the whole view turned the fog's color. The Game view
  keeps the real fog.
- Hook: `TRINO_EDITOR_RIPPLE=1` keeps a ripple ringing on the selection, for
  screenshots. A ripple lives 2.2 s and the editor's first frames are slow: take the
  shot at `TRINO_EDITOR_SCREENSHOT_FRAME=40`, not 90, or the ring has faded.

## Texture inspector (ADR 0017 §8)

- Assets › a sprite or font: `panels/texture.rs` (quality, `min_psnr`, a format combo
  per platform, the PC / N64 / 3DS grid of format, bytes, PSNR and memory share, the
  reasons, the master vs each platform's decoded preview). Every control writes the
  manifest through `manifest::edit::set_texture_format` / `set_texture_key`
  (`Cmd::SetTextureFormat`, `Cmd::SetTextureKey`), and the watcher rebakes.
- Measuring runs on worker threads (`textures.rs`): `texture::master` + `choose` +
  `report` per platform, the same calls as the bake and `texture report`, cached on
  (project generation, manifest text hash). Tests poll with `wait_for`, never a fixed
  number of `run_steps`.
- The watcher only prints a failed rebake to stderr; the inspector runs `choose`
  itself, so the panel (red) and the Console show the bake error.
- The Platform budget's texture lines (`texture_memory()`) are project-wide: a console
  loads every texture at boot.
- kittest: close a combo left open by `click_accesskit` by clicking a label no list
  covers (the grid's "Memory"); clicking the combo again can land on the list's first
  item. Budget line queries need exact prefixes (`"3DS {tris} tris"`), since the
  texture line also starts with "3DS".

## Hardware tiers (issue #54, ADR 0020)

- Game view picker: **+Pak** after N64 and **New** after 3DS (accessible names "N64 +
  Expansion Pak", "New 3DS"); below 560 px only the active console's tier toggle shows.
  `GameView::target()` is the console with its tier (`set_profile_tier`), and it is what
  `App::resolved_game` and Play here resolve; the renderer gets `set_hw_tier`.
- HUD on a tier: the title names it (`N64+Pak budget`), a base-vs-expanded table
  (`budget::tier_table`: RAM, CPU, frame or triangles, draws, the scales when they
  differ), the RAM line and the hardware line; `budget::platform_lines_on(.., shown)`
  reads the shown console on its tier (`N64+Pak ≈…`, `New 3DS … tris`), the other
  console on its base.
- Inspector › Platforms, LOD and particle caps: one shared **Tiers** switch row
  (`panels::platforms::tier_toggles`); a tier column sits after its console, starts from
  the console's value and writes the tier key only when edited (its reset appears then).
  A tier with its own value is always shown. Columns need 330 px + 70 px per shown tier
  (LOD 470 + 120), else they stack.
- The editor loads the PC bake only: a model named for a tier alone (`platforms =
  ["n64_expanded"]`) is counted in the budgets (`console_models` bakes it) but not drawn,
  like N64-only models.

## The Web (issue #47, ADR 0019)

- Game view › **Web**: `SimProfile::Web` (the native picture, `Caps::WEB`), a canvas
  combo (`WebCanvas`: responsive = the panel itself, 1280×720, 1920×1080) whose picture is
  what a browser renders (`sim::native_internal`). Its HUD starts with what the view just
  drew (`budget::web_measured_line`: calls, pipeline switches, VRAM), then the scene
  estimate's `Web … calls · … pipelines · … switches` and the phone warnings. HUD text is
  painted, not accessible: kittest checks the Inspector's platform budget instead.
- Web columns: Inspector › Platforms (four columns from 330 px, stacked below; the Web
  column starts from the PC's variant, an empty Web variant beside a PC one means "as
  written"), the LOD panel (`=PC` when the Web follows the PC's start), the texture
  inspector, a fourth Render path pill. Counting variants uses `PlatformVariants::own`,
  not `get` (which falls back to the PC for the Web).
- Play menu and palette: **Run in Browser** (`Cmd::RunInBrowser`, `cargo xtask run web`,
  held in `play` so Stop ends the server) and **Export Web Package** (`Cmd::ExportWeb`,
  `build web --release --out dist/web`, a toast when it ends). `App::web_command` builds
  both command lines (the engine's xtask by manifest path, `--project` outside the engine).
- The Inspector grew with the Web: a kittest click on a section header below the fold
  must use `click` (it scrolls into view), not `get_by_label(..).click()`.

## Audio inspector and Mixer (issue #53, ADR 0021)

- Assets › a sound or music track: `panels/audio.rs` measures it on every platform with
  the bake's own calls (`decl.audio_settings`, `resolve_source`, `decode_master`,
  `audio::encode`, `Encoded::snr_db`, `Encoded::footprint`) on a worker, keyed on the
  manifest text and the project generation. Codec and rate per platform, stream, loop,
  bus and Vorbis quality go to the manifest through `manifest::edit::set_audio_platform`
  / `set_audio_key` (comments kept; "default" removes the key). N64 Opus has no host
  decode: no SNR, no preview, an estimated size (≈).
- SNR colors use `audio::snr_floor` — the floors `asset-pipeline/tests/audio_codecs.rs`
  asserts (VADPCM 25, DSP-ADPCM 30, Vorbis q6 26 dB).
- The Mixer tab edits `assets/audio.toml` (`MixFile`): written when the control is let
  go, reloaded live on an outside edit, an invalid file shows its error and keeps the
  last good mix. Mutes there are preview-only.
- Budget lines (scene settings and the Game view HUD): storage per platform, resident
  RAM, streamed count and the per-stream buffer, from `Encoded::footprint` — the same
  numbers as `cargo xtask audio report`.
- egui pitfalls this hit:
  - A `Slider`/`DragValue` with `max_decimals` or `step_by` snaps the bound value and
    reports `changed()` every frame unless the value already sits on its grid. Pre-round,
    or the editor rewrites the file every frame.
  - Never call `widget_info` on a Slider response: its rect covers the value box and the
    layout audit flags the overlap. Name it with `panels::audio::name_slider`.
  - `theme::prop(ui, "X", …)` next to a widget named "X" gives two nodes with that label:
    query with `get_by_role_and_label`.
  - kittest 0.35 has no `set_value`: send `AccessKitActionRequest { SetValue,
    NumericValue }` (`ui_tests::set_value`); close combo lists left open by
    `click_accesskit` with `egui::Popup::close_all(&h.ctx)`.
  - Lines that appear when a worker finishes shift what sits below them: draw the same
    number of placeholder lines while measuring.

## Quality tab and the Game view's preset (issue #55, ADR 0023)

- The bottom dock's Quality tab (`panels/quality.rs`) edits `assets/quality.ron` (or what
  `[pc.quality] presets` names) through `quality.rs::QualityState`: one column per
  preset, one row per setting, Reset per changed preset, an eye that previews the
  preset in the Game view. Edits are written once the control is let go (only what
  differs from the built-in values, `QualityFile::from_table`); an outside edit reloads
  live; an invalid file shows its error and is never written back. `[pc.quality]
  default` is shown, edited in `trino.toml`. Same file and checks as `cargo xtask
  quality set|reset|check`.
- The Game view's PC and Web profiles have a preset dropdown ("Game view quality") and a
  "Dynamic resolution" checkbox; the console profiles have none (fixed quality). Play on
  the PC profile passes the preset as `TRINO_QUALITY`. The budget HUD line reads
  `Quality Low · 3D 0.81× (dynres 0.50-1.00) · MSAA off · AO off`
  (`budget.rs::quality_line`).
- Proof: `ui_tests::quality_tab_writes_the_presets_and_the_game_view_previews_them`
  (`--ignored --test-threads=1`) and the layout audit's "Quality tab" and "PC Game view"
  steps. Screenshot: `TRINO_EDITOR_TAB=quality TRINO_EDITOR_SCREENSHOT=q.png cargo xtask
  editor`; a preset in the Game view: `TRINO_EDITOR_TAB=game TRINO_EDITOR_GAME=pc
  TRINO_EDITOR_QUALITY=low,dynres`.

## Asset groups and the Memory tab (issue #56, ADR 0022)

- `src/groups.rs`: the manifest's groups (re-read when its text changes, so an agent's
  edit or `cargo xtask assets group` shows at once), each asset's groups, the Assets
  chip filter, and the memory plans of the four console tiers (`TIERS`), measured on a
  worker keyed on the manifest and the bake generation. `toggles()` is the chip row the
  scene's Requires and a cell's Groups use (an undeclared group shows as a warning chip
  to take off).
- Assets panel: a Groups row of chips (All, each group with its count, "new group"):
  click filters, dropping a tile on a chip moves it there (`Cmd::SetAssetGroups`, the
  same manifest line `assets group` writes). A tile shows its group as a pill; its
  context menu has a Groups submenu (picking boot resets; another drops boot).
- Inspector: the scene settings' Asset groups (Requires, the cells that stream, `assets
  check`'s problems, per-tier %, a button to the Memory tab); a Cell's Groups.
- `src/panels/memory.rs`: one card per tier (RAM split, the open scene's bar, by
  category, every group). The Game view HUD adds `<target> assets used / budget RAM (%)`
  under its RAM line (`budget::assets_ram_line`).
- UI test: `ui_tests::asset_groups_chips_requires_cells_and_memory`.

## Accessibility tab (issue #60, ADR 0027)

`access.rs` (`AccessState`: the palettes, `preview` = the views' simulation, the
checklist) and `panels/access.rs` (the tab, `simulation_menu` for View › Accessibility
simulation). The tab writes `assets/palettes.ron` through `write_echo` and then marks the
checklist stale (it reads the file: running it before the write showed the old result);
the watcher reloads outside edits and marks it stale on any `assets/` or `ui/` change.
`TabView::ui` copies `preview.cvd` into both viewports' `cvd_view` every frame; the
viewport re-renders when it changes (`cvd_rendered`). The UI tab takes the same
settings (`UiScreens::access`, part of its render key). The colors table's header names
the modes too: a kittest click on "Deuteranopia" must pick the first match.

## Project tab (issue #61, ADR 0029)

- `shipping.rs` (state) + `panels/shipping.rs` (UI): **Achievements** edits
  `assets/achievements.ron` (written when the user settles, like the Quality tab; outside
  edits reload through the watcher), runs `trino_scene::services::check` every frame,
  writes the lock and Steam's files (`trino_asset_pipeline::services::export_steam`, the
  CLI's function), and previews the engine's list and popup in a `Preview` on the N64
  look (Test unlock pushes a popup there). **Packaging** sets `trino.toml` keys through
  `App::write_ship_key` → `trino_asset_pipeline::ship::set` (comments kept, refused
  values rejected) and runs `cargo xtask package pc|deck check` as a job
  (`App::run_xtask_job`, `shipping.job` polled in `ui`).
- One tab with two pages, in the center dock: two more tabs didn't fit the bars at
  960×600. `trino.toml` is watched when it exists.
- Tests: `project_tab_edits_achievements_and_packaging_as_text`, the layout audit visits
  both pages.

## Profiler tab (issue #62, ADR 0030)

- `profiler.rs` (state, pure and unit-tested: `TimeView` zoom/pan, `lanes`, `visible`,
  `measured_session`, `list_traces`, `parse_hook`, `load`) + `panels/profiler.rs` (UI).
  Everything goes through `trino-trace`, the crate `cargo xtask trace report|diff` uses:
  the tab and an agent read the same numbers.
- Lists `target/traces/*.json|*.log` of the project (Refresh button, re-listed every
  poll). Trace A (file + session combos, accessible names "Trace A", "Session A") drives
  the timeline, the frame view and the Zones table; trace B ("Trace B", "none" closes it)
  adds the Diff (B − A) table: Δ mean, %, Δ p95 (+ A/B means and Δ/frame when wide), red
  when B is slower, green faster, faint within 2 %; `new`/`gone` for zones on one side.
  B's session follows A's through `pair_sessions` until one is picked.
- Timeline: painted (`paint_tracks`), a row per track and depth, frame marks, a ruler;
  wheel (or pinch/Ctrl+wheel) zooms around the cursor, drag pans, double-click or Fit
  shows everything. It sits outside the tab's scroll area and zeroes
  `smooth_scroll_delta` when hovered, so the wheel never also scrolls the tables. Its
  accessible names: "Timeline", "Frame flame graph". Hover text: name, track, duration,
  start, depth. Zones under ~1.5 px on a row merge with their neighbour (the PC's 1 MB
  trace stays fast).
- The frame view: ‹ slider › over `frames`, that frame's `frame_range` as a flame graph.
- Tables are rows of fixed-width cells (no horizontal scroll: the layout audit). Above
  1000 px with B open, Diff and Zones sit side by side.
- Live: both files are polled once a second (`ProfilerState::poll`, `(mtime, len)`), the
  panel asks for a repaint after `POLL`. A reload that fails (a log caught mid-write)
  keeps the last good trace, says so faintly, and retries on every poll. Following
  `pc.log` while `cargo xtask run pc --trace` runs is the live view of a Play run.
- Files load through `trino_trace::chrome::load`, which tries JSON first and then
  `TRINO_TRACE:` lines (logs start with `[ares]`/`[game]`: the first character can't
  tell them apart).
- Estimated vs measured: the scene settings' Platform budget has "N64 measured X ms
  (session, n frames)" under "N64 ≈" when a `target/traces/n64*.json` holds an `n64-*`
  session of that scene with two frame marks (`n64-<scene>`, else `n64-<scene>-…`; never
  another scene's: the showcase once read the castle64 hub's 20 ms beside its own 25.8 ms
  estimate; newest file first; re-read when the files change). Its **Profiler** button opens that trace
  and session here. Not in the Game view HUD (painted; kittest can't see it).
- Tests: `ui_tests::profiler_tab_reads_compares_and_follows_traces` (A as Chrome JSON, B
  as a log, a live reload, the measured line and its button), the layout audit's
  "Profiler (empty)" and "Profiler A vs B" steps, unit tests in `profiler.rs`.
- Screenshots:
  `TRINO_EDITOR_TRACE=target/traces/n64-trace.json@n64-hub
  TRINO_EDITOR_TRACE_B=target/traces/n64-trace.json@n64-cpu-hub
  TRINO_EDITOR_SCREENSHOT=p.png cargo xtask editor .`
- `section_title` upper-cases its text: query "DIFF (B", not "Diff (B". `query_by_label`
  panics on two matches (a zone name is in both tables): use `query_all_by_label`.

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
- **Never assert on `log.last()` after a command.** The watchers (asset rebakes, text
  files) log on their own schedule, and on CI's slower lavapipe runner a late
  "reloaded …" landed after `NewScene("Bad Name")`'s error, failing the flow on main
  and on PR #44. The flow's `cmd` returns the lines logged while it ran; look for the
  message among them (`said`).
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
- **Ambient occlusion in the views (ADR 0014):** `Viewport::render` passes the scene's
  `env.core_ao()` through `RenderCam::ao` (radius and bias are lengths: the
  near-orthographic world is scaled). The Scene view has its own `ao_view` (toolbar menu
  and View › Ambient occlusion). A change of mode re-renders through `ao_rendered`, not
  the `RenderKey`. `Stats::ao` says whether the pass ran. Only the native profile runs
  it: the Game view's N64/3DS show the baked AO.
- **Shadows follow the same split as AO** (ADR 0025): `env.core_shadows()` and
  `resolved.receivers()` go to the renderer every render. The Scene view has its own
  `shadow_view` (the sun-on-horizon toolbar menu, View › Shadow view), re-rendered
  through `shadow_rendered`; the Game view always draws `ShadowView::Game` at the
  previewed profile's tier (blobs on N64, planar on 3DS, the map on PC). The View
  submenu is "Shadow view", not "Shadows": kittest's label queries would hit the
  Inspector's Shadows header too.
- **The Scene view's floating toolbar is full at 960×600.** The AO menu pushed the
  camera button onto the axis widget, and the layout audit failed in two flows. A new
  toolbar button checks `ui.available_width()` and steps aside when there's no room
  (`viewport.rs` › `room`), with the same action kept in a menu (View).
- **A radio menu must close itself.** `ui.radio_value` inside `menu_button` leaves the
  menu open, and the next click (a test's, or the user's) only closes it. Call
  `ui.close()` on `clicked()` (`viewport::ao_menu`).
- **Toasts cover the Inspector's lower right.** A test that saved or undid just before
  clicking a button there can hit the toast. Click such buttons with
  `click_accesskit()` (the AO bake flow).
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

- **The Game view's budget panel sizes itself to its longest line** (`budget::hud_width`:
  at least 210 px, never past the view; 2026-10-17). Platform lines that name a reason
  (`3DS … · 22 flat shading`, `PC GPU … · 3 skinned mesh`) were cut at 210 px.
