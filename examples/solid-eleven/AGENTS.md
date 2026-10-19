# Solid Eleven — Agent Guide

A low-poly 3D football game (issue #39) and the engine's `football` project
template. The engine-wide guide (repository root `AGENTS.md`) is the canonical
reference; the player-facing guide is `docs/SOLID_ELEVEN.md`. This file covers
what is specific to this crate.

## Map

| Path | What it is |
|---|---|
| `src/sim/` | The match and the league: pure Q16.16 fixed point at 60 Hz. Never imports `trino_core`, uses no floats (`xtask/tests/solid_eleven_layers.rs` fails otherwise). |
| `src/sim/fx.rs` | `Fx`, `V2`, `V3`, `Rng`, `Hasher` (the cross-target state hash) |
| `src/sim/ball.rs` | Flight, drag, Magnus curl, bounces, roll (wet: faster), posts, bar, net; `pass_speed` from a const roll table |
| `src/sim/rules.rs` | The referee as pure functions + the rules matrix tests |
| `src/sim/engine.rs` | `Match`: phases (setup, ready, play, dead, goal, half time), touches, offside watch, tackles and slides, fouls and cards, keepers, substitutions, officials, events, stats |
| `src/sim/ai.rs` | Shape (`anchor`), carrier choices (`carrier`), support runs, pressing, cover, zone marking, the keeper, set pieces, the human's pass/shot plans, the shared ball trajectory (`Traj`) |
| `src/sim/team.rs`, `league.rs` | Clubs, formations, attributes and point budgets; the career, transfers, the 1.8 KB save |
| `src/sim/tests.rs` | AI-sample metrics (a regression guard), full-match determinism, substitutions; `trace_match` + `tools/solid_eleven/trace_view.py` draw a match top-down (tuning) |
| `src/game/` | `app.rs` (screens, career flow, saves), `session.rs` (the fixed-tick match, banners, visuals), `input.rs` (charged kicks), `menu.rs` (navigation), `e2e.rs` (button-only end-to-end tests) |
| `src/render/` | `mod.rs` (camera, culling, stadium, players, ball, markers, PC effects), `anim.rs` (clip sampling, blending, joint matrices), `clips_gen.rs` (generated), `hud.rs`, `menu.rs` (menu backdrops) |
| `src/ui/` | `mod.rs` (the immediate-mode toolkit), `screens.rs` (every screen), `i18n.rs` (EN / PT-BR) |
| `src/selftest.rs` | The console self-test's game half: the scripted match, its pinned hash, `NullRenderer` |
| `assets/` | This game's own asset set (manifest + masters): the template copies it instead of the engine's |

## Rules for this crate

- `sim/` stays deterministic: integer math only, the match's `Rng` only, no
  `trino_core`. The same pads give the same match on PC, ares and Azahar
  (`selftest::EXPECTED_HASH`, `TRINO_SE_HASH` in the console logs).
- Every console draw must stay on the N64's RSP (`xtask/tests/solid_eleven_budget.rs`):
  opaque, untextured, vertex colors, single-sided, a white light. Color comes
  from tints: kits, skin, and the sky's dusk/night tint (`Sky::tint`).
- The frame cost is pinned (`tests/perf/solid_eleven_work.toml`). Re-bless with
  `TRINO_BLESS=1 cargo test -p xtask --test solid_eleven_budget` and review: a
  number going up is a cost the N64 pays.
- Strings go through `ui/i18n.rs` and must use only `ui::FONT_CHARS` (uppercase,
  digits, punctuation, Portuguese accents).
- Models come from `tools/blender/solid_eleven_models.py`
  (`cargo xtask blender solid-eleven`); clips from
  `tools/blender/solid_eleven_anim.py` (it writes `src/render/clips_gen.rs`). Never
  edit the generated file.

## Commands

```
TRINO_GAME=solid-eleven cargo run -p trino-app-pc            # play (TRINO_SIM=n64|3ds|native)
cargo test -p solid-eleven                                    # rules, physics, AI sample, e2e
cargo test -p xtask --test solid_eleven_budget                # RSP + pinned frame cost
cargo test -p xtask --test solid_eleven_layers                # layer rules, fonts, clips
cargo test -p xtask --test solid_eleven_render                # PC goldens (TRINO_BLESS=1 to rewrite)
cargo test -p xtask --test solid_eleven_shots -- --ignored    # screenshots in target/solid_eleven_shots
cargo test -p solid-eleven --release print_metrics -- --ignored --nocapture   # AI tuning numbers
cargo xtask blender solid-eleven [--only a,b]                 # rebuild models + clip tables
cargo xtask new <name> --standalone <dir> --template football # a project from this game
cargo xtask test n64 --project <that dir>                     # its ares self-test (TRINO_ARES=docker)
```

## Lessons (also in the skills)

- A 20 m ground tile under the camera doesn't fit the RSP's clip band: coarse
  tiles far, fine tiles near, culled by their exact rectangle.
- Chasers and keepers predicting the ball on their own cost ~740 ball steps a tick
  on the N64: one shared trajectory per tick (72).
- Keep the camera's `target - eye` bit-exact while it pans (snap the focus to
  1/4096): the N64's draw memo then patches the stadium's draws.
- A human side waits up to 8 s at a kick-off: scripts must press a kick once
  the players are set (the self-test once idled three seconds of its window).
- Restarts are walked, never glided (issue #86): `walk_in` caps a step at the
  player's legs and play restarts once the taker is set and the positions are
  legal (`restart_legal`). A Setup that waits for *everyone* to arrive costs
  seconds per restart; a Setup that times out and teleports is what the issue
  removed. `restarts_never_teleport_anyone_and_takers_line_up` fails the old code.
- A throw-in starts from the ball's spot and the ball stays overhead through the
  wind-up (`release`, `ball_tick`). Starting it from the thrower's feet + facing
  put the ball over the line once the thrower turned to aim, and a corner-flag
  throw-in looped forever (a match never finished: a balance test "hangs").
- `think` used to overwrite the carrier's `target` with his shape slot every
  tick, so between his thoughts (3 ticks in 4) he ran back toward it: a 1v1
  runner turned round at the box. The carrier now keeps the run he chose.
- Possessions won alternate between the sides, so a "share of balls won" is
  always ~50 %: measure pressing as balls won in the opponents' half, possession
  and fouls in a duel against the old pressing (`press_duel`, `urgency_test`).
- The balance tests (`sim/balance.rs`) sit near their limits on fixed seeds:
  side bias and club spread move by chance with any sim change. Tune on the
  60-match aids (`print_big_metrics`, `print_press_duel`, `print_side_bias`) and
  pick constants that pass every fixed-seed test, not one that passes one.
- `solid_eleven_render::the_match_passes_strict_mode_on_the_n64_profile` checks
  only the worst frame: an offscreen draw counts as an N64 CPU draw
  (`RspReject::Offscreen`). Cull the guides like the players (`draw_guides` takes
  the frustum) or a moved match fails it.
