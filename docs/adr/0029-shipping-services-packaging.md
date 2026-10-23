# ADR 0029: Shipping games — platform services, packages, crash reports

Status: accepted (October 2026, GitHub #61). Implemented in `trino-core` (`services`,
`version`), every platform backend, `trino-runner-pc` (packaged mode, the store layer,
the Steam backend, crash reports), `trino-scene` (`services`, `steam`),
`trino-asset-pipeline` (`services`, `ship`), xtask (`services`, `package`, `deck`) and the
editor's Project tab. Guide: [docs/SHIPPING.md](../SHIPPING.md).

## Context

`release.yml` publishes the engine. A game made with Trino had no way to a store: no
achievements, stats or cloud saves, no package per OS (the PC host baked its assets from
the build machine's source tree at every start and saved under `target/`), no signing,
no crash reports, no version a player could quote, and nothing checked the Steam Deck's
requirements.

## Decisions

### 1. One services API in the core, every target behind it

- `trino_core::services` (`no_std`): `AchievementId`/`StatId` (`asset_id` of the API
  name), the `TACH` table baked from `assets/achievements.ron` into the
  `services/achievements` blob (no manifest entry: it rides `asset_pipeline::ui::bake`,
  so every target's bake has it), `AchievementState` (a bit per achievement, an `i32` per
  stat; `MAX_ACHIEVEMENTS` 128, `MAX_STATS` 16) and `HostedServices`, which every backend
  holds.
- The `Platform` methods are **defaults** working through
  `services()`/`services_mut()` — `unlock`, `is_unlocked`, `set_stat`, `stat`,
  `achievements`, `restore_achievements`, `reset_achievements`, `next_unlock`,
  `set_presence`, `cloud_sync_hint`, `services_info`. A backend implements the two
  accessors; no `GAME_API_VERSION` bump (core methods with defaults, ADR rule).
- Stat thresholds unlock in the core (`set_stat`), so every target agrees.
- Unknown ids are no-ops, not panics: a game may run without the table.

### 2. Consoles keep achievements in the game's save

The N64 has no second medium, and an engine-reserved EEPROM region would move the
two-slot layout's slot B and shrink `save_capacity` for every game (Solid Eleven already
stores ~1.8 KB of 2 KB). So the consoles keep the state **for the session**, like the
accessibility settings (ADR 0027), and the game stores `AchievementState::pack_for` in
its own save: `3 + ⌈n / 8⌉ + 4 × stats` bytes, after `version, n, stats`. Restoring
merges (bits add up, increment-only stats keep the higher value, others take the saved
one) and shows no popup, so the same calls are harmless on the PC and the Web, which
keep their own copy. Considered and rejected: a services region at the EEPROM's end
(layout migration, capacity loss for games without achievements).

### 3. The bit is the place in the file, frozen by a lock

Saves store bits, so the order is the format. `assets/achievements.lock` (one id per
line, `services lock`) records the shipped order; `services check` fails a list that
doesn't start with it. The PC's own file stores API names, so a reordered table before
the first lock still reads right there.

### 4. The overlay is core code

`AchievementToast` (queue of 4, 3 s, a slide unless `reduce_motion`) and
`AchievementList` (rows, focus, scroll, hidden rows as `???`, a progress bar toward a
threshold) draw with `fill_rect`, `draw_sprite` and `draw_text`, text through
`UiStrings` and its `font()` remap. `ServicesInfo::store_toasts` tells a game that the
store draws its own popup (Steam).

### 5. Stores stay out of the core: a store layer in the runner

`trino_runner_pc::store`: `StoreApi` (the Steamworks subset Trino uses) and `StoreSync`,
the logic: merge both ways at start (local unlocks earned offline go up, the store's come
down without popups), forward each frame's unlocks, stats, presence, reset and cloud hint,
`StoreStats` at once on an unlock or a cloud hint and at most once a second otherwise,
publish the overlay's state (`ServicesInfo::overlay_open`), fold Steam Input's actions
into the frame's `InputState` and its origin names into the gamepad labels.
`trino_runner_pc::steam` (feature `steam`, the `steamworks` crate) implements
`StoreApi`; `StoreSync` is tested with `MockStore`, no Steam. A failed `SteamAPI_Init`
logs why and keeps the local file. Presence keys map to Steam tokens
(`presence.stage` → `#presence_stage`, `{x}` → `%x%`), the same function the exporter
uses.

