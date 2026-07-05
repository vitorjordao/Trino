---
name: build-3ds
description: Build the Nintendo 3DS app (target/3ds/trino.3dsx) with the local devkitPro toolchain, and troubleshoot the 3DS pipeline (devkitARM, citro2d shim, tex3ds assets). Use for any "build for 3DS / 3dsx / libctru" request.
---

# build-3ds

```
cargo xtask build 3ds      # shim + assets + Rust + .3dsx (all local, no Docker)
```

Produces `target/3ds/trino.3dsx`. Everything runs on the local devkitPro
install (auto-detected at `$DEVKITPRO`, `C:\devkitPro` or `/opt/devkitpro`).

## How the build works (xtask/src/n3ds.rs)

1. `compile_shim` — compiles `crates/platform-3ds/shim/trino_shim_3ds.c`
   with devkitARM's `arm-none-eabi-gcc` against the installed libctru
   headers (mtime-cached to `target/3ds/shim.o`).
2. `bake_assets` — `tex3ds` (sprites → `.t3x`) + wav→raw-PCM16 conversion
   (sounds → `.pcm16`, 12-byte header) into `target/3ds/romfs/` + `index.tsv`.
3. `cargo +nightly build` — the **built-in Tier 3 target**
   `armv6k-nintendo-3ds` (no custom JSON) with `-Zbuild-std=core,alloc`;
   RUSTFLAGS add the shim object and `-lcitro2d -lcitro3d -lctru -lm`, and
   the target's own linker recipe (arm-none-eabi-gcc + `3dsx.specs`) does
   the rest.
4. `pack_3dsx` — `smdhtool` (metadata) + `3dsxtool --romfs` → `.3dsx`.

## The shim rule

Unlike the N64 there is no ABI hazard (both sides are ARM EABI-hf), but the
shim (`trino_shim_3ds.c` + `crates/platform-3ds/src/ffi.rs`, matching pairs)
is still the only FFI surface: citro2d is mostly static-inline C that Rust
cannot call, and the discipline (≤4 scalar/pointer args, no by-value
structs, no variadics) keeps the console backends symmetrical. Never call
libctru/citro2d directly from Rust.

## Troubleshooting

- `devkitPro not found`: install from https://devkitpro.org with the
  `3ds-dev` package group.
- Shim compile errors: libctru/citro2d API drift — fix the shim against the
  installed headers (that is the shim's job).
- Link errors about missing C2D_/C3D_/ctru symbols: check the library order
  in `cargo_build` (citro2d → citro3d → ctru → m).
- Game code gating: consoles are `no_std` — the 3DS is
  `target_os = "horizon"`, the N64 is `target_os = "none"`; game crates gate
  with `any(...)` (see `examples/hello-sprite/src/lib.rs`).
- Nightly missing: `rustup toolchain install nightly --component rust-src`.
