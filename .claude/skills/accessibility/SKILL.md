---
name: accessibility
description: Make a Trino game accessible on every target — color vision through semantic palettes (`assets/palettes.ron`) and icons (`cvd_icon`), captions for sounds, UI text scale up to 200 % and high contrast, reduced motion, the flash guard, hold-to-toggle — validated with `cargo xtask access check`, reviewed with `access shot` and the editor's Accessibility tab. Use for "color blind", "CVD", "captions/subtitles", "text size", "high contrast", "photosensitivity/flashes", "camera shake", "hold vs toggle", or when an accessibility check fails.
---

# accessibility (issue #60, ADR 0027, docs/ACCESSIBILITY.md)

The player's settings come from `platform.access()` (`AccessSettings`); everything that
honors them is content (text files) plus small core helpers, so the N64, the 3DS, the PC
and the Web run the same thing. The guide with code is `docs/ACCESSIBILITY.md`.

## The loop

1. Edit the text:
   - `assets/palettes.ron`: semantic colors (`base` + `protanopia`/`deuteranopia`/
     `tritanopia`/`grayscale` alternatives; `Some(..)` is optional), `distinct` groups,
     `contrast` pairs (`min`, default 4.5), `min_delta_e` (default 20).
   - The manifest: `caption = "cap.x"`, `important`, `caption_priority`, `direction`,
     `caption_seconds` on sounds/music; `cvd_icon = "sprites/x"` on sprites.
   - `assets/i18n/<lang>.ron`: every caption key in every language.
   - `ui/themes/high_contrast.ron`: a screen theme; left-out fields are the engine's.
   - UI nodes: `cvd_icon: "sprites/x"`; controls with `min:` + `padding:` (not fixed
     `size: (Px, Px)`), long lists in `Scroll(..)` with `max: (0, h)`.
2. `cargo xtask access check [dir] --json` — exit 0 means clean (warnings fail too):
   - `they look 11.1 apart (ΔE), under the 20 they must keep: give one of them a
     \`deuteranopia\` alternative` — write that alternative (pick it with the simulation:
     `trino_core::access::delta_e_seen`, or a few candidates through the CLI);
   - `contrast 1.56:1, under 4.5:1` — a pair, in the named mode;
   - `[theme] text on panel: contrast …, under 7.0:1`;
   - `an important sound has no caption`, `pt-BR has no "cap.x"`;
   - `text overflows its box at 200 % [n64, en-US]`, `reaches past the safe area at
     200 %` — the screen fixes above.
   `dir` checks an example with its own assets (`examples/solid-eleven`,
   `templates/standalone`).
3. Look: `cargo xtask access shot <screen|scenes/x.scene.ron> --cvd deuteranopia
   [--text 200] [--contrast] [--platform n64]` (`--view off`: the player's palette as
   drawn, without the simulation). Editor: the **Accessibility** tab, View › Accessibility
   simulation; `TRINO_EDITOR_ACCESS=cvd=deuteranopia,text=200,contrast=on
   TRINO_EDITOR_TAB=accessibility` for screenshots.
4. `cargo xtask test` (the `accessibility` self-test step, `access_cli`, `access_goldens`).

## Game code

- `AccessBlob::get(platform)` in `update_with`, then `data.color(name, settings.cvd)` /
  `data.palette([..4], cvd)` → the tint or palette the draw already has. A tint keeps an
  N64 draw on the RSP; a palette moves it to the CPU T&L (`RspReject::Palette`).
