# ADR 0008: Console packaging for external projects

- Status: accepted (2026-10-14)
- Context: BACKLOG P1 #3. The `.z64`/`.3dsx` builds lived inside this repository
  because `apps/n64` and `apps/3ds` link castle64. A commercial game needed its own
  repository and still had to ship on every target.

## Decision
- A project is described by **`trino.toml`**: name, ROM title, save type, the 3DS
  title/author/icon, the host crates' package names, and test gates. The xtask's
  console commands take `--project <dir>`, `$TRINO_PROJECT`, or a `trino.toml` in the
  current directory. Without one, they package the engine's own demo.
- **The engine stays the owner of everything native**: the shims, the target spec,
  the Docker image, the emulators. The container mounts the engine at `/engine` and
  the project at `/project`. The shim and RSP objects build in the engine's `target/`;
  assets, the staticlib and the ROM build in the project's.
- **The game's console crates are thin.** `n64_main!(make, self_test[, setup])` and
  `n3ds_main!(make, self_test)` generate `trino_rust_main` over `N64Host` / `N3dsHost`.
  The self-test reports through the same magic strings; `TRINO_FRAME` lines feed a
  project frame gate (`n64_max_frame_us`); dumped frames feed project goldens.
- **A project needs a local engine checkout** (a path dependency or a git submodule
  pinned to a commit), not only git dependencies: the native toolchain inputs are
  files in the engine.

- **Target platforms (2026-10-16).** `[project] platforms = ["pc", "n64", "3ds"]`
  says which platforms a project ships on; missing means all three, so older files
  behave as before. Every platform command checks it: naming a left-out platform
  (`build n64`) fails with the field and the file in the message, and the batch
  commands (`build all`, `assets all`, `test all`) skip it with a line saying so.
  `[n64]` and `[3ds]` are required only for listed consoles. The PC commands run on
  the project too: `build|run pc` use its desktop host (`[pc] package`, default
  `<name>-pc`), `assets pc` bakes its `assets/`, `test pc` runs its workspace.

## Consequences
- The standalone template ships `n64/`, `n3ds/`, `trino.toml` and a `cargo trino`
  alias, and a fresh scaffold builds and passes in ares and Azahar.
- Engine improvements a project needs land here, like the ones the first external project drove:
  pointer input, `fill_rect`, the resident text atlas, `RSP_BAND`, RSP streaming. The
  project bumps its submodule.
- Not solved: publishing crates (P1 #4); building a project against git deps alone.
