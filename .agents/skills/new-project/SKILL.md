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
  - `trino-runner-pc` keeps two text files next to the save: `<save>.display` (window,
    scales) and `<save>.input` (key and pad bindings the game rebinds through
    `Platform::rebind`). Both are applied after the frame (the game asks, the host
    applies and persists); a broken line falls back to the default. Linux builds have
    no `gamepad` feature by default (it needs `libudev-dev`): the game then sees
    `GamepadStatus::Unsupported` and can still rebind the keyboard.
  - **Consoles:** the template also ships `n64/` (a staticlib using `n64_main!`),
    `n3ds/` (a bin using `n3ds_main!`, with a `build.rs` that relinks when the shim
    changes) and `trino.toml`.
    - `cargo trino build|run|test|watch n64|3ds` is the engine's xtask, aliased in the
      project's `.cargo/config.toml`. It finds `trino.toml` in the cwd; `--project <dir>`
      and `$TRINO_PROJECT` also work.
    - Outputs go to the project's `target/n64/<name>.z64` and `target/3ds/<name>.3dsx`.
    - A local engine checkout is required (a path dep, or an `engine/`
      submodule): the shims, target spec and Docker image live there.

## Console projects (`trino.toml`, ADR 0008)

| Key | Meaning |
|---|---|
| `[project]` | `name` (file stem); `title` (≤ 20 ASCII, the ROM header and 3DS menu); `description`, `author` |
| `[n64]` | `package` (staticlib crate); `save` (`eeprom16k` = 2 KB: framing leaves 2040 B) |
| `[3ds]` | `package`, `bin`, `icon` (a 48×48 PNG) |
| `[test]` | `n64_goldens`/`n3ds_goldens` (frames the self-test dumps, in order); `n64_max_frame_us` (gate on `TRINO_FRAME` averages); `watch` dirs |

- **The self-test** is the macro's second argument, `fn(&mut N64Host, &mut G) ->
  Result<(), String>`. Drive frames with `host.step`, time them with `host.measure` +
  `host.report_frames`, and dump goldens with `host.dump_framebuffer`. The N64 macro
  fails the test above 60 KB of stack.
- **`n64_main!(make, self_test, setup)`**: `setup(&mut N64Host)` runs after boot, for
  renderer options such as `set_rsp_stream`.
- **Pitfall:** the N64 crate must be `#![cfg_attr(target_os = "none", no_std)]`. A
  plain `no_std` breaks the host `cargo check --workspace` (no panic handler).
- **Gate:** `xtask/tests/standalone.rs` renders the template, and `project::tests`
  parses `trino.toml`. The console path was verified end to end on a fresh scaffold:
  it builds and passes in ares and in Azahar.

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
