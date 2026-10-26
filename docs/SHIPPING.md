# Shipping a game

How a Trino game goes from a project to something players download — on Steam, itch.io
or as a plain download — and what it keeps for them: achievements, stats, rich
presence, cloud saves, crash reports and a version they can quote. The decisions behind
it are in [ADR 0029](adr/0029-shipping-services-packaging.md); the step-by-step workflow
for agents is the [`shipping` skill](../.agents/skills/shipping/SKILL.md).

## Achievements, stats and presence

One file, `assets/achievements.ron`, names everything a store or the game's own list
shows. The editor's **Project › Achievements** page edits the same file.

```ron
(
    version: 1,
    achievements: [
        (id: "FIRST_COIN", name: "ach.first_coin.name", description: "ach.first_coin.desc",
         icon: "sprites/coin"),
        (id: "COINS_25", name: "ach.coins_25.name", description: "ach.coins_25.desc",
         stat: "COINS", threshold: 25),
        (id: "BOAR_STOMP", name: "ach.boar_stomp.name", description: "ach.boar_stomp.desc",
         hidden: true),
    ],
    stats: [(id: "COINS", min: 0, max: 9999, increment_only: true)],
    presence: ["presence.hub", "presence.stage"],
)
```

- `id` is the store's API name (letters, digits, `_`); the game names it
  `AchievementId::new("FIRST_COIN")`.
- `name` and `description` are string keys (`assets/i18n/<lang>.ron`); the list and the
  Steam export read every language from there.
- `stat` + `threshold`: the platform unlocks the achievement when the stat reaches it.
- An achievement's **bit** in a save is its place in the file. Before the first release,
  run `cargo xtask services lock` (or **Lock order** in the editor): it writes
  `assets/achievements.lock`, and from then on `services check` fails a list that does
  not start with the locked order. Append; never reorder or remove.

The game's side is the same on every target:

```rust
platform.unlock(AchievementId::new("FIRST_COIN"));
platform.set_stat(StatId::new("COINS"), coins as i32);
platform.set_presence(StrKey::new("presence.stage"), &[Arg::int("stage", 2)]);
platform.cloud_sync_hint(); // after a save
```

and the engine draws the unlock popup and the list (`AchievementToast`,
`AchievementList`, drawn with 2D calls only: every backend shows them):

```rust
self.toast.update(platform, dt);                 // in update_with
self.toast.draw(renderer, strings, &table, &style); // after the HUD
```

| Target | Where achievements live | Presence, cloud |
|---|---|---|
| PC, no store | `<save folder>/<game>.achievements` (by API name) | no-ops |
| PC on Steam (`steam` feature) | Steam (merged with the local file both ways at start) | Steam rich presence, Steam Cloud |
| Web | `localStorage` (`trino-services:<save>`) | no-ops |
| N64, 3DS | in RAM for the session; **the game's save** holds them | no-ops |

