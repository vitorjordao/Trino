---
name: robustness
description: Keep Trino's parsers and saves safe on bad data — the fuzz targets (crates/fuzz, fuzz/, `cargo xtask fuzz list|corpus|smoke|run|repro|keep`), adding a target for a new format, triaging a crash or a hang, the A/B save slots and their recovery status (`Platform::save_status`, `save_fault`), versioned saves and `Game::migrate_save`, `cargo xtask save inspect|corrupt`, and the nightly Miri / AddressSanitizer / fuzz job. Use for "fuzz", "a parser panics on a bad file", "corrupted save", "power cut", "migrate a save", "Miri", "ASan", "sanitizer", or when adding any binary format or player file.
---

# robustness (issue #68, ADR 0037)

Every parser the engine runs on data from outside the build has a fuzz target, and
every save survives a power cut, bit rot and a cloud sync. The facts below are verified
by `cargo test -p trino-fuzz`, `crates/core/tests/save_recovery.rs` and the `save
recovery` self-test step (PC, ares, Azahar, the browsers).

## The fuzz targets

- **One body per parser** in `crates/fuzz/src/{core_targets,host_targets}.rs`, listed in
  `trino_fuzz::TARGETS` (name, formats, body, `max_len`, where seeds come from). The
  `host` feature (default) adds the PC/Web/RON/network parsers; without it the crate is
  trino-core only.
- **A body does what a caller does after `Ok`**: pose and draw the mesh through
  `render3d::tessellate`, lay a screen out and draw it on a null renderer, look strings
  up, run a graph, decode every bundle entry. Most of what fuzzing found was "parsed
  fine, then an accessor read out of range".
- **Round trips where a writer exists** (`assert_eq!` of write → parse → write), and
  allocations capped by `core_targets::MAX_DECODE`.
- **Three callers:** `fuzz/fuzz_targets/<t>.rs` (one line: `trino_fuzz::run("<t>",
  data)`), the stable test `crates/fuzz/tests/targets.rs`, and the `trino-fuzz` binary.

### Commands

```
cargo xtask fuzz list [--json]              # targets, formats, seeds, regressions, corpus
cargo xtask fuzz corpus [--commit-seeds]    # bake engine + Solid Eleven (PC, Web), sort by magic into fuzz/corpus/
cargo xtask fuzz smoke [<t>..] [--iters N] [--json]   # stable mutation run, 10 s watchdog per input
cargo xtask fuzz run <t>|all [--time 60] [--jobs N]   # libFuzzer + ASan + overflow checks (nightly, cargo install cargo-fuzz)
cargo xtask fuzz repro [<t>] <file> [--json]          # replay one input (its folder names the target)
cargo xtask fuzz keep <file>                # copy a crash into fuzz/regressions/<t>/ (replayed by cargo test)
cargo xtask fuzz seeds [<t>..]              # write the writers' seeds (seedgen.rs) into fuzz/seeds/, no bake
cargo xtask fuzz coverage [<t>..|all] [--json]        # cargo fuzz coverage + llvm-cov: fuzz/coverage/summary.{json,md}, <t>/report.txt
TRINO_FUZZ_ITERS=20000 cargo test -p trino-fuzz --test targets   # the PR test, harder
```

