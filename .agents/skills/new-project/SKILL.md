---
name: new-project
description: Scaffold a Trino game — an in-repo example crate (cargo xtask new <name>) or a standalone project outside the repo (--standalone, path or git engine deps) — and maintain the templates and trino-runner-pc behind them. Use for "start a new game", "create a project", "use Trino from another repo", or when editing templates/ or crates/runner-pc.
---

# new-project

```
cargo xtask new my-game                                     # examples/my-game, in the workspace
cargo xtask new space-cat --standalone ~/games/space-cat     # engine via path deps
cargo xtask new space-cat --standalone <dir> --git https://github.com/vitorjordao/Trino --rev <commit>
cargo xtask new space-cat --standalone <dir> --template castle64   # start from a demo (blank|castle64|showcase|football)
cargo xtask new                                              # asks for name, folder, template on stdin
cargo xtask editor                                           # the editor's launcher can create one too
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

## Templates and the shared scaffolder (`crates/scaffold`)

- **`trino-scaffold`** renders standalone projects. `cargo xtask new --standalone`
  (`xtask/src/standalone.rs` only parses the arguments and runs the prompts) and the
  editor's project launcher both call `trino_scaffold::scaffold`, so they write the
  same files. The in-repo `new <name>` (`examples/<name>`) stays in xtask.
- **`--template`** (needs `--standalone`):
  - `blank`, the default, uses `templates/new-game/`.
  - `castle64` and `showcase` copy `examples/<demo>/` as the game crate. The demo's
    game type (`Castle64Game`, `ShowcaseGame`) becomes `<Camel>Game` and its crate
    path becomes `<snake>::`. The package is renamed in the demo's own `Cargo.toml`,
    which keeps features such as showcase's `hot-reload`.
  - Nothing else is renamed. `include!("showcase_gen.rs")` and
    `BlobId::from_path("scenes/showcase")` name files and manifest keys, not the
    crate.
  - A demo also brings the engine's whole `assets/` (the manifest replaces the
    template's), `scenes/` and `prefabs/` (`[scenes.*]` paths are relative to the
    project root).
- **Hosts call `{{game_new}}`.** It is the constructor expression: `x::XGame::new`,
  or `|_| x::XGame::new()` for showcase, whose `new` takes no screen. Templates under
  `templates/standalone/` must use it, never a hardcoded `::new`.
- **Interactive `new`** (no name): `standalone::prompt` reads name, folder and
  template from any `BufRead`.
  - It asks again after a wrong answer.
  - The folder defaults to the engine checkout's parent directory.
  - When stdin ends early (a closed or null stdin, as under `Command::output()`), it
    prints the old usage error. Scripts see the same failure as before the prompts
    existed.
- **Gates:**
  - `crates/scaffold` unit tests render each template and assert the renames and the
    host text. The console hosts are cfg'd to their targets, so a host
    `cargo check` never sees their bodies.
  - `xtask/tests/standalone.rs::every_template_compiles_out_of_the_box` (ignored)
    runs `cargo check --all-targets` on the game crate and PC host of each template.
  - All of them scaffold under `target/test-projects/`, never the system temp dir.
  - `football` copies `examples/solid-eleven` (crate `solid-eleven`, type
    `SolidElevenGame`; `Demo::crate_name` differs from the key). It brings **its own
    `assets/`** (`Demo::own_assets`) instead of the engine's, so a project doesn't carry
    castle64's art.
- **Overlays:** `templates/<key>/` (when it exists) is copied over the standalone base
  after rendering. Text files go through the same placeholders, `*.tmpl` drops its
  suffix, and PNGs are copied byte for byte. The football overlay brings console hosts
  whose self-test plays a scripted match and checks the sim hash, plus a `trino.toml`
  with `n64_max_frame_us` and golden lists, the goldens under `tests/golden/`, and the
  project's AGENTS.md.
  - **After changing how the match plays or looks:** re-pin
    `solid_eleven::selftest::EXPECTED_HASH`, then re-bless the goldens.
  - **To re-bless:** scaffold a project, run `cargo xtask test n64|3ds --project
    <dir> --bless`, view the PNGs, and copy them back into
    `templates/football/tests/golden/`.
  - **Then confirm:** a fresh scaffold must pass without `--bless`; CI does exactly
    that.
- **Demo-based projects run as they are.** A castle64 copy and a showcase copy each
  ran the 30-frame PC smoke (`TRINO_RUN_OK`).

## Console projects (`trino.toml`, ADR 0008)

| Key | Meaning |
|---|---|
| `[project]` | `name` (file stem); `title` (≤ 20 ASCII, the ROM header and 3DS menu); `description`, `author`; `platforms` (below) |
| `[pc]` | `package` (desktop host crate; optional, default `<name>-pc`) |
| `[n64]` | `package` (staticlib crate); `save` (`eeprom16k` = 2 KB: framing leaves 2040 B). Needed only when n64 is targeted |
| `[3ds]` | `package`, `bin`, `icon` (a 48×48 PNG). Needed only when 3ds is targeted |
| `[web]` | `package` (the wasm host crate, default `<name>-web`), `canvas_id`, `page_title`; all optional |
| `[test]` | `n64_goldens`/`n3ds_goldens` (frames the self-test dumps, in order); `n64_max_frame_us` (gate on `TRINO_FRAME` averages); `watch` dirs |

- **Target platforms:** `platforms = ["pc", "n64", "3ds", "web"]` under `[project]`
  (missing = pc, n64 and 3ds, the old behavior: the Web needs a host crate, so it is listed
  by name; new projects list it).
  - Each platform command checks it: `cargo trino build n64` on a project without
    `"n64"` fails, and the message names the field and the file.
  - `build all`, `assets all` and `test all` skip the left-out platforms and print
    a line for each. Bare `build` and `assets` mean `all`; bare `test` is still
    the host suite only.
  - The gate is `Project::require`, called from `gate` in `xtask/src/main.rs`
    after `hardware::parse`, so `run --target n64` is gated too. The batch plan is
    `Project::batch`.
  - PC commands follow the selected project: `build|run pc` build its `[pc]`
    package, `assets pc` bakes its `assets/`, and `test pc` runs its workspace.
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

## The Web host (`web/`, issue #47)

- New projects have a `web/` crate: a `cdylib` whose `start` calls
  `trino_platform_web::run_game(WebConfig::new(name, &query), "assets/trino.pak", ctor)`
  — the Web twin of `trino-runner-pc`. It builds empty off wasm32, so `cargo check` of
  the workspace stays host-only.
- `cargo trino build web [--release] [--out dist/web]`, `run web`, `test web` (a smoke
  run, `?frames=120`, in each browser: the engine's goldens don't apply to a project).
  The wasm-bindgen CLI follows the project's `Cargo.lock` (copied from the engine's at
  scaffold time).
- Older projects add it by hand: copy `templates/standalone/web/` (render the
  `{{...}}` placeholders), add `"web"` to the workspace members, `wasm-bindgen = "0.2"`
  and `trino-platform-web` (path or git, like the other engine crates) to
  `[workspace.dependencies]`, and `"web"` to `[project] platforms`.
