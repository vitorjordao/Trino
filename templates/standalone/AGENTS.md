# {{name}} — Agent Guide

A standalone [Trino](https://github.com/vitorjordao/Trino) game: one
codebase for PC, Nintendo 64, Nintendo 3DS and the Web. This file is the entry point
for AI agents and humans.

## Layout

| Path | What it is |
|---|---|
| `game/` | The game crate (`{{name_snake}}`). Portable: depends only on `trino-core` + `trino-game-api`. |
| `pc/` | Desktop host (`trino-runner-pc`): window, renderer, audio, input, asset bake. |
| `n64/`, `n3ds/` | Console hosts (`n64_main!` / `n3ds_main!`) and their emulator self-tests. |
| `web/` | Browser host (`trino_platform_web::run_game`): the game in a tab, on WebGPU or WebGL2. |
| `trino.toml` | Target platforms (`[project] platforms`), console packaging (ROM title, save type, 3DS icon), test gates. |
| `assets/manifest.toml` | Every asset by logical path; masters in `assets/shared/`. |
| `ui/` | Menus as UI screens (`title`, `pause`, `options` to start): RON files the bake turns into `ui/<name>` blobs; the game drives them with `ScreenBlob` + `Ui`. |
| `assets/i18n/` | String tables (`en-US.ron`) the screens' `key`s read. |
| `Cargo.toml` | Workspace; the engine dependencies (path or git) are declared once here. |

## Rules for game code

- `game/` depends **only** on `trino-core` (+ `trino-game-api`): no external
  crates, no std APIs. That is what lets it run on the consoles unchanged.
- Keep `update` deterministic: same inputs + same state = same result.
- Assets are referenced by logical path and declared in `assets/manifest.toml`.
- Screen space is X-right, **Y-down**, origin top-left; stick input is Y-up.
- If a feature cannot work on the N64, the engine does not expose it —
  budgets live in `trino_core::Caps`.
- Menus are data: edit `ui/*.ui.ron` (or the editor's UI tab) and run `cargo trino ui
  check`, which proves on every target that the pad reaches every control, the screen
  fits the console's UI budget and no text overflows. A running game reloads a saved
  screen. See the engine's `ui-authoring` skill.

## Commands

```
cargo run -p {{name}}-pc                 # play (bakes assets on start; saving a master reloads it)
TRINO_SIM=3ds cargo run -p {{name}}-pc   # 3DS look (native = full-resolution PC)
cargo test                              # the game's unit tests
cargo clippy --all-targets -- -D warnings
```

Consoles (`cargo trino` is the engine's xtask, aliased in `.cargo/config.toml`;
it needs a local engine checkout, Docker for the N64 and devkitPro for the 3DS —
see the engine's `build-n64` / `build-3ds` skills):

```
cargo trino build n64      # target/n64/{{name}}.z64
cargo trino run n64        # ... in ares
cargo trino test n64       # boot the self-test in ares: TRINO_TEST_PASS + frame budget
cargo trino build 3ds      # target/3ds/{{name}}.3dsx
cargo trino run 3ds        # ... in Azahar
cargo trino test 3ds       # boot the self-test in Azahar
cargo trino watch n64      # rebuild + relaunch on save (also 3ds)
cargo trino build all      # every platform in trino.toml's [project] platforms
cargo trino editor         # the Trino editor on this project
cargo trino ui check       # every UI screen on every target and language (--json)
cargo trino ui shot title --platform 3ds --out title.png   # see a screen as a target shows it
cargo trino build web      # target/web: index.html, trino.js, trino.wasm, assets/ (WebGPU or WebGL2)
cargo trino run web        # ... served on 127.0.0.1 and opened in the browser
cargo trino build web --release --out dist/web   # a folder to upload (itch.io, GitHub Pages)
cargo trino test web       # a smoke run in Chromium, Firefox and WebKit (needs Node.js)
```

`[project] platforms` in `trino.toml` lists the platforms this game ships on
(missing = pc, n64 and 3ds; this project lists the Web too). Commands for a
platform not listed fail and name the field; `build all`, `assets all` and `test all` skip it.

The self-tests live in `n64/src/lib.rs` and `n3ds/src/main.rs`: drive frames
with `host.step`, time them with `host.measure` + `host.report_frames`
(`trino.toml` › `n64_max_frame_us` gates the N64 average), dump golden frames
with `host.dump_framebuffer` (`n64_goldens` / `n3ds_goldens`, blessed with
`--bless`). See the engine's `AGENTS.md` and `.agents/skills/`.