### 6. A package is self-contained: the runner's packaged mode

`trino-package.txt` beside the executable (or in a bundle's `Contents/Resources`) holds
the game's identity, the baked assets' path, the profile, the Steam app id and the crash
endpoint. With it the runner skips the bake and the watcher, reads the package's assets,
starts on its profile (native by default), shows its icon on the window, and puts every
per-player file in the OS's user data folder — the same folder for saves, settings,
achievements and crash reports, and the one Steam Auto-Cloud is given.

### 7. `cargo xtask package`: native builds, hand-written formats

- One build per OS on that OS (the CI matrix); Windows also from Linux with mingw. No
  cross-compiling to macOS.
- `dist` profile on the command line (`--config profile.dist.*`): release, fat LTO, one
  codegen unit, debug info stripped but symbols kept (crash backtraces name functions).
- The formats are written by xtask, testable on Linux: `.ico` and `.icns` (PNG
  payloads), the Windows `.res` (icons + `VS_VERSIONINFO`; `link.exe` takes it,
  `windres` converts it for GNU), zips with Unix modes. The AppImage is a pinned type-2
  runtime (SHA-256 checked) concatenated with `mksquashfs`'s image.
- Steam's SteamPipe layout and butler's `.itch.toml` are written beside; uploads stay a
  human or CI step with secrets.
- Signing (Authenticode by certificate or Azure Trusted Signing; `codesign` +
  `notarytool`) runs when its secrets are set and reports `none` otherwise.
- `--smoke` runs the artifact from outside the repository (proving the packaged mode),
  writes its last frame, then crashes it on purpose (`TRINO_CRASH_AFTER`) and requires
  the report.

### 8. Version stamping in the game's crate

`trino_core::build_info!()` expands `env!("CARGO_PKG_VERSION")` and `option_env!` for
`TRINO_GAME_VERSION`, `TRINO_GIT_COMMIT`, `TRINO_BUILD_DATE` in the caller's crate, so it
names the game, not the engine. `package` sets them; console builds don't by default (a
commit string in rodata must not perturb the N64 perf gates' layouts).

### 9. Crash reports: local first, upload only on consent

A panic hook writes a text report (identity, OS, GPU from the adapter, panic, location,
backtrace, the last 200 lines of `Platform::log` kept by `trino_platform_pc::system`).
`Platform::crash_reports`/`set_crash_consent` (defaults `None`/no-op: consoles and the
Web write none) let a game ask once; the answer is kept in `<save>.crash`. Plain `http://`
upload is a few lines over a socket (testable with a local listener); `https://` needs the
`crash-upload` feature (`ureq` + rustls). Minidumps and an input-replay tail (#67) are
follow-ups.

### 10. The 3DS as a `.cia`

`package 3ds --cia` = `build 3ds` + `bannertool` + `makerom` with a generated RSF
(homebrew unique id range, 804 MHz and L2 cache asked for, the services the shim uses),
plus the same NCCH as a `.cxi`, which Azahar boots: CI runs the whole self-test on it
through `TRINO_3DS_APP`. Tools pinned by release and checksum.

### 11. AI and editor parity

Every editor action writes the text the CLI writes: the Project tab's Achievements page
edits `assets/achievements.ron` (and the lock, and Steam's export through
`trino_asset_pipeline::services::export_steam`); its Packaging page sets `trino.toml`
keys through `trino_asset_pipeline::ship::set` (`toml_edit`, comments kept), the function
behind `cargo xtask package config set`.

## Consequences

- A game ships on Steam or itch.io with no store code in its crate, and the same game
  code keeps achievements on every console.
- Packages are verified in CI on all three OSes by running them; the Steam backend is
  verified against a mock in CI and by hand against app 480.
- Not done here: minidumps, Epic/GOG backends, the Web's itch.io/Newgrounds adapters, a
  Flatpak; the `.cia` is checked in Azahar, not yet on a console.
