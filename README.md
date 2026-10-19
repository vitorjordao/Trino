# Evidence for PR #91 (issue #60, accessibility)

- `pc/`: the PC goldens of the palette swap and the deuteranopia simulation view.
- `games/`: castle64's doors in each color-vision mode, castle64's captions on the
  N64, 3DS and PC profiles, Solid Eleven's kits for a deuteranope; the high-contrast
  outlines (castle64's player and boar on each profile, Solid Eleven's ball); Solid
  Eleven's menus after the box check (long club names wrap, the Portuguese club
  buttons fit).
- `ui/`: the starter Accessibility and Options screens at 100 %, 200 % and in high
  contrast on each target; the Options golden before/after; Solid Eleven's pause menu at
  100 % and 200 % in both languages.
- `editor/`: the Accessibility tab, the UI tab at 200 % + high contrast + the
  color-vision icon, the Game view through the protanopia simulation.
- `logs/`: workspace tests, lints, ares (both RAM tiers), Azahar (New and old 3DS),
  Chromium (WebGL2 and WebGPU) and Firefox, the football template, `access check`, the
  Solid Eleven menu crawler, the editor's UI tests.

Regenerate the pictures with
`cargo test -p xtask --test access_shots -- --ignored` and
`cargo test -p xtask --test access_shots_solid_eleven -- --ignored`.
