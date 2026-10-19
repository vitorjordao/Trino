# ADR 0024: A declarative UI system for every target

Status: accepted (October 2026, GitHub #58). Every phase of the issue is implemented: the
runtime in the core (§1–§5), the screen files, their bake and their static export (§6),
the CLI (§7), the budgets (§8), the editor's UI tab (§9), the starter screens (§10) and the
first adopter, Solid Eleven's pause menu (§11).

## Context

Every menu, HUD and dialog was hand-written: `draw_text` and `fill_rect` at coordinates a
person picked for one resolution, the focus a `usize` the game moved by hand, and input
hit boxes kept in step with the drawing by discipline. Solid Eleven's screens are a few
thousand lines of that, and its pause menu shows how it fails: the logic walked three
rows (resume, substitute, quit) while the drawing showed two, so the second row the
player saw, "QUIT MATCH", asked for a substitution. Nothing could check a screen for the
things that differ by target — a 320×240 N64 picture whose edges the TV hides, a 3DS whose
second screen is the touch screen, a PC at 1080p, 4K or 21:9, a phone browser with a notch
— or prove that the pad reaches every control on a console with no mouse.

## Decisions

### 1. A runtime in the core, `no_std`, no allocation, no backend code

`trino_core::ui` holds the whole runtime. A screen is a flat array of `UiNode`s in
depth-first order (`UiScreen`: a static slice, or a view over a baked blob, §6); `Ui<N>`
holds what changes — `N` laid-out slots with their rects and flags, the focus, scroll
offsets, up to 24 bound values, the press in progress — in a fixed size the game picks
(`Ui<16>` on the N64 is about 2 KB). It draws only with the 2D calls every renderer has
(`fill_rect`, `draw_sprite`, `draw_text`; `draw_model` for a 3D model in a box), so the
N64, the 3DS, the PC and the Web run it with no backend change; two default `Renderer`
methods are new (`safe_area`, `ui_stats`, §3 and §8), so `GAME_API` does not move.

Containers: `Overlay` (children by anchor), `Stack` (direction, gap, align, justify),
`Grid` (columns), `Scroll` (a stack the focus scrolls; children outside it are not drawn,
because 2D has no scissor on the consoles) and `List` (its one child repeated by a bound
count). Leaves: `Text`, `Image` (stretch, contain, whole-pixel; nine-slice), `Rect` (with
a border), `Button`, `Toggle`, `Slider` and `Model3d`. Any node may hold children; a leaf
lays them out like an overlay (a panel is a `Rect` with a `Stack` in it).

### 2. Layout: virtual units, whole-pixel scaling, the real edges

Sizes (`Fit`, `Px`, `Percent`, `Fill`), offsets, padding and min/max are virtual units
against the screen's `reference` size (320×240 by default). The scale is the largest that
fits the reference into the screen — floored to whole pixels by default (`Scaling::Integer`,
so pixel art and bitmap fonts stay crisp; `Fractional` exists) — times the player's
interface scale, and the root then fills the whole screen at that scale: 1080p is ×4 with
the anchors at the monitor's real corners, 4K ×9, 2560×1080 ×4 with the extra width on
the sides, the consoles ×1. Each node has an `anchor` (nine points or `Fill`), a `screen`
(`Top` or `Bottom`), `visible_on` (a set of targets: a "press START" hint on the consoles,
a "click" hint on the PC), and `visible_if`/`enabled_if` bound to values.

The layout runs lazily: it is redone only when the screen, its values, the language, the
screen size or the interface scale change (one key), and costs a few float operations per
node. A game that updates a menu before it ever draws it (a test, a server) gets a
headless layout at the last environment or the screen's reference size.

### 3. Safe areas and the 3DS's second screen

The root keeps inside the target's safe area (`Renderer::safe_area(screen) -> Insets`):
`Caps::safe_area` gives the N64 16×12 pixels of TV overscan and the others none; the Web
reads CSS `env(safe-area-inset-*)` (the page sets `viewport-fit=cover`) and the PC
renderer turns those window pixels into picture pixels past the letterbox's bars
(`PcRenderer::set_window_safe_area`). A `bleed` node (a background) ignores it. Nodes
with `screen: Bottom` go to the 3DS touch screen; a target with one screen lays them out
on its only one, so one file serves both.

### 4. Input: the pad first, then the mouse and touch

The D-pad, the stick and the second stick (the N64's C buttons) move the focus to the
nearest control in that direction by geometry, with per-node `nav` overrides and an
optional wrap; a held direction repeats after 0.4 s every 0.1 s. A accepts, B (or Escape)
sends the screen's `on_back`. Left/right moves a focused slider. The mouse focuses what it
hovers and clicks on release over the same control; a touch on the 3DS bottom screen (or
a phone) is a tap. `Ui::update` returns at most eight events a frame (`Click`, `Changed`,
`Back`, `Moved`) as borrowed `&str` actions, so a game matches on names. `Ui::unreachable`
walks the pad's graph from the first control and names a control it cannot reach: on a
console that is a bug the player cannot get around.

### 5. Data and text: string keys, string tables, overflow reports

A node shows `text` (a `{}` takes the bound value, else it is appended), a `key` looked up
in the current string table (`UiStrings`: the baked `StringTable`, a `Fallback` chain, or
the game's own adapter — Solid Eleven keeps its tables), or a `bind`. Values are set by
string keys (`ui.set("volume", 7)`, `set_bool`, `set_text`, `set_item` for list rows);
toggles and sliders write them back. Text goes through `draw_text` and `measure_text`, so
each target's fonts measure it. A text past its box is reported (`UiStats::overflow` and a
slot flag) unless its node chose `Ellipsis`; `Clip` cuts it, `Wrap` breaks it at spaces.

### 6. Authoring: `ui/<name>.ui.ron`, baked or exported

A screen is a text file in the project's `ui/` folder: versioned RON whose unknown fields
are errors (a misspelt field would otherwise vanish on the editor's next save), with
defaults left out on save, a `theme` (font, colors, focus colors), and `preview` values
that tools show. The asset bake turns each into a `TUIL` blob, `ui/<name>`, on every
target — an 84-byte header, 94-byte node records and a string pool, checked on load
(`UiScreen::parse` refuses bad magic, versions, strings and trees) — and each
`assets/i18n/<lang>.ron` (`Strings(lang, fallback, entries)`) into a `TSTR` table,
`i18n/<lang>`, sorted by hash. Neither needs a manifest entry. `ScreenBlob` and
`StringsBlob` keep them parsed and re-parse when the PC's live reload hands out new bytes,
so an edit shows in the running game; `watch n64|3ds` rebuilds on a `ui/` change.

A game that wants no blobs (its tests have no platform) exports the screen as static Rust
with `cargo xtask ui export` and `include!`s it (rustfmt leaves it alone); the test suite
fails when a committed export is stale.

### 7. The CLI: `cargo xtask ui check|shot|export|list`

`ui check` validates each screen (ids, nav targets, kinds, ranges) and audits it per
target and per language: an unreachable control (an error on the N64 and the 3DS, a
warning on the PC and the Web, where the mouse reaches it), a budget overrun (§8), a text
that overflows, a glyph a font lacks, a font the project does not have, a key in no table.
Any issue fails it, warnings included. `ui shot` renders a screen headless on a target's
profile and size (the 3DS's two screens stacked), with `--lang`, `--scale` and `--focus`.
`ui list` names the screens and languages. Every command takes `--json`. A screen in an
example with its own assets (`ui/` next to an `assets/manifest.toml`) checks against them.

### 8. Budgets: `Caps::max_ui_nodes` and `Caps::max_ui_draws`

| Target | Nodes | 2D draws a frame | Safe area |
|---|---|---|---|
| N64 | 64 | 384 | 16 × 12 |
| 3DS | 128 | 2048 | 0 |
| PC, Web | 512 | 16384 | 0 (Web: the notch) |

`ui check` fails a screen past them; at run time `Ui::draw` reports its counts through
`Renderer::ui_stats`, and the PC renderer in strict mode (`TRINO_STRICT=1`) panics naming
the cap, so a screen that would be too heavy for a console fails on the PC first.

### 9. The editor's UI tab

A **UI** tab edits `ui/*.ui.ron`: a screen picker, a target picker (N64, 3DS with both
screens, PC at 1080p, 4K and 21:9, Web), the hierarchy (drag to reorder or re-parent), an
inspector, a live preview through the same runtime and renderer as the game, a focus-order
overlay, a language switcher, and overflow highlights. It writes the file on every edit and
reloads it when it changes on disk, so the editor, a text editor and an agent can work on
the same file. `TRINO_EDITOR_UI=screen,target,lang,node` opens it at a state for
screenshots and tests.

### 10. Starter screens in every new project

`templates/standalone/ui/` holds `title`, `pause` and `options` screens and an `en-US`
table; new projects get them with the `trino5x7` font, and the template game drives its
title, pause and options modes through them (the options' toggle and slider set the
music volume, the PC's display settings give the interface scale). `ui check` passes on
them on every target.

### 11. First adopter: Solid Eleven's pause menu

`examples/solid-eleven/ui/pause.ui.ron`, exported to static code and drawn through an
adapter to the game's own EN/PT tables. It draws the three rows its logic walks.

## Consequences

- A menu is a file an agent, a designer or the editor edits, checked per target before it
  runs; the pad's reach, the budgets and overflowing translations fail in CI, not on a TV.
- One file serves every target: the layout adapts to the picture, the safe area and the
  second screen; the game only maps actions.
- No text entry, animation or transitions yet; a game draws those itself next to the UI.
  The UI draws after the world in the 2D pass, so it never changes a golden frame that has
  no UI in it.

## Verification

- Core (`core/tests/ui.rs`, 35 tests): anchors, stacks, grids, sizes, scaling at 1080p, 4K,
  21:9 and below the reference, the interface scale, the N64 safe area and `bleed`,
  `visible_on`/`visible_if`, the 3DS bottom screen and one-screen targets, overflow and
  ellipsis, labels and keys, the pad (repeat, sticks, C buttons, grids, overrides, wrap,
  disabled controls), `unreachable`, the mouse and touch, toggles, sliders, lists, scroll,
  focus colors and stats, nine-slice, `Model3d`, the TUIL round trip and damaged blobs, the
  TSTR tables, values, `ScreenBlob`/`StringsBlob` live replacement, menus updated before
  their first draw.
- Scene (`scene/tests/ui.rs`): the RON model, validation, the audit per target and
  language, bake and export, unknown fields.
- CLI (`xtask/tests/ui_cli.rs`): `check` on the shipped screens and on one fixture per
  problem (`xtask/tests/ui_fixtures/`), `shot` on every target, `export` (and that
  Solid Eleven's committed export is fresh), `list`, `--json` everywhere.
- Goldens (`xtask/tests/ui_goldens.rs`): the starter screens on the N64 and 3DS profiles
  and the PC at 1080p against `tests/golden/pc/ui_*.png`; the title at 1080p, 4K and 21:9.
- Self-test step `ui layout` (castle64): parse the baked `ui/selftest`, check the focus,
  reach and the safe area, move with the pad, accept, read back the focus and button
  colors, tap the 3DS touch screen's button; on the PC profiles (N64, 3DS, 1080p, 4K,
  21:9), in the browsers (Chromium WebGL2 and WebGPU, Firefox, WebKit), on ares and Azahar.
- Renderer: window insets become picture insets past the letterbox; strict mode fails a
  screen over budget.
- Solid Eleven: its pause menu draws three rows in English and Portuguese and the pad
  walks them; its menu crawler draws every state cleanly.
- Editor: the UI tab's tests and the layout audit (see TESTING.md).