`fuzz coverage` needs `rustup component add llvm-tools-preview --toolchain nightly`.
Dependencies, std and the harness are filtered out. The headline is **the code regions
of the functions the corpus entered** ("of the code this parser reaches, how much
ran"); the reached files' whole-file lines/functions/regions are beside it but count
every unrelated function a reached file holds (`mem.rs`, the allocator counter, shows
up in every target). C code (`t3au_check.c`) isn't instrumented for coverage. First
local numbers (corpus of one 60 s run each): `asset_index` 97.7%, `t3au_shim` 99.0%,
`tsn2_decode` 84.9% of reached-function regions; read `fuzz/coverage/<t>/report.txt`
for the files.

`fuzz run` passes `-a` (debug assertions): an overflowing size product panics instead
of wrapping. Five of the bugs found were products that wrap a 32-bit `usize` (the N64,
the 3DS, wasm32) and only the overflow checks saw them on a 64-bit host. The stable
test runs in the dev profile, which keeps overflow checks on.

### Targets that aren't one Rust parser (issue #121)

- **C:** `t3au_shim` compiles the 3DS shim's own header check
  (`crates/platform-3ds/shim/t3au_header.h`, `<stdint.h>` only, included by the shim) on
  the host (`crates/fuzz/build.rs`, `c/t3au_check.c`) and is **differential** against
  `T3au::parse`: both accept or both refuse, and what both accept reads the same. Under
  `cargo fuzz` the C gets `-fsanitize=address` (gcc links against rustc's ASan runtime
  fine) and, with clang, `-fsanitize=fuzzer-no-link` (gcc has no libFuzzer coverage:
  there the Rust side's branches steer). A C rule that changes changes in both, or the
  differential fails. `TRINO_T3AU_BUSES` is pinned to `Bus::COUNT` by a test.
- **Console-only Rust:** code behind `cfg(target_os = "none"|"horizon")` never runs on
  the host. Move the pure part into `trino-core` (the asset index lines are
  `trino_core::asset::IndexLine`, used by both loaders) and fuzz that (`asset_index`).
- **GPU:** `pc_renderer` keeps one headless `PcRenderer` per thread, **leaked** (a
  thread-local drop at exit polls wgpu after its TLS is gone and panics), skips without
  an adapter (asserts under `TRINO_REQUIRE_GPU`), uses fixed ids and re-uploads default
  sprites per input (an input's frame must not depend on the one before), and gets 48
  mutations in the stable runs (`trino_fuzz::iters_for`, `SLOW_TARGETS`). `xtask fuzz
  run` turns LeakSanitizer off for it (`-detect_leaks=0` and `ASAN_OPTIONS
  detect_leaks=0`: the Vulkan driver's allocations; the exit-time leak report otherwise
  "crashes" on the empty input). ~200–240 exec/s on RADV and on lavapipe
  (`VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json`, what CI has); the first
  input, device and pipelines included, takes ~0.5 s, far under libFuzzer's 10 s.
- **Vorbis check vs real files:** every baked TSN2 of the engine and Solid Eleven (21
  Vorbis) and the pipeline's encoder at q −1..10 × 8–48 kHz pass `vorbis::check`.

### Adding a format (the `engine-feature` checklist item)

1. A body in `core_targets.rs` (or `host_targets.rs`): parse, then use every accessor.
2. A `Target` in `TARGETS` whose `formats` names the magic; `Seeds::Magic(&[b"XXXX"])`
   when a bake writes it, else `Seeds::Committed` and a writer seed in `seedgen.rs`
   (`cargo xtask fuzz seeds <t>` writes them without the full corpus bake).
3. `fuzz/fuzz_targets/<t>.rs` and a `[[bin]]` in `fuzz/Cargo.toml`
   (`cargo_fuzz_has_a_binary_per_target` fails otherwise).
4. `cargo xtask fuzz corpus --commit-seeds`, then `cargo test -p trino-fuzz`.

`every_binary_format_has_a_target` reads every `pub const …MAGIC: [u8; 4] = b"…"` in
`crates/core/src` and `crates/platform-web/src`: a new magic without a target fails it.

### Triage

- **A panic:** `cargo xtask fuzz repro <file>` prints it; `RUST_BACKTRACE=1
  target/debug/trino-fuzz repro <file>` gives the line. Fix the **parser's validation**
  (it runs once at load), never the hot path, then `fuzz keep <file>`.
- **A hang** (`timeout-*`, or the smoke's watchdog): `gdb -batch -ex run -ex bt --args
  target/debug/trino-fuzz repro <file>` in the background, then `kill -INT` the
  process (ptrace attach is blocked here: `ptrace_scope` 1). A frame number alone can
  mislead: count work with a counter (the TMDL hang showed `splits: 50056970` in
  `info locals` of an unoptimized core build: `CARGO_PROFILE_DEV_OPT_LEVEL=0 cargo build
  -p trino-core --example …`).
- **An OOM** (`malloc_limit`): a size from a header reached `vec![0; n]` or
  `with_capacity(n)`. Bound it by the bytes that could produce it (TBND: stored for raw,
  ≤ 255× + 16 for LZ4; TPAK: 9 bytes per entry).
- **`ASAN_OPTIONS=detect_odr_violation=0`**: basis_universal is linked through two
  crates; `xtask fuzz run` and the CI job set it.

### What the fuzzing found (each in `fuzz/regressions/`)

- TMDL indices past the vertex count; TMDL lengths that wrap on 32 bits.
- **The T&L's depth bisection never ended** on a vertex at ~1e15: the explicit DFS stack
  bounds the pending work, not the depth — where `(a + b) / 2 == a` the left child is
  its parent. `SPLIT_BUDGET` (16 384 bisections per draw) compares the existing
  `stats.splits`; a per-triangle counter cost 196 bytes of `tessellate_joints` and
  pushed it past the N64's 16 KB I-cache (the `build n64` guard caught it), this costs 36.
- `c += 1.0` cycle loops in `EventTrack::fire` and `RootTrack::delta` (stuck past 2^24).
- The tilemap's stride from the first row only; TPAK `with_capacity(count)`; sprite size
  products (u32, then even u64 at `u32::MAX²·4`); TBND `raw` allocations; T3AU samples
  past the data (the pipeline asserted, the 3DS shim's DSP read past it); TUIL padding
  sums; ggrs messages decoded without a size limit (`trino_net::decode_message`).
- From the PR's first CI fuzz run (45 s per target): a slider track narrower than its knob
  (`f32::clamp` with crossed bounds panics in release too: `math::clamp_or` at every clamp
  bounded by data); an EEPROM sequence bump half the circle past the intact save (the new
  save read as older). CI's fresh corpus finds what a warm local one doesn't: download
  `fuzz-artifacts-<shard>` (`gh run download <run> -n fuzz-artifacts-2`; the run id is the
  latest push's), repro, fix, `fuzz keep`.
- Issue #121:
  - **The 3DS shim's T3AU check disagreed with the pipeline** (`t3au_shim`): a streamed
    sound had no data-size check (a 2-byte file queued 14 336-sample chunks to the DSP),
    any bus byte and any file length passed, `nsamples + chunk - 1` summed in u32, a
    PCM chunk's byte count could wrap; the pipeline took any channel count. One rule set
    now (exact length, bus, mono, contexts ≤ 2^20, chunk ≤ 2^24).
  - **`PcRenderer::upload_sprite`** with a zero side or one past
    `max_texture_dimension_2d` reached wgpu's validation (abort); lengths were checked
    with a u32 product. Refused and logged now; `upload_sprite_compressed` returns false.
  - **`ktx2::ktx2_to_basis`** sliced the level and ETC1S codebooks unchecked.
  - **lewton's OOM** (`tsn2_decode`, 120 s run): it reserves a codebook's lookup table
    from the header (2^40 values from a 23 KB file) before reading it.
    `trino_core::vorbis::check` bounds the setup header before every lewton call (PC
    decode and stream, the Web, the pipeline). Third-party allocations from header
    counts need a check in front; the fuzzer's `-malloc_limit_mb` is what finds them.
  - Allocation OOMs show a useless stack without a symbolizer: `addr2line -e
    fuzz/target/x86_64-unknown-linux-gnu/release/<t> -f -C -i <addrs>` names the frames
    (rustup's llvm-tools ship no `llvm-symbolizer`).

### Measured (issue #121)

libFuzzer `-O -a` for 60 s on a frozen corpus (120 s grown from the seeds), twice,
on a 14 GB laptop shared with another build (noisy: compare ratios):

| target | before (exec/s) | after (exec/s) | why |
|---|---|---|---|
| `ron` | 26, 26 | 58, 76 | scenes and prefabs moved to `ron_scene` (each full RON parse of a ~20 KB scene ≈ 1 ms natively; `ron` paid six) |
| `ron_scene` | — | 65, 50 | Scene parse + round trip + prefab |
| `tsn2_decode` | 85, 81 | 162, 136 | lewton's setup header (~0.9 ms natively per input) is the floor; the body decodes ≤ 16 384 samples (`decode_blob_limit`) |
| `asset_index` | — | 10 202 | |
| `t3au_shim` | — | 9 601 | |
| `pc_renderer` | — | 180–240 | one GPU frame per input |

What makes a target slow: profile its body natively over the corpus (time each parser
with a throwaway example in `crates/fuzz/examples/`), then split what dominates into its
own target or bound it. Under ASan + `-a` expect ~10x the native time.

## Saves

- **Layouts.** N64: the EEPROM's A/B slots (`save::plan_write`, a block a step). PC:
  `<save>.a` / `<save>.b` (`trino_platform_pc::system::SaveFiles`). Web: `<key>.a` /
  `<key>.b` (`trino_platform_web::saves`). 3DS: `sdmc:/3ds/trino/<save_name>/save.a` /
  `save.b` (GitHub #119, `trino_platform_3ds::savedir`; the shim opens files 0–4: own
  A/B, then the shared folder's `save.bin`, `save.a`, `save.b`). All files use
  `save::SlotStore` + `read_slot_files` / `write_slot_files`; the save from before slots
  (PC raw, Web bare key) is read only when no slot is intact.
- **3DS save folders (#119).** `trino.toml` `[3ds] save_name` (default `[project] name`
  made FAT-safe; the engine: `castle64`, like its PC and Web saves) is staged as the
  RomFS file `save_name`; self-test builds use `<name>-selftest`, `save soak` builds
  `<name>-soak`. A game's folder with **no slot file at all** reads the shared
  `sdmc:/3ds/trino/save.{a,b}` (then `save.bin`) once, as an intact save (status `Ok`:
  a move, not damage), and never writes it. A folder with a damaged slot does not
  migrate: `read_slot_files` calls `read_legacy` whenever no slot is *intact*, so gate a
  migration on the files being absent, or one game's damaged save loads another's. Boot
  logs `TRINO_SAVE_PATH <dir> own=0|1`; `test 3ds` seeds the shared folder, empties the
  self-test's, checks the `save folder` step read the seed (`seed=1`) and that the shared
  files are byte-identical after the run, then restores them (`xtask/src/n3ds_saves.rs`).
- **The checksum covers the header** (flag 4): a flipped sequence number can't reorder
  saves. Old EEPROM saves without the flag still read.
- **`Platform::save_status()`** after a read: `Empty`, `Ok`, `RecoveredFromBackup`,
  `Corrupted`. Tell the player on `RecoveredFromBackup`, start from defaults on
  `Corrupted` (the next write repairs the medium). A slot with no magic claims nothing.
- **Versions:** write `save::write_versioned(V, data, out)`; read with
  `save::load_game(self, platform, buf, V)` and implement `Game::migrate_save(from,
  data, len)`. A newer version loads nothing (`from > V`): don't overwrite it silently.
  The adopters (#121): castle64 (`progress.rs`, version 1: stars, coins per level,
  stomps, the achievements' pack), Solid Eleven (`SAVE_FORMAT` 1; version 0 is the
  `SE11` v1–v3 career image, which `Career::load` reads and `migrate_save` rewrites) and
  the starter (version 0 is its bare achievements pack, kept as is). Each keeps a
  `blocked` flag on `from > V` and never saves after it.
- **Saves off in tests.** castle64's save is off unless the host asks
  (`with_saves` / `set_saves`): the PC outside smoke, bot, replay and recording runs, the
  N64 and 3DS outside the self-test, the Web outside `?frames`, `?replay`, `?record`.
  `reset()` (a replay's start) turns it off. A game that loads a save at boot would
  change goldens, the bot run, the perf scenes and replays. A console host flips it in
  place (`set_saves`): `game = game.with_saves()` moves the struct on the N64's stack.
- **Tell the player.** After `load_game`: `RecoveredFromBackup` → "restored from its
  backup", `Corrupted` → "lost, starting over", `from > V` → "a newer version's save,
  not saving". Solid Eleven: an amber `warn` line in `ui/toast.ui.ron` (re-export with
  `ui export`, strings in both tables, `i18n export --rust`). The starter:
  `ui/save_notice.ui.ron` once at start, before the crash question when both wait.
- **Fault injection:** `platform.save_fault(SaveFault::{FlipNewest, TearNewest,
  FlipAll})` (tests only); `cargo xtask save corrupt <file> --mode flip|truncate
  [--slot newest|a|b|all] [--at N]` by hand; `cargo xtask save inspect <file> --json`
  decodes slots, sequence numbers and the status (PC slot pair, one slot file, an N64
  EEPROM image with `--medium` or at 512 B / 2 KB). The editor's Saves tab shows the same
  for `target/save/`.
- **N64 EEPROM access is whole blocks** (`Eeprom` in `platform-n64/src/system.rs`):
  libdragon's `eeprom_read_bytes`/`eeprom_write_bytes` (pinned 2c3d5db) underflow
  `bytes_left` on a span that starts and ends inside one block — `read(21, 1)` hung in
  ares, a write would rewrite the EEPROM from memory past `src`. Anything that isn't
  whole blocks from a block's start is a read-modify-write in Rust.
- The `save recovery` step logs `TRINO_STEP:save recovery k/7` per stage: a hang names
  its stage.
- **Power cuts on a real medium:** `cargo xtask save soak 3ds` (Azahar killed with
  SIGKILL after 3–40 saves, `--cycles` times; each boot must read the last finished save
  or a later one, never `corrupted`/`empty`), `--device sc64|everdrive|network` for a
  human to cut the power (`docs/HARDWARE.md` › Save power cuts). ares can't stand in for
  the N64: it writes `<rom>.eeprom` only when it exits. The log lags the writes by a
  poll, so the check is "≥ the last `wrote` seen", not "==".
- **Tools for console saves:** `save inspect <dir>` reads a 3DS SD folder (`saves::
  SdFolder`, the same migration rule as `savedir`), an ares `.eeprom` is a medium
  image (2 KB for 16 Kbit; decoded as written, no byte swap). The editor's Saves tab has
  PC / N64 (ares) / 3DS (Azahar) sources (`TRINO_EDITOR_SAVES`, `TRINO_AZAHAR_SDMC`).

## Miri and AddressSanitizer

```
MIRIFLAGS="-Zmiri-strict-provenance -Zmiri-ignore-leaks" \
  CARGO_TARGET_DIR=target/miri cargo +nightly miri test -p trino-core --lib     # ~6 min, 285 tests
MIRIFLAGS="-Zmiri-strict-provenance -Zmiri-ignore-leaks" \
  CARGO_TARGET_DIR=target/miri cargo +nightly miri test -p trino-core --test save_recovery --test save_slots  # ~3 min
ASAN_OPTIONS=detect_odr_violation=0:detect_leaks=0 RUSTFLAGS=-Zsanitizer=address \
  CARGO_TARGET_DIR=target/asan cargo +nightly test -Zbuild-std --target x86_64-unknown-linux-gnu \
  -p trino-core -p trino-asset-pipeline --no-fail-fast                          # ~20 min cold, 64 test binaries
# weekly (robustness-weekly.yml): the platform crates, GPU tests on the machine's adapter
ASAN_OPTIONS=detect_odr_violation=0:detect_leaks=0 RUSTFLAGS=-Zsanitizer=address \
  CARGO_TARGET_DIR=target/asan cargo +nightly test -Zbuild-std --target x86_64-unknown-linux-gnu \
  -p trino-platform-pc -p trino-render-wgpu -p trino-runner-pc -p trino-platform-3ds -p trino-platform-n64 --no-fail-fast
```

- Issue #121, locally on RADV (4 jobs, ~13 min cold for the three runs): platform-pc's
  lib + 21 test binaries, runner-pc's lib + 5, render-wgpu's 58 lib tests, the 3DS's 12
  and the N64's 3 host tests: no memory error. Vulkan drivers run uninstrumented under
  ASan without complaint; leaks stay off (`detect_leaks=0`: the drivers keep theirs).
- Miri's `save_slots` took 1112 s with its every-byte power-cut loop; the loops stride
  under `cfg(miri)` (every 13th body byte and the whole header; every 7th bit/byte in
  `save_recovery`), 170 s + 6 s. Clippy rejects `c % STRIDE` with a constant 1
  (`modulo_one`): `c.is_multiple_of(STRIDE)`.

- `rustup component add --toolchain nightly miri rust-src` first.
- `-Zmiri-ignore-leaks` and ASan's `detect_leaks=0`: tests leak `'static` blobs on
  purpose (`BakedScene` borrows `'static`); LeakSanitizer reported 637 bytes in
  `asset_kinds` that way.
- Miri perturbs std's float functions: a test comparing with `f32::log2` is
  `#[cfg_attr(miri, ignore = …)]`. Miri also places a stack `[u8; N]` at odd addresses,
  so a test that needs alignment declares it (`mem::tests::arena_over_borrowed_memory`).
- CI: `.github/workflows/robustness.yml` (the `ci` skill).
