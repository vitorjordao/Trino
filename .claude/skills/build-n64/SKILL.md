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
   `mips64-elf-gcc -mabi=o64` in-container (mtime-cached to `target/n64/shim.o`),
   and the RSP overlay `shim/rsp_trino.S` the way `n64.mk` builds RSP ucode:
   `-march=mips1 -mabi=32 -Wl,-Trsp.ld`, then `.text`/`.data` cut to binaries and
   wrapped as `rsp_trino_{text,data}_{start,end}` for `DEFINE_RSP_UCODE`
   (`target/n64/rsp_trino.o`; `mips64-elf-size -G` prints its IMEM/DMEM use, which
   must stay under 4 KB each with the rspq kernel).
3. `bake_assets` — mksprite/audioconv64 into `target/n64/filesystem/` + `index.tsv`.
   Sprite formats come from `texture::choose` (declared, or `auto`); an I4/I8 mask is
   re-encoded with its coverage as the intensity (`texture::n64::stage`) before
   mksprite, which otherwise keeps a gray master's red channel and ignores alpha.
   Any other sprite is copied as its master file (an indexed PNG keeps its palette).
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

## The Expansion Pak (issue #54, ADR 0020)

- `trino_memory_size()` (libdragon's `get_memory_size()`) is read once in
  `runtime::init`; 8 MB or more is the expanded tier (`src/hw.rs`, host-tested), logged as
  `TRINO_HW n64 ram=8M tier=expanded`. `renderer.caps()` becomes `Caps::N64_EXPANDED`.
- `src/assets.rs` skips index lines whose 4th column names the other tier (twins are
  `<id>.x.blob` in the DFS). The ROM must boot and pass in 4 MB: `cargo xtask test n64
  --expansion-pak off` (CI runs `both`).

## Asset groups and compression (issue #56, ADR 0022)

- The bake routes each index line through `xtask/src/console_groups.rs`: boot lines stay
  in `index.tsv`, a group's lines become `groups/<name>.tbnd` (a table of external
  entries: the DFS file and, for sounds, its columns), named by a `<id> \t group \t
  groups/<name>.tbnd` line.
- Compression runs in the container after `mksprite`/`audioconv64`: `mkasset -c <level>
  -w 256 -o <dir> <files>` in place (`n64_compress_script`), `.sprite` files skipped
  (mksprite wrote them at `--compress <level>`), `.wav64` left alone. Levels 2/3 put
  `<level> \t compression \t libdragon` lines first in the index; `src/assets.rs` calls
  `trino_asset_init_compression` for them before any asset. `[groups.boot] compression =
  { n64 = 1 }` compresses boot too (castle64 and Solid Eleven ship it).
- Runtime: `N64Assets::pump` reads entries for `Caps::load_budget_us` (4 ms) through
  `asset_load` / `sprite_load`, each locator in a `FrameArena` (no heap churn). Boot logs
  `TRINO_MEM: ram= heap_total= before_assets= boot_assets=`; `check_heap` fails the
  self-test when the heap's peak leaves < 128 KB (`TRINO_HEAP:` lines).
- **The base N64's heap (4 MB, no Pak) is what's left after the image.** Every KB of
  new code comes straight out of the self-test's top free block, and that block is what
  a big `Vec` finds once the run has fragmented the rest. `TRINO_HEAP … top=` (newlib's
  top chunk + the unclaimed `sbrk` room, `runtime::heap_top_free`, allocates nothing)
  shows it; free bytes don't. #67 added ~45 KB of code and the micro scenes' 900-triangle
  batch no longer fit. Levers, in the order they paid off:
  - the draw memo is the biggest boot allocation: 1 024 slots × 304 B on the Pak,
    768 on the base tier (`memo_slots`, −78 KB, no measurable frame cost);
  - size a big batch once and exactly before the frame that needs it
    (`N64Renderer::reserve_batch`), never through `Vec` doubling, which holds the old
    block while asking for one twice its size;
  - an asset the N64 never draws gets a tiny `assets/n64/` override (the 3DS format
    photos, −12 KB). Draw it in colors the format stores exactly, or a `min_psnr` on
    the table fails the N64 bake (the editor's texture test caught a gray card);
  - out-of-line helpers for code called per field or per player (`StateHasher`, the
    TREC per-player codecs): ~5 KB.
  - **test-only meshes go in last and come out whole** (#122): the palette checks' five
    slotted meshes (~55 KB with their RSP and flat copies), registered at boot, took the
    block the 900-triangle batch reserve needed; registered right after the reserve,
    the batch's other buffers then grew past them and the water step's 43 KB found
    only 38 KB (`top=` 81 → 38 KB). Registered after every scene that grows the batch
    and forgotten after (`N64Renderer::unregister_mesh`, then a few empty frames: the
    renderer frees a copy four frames after its last use), `top=` is the same before
    and after (`TRINO_HEAP … the palette meshes`).
  - the RSP mesh copies can have their own region of uncached RDRAM, reserved before
    any mesh registers: `N64Renderer::reserve_rsp_arena(bytes)` before
    `N64Renderer::new`, or `trino.toml` `[n64] rsp_arena_kb = N` (xtask writes
    `rom:/rsp_arena`, `N64Host::boot` reserves it and logs `TRINO_RSP_ARENA:`). A
    `trino_core::mem::Blocks` free list (best fit, neighbors merge) carves it; copies
    past it take the heap. Their holes stay theirs, so a group's load and unload no
    longer leave free bytes and no 45 KB block (GitHub #110). **It is opt-in and costs
    heap**: measured on the base tier, 416 KB (room for every copy) made a 79 KB batch
    fail mid-run, 376 KB (the resident peak) passed with `top=` 11 KB lower than no
    arena, and with no arena the strider group's load and unload left `top=` intact
    (the segment staging and the trim below did that). The engine ROM reserves 64 KB, so
    every run exercises it. Reserve one for a game that loads and unloads groups of
    models many times, sized from `TRINO_HEAP … rsp=live/peak` and checked with a
    base-tier self-test; `arena=used/size` shows the fill;
  - a copy is staged one segment at a time: the old whole-copy staging `Vec` asked the
    heap for a second block as big as the copy;
  - the batch buffers shrink by themselves: `N64Renderer::trim` keeps each buffer's
    peak over 120 frames and `trim_to` gives back what holds more than twice that and
    16 KB beyond (`mem::TrimWindow`, `shrink_to`; newlib's realloc shrinks in place).
    The trim never goes below what `reserve_batch` asked for (a shrink there would bring
    back the doubling it avoids); `TRINO_HEAP:batch= trims=` after the micro scenes.
    `trim_buffers` (empty them now) and `reserve_batch` (size once, exactly) stay for the
    self-test's scene changes;
  A malloc/free bisection to find the "largest block" changes the arena it measures
  (newlib trims and re-extends the top): use `top=`.
- Sizes: `cargo xtask build n64` prints each group's entries, raw and stored KB; `cargo
  xtask assets report` the RAM per tier.

## Console output conventions (keep PC in sync)

- **16-bit output**: `trino_mode_standard()` sets magic-square RGB dither, and the VI
  runs with `FILTERS_RESAMPLE_ANTIALIAS_DEDITHER`. The PC N64 look emulates exactly
  this, so a change here needs the matching change in
  `crates/render-wgpu/src/renderer/shaders.wgsl`, and then
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
