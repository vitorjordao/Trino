---
name: new-project
description: Scaffold a Trino game — an in-repo example crate (cargo xtask new <name>) or a standalone project outside the repo (--standalone, path or git engine deps) — and maintain the templates and trino-runner-pc behind them. Use for "start a new game", "create a project", "use Trino from another repo", or when editing templates/ or crates/runner-pc.
---

# new-project

```
cargo xtask new my-game                                     # examples/my-game, in the workspace
cargo xtask new space-cat --standalone ~/games/space-cat     # engine via path deps
cargo xtask new space-cat --standalone <dir> --git https://github.com/vitorjordao/Trino --rev <commit>
```

- **In repo** (`templates/new-game/`): a game crate plus its own AGENTS.md. The
  workspace glob picks it up, and it builds for N64 and 3DS like castle64.
- **Standalone** (`templates/standalone/`, rendered by `xtask/src/standalone.rs`):
  - its own workspace: `game/` (the portable crate) and `pc/` (a host on
    `trino-runner-pc`, with live asset reload);
  - `assets/manifest.toml`, an AGENTS.md and a pinned `rust-toolchain.toml`;
  - run it with `cargo run -p <name>-pc`; `TRINO_SIM=3ds` gives the 3DS look.
  - Console packaging of standalone games does **not** exist yet (BACKLOG P1 #3).
    The `.z64`/`.3dsx` builds still link castle64 inside this repo.

## Maintaining the templates

- **Manifests are `Cargo.toml.tmpl`.** A file named `Cargo.toml` anywhere in this
  repository, placeholders included, breaks every user of the engine as a git
  dependency: cargo scans the whole checkout and fails on the bad manifest.
  `xtask/src/standalone.rs` renames the files when it renders them.
- **Paths:** use `std::path::absolute` plus normalization, not `canonicalize`. On
  Windows `canonicalize` yields `\\?\C:\…`, which ends up in path deps.
- **The gate:** `xtask/tests/standalone.rs` scaffolds into a temp dir, then builds
  and tests with plain `cargo` from inside it, reusing this workspace's `target/`.
  With `TRINO_REQUIRE_GPU=1` and a display, it also runs a 30-frame smoke and expects
  `TRINO_RUN_OK`. CI runs it under `xvfb-run`.
- Test git deps by pointing `--git` at this checkout (`file://` plus a local commit)
  before you announce a URL to users.
- **The game crate rule** holds for scaffolds too: it depends only on `trino-core`
  and `trino-game-api`. In-repo, `dep_graph` enforces it; standalone, the template
  is the only guard.
- **Template AGENTS.md files:** they are what a fresh agent reads in a new project.
  Keep their commands in sync with this skill.