- Fixed colors (club kits): compare as the player sees them,
  `access::delta_e_seen(cvd, a, b)` (Solid Eleven's `dress_for`).
- Captions: `captions.configure(blob, &settings)`, `let mut c = self.captions;
  c.update(dt); self.update(input, &mut c.wrap(audio), dt); self.captions = c;`, then
  `captions.draw(renderer, strings, &style)` **last** (`SecondScreen` switches to the 3DS
  touch screen). The wrapper forwards every `Audio` call.
- UI: `UiEnv::of(r).with_access(&settings).with_contrast_theme(data.contrast_theme())`.
- Outlines (high contrast): `access::draw_outlined(r, &camera, model, &transform,
  material, &params, settings.high_contrast.then_some(&ring))` with
  `Outline::new(color, center, radius)` — the object's bounding sphere, shared by every
  part of a character; `ring.draw(..)` for a `draw_model_matrix` part. Draw the ring
  before the object.
- `CameraFx` (`apply` to the camera), `ScreenFlash` (`trigger` with the settings, `draw`
  after the world), `HoldToggle::update(input, prev, button, &settings)`.
- Consoles keep the settings for the session: store `settings.pack()` (3 bytes) in the
  save and `AccessSettings::unpack` on load (`None` for a blank save).
- PC runs: `TRINO_ACCESS=...` forces settings (nothing saved), `TRINO_CVD=...` shows a
  window through the simulation; the Web: `?access=...`.

## Traps (verified)

- The defaults must draw exactly what they drew before: `UiAccess::NONE` multiplies by
  exactly 1 and keeps the screen's theme (every golden unchanged). A new setting goes in
  the layout key (`Ui::layout_key`), or changing it doesn't re-lay out.
- Text scale is whole glyph pixels where the UI scales by whole pixels: 125 % at ×1 is
  100 %. By design (crisp bitmap fonts, the N64's integer text path).
- `access check` counts a node past the safe area only at 200 %; `ui check` doesn't check
  it at 100 %.
- A root `assets/i18n` table makes `ui check` audit every root screen in that language
  too: fixtures report each issue twice (`ui_cli.rs` counts it).
- The selftest's caption probe must read the box's padding at mid-height: the
  high-contrast box has yellow rules on its first and last rows.
- The 3DS touch screen's clear is not the frame's: a probe there needs its own fill
  (`selftest::accessibility` fills it gray).
- `access check` doesn't know the font a game draws captions with: keep caption texts
  inside it. Solid Eleven's `fonts/se_ui` has no brackets, so `[WHISTLE]` drew as
  `?WHISTLE?` until its texts lost them (look at `access_shots_solid_eleven`).
- `palettes.ron` with a `distinct` group a mode can't separate fails the **bake**, not
  only the check: fix the palette before `cargo xtask build`.
- An outline copy is only a ring if it is solid and behind: `tint` multiplies vertex
  colors (a colored model under a yellow tint is not yellow), so the copy is a black tint
  plus the color as `emissive`. Pushed entirely behind the bounding sphere, the floor
  behind a standing character hid most of its ring; the push is the smallest that keeps
  each point of the copy behind the point it copies, plus 0.3R for slanted top faces
  (`an_outline_is_a_larger_unlit_copy_behind_the_object` checks it per pixel).
- Hand-drawn menus (Solid Eleven's `ui::Ui`) don't take the text scale: their
  fixed boxes have no room (57 % of the texts can't grow 125 %). Check them against
  their boxes instead: `solid_eleven_menus` reads each text's ink from the font atlas
  and its box from the fills drawn before it (a slant's shifted strips merge, a
  crest's straight spans don't). Its overlap test compares ink, not line boxes (two
  lines of a wrapped name touch only in their line boxes).
- Solid Eleven's 44 px PC font inks about a pixel lower than its 11 px font times 4: a
  label snug in a 9-unit chip spilled at 1080p only. Test every picture size.
- Parallel GPU test binaries can SIGSEGV inside `libvulkan` on some drivers (device
  creation, GitHub #94): rerun with `--test-threads=1` before suspecting the code.

## Where the code is

| Piece | Path |
|---|---|
| Settings, color math, `TACC`, captions, motion, outlines | `crates/core/src/access/{mod,color,data,captions,motion,outline}.rs` |
| UI integration | `crates/core/src/ui/{mod,layout,draw,tuil}.rs` (`UiAccess`, `text_mul`, `cvd_icon`) |
| RON models and checks | `crates/scene/src/access.rs` |
| Bake and `check_project` | `crates/asset-pipeline/src/access.rs` (called by `ui::bake`) |
| CLI | `xtask/src/access.rs` |
| PC simulation view, strobe monitor | `crates/render-wgpu/src/renderer/cvd.rs`, `mod.rs` (`set_cvd_view`, `flash_strobes`) |
| Hosts | `PcPlatform` (`load_access`, `take_access_change`), `runner-pc` (`load/save_access_settings`), `platform-web/runtime.rs`, N64/3DS `system.rs` |
| Editor | `crates/editor/src/access.rs`, `panels/access.rs` |
| Self-test step | `examples/castle64/src/selftest.rs::accessibility` |
