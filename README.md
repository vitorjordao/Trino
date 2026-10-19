# Evidence for PR #92 (issue #59, localization)

- `logs/`: the local runs: workspace tests, editor UI tests, desktop CI smokes, ares
  (both Expansion Pak tiers), Azahar (New and old 3DS), the football template on both
  consoles, Chromium (WebGL2 + WebGPU), and main's own run of the two GPU tests that fail
  on this machine without this branch.
- `web/`: the browsers' console logs (`TRINO_STEP:i18n`, `TRINO_TEST_PASS`).
- `images/se_ab/{main,branch}`: Solid Eleven's menus, match and pause in English and
  Portuguese on the N64, 3DS and PC looks, from main (86e3364) and from this branch; all 78
  are byte-identical (`solid_eleven_ab.txt`). `solid_eleven_ab.png` is a contact sheet.
- `images/languages_*.png`: the engine's tables in en-US, pt-BR and es-ES, each drawn with
  its own glyph-subset atlas (`cargo test -p xtask --test i18n_shots -- --ignored`).
- `images/pseudo_options_n64.png`: a new project's options screen in en-US and in the
  pseudo-locale (`cargo trino i18n pseudo`, `cargo trino ui shot options --lang qps-ploc`).
- `images/editor_*.png`: the editor's Localization tab and the UI tab in pt-BR.
