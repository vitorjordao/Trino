---
name: ui-authoring
description: Build menus, HUDs, dialogs and options pages for Trino games as `ui/*.ui.ron` screens — the same files the editor's UI tab edits — laid out per target (N64 overscan, 3DS touch screen, PC 1080p/4K/21:9, a phone's notch), checked with `cargo xtask ui check`, seen with `ui shot`, driven from game code with `ScreenBlob` + `Ui`. Use for "make a title/pause/options screen", "add a button", "translate the menu", "the pad can't reach X", "text overflows", or porting a hand-drawn menu.
---

# ui-authoring (screens as text)

A screen is a RON file in the project's `ui/` folder (ADR 0024, `docs/ASSETS.md` › UI
screens has the whole format). The asset bake turns it into the `ui/<name>` blob on every
target, string tables `assets/i18n/<lang>.ron` into `i18n/<lang>` blobs; no manifest entry.
A running PC game, the editor's UI tab and an agent editing the file all see the same thing.

## The loop

1. Edit `ui/<name>.ui.ron` (start from `templates/standalone/ui/{title,pause,options}.ui.ron`).
2. `cargo xtask ui check <name> --json` — exit 0 means every target × language is clean:
   - `the pad cannot reach this control` — an error on the N64 and the 3DS: move it in
     line with the others or add a `nav: (down: "id")` override;
   - `N nodes laid out: over the target's 64 (Caps::max_ui_nodes)` / `max_ui_draws` —
     fewer list rows, fewer nodes, shorter texts (every glyph is a draw);
   - `text overflows its W×H box` — in which `[platform]` and language; widen the box,
     `overflow: Ellipsis` (a choice, not an error), or `Wrap`;
   - `font X has no glyph for 'ã'` — the font (trino5x7 is ASCII) or the text;
   - `key "x" is in no table` — add it to `assets/i18n/<lang>.ron`.
   Warnings fail the check too.
3. `cargo xtask ui shot <name> --platform n64|3ds|pc|web [--size 2560x1080] [--lang pt-BR]
   [--focus quit] --out x.png` and *look* (the 3DS shot stacks both screens).
4. `cargo xtask test` — the goldens hold the starter screens.

## Writing screens that work everywhere

- Design at the reference size (320×240 by default). Integer scaling makes 1080p ×4,
  4K ×9; extra width (21:9) goes to the sides, so anchor HUD corners with `anchor:
  TopRight` etc., not offsets from the left.
- Backgrounds get `bleed: true` (they cover the N64's 16×12 overscan margin; content must
  not). Never put a control under `bleed`.
- One file serves the 3DS and everyone else: `screen: Bottom` puts a node on the touch
  screen and on the main screen elsewhere. Hints per input: `visible_on: [N64, N3ds]`
  ("press START") vs `[Pc, Web]` ("click").
- Kinds with settings need `()` even without any: `Stack()`, `Grid()`, `Rect()`,
  `Slider()`. Unknown fields are errors (a typo would otherwise vanish on the editor's save).
- `Button` text is centered by default (`align: Auto`); `text: "Volume"` on a slider shows
  `Volume 7` (the bound value is appended; `{}` places it).
- `preview: {"volume": Int(7), "music": Bool(true), "rows": Int(3)}` is what tools show;
  the game sets the real values.

## Driving a screen from game code

```rust
// fields: pause: ScreenBlob (ScreenBlob::new("pause")), strings: StringsBlob, ui: Ui<32>
fn update_with(&mut self, platform: &mut dyn Platform, input: &InputState, audio: &mut dyn Audio, dt: f32) {
    self.pause.get(platform);            // the platform exists only here; re-parses after a live edit
    self.strings.get(platform);
    self.interface_scale = platform.display().map_or(1.0, |d| d.settings.interface_scale());
    self.update(input, audio, dt);
}
// in update:
if let Some(screen) = self.pause.screen() {
    for e in self.ui.update(screen, input, &self.prev, dt).iter() {
        match e {
            UiEvent::Click { action: "resume", .. } | UiEvent::Back { .. } => self.go(Mode::Play),
            _ => {}
        }
    }
}
// in render, after the world:
let env = UiEnv::of(renderer).with_interface_scale(self.interface_scale);
match self.strings.table() {
    Some(t) => self.ui.draw(renderer, screen, &env.with_strings(t)),
    None => self.ui.draw(renderer, screen, &env),
}
```

- `ui.reset()` when switching screens (focus back to the first control).
- Values: `ui.set("volume", 7)`, `set_bool`, `set_text`, `set_item(key, row, v)` for list
  rows; toggles and sliders write theirs back (`UiEvent::Changed`).
- `Ui<N>`: `N` laid-out slots (list rows count each); `Ui<16>` is ~2 KB. Too small drops
  nodes and `stats().dropped` says so.
- A game without blobs (its tests run without a platform, like Solid Eleven) exports:
  `cargo xtask ui export ui/pause.ui.ron src/ui/pause_gen.rs` and
  `pub mod pause_gen { include!("pause_gen.rs"); }` — `include!` keeps rustfmt off it.
  Re-export after every edit of the RON.
- A game with its own translations implements `UiStrings` (Solid Eleven's `UiTr` maps
  keys to its `tr(lang, S)`; `revision` changes with the language so the layout reruns).
- `Ui::update` before any draw lays out headless, so unit tests can press buttons through
  a menu with no renderer.

## Per target

| | Screen(s) | Safe area | Budget (nodes / draws) | Input |
|---|---|---|---|---|
| N64 | 320×240 | 16×12 | 64 / 384 | D-pad, stick, C buttons, A/B |
| 3DS | 400×240 + 320×240 touch | 0 | 128 / 2048 | pad + taps on the bottom screen |
| PC | the window (1080p ×4, 4K ×9) | 0 | 512 / 16384 | pad, keyboard (Enter/Escape), mouse |
| Web | the canvas | CSS `env(safe-area-inset-*)` | 512 / 16384 | pad, keyboard, mouse, touch |

## Verifying engine-side UI changes

- Core: `cargo test -p trino-core --test ui`; formats and audit: `cargo test -p
  trino-scene --test ui`; bake: `cargo test -p trino-asset-pipeline --test ui_bake`;
  CLI: `cargo test -p xtask --test ui_cli`; goldens: `cargo test -p xtask --test
  ui_goldens` (`TRINO_BLESS=1` rewrites `tests/golden/pc/ui_*.png` — only when the change
  is intended).
- The self-test step `ui layout` (`castle64::selftest::ui_layout`, screen
  `ui/selftest.ui.ron`) runs on the PC profiles (`engine_checks`), ares, Azahar and the
  browsers (where the pixel probes report skipped: no synchronous read-back).
- A `Model3d` node sets its own camera: draw screens after the world's 3D.
