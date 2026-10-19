# Evidence for PR #91 (issue #60, accessibility)

- `pc/`: the PC goldens of the palette swap and the deuteranopia simulation view.
- `games/`: castle64's doors in each color-vision mode, castle64's captions on the
  N64, 3DS and PC profiles, Solid Eleven's kits for a deuteranope.
- `ui/`: the starter Accessibility and Options screens at 100 %, 200 % and in high
  contrast on each target; the Options golden before/after.
- `editor/`: the Accessibility tab, the UI tab at 200 % + high contrast + the
  color-vision icon, the Game view through the protanopia simulation.
- `logs/`: workspace tests, lints, ares (both RAM tiers), Azahar (New and old 3DS),
  Chromium (WebGL2 and WebGPU), the football template, `access check`.

Regenerate the pictures with
`cargo test -p xtask --test access_shots -- --ignored` and
`cargo test -p xtask --test access_shots_solid_eleven -- --ignored`.
