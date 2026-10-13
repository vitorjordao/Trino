---
name: build-n64
description: Build the Nintendo 64 ROM (target/n64/trino.z64) through the Docker toolchain, and troubleshoot the N64 pipeline (Docker/WSL2, ABI, assets). Use for any "build for N64 / ROM / libdragon" request.
---

# build-n64

```
cargo xtask build n64      # image bootstrap (first time ~10 min) + shim + assets + ROM
```

Produces `target/n64/trino.z64`. Everything native runs inside the pinned
`trino-n64` Docker image (`docker/n64/Dockerfile`); only rustc runs on the host.

## How the build works (xtask/src/n64.rs)

1. `ensure_image` — builds the `trino-n64` image if missing (libdragon pinned by commit).
2. `compile_shim` — compiles `crates/platform-n64/shim/trino_shim.c` with
   `mips64-elf-gcc -mabi=o64` in-container (mtime-cached to `target/n64/shim.o`).
3. `bake_assets` — mksprite/audioconv64 into `target/n64/filesystem/` + `index.tsv`.
4. `cargo +nightly build` — Rust as a **staticlib** for the custom target
   `platforms/n64/mips-nintendo64-none.json` (o32, `+noabicalls`, build-std).
5. `pack_rom` — in-container link with `mips64-elf-g++ -mabi=o64
   -Wl,--no-warn-mismatch` + libdragon's `n64.ld`, then n64sym / strip /
   n64elfcompress / mkdfs / n64tool (mirrors n64.mk's recipes).

## The ABI rule (do not break it)

libdragon is `-mabi=o64`; LLVM has no o64, so Rust codegens **o32** and GNU ld
links the mix. That is only safe because every C<->Rust call goes through the
shim, which is restricted to **≤4 scalar/pointer args, no by-value structs, no
variadics, and no scalar `float` after an integer argument** (the float lands in
the wrong register between o32 and o64; pass 16.16 fixed point as `u32` or a
pointer to floats). When adding FFI: extend `trino_shim.c` + `crates/platform-n64/src/ffi.rs`
in matching pairs and keep the restriction. Never call libdragon directly from Rust.

## Console output conventions (keep PC in sync)

- **16-bit output**: `trino_mode_standard()` sets magic-square RGB dither, and the VI
  runs with `FILTERS_RESAMPLE_ANTIALIAS_DEDITHER`. The PC N64 look emulates exactly
  this, so a change here needs the matching change in
  `crates/platform-pc/src/renderer/shaders.wgsl`, and then
  `cargo xtask test n64 --bless` plus the `n64_parity` test.
- **Saves**: EEPROM 16 Kbit (`ed64romconfig --savetype eeprom16k`), framed by
  `trino_core::save`.
- **Test hooks**: `trino_debug_pixel` and `trino_debug_fb_dump` (the golden frame).

## Troubleshooting

- `docker not reachable` (Windows): docker runs inside WSL2 without Docker
  Desktop; the daemon may need a manual start:
  `wsl -u root -e sh -c "pgrep dockerd >/dev/null || (nohup dockerd >/var/log/dockerd.log 2>&1 &)"`
- Shim compile errors: libdragon API drift — fix the shim against the pinned
  headers (that is the shim's job: turn drift into compile errors, not UB).
- Rust target JSON rejected: nightly moved; the compiler error names the
  offending field (e.g. data-layout strings are matched exactly).
- Link errors about ABI/e_flags: keep `-Wl,--no-warn-mismatch`; if GNU ld ever
  hard-errors, see `docs/adr/0002-n64-abi-o32-staticlib.md` for the fallback.
- Nightly missing: `rustup toolchain install nightly --component rust-src`.
