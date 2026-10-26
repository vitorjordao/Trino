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
TRINO_FUZZ_ITERS=20000 cargo test -p trino-fuzz --test targets   # the PR test, harder
```

`fuzz run` passes `-a` (debug assertions): an overflowing size product panics instead
of wrapping. Five of the bugs found were products that wrap a 32-bit `usize` (the N64,
the 3DS, wasm32) and only the overflow checks saw them on a 64-bit host. The stable
test runs in the dev profile, which keeps overflow checks on.

### Adding a format (the `engine-feature` checklist item)

1. A body in `core_targets.rs` (or `host_targets.rs`): parse, then use every accessor.
2. A `Target` in `TARGETS` whose `formats` names the magic; `Seeds::Magic(&[b"XXXX"])`
   when a bake writes it, else `Seeds::Committed` and a writer seed in `seedgen.rs`.
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

## Saves

- **Layouts.** N64: the EEPROM's A/B slots (`save::plan_write`, a block a step). PC:
  `<save>.a` / `<save>.b` (`trino_platform_pc::system::SaveFiles`). Web: `<key>.a` /
  `<key>.b` (`trino_platform_web::saves`). 3DS: `sdmc:/3ds/trino/save.a` / `save.b`
  (the shim's slot argument). All files use `save::SlotStore` + `read_slot_files` /
  `write_slot_files`; the save from before slots (PC raw, Web bare key, 3DS `save.bin`)
  is read only when no slot is intact.
- **The checksum covers the header** (flag 4): a flipped sequence number can't reorder
  saves. Old EEPROM saves without the flag still read.
- **`Platform::save_status()`** after a read: `Empty`, `Ok`, `RecoveredFromBackup`,
  `Corrupted`. Tell the player on `RecoveredFromBackup`, start from defaults on
  `Corrupted` (the next write repairs the medium). A slot with no magic claims nothing.
- **Versions:** write `save::write_versioned(V, data, out)`; read with
  `save::load_game(self, platform, buf, V)` and implement `Game::migrate_save(from,
  data, len)`. A newer version loads nothing (`from > V`): don't overwrite it silently.
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

## Miri and AddressSanitizer

```
MIRIFLAGS="-Zmiri-strict-provenance -Zmiri-ignore-leaks" \
  CARGO_TARGET_DIR=target/miri cargo +nightly miri test -p trino-core --lib     # ~6 min, 285 tests
ASAN_OPTIONS=detect_odr_violation=0:detect_leaks=0 RUSTFLAGS=-Zsanitizer=address \
  CARGO_TARGET_DIR=target/asan cargo +nightly test -Zbuild-std --target x86_64-unknown-linux-gnu \
  -p trino-core -p trino-asset-pipeline --no-fail-fast                          # ~20 min cold, 64 test binaries
```

- `rustup component add --toolchain nightly miri rust-src` first.
- `-Zmiri-ignore-leaks` and ASan's `detect_leaks=0`: tests leak `'static` blobs on
  purpose (`BakedScene` borrows `'static`); LeakSanitizer reported 637 bytes in
  `asset_kinds` that way.
- Miri perturbs std's float functions: a test comparing with `f32::log2` is
  `#[cfg_attr(miri, ignore = …)]`. Miri also places a stack `[u8; N]` at odd addresses,
  so a test that needs alignment declares it (`mem::tests::arena_over_borrowed_memory`).
- CI: `.github/workflows/robustness.yml` (the `ci` skill).