On a console a game stores `platform.achievements().pack_for(&table, &mut buf)` in its
own save and calls `platform.restore_achievements(&state)` after loading it — `3 +
⌈achievements / 8⌉ + 4 × stats` bytes (castle64's five achievements and two stats: 12
bytes of the N64's EEPROM). The same two calls are harmless on the PC and the Web, which
merge. The starter template does exactly this.

Check and export:

```
cargo xtask services check [--json]          # ids, stats, icons, strings in every language, the lock
cargo xtask services list [--json]           # the table and its save cost
cargo xtask services export steam [--json]   # dist/steam-config: CSV, localization VDF, rich presence,
                                             # the Steam Input action manifest, 64 px icons
```

## Packaging the PC game

```
cargo xtask package pc [--target linux|windows|macos] [--steam] [--itch] [--smoke] [--json]
```

builds the project's PC host with the `dist` profile (fat LTO, one codegen unit),
stamps the version, bakes the assets **into** the package and lays it out for the OS:

| OS | Output (`dist/`) | Details |
|---|---|---|
| Linux | `<name>-<version>-linux-x86_64.AppImage` and `.zip` | the AppImage is a pinned type-2 runtime + `mksquashfs` (install `squashfs-tools`); it runs without FUSE with `APPIMAGE_EXTRACT_AND_RUN=1` |
| Windows | `<name>-<version>-windows-x86_64.zip` | the `.exe` carries the icon and version information (a `.res` written by xtask) and the GUI subsystem (no console window) |
| macOS | `<name>-<version>-macos-arm64.zip` and `.dmg` | `<Title>.app` with `Info.plist` and an `.icns`; the `.dmg` needs `hdiutil` (a Mac) |

A package builds on its own OS; CI's `package` job has a runner for each. Windows also
builds on Linux where `x86_64-w64-mingw32-gcc` is installed.

A packaged game is self-contained. `trino-package.txt` beside the executable (in
`Contents/Resources` on macOS) switches `trino-runner-pc` to **packaged mode**: assets
come from the package (no bake at start), and saves, settings, achievements and crash
reports go to the player's data folder — `%APPDATA%\<Title>`, `~/Library/Application
Support/<Title>`, `$XDG_DATA_HOME/<Title>` (`~/.local/share/<Title>`). `TRINO_SAVE_DIR`
overrides it.

`--smoke` copies the result outside the repository, runs it for 30 frames (writing the
last frame to `dist/<stem>-smoke.png`), then runs it again with `TRINO_CRASH_AFTER=3`
and checks that the deliberate panic left a crash report and its input tail (`crash-*.trec`, #67) in the save folder.

### `trino.toml`

Every key is optional; `cargo xtask package config [set key=value]` and the editor's
**Project › Packaging** page edit them in place (comments kept).

```toml
[pc]
package = "my-game-pc"
icon = "assets/platform/pc/icon.png"   # square PNG, 256 x 256 or bigger
identifier = "com.example.mygame"      # macOS bundle id / Linux app id
publisher = "Example Games"            # default [project] author
version = "1.0.0"                      # default the pc crate's version
features = ["gamepad"]                 # the pc crate's features in a package
profile = "native"                     # or n64 / 3ds: the look it starts in
crash_upload = "https://crash.example.com/report"

[steam]
app_id = 480                           # 480 = Spacewar, Valve's test app
depots = { windows = 481, linux = 482, macos = 483 }   # default app id + 1, 2, 3

[itch]
target = "user/my-game"
```

### Version stamp

`trino_core::build_info!()` expands in the game's crate to a `BuildInfo { name, version,
commit, date }`: `TRINO_GAME_VERSION` (from `[pc] version`) else the crate's version,
`TRINO_GIT_COMMIT` (`git rev-parse --short=10 HEAD`, `-dirty` with changes) else `dev`,
`TRINO_BUILD_DATE`. `package` sets all three; any build may. The starter template's
**About** screen (`ui/about.ui.ron`) shows them, and crash reports carry them.

### Steam

1. Build with the PC crate's `steam` feature: `package pc --steam` (with `[steam] app_id`)
   puts `libsteam_api` beside the executable and writes the SteamPipe layout:
   `dist/steam/content/<os>`, `dist/steam/scripts/app_build_<app>.vdf` and a
   `depot_build_<depot>.vdf` per OS, and `dist/steam/README.txt` with the upload command
   (`steamcmd +run_app_build`) and the Auto-Cloud settings.
2. **Steam Cloud:** Auto-Cloud with one root per OS — `WinAppDataRoaming`,
   `MacAppSupport`, `LinuxXdgDataHome` — subdirectory `<Title>`, patterns `*.sav` and
   `*.achievements`. Display, quality and input settings stay per machine.
3. **Achievements:** create them in Steamworks in the file's order (Steam's tokens
   follow creation order), then upload `achievements_loc.vdf` and the icons from
   `services export steam`. **Rich presence:** upload `rich_presence/<language>.vdf`;
   the runtime sets `steam_display` to `#presence_stage` with each argument as
   `%name%`.
4. **Steam Input:** `game_actions_<app>.vdf` maps every Trino button, both sticks and the
   triggers. A package next to it is read in development; for a release, set it as the
   default configuration in Steamworks.

Without a Steam client (or the feature) the runner says why on stderr and keeps the local
file. The overlay's state reaches games as `services_info().overlay_open` — pause on it.

**Steam Deck:** `cargo xtask deck check [--json]` checks what the Verified program
looks at that text can tell: gamepad support, controller glyphs, the Steam Input
manifest, every UI screen's smallest text at 1280 × 800 (≥ 9 px), the starting profile,
the Deck preset, no launcher, and a clamped `dt` after suspend.

### itch.io

`package pc --itch` writes `dist/itch/<os>` with an `.itch.toml` (the play action) and
prints the `butler push dist/itch/<os> <target>:<os> --userversion <version>` to run.

### Signing

Signing runs when its secrets are in the environment, and says it skipped otherwise.
Keep the secrets in CI, never in the repository.

| OS | Variables | Tool |
|---|---|---|
| Windows | `TRINO_WINDOWS_SIGN_CERT` (.pfx) + `TRINO_WINDOWS_SIGN_PASSWORD` | `signtool` on Windows, `osslsigncode` elsewhere |
| Windows | `TRINO_AZURE_SIGNING_DLIB` + `TRINO_AZURE_SIGNING_METADATA` | Azure Trusted Signing through `signtool` |
| macOS | `TRINO_MACOS_SIGN_IDENTITY` (Developer ID Application) | `codesign --options runtime` (ad hoc without it) |
| macOS | `TRINO_NOTARY_PROFILE`, or `TRINO_NOTARY_APPLE_ID` + `_PASSWORD` + `_TEAM_ID` | `notarytool submit --wait`, `stapler staple` |

`TRINO_WINDOWS_TIMESTAMP_URL` overrides the timestamp server.

## Crash reports

`trino-runner-pc` installs a panic hook. A panic writes
`<save folder>/crashes/crash-<UTC time>-<pid>.txt` with the game, version, commit, build
date, time, OS, GPU, profile, thread, the panic and its location, a backtrace and the
last 200 lines of `Platform::log`.

Nothing leaves the machine unless the package names an endpoint (`[pc] crash_upload`)
**and** the player agreed. The game asks once — `platform.crash_reports()` says when
(`should_ask()`: reports waiting, an endpoint, no answer yet) — and answers with
`platform.set_crash_consent(CrashConsent::Allow | Deny)`, kept in `<save>.crash`. The
starter template's `ui/crash.ui.ron` is that question. With a yes, the next launch POSTs
each waiting report as `text/plain` and renames it `*.sent`. Plain `http://` (a local
collector) needs nothing; `https://` needs `trino-runner-pc`'s `crash-upload` feature.

## The 3DS as an installable title

```
cargo xtask package 3ds --cia [--selftest]
```

runs `build 3ds`, then `bannertool` (the HOME Menu banner: `[3ds] banner` /
`banner_audio`, else the icon on a dark band and a silent jingle) and `makerom` with a
generated RSF (`[3ds] product_code` and `unique_id`, else derived from the name), and
writes `dist/<name>.cia` and the same NCCH as `dist/<name>.cxi`. `ctrtool` reads the
`.cia` back. The tools are pinned and fetched on Linux x86_64 (`$TRINO_MAKEROM`,
`$TRINO_BANNERTOOL`, `$TRINO_CTRTOOL` or `PATH` elsewhere). The `.cxi` boots in Azahar:
`TRINO_3DS_APP=dist/<name>.cxi cargo xtask test 3ds --no-build` runs the whole self-test
on it, as CI does.
