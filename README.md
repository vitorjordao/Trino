<div align="center">

# 🎮 Trino

**One Rust codebase. Three consoles.**
A game engine that ships the same game to **Nintendo 64**, **Nintendo 3DS** and **PC** —
with a modern visual editor, live reload, and console-accurate preview modes.

[![CI](https://github.com/OWNER/trino/actions/workflows/ci.yml/badge.svg)](https://github.com/OWNER/trino/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](rust-toolchain.toml)

*(hero GIF coming in Fase 6: the editor and the same scene running on all three consoles)*

</div>

---

## Why Trino?

- 🕹️ **Write once, run on real retro hardware.** Your game code depends on one small,
  `no_std` crate — the engine maps it to libdragon (N64), citro (3DS) and wgpu (PC).
- 🖥️ **A real editor.** Unity-style viewport, scene hierarchy, inspector and asset
  browser, built with egui. Press play, edit assets, watch them hot-reload.
- 📺 **Console simulation on PC.** Preview with N64 3-point filtering, RGBA5551 dither
  and 320×240 output — or strict mode, which *fails the build* when a texture would not
  fit in the N64's 4 KB TMEM.
- 🔁 **Live reload everywhere.** Game code reloads as a dylib on PC; ROMs rebuild and
  relaunch in the emulator (or re-upload to a flashcart) on file save.
- 🧪 **Emulator-tested.** CI boots your game in ares, mupen64plus and Citra and asserts
  on debug output and golden screenshots.
- 🤖 **AI-friendly by default.** The repo — and every game scaffolded by `trino new` —
  ships `AGENTS.md` and Claude Code skills, so AI agents are productive from clone.

## Status

🚧 **Early development — Fase 3 (visual editor v1) done.** The roadmap with per-phase
acceptance criteria lives in [PLANO_EXECUCAO_TRINO.md](PLANO_EXECUCAO_TRINO.md).

| | PC | Nintendo 64 | Nintendo 3DS |
|---|---|---|---|
| Window/boot | ✅ | 🚧 Fase 4 | 🚧 Fase 5 |
| 2D sprites, input, audio | ✅ | 🚧 Fase 4 | 🚧 Fase 5 |
| Console-sim resolutions + golden tests | ✅ | 🚧 Fase 4 | 🚧 Fase 5 |
| Asset pipeline + live reload | ✅ | 🚧 Fase 4 | 🚧 Fase 5 |
| Visual editor | ✅ v1 | — | — |
| 3D | 🚧 Fase 7 | 🚧 Fase 7 | 🚧 Fase 7 |

## Quickstart

```sh
git clone https://github.com/OWNER/trino
cd trino
cargo xtask run pc     # opens the (for now, empty) engine window
cargo xtask test       # full test suite — same gates as CI
```

That's it for PC. Console toolchains (Docker for N64, devkitPro for 3DS) are only needed
from Fases 4/5 — see [CONTRIBUTING.md](CONTRIBUTING.md) for setup.

## How it works

```
            ┌──────────── your game ────────────┐
            │   depends ONLY on trino-core      │
            └───────────────┬───────────────────┘
                            ▼
            trino-core  (traits + math, no_std, zero deps)
                            ▲
        ┌───────────────────┼───────────────────┐
  platform-pc         platform-n64         platform-3ds
  (wgpu/winit)        (libdragon FFI)      (ctru/citro FFI)
```

- **Materials are presets, not shaders** — the N64's RDP is the design ceiling, so
  everything the engine exposes runs on all three targets.
- **Assets have shared masters + per-platform overrides**; a format a console can't
  represent is a build error, never a silent fallback.
- **Everything goes through `cargo xtask`** — build, run, test, bake assets, watch,
  scaffold. No per-platform incantations to memorize.

## Working on Trino (humans & AIs)

Start with [AGENTS.md](AGENTS.md) — repository map, architecture rules, commands and
pitfalls. Using Claude Code? It picks the guide up automatically, and the
`.claude/skills/` folder teaches it the project's workflows.

## Contributing

PRs welcome — see [CONTRIBUTING.md](CONTRIBUTING.md). Every feature lands with tests.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
