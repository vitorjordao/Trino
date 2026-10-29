# ADR 0037 — Fuzzing every parser, save slots and recovery, sanitizers in CI

- Status: accepted (2026-10-26)
- Issue: GitHub #68
- Supersedes: nothing. The EEPROM layout of `trino_core::save` (ADR 0005 §5) gains a
  flag; every save it wrote before still reads.

## Context

Trino parses its own binary formats on every target, from data that can come from
outside the build: a bad bake, a disk error, a PC or Web mod, an EEPROM that wears out,
an SD card a player pulls, a file a player edits, a cloud sync that brings back two
machines' files. On the N64 a panic or an endless loop is a frozen console. The formats
had round-trip and fixture tests, but no adversarial input.

## Decision

### 1. One body per parser, three callers (`crates/fuzz`, `fuzz/`)

- `trino_fuzz::TARGETS` lists one `Target` per parser: a name, the formats it covers,
  the body, the largest input worth generating, and where its seeds come from. 37 targets:
  every four-letter format the engine parses (`TMDL` and its sections, `TSCN`, `TPFX`,
  `TIMP`, `TFNT`, `TSN2`, `TMIX`, `TUIL`, `TSTR`, `TACH`, `TACC`, `TANG`, `TLUT`, `TCOL`,
  `TPHY`, `TBND`, `TREC`, `TTRC`, `TPAK`, `T3AU`), LZ4 and the save compressor, the save
  frame on a raw medium and as slot files, the small save forms (bus settings, the
  accessibility and achievement packs, the network pad), the PC's player files
  (`.input`, `.display`, `.access`, `.audio`, `.achievements`, `.crash`,
  `trino-package.txt`), the core's text parsers (quality presets, the tilemap), every RON
  format, the lobby's lines and ggrs messages, and the PC's baked asset files and group
  bundles as the runner loads them.
- **A body does what a caller does after `Ok`**: it poses and draws a model through the
  CPU T&L, lays a screen out and draws it, looks strings up through a `Localizer`, runs
  an animation graph, decodes every bundle entry. The panics that matter were "validated,
  then an accessor indexed out of range", and only these second halves reach them.
- **Where a writer exists, the body round-trips:** a written TPFX, TIMP, TMIX, TREC
  header, achievement pack, network pad or scene RON parses back to the same bytes.
- **Bounded memory and time:** a body allocates at most `MAX_DECODE` (1 MB) per size an
  input announces; libFuzzer runs with `-rss_limit_mb=2048 -malloc_limit_mb=1024
  -timeout=10`, `trino-fuzz smoke` has a 10 s watchdog per input.
- The three callers: the cargo-fuzz binaries in `fuzz/` (one line each, nightly,
  libFuzzer + ASan, `-a` for overflow checks), `cargo test -p trino-fuzz` on stable on
  every PR (committed seeds, crash reproducers, a deterministic mutation run), and the
  `trino-fuzz` CLI behind `cargo xtask fuzz`.
- **Guards:** `every_binary_format_has_a_target` reads every `pub const …MAGIC: [u8; 4]`
  in `crates/core/src` and `crates/platform-web/src` and fails on a format no target
  names; `cargo_fuzz_has_a_binary_per_target` holds `fuzz/Cargo.toml` to `TARGETS`.
- **Corpora** come from the real bakes: `cargo xtask fuzz corpus` bakes the engine's and
  Solid Eleven's assets for the PC and the Web, sorts every blob by magic (and the
  consoles' staged files when built), adds the repo's authored text and recordings and
  the writers' seeds (`trino_fuzz::seedgen`) into `fuzz/corpus/<target>/` (ignored by
  git); `--commit-seeds` keeps up to four per target in `fuzz/seeds/` (≤ 24 KB each,
  934 KB in all).

### 2. What the fuzzing found, and the fixes

