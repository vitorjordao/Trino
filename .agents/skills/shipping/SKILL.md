---
name: shipping
description: Ship a Trino game to players — achievements, stats, rich presence and cloud saves through `Platform` on every target (`assets/achievements.ron`, `cargo xtask services check|lock|list|export steam`), PC packages per OS (`cargo xtask package pc`: AppImage, zip with the .exe's icon and version, .app/.dmg, SteamPipe and butler layouts, signing), the Steam backend (`steam` feature), crash reports with consent, the version stamp (`build_info!`), `deck check`, and the 3DS `.cia`. Use for "achievements", "Steam", "itch.io", "package/release/distribute the game", "installer", "AppImage", "sign/notarize", "crash report", "version number", "Steam Deck", "Steam Cloud", ".cia".
---

# shipping: from a project to players

Guide: `docs/SHIPPING.md`. Decisions: `docs/adr/0029-shipping-services-packaging.md`.

## Achievements in a game

1. Write `assets/achievements.ron` (or use the editor's **Project › Achievements**):
   ids are Steam API names, `name`/`description` are string keys, `stat` + `threshold`
   unlock in the core. Add the keys to every `assets/i18n/<lang>.ron`.
2. `cargo xtask services check` — 0 errors before going on. `services list` shows the
   save cost.
3. Game code: `platform.unlock(AchievementId::new("X"))`, `platform.set_stat(..)`,
   `platform.set_presence(StrKey::new("presence.x"), &[..])`, `cloud_sync_hint()` after
   a save; `AchievementToast::update(platform, dt)` + `draw` after the HUD;
   `AchievementList` for the menu. Call the platform only when something changed
   (castle64's `report_services` compares one tuple a frame).
4. **Consoles:** store `platform.achievements().pack_for(&table, &mut buf)` in the
   game's save and `platform.restore_achievements(&state)` after loading it. The starter
   template (`templates/new-game/src/lib.rs`) does it; restoring shows no popups.
5. Before the first release: `cargo xtask services lock`. After it, only append.

## Packaging

```
cargo xtask package pc --project <dir> --smoke [--steam] [--itch] [--json]
cargo xtask package config --project <dir>                 # the keys and what package uses
cargo xtask package config set pc.identifier=com.x.y steam.app_id=480 --project <dir>
cargo xtask deck check --project <dir>
cargo xtask package 3ds --cia [--selftest] --project <dir>
```

- `--smoke` is the proof a package works: it runs the artifact from outside the
  repository (so the packaged mode, not the source tree, serves the assets), saves the
  frame to `dist/<stem>-smoke.png`, and crashes it once on purpose (`TRINO_CRASH_AFTER=3`)
  expecting a report. Read `[package] smoke:` lines.
- Building takes minutes (fat LTO). Give a scratch project its own `CARGO_TARGET_DIR`
  only if you accept a full rebuild; `target/test-projects/<name>` with the engine's
  `target/` is fastest.
- Linux needs `mksquashfs` (`squashfs-tools`); the AppImage runtime downloads once into
  `target/tools` (SHA-256 pinned). Without FUSE: `APPIMAGE_EXTRACT_AND_RUN=1`.
- Windows and macOS packages build on their own OS (CI's `package` job). Local proof
  on Linux stops at the format writers' unit tests (`xtask/src/package/*.rs`).
- `--json` prints one JSON document on stdout; every subprocess's stdout goes to stderr
  there (mksquashfs's progress bar once broke the JSON).

## Steam

- The PC crate needs `steam = ["trino-runner-pc/steam"]` (the templates have it) and
  `[steam] app_id`. The runner connects with `Client::init_app(app_id)`; without a client
  it prints `[trino] Steam is off (...)` and keeps the local file — so CI can smoke a
  `--steam` package with no Steam.
- `libsteam_api` is linked dynamically: `package` copies it beside the executable
  (Linux also gets `-rpath $ORIGIN`). `cargo run`/`cargo test` find it through cargo's
  `LD_LIBRARY_PATH`; a binary moved elsewhere without it won't start.
- `steamworks::sys` is private unless the crate's `raw-bindings` feature is on (Steam
  Input's handle types need it).
- Logic is tested with `store::MockStore`: change `StoreSync`, not the glue.
- Manual checklist with a client (app 480, Spacewar): unlock → Steam popup; stats
  survive a restart; reset clears; Cloud: save on machine A, load on B; rich presence in
  the friends list; Shift+Tab sets `services_info().overlay_open`.

## Crash reports

- The runner's hook writes `<save folder>/crashes/crash-*.txt`; `Platform::log` lines are
  kept (`trino_platform_pc::system::remember_log`), `println!` is not.
- Test a panic hook in a **child process** (`crates/runner-pc/tests/crash_hook.rs`
  re-runs its own test binary with an env var): a hook is process-wide.
- Upload only with `CrashConsent::Allow` and an endpoint; `http://` to localhost works
  without features (tests use a `TcpListener`), `https://` needs `crash-upload`.

## The 3DS `.cia`

- `package 3ds --cia` fetches makerom 0.19.0, bannertool 1.2.0 and ctrtool 1.3.0
  (pinned; needs `curl`, `unzip`, `sha256sum`), writes `.cia` + `.cxi`.
- Prove it boots: `TRINO_3DS_APP=$PWD/dist/<name>.cxi cargo xtask test 3ds --no-build`
  after `package 3ds --cia --selftest` (the whole self-test and the 3DS goldens).
- `--selftest` is parsed globally by xtask (`main.rs`) and handed back to `package`;
  a new subcommand that wants it must do the same (it once built a non-test CIA and
  the harness waited forever at `test_mode=false`).

## Pitfalls

- A packaged game must not read `CARGO_MANIFEST_DIR` paths: that's the build machine.
  Everything per player goes through `RunConfig::save_dir()`.
- The issue's `[pc.package]` collides with `[pc] package` (the crate); the packaging keys
  live directly in `[pc]`.
- `pkill -f <pattern>` from a shell whose own command line contains the pattern kills
  that shell (exit 144). Kill an emulator by its PID (`pgrep -f squashfs-root/AppRun`).
- Adding strings to `assets/i18n/*.ron` grows `fonts/i18n@<lang>` atlases
  (`chars = "auto"`): castle64's achievement texts took pt-BR and es-ES from 2 KB to
  4 KB of I4 (the whole TMEM). Check `cargo xtask i18n fonts --platform n64`.
