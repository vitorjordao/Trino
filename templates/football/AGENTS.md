# {{name}} — Agent Guide (football template)

A standalone [Trino](https://github.com/vitorjordao/Trino) game: one
codebase for PC, Nintendo 64 and Nintendo 3DS. This file is the entry point
for AI agents and humans.

## Layout

| Path | What it is |
|---|---|
| `game/` | The game crate (`{{name_snake}}`). Portable: depends only on `trino-core` + `trino-game-api`. |
| `pc/` | Desktop host (`trino-runner-pc`): window, renderer, audio, input, asset bake. |
| `n64/`, `n3ds/` | Console hosts (`n64_main!` / `n3ds_main!`) and their emulator self-tests. |
| `trino.toml` | Target platforms (`[project] platforms`), console packaging (ROM title, save type, 3DS icon), test gates. |
| `assets/manifest.toml` | Every asset by logical path; masters in `assets/shared/`. |
| `Cargo.toml` | Workspace; the engine dependencies (path or git) are declared once here. |

## Rules for game code

- `game/` depends **only** on `trino-core` (+ `trino-game-api`): no external
  crates, no std APIs. That is what lets it run on the consoles unchanged.
- Keep `update` deterministic: same inputs + same state = same result.
- Assets are referenced by logical path and declared in `assets/manifest.toml`.
- Screen space is X-right, **Y-down**, origin top-left; stick input is Y-up.
- If a feature cannot work on the N64, the engine does not expose it —
  budgets live in `trino_core::Caps`.

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
```

`[project] platforms` in `trino.toml` lists the platforms this game ships on
(missing = all three). Commands for a platform not listed fail and name the
field; `build all`, `assets all` and `test all` skip it.

The self-tests live in `n64/src/lib.rs` and `n3ds/src/main.rs`: drive frames
with `host.step`, time them with `host.measure` + `host.report_frames`
(`trino.toml` › `n64_max_frame_us` gates the N64 average), dump golden frames
with `host.dump_framebuffer` (`n64_goldens` / `n3ds_goldens`, blessed with
`--bless`). See the engine's `AGENTS.md` and `.agents/skills/`.

## This project: a football game (Trino's `football` template)

It started as a copy of **Solid Eleven**, the engine's football demo. The
engine's `docs/SOLID_ELEVEN.md` is the full guide: rules, AI, career,
performance and tests.

| Path | What it is |
|---|---|
| `game/src/sim/` | The match and the league: pure Q16.16 fixed point at 60 Hz (rules, ball physics, tactical AI, clubs, career, save). Keep it free of floats and of `trino_core`: the same inputs must give the same match on every target. |
| `game/src/game/` | Screens and flows (`app.rs`), the match session, charged-kick controls, menu navigation, button-driven end-to-end tests |
| `game/src/render/` | Broadcast camera, culling, stadium, players posed from `clips_gen.rs`, ball, markers, HUD, menu backdrops |
| `game/src/ui/` | The UI toolkit, every screen, the string ids (`i18n.rs`) and their tables exported as static code (`strings_gen.rs`) |
| `assets/i18n/` | The strings in English and Portuguese (the engine's tables): edit, then `cargo trino i18n export --rust game/src/ui/strings_gen.rs` and `cargo trino i18n check`. Keep to `ui::FONT_CHARS` |
| `game/src/selftest.rs` | The console self-tests' scripted match and its pinned sim hash |
| `assets/` | The game's models (built by the engine's `tools/blender/solid_eleven_models.py`), fonts, sounds |
| `tests/golden/{n64,3ds}/` | The self-tests' golden frames |

Rules that keep it running on the N64 at 30 fps:
- Every console draw stays on the RSP: opaque vertex colors, single-sided,
  a white light. Color comes from per-draw tints (kits, skin, the sky).
- Draw only what the camera sees (the game culls before the engine does),
  and keep `target - eye` of the camera bit-exact while it pans.
- After changing how a match plays or looks: re-pin
  `selftest::EXPECTED_HASH` (`cargo test` prints the new one), then re-bless
  the goldens (`cargo trino test n64 --bless`, `cargo trino test 3ds --bless`).