| Format | Input | Before | Fix |
|---|---|---|---|
| TMDL | an index past the vertex count | the T&L read out of bounds (panic) | `Mesh::from_tmdl` refuses it (`MeshError::BadIndex`) |
| TMDL | counts whose products wrap a 32-bit `usize` | (consoles) in-bounds slices of a huge mesh | checked arithmetic for every length |
| TMDL | a vertex at 1e30 in a split draw | ~2^47 depth bisections: a hang | `SPLIT_STACK` 48 → 16 bounds the depth: ≤ 2^15 triangles per source triangle; no new code in `tessellate_joints` |
| TANG / clips | a tiny clip duration | `c += 1.0` past 2^24 never moved: a hang in `EventTrack::fire` and `RootTrack::delta` | integer cycle counters |
| tilemap | a blank first line, `\r` inside a row | `cells()` read past the text | every row's byte length must equal the first's |
| TPAK | an entry count from the header | `Vec::with_capacity(count)`: a 100 GB allocation | the count is bounded by the bytes left |
| TPAK / PC loader | sprite sizes | `w * h * 4` wrapped (u32, wasm32's `usize`, then u64) | checked products |
| TBND | an entry's `raw` size | the loaders allocated it (4 GB) before decoding | `Bundle::parse` refuses a `raw` the codec can't produce (stored: equal; LZ4: ≤ 255× + 16) |
| T3AU (pipeline, 3DS shim) | samples past the data | the decoder asserted; the DSP read past the buffer | `T3au::parse` and `trino_wav_load` check the data holds the samples |
| TUIL | two u16 paddings | the sum overflowed | summed as f32 |
| TUIL (found by the CI run) | a slider track narrower than its knob | `f32::clamp` with crossed bounds panics in release builds too | `math::clamp_or` (`clamp` when the bounds are in order); the same at every clamp whose bounds come from data: IK bone lengths, clip durations, slider steps, stat ranges, the dynamic-resolution range, physics boxes and mesh heights |
| saves (found by the CI run) | a damaged slot whose sequence number is half the circle from the intact one's | the next save, numbered past the damage, read as older than the intact save | the bump only applies while the result stays newer |
| ggrs messages | a datagram from anyone | `bincode::deserialize` without a limit | `trino_net::decode_message`, 64 KB limit, one codec for UDP and WebRTC |
| saves | see §3 | a flipped sequence number could order saves wrongly | the slot checksum covers the header |
| N64 EEPROM (libdragon 2c3d5db) | a read or write that starts and ends inside one 8-byte block (the `save recovery` step's 1-byte flip) | `bytes_left` underflows: the read copies past `dest` and loops over the EEPROM (a hang in ares), the write would rewrite the EEPROM from memory past `src` | the `Eeprom` medium sends only whole blocks from a block's start (a read-modify-write in Rust) |

Each input is in `fuzz/regressions/<target>/` and replays in `cargo test`.

### 3. Saves: two slots everywhere, a recovery status, versions

- **The N64 EEPROM** keeps its A/B slots (ADR 0005 §5). New: flag 4 (`FLAG_SUM_HEADER`)
  — the checksum covers the header's sequence number, lengths and flags, so bit rot in a
  sequence number can't make an older save look newer. Every write sets it; a save
  without it (an older build's) still checks its body only. An older build reads a new
  save as damaged (no downgrade path; none is needed before a release).
- **Steam Cloud** syncs `*.sav`, `*.sav.a`, `*.sav.b` and `*.achievements`
  (`package pc --steam`'s README, `docs/SHIPPING.md`).
- **Slot files** (`save::SlotStore`, `read_slot_files`, `write_slot_files`): the PC
  (`<save>.a` / `<save>.b`, each a flushed temp file renamed), the Web (`<key>.a` /
  `<key>.b` in `localStorage`, one `setItem` each) and the 3DS (`sdmc:/3ds/trino/save.a`
  / `save.b`; the shim's `trino_save_load`/`store` take a slot). A slot file is the
  EEPROM's 16-byte slot header (flag 4 always) and the data. A save goes to the slot that
  doesn't hold the newest intact one. The save from before slot files (the PC's raw
  file, the Web's bare key, the 3DS's `"TS"` `save.bin`) is read only when no slot is
  intact, and is never written again.
- **Recovery status:** `Platform::save_status() -> SaveStatus` reports the last read:
  `Empty`, `Ok`, `RecoveredFromBackup` (a slot claiming a newer save failed its check:
  the previous save was read), `Corrupted` (saves were found, none intact: 0 bytes, the
  game starts from defaults). A slot whose header has no magic claims nothing. After a
  `Corrupted` read the next write repairs both slots (files) or numbers past the damage
  (EEPROM), so the read after it is `Ok`.
- **Ties** (two machines' slots with the same sequence number after a cloud sync) go to
  slot A, the same on every machine; sequence numbers wrap (`0` is newer than `0xFFFF`).
- **Versions and migration:** `save::write_versioned(version, data)` puts `"SV" u16`
  in front; `save::load_versioned(platform, buf, current, migrate)` (or `load_game` with
  `Game::migrate_save(from, data, len)`, a defaulted method: no `GAME_API_VERSION`
  bump) hands an older version to the migration, loads the current one as is and refuses
  a newer one (`len` 0, `from` says why). A save without the tag is version 0.
- **Fault injection:** `Platform::save_fault(SaveFault::{FlipNewest, TearNewest,
  FlipAll})` damages the medium on purpose (defaults to `Unsupported`): the self-test's
  `save recovery` step on the PC, in ares (EEPROM), in Azahar (SD) and in the browsers
  (`localStorage`), and `cargo xtask save corrupt`.

### 4. Sanitizers and Miri

- `.github/workflows/robustness.yml`, nightly at 03:17 UTC and on PRs that touch it:
  Miri over trino-core's unit tests (`-Zmiri-strict-provenance -Zmiri-ignore-leaks`:
  tests leak `'static` blobs on purpose), AddressSanitizer over trino-core's and
  the asset pipeline's tests (`-Zbuild-std`; the fuzz targets run under ASan in the fuzz
  job), and six fuzz shards
  of 10 minutes per target with the corpus cached per shard.
- Locally Miri ran 285 core tests in 6 min: no undefined behavior. It found a test that
  assumed a stack `[u8; 32]` is 2-byte aligned (`mem::tests::arena_over_borrowed_memory`)
  and a test comparing `log2` with std's (Miri perturbs std's float functions; skipped
  under Miri).
- Locally ASan ran the 64 test binaries of trino-core and the asset pipeline (std
  instrumented too) with no memory error. `detect_leaks=0` there: the tests' `'static`
  leaks are on purpose.
- `ASAN_OPTIONS=detect_odr_violation=0`: basis_universal's transcoder reaches the fuzz
  binaries through two crates, and ASan reports its duplicated globals.

### 5. Tools and the editor

- `cargo xtask fuzz list|corpus|smoke|run|repro|keep` (`--json` where it reports),
  `cargo xtask save inspect <file> [--medium] [--json]` and `save corrupt <file> --mode
  flip|truncate [--slot newest|a|b|all] [--at N]` (`trino_asset_pipeline::saves`).
- `trino_asset_pipeline::load_dir_report`/`load_boot_report`: every intact asset loads,
  each damaged one is an `AssetError` (logical path, file, message); `load_dir` keeps
  failing on the first. The editor lists each as a Console error with a **Re-bake**
  button (`log::Action::Rebake`: a bake rewrites the damaged file from its master), and
  the PC runner logs them and runs without the asset.
- The editor's **Saves** tab decodes `target/save/` read-only (`TRINO_EDITOR_TAB=saves`).

## Deviations from the issue

- **`SaveError::RecoveredFromBackup`** is `SaveStatus::RecoveredFromBackup` from
  `Platform::save_status()`: an `Err` from `save_read` would have dropped the bytes the
  game must load.
- **EEPROM slots "when the save is ≤ half of 2 KB"** were already the N64's layout (a save
  over half the medium is written wide, the one case without a fallback); this issue
  added the recovery status and the header checksum.
- **Fault injection on consoles** is the defaulted `Platform::save_fault`, not a power
  cut: ares and Azahar can't cut power mid-write. The `save recovery` step also cuts a
  `save_begin` short after one step (a real power cut's state on the N64), and the host
  tests cut writes at every byte (`save_slots.rs`, `save_recovery.rs`).
- **The C side of the 3DS T3AU loader** is checked by hand against `T3au::parse`, not
  fuzzed (it is C).

## Addendum (2026-10-29): saves per game, versioned games, power cuts (GitHub #119, #121)

- **3DS saves per game (#119).** `sdmc:/3ds/trino/<save_name>/save.{a,b}`, the name from
  `trino.toml` `[3ds] save_name` (default `[project] name`; the engine's castle64 is
  `castle64`, its PC and Web name), staged in the RomFS; self-test builds use
  `<name>-selftest`. The decisions are plain Rust over a `Files` trait
  (`trino_platform_3ds::savedir`, tested on the host); the shim only opens files 0–4
  (own A/B, shared `save.bin`/A/B) and writes only the own two. A folder with no slot
  file at all migrates the shared save once (read, never written); one with a damaged
  slot does not, so a damaged save of one game never loads another's. The shared folder
  stays as it was: an older build of any game still reads it. A `.cia` could use the
  title's own save archive (`ARCHIVE_SAVEDATA`) instead: a separate item.
- **Versioned games (#121).** castle64 (new: its progress and achievements, version 1),
  Solid Eleven (version 0 = its `SE11` career image, migrated by `Career::load`) and the
  starter (version 0 = its bare achievements pack) save through `write_versioned` and
  `load_game`, never overwrite a newer version's save, and say what the read found
  (Solid Eleven's amber/red toast, the starter's `save_notice` screen). castle64's save
  is opt-in per host so no test reads the player's progress.
- **Power cuts.** `cargo xtask save soak` runs a build that only saves a counter
  (`castle64::soak`); in Azahar the harness kills the emulator (SIGKILL) mid-run and
  checks every boot read the last finished save or a later one. On hardware the same
  build runs through `--device`; a person cuts the power (`docs/HARDWARE.md`). Not yet
  done on a real SC64 or 3DS. ares can't stand in for the N64 (it writes the EEPROM file
  at exit only).
- **Tools.** `save inspect` reads a 3DS SD folder (the same migration rule), and the
  editor's Saves tab has the PC, ares's `.eeprom` and Azahar's SD folders.
