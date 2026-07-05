# Contributing to Trino

Thanks for your interest! The roadmap in `PLANO_EXECUCAO_TRINO.md` (Portuguese) and
the current-state section of [AGENTS.md](AGENTS.md) tell you exactly what exists.

## Getting set up

### PC development (all you need for most work)

1. Install Rust via [rustup](https://rustup.rs). The workspace builds on stable;
   `rust-toolchain.toml` picks the right one automatically. Console builds also
   need nightly: `rustup toolchain install nightly --component rust-src`.
2. `cargo xtask run pc` — if a window opens, you are set.

### N64

- Docker reachable from the shell (on Windows, WSL2 with dockerd is enough — no
  Docker Desktop required). `cargo xtask build n64` builds the pinned libdragon
  toolchain image and the ROM for you — no manual toolchain setup.
- Testing: [ares](https://ares-emu.net) at `ares-v148/` in the repo root;
  `cargo xtask test n64` drives it automatically.
- Real hardware (optional): a flashcart (SummerCart64 / EverDrive) + UNFLoader.

### 3DS

- [devkitPro](https://devkitpro.org/wiki/Getting_Started) with the `3ds-dev` group
  (Windows has a graphical installer). No cargo-3ds needed — xtask drives the
  built-in `armv6k-nintendo-3ds` target directly.
- Testing: [Azahar](https://azahar-emu.org) (auto-detected, or set `TRINO_AZAHAR`);
  `cargo xtask test 3ds` drives it automatically.

## Workflow

1. Fork, branch from `main`.
2. Make your change **with tests** — every feature ships with tests in the same PR.
3. Run the CI gates locally before pushing:
   ```
   cargo fmt --all --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
4. Open a PR using the template. CI must be green.

## Architecture rules (the short version)

- `trino-core` has zero dependencies and stays `no_std`.
- Games depend only on `trino-core`/`trino-game-api`. Platform crates never leak their
  native APIs upward. A test (`xtask/tests/dep_graph.rs`) enforces this.
- The N64 is the design ceiling: no feature lands that cannot ship on all three targets.
- Golden images are regenerated only via `cargo xtask test --bless`, and the diff must
  be reviewable in the PR.

Full agent/human guide: [AGENTS.md](AGENTS.md).

## Code of Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md).

## License

By contributing, you agree that your contributions are dual-licensed under
[MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE), without any additional terms.
