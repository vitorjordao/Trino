# ADR 0002 — N64: Rust as an o32 staticlib, linked by GNU ld with `--no-warn-mismatch`

Status: accepted (Fase 4, 2026-07)

## Context

libdragon and its entire toolchain are built with `mips64-elf-gcc -mabi=o64`
(64-bit registers, 32-bit pointers — the classic N64 homebrew ABI). LLVM has
**no o64 implementation**, so rustc cannot emit o64 objects, and rust-lld
refuses to link the closest LLVM options (n32-flagged objects) against
libdragon's o64 archives: MIPS e_flags mark the objects as different formats
(`*.rcgu.o is incompatible with elf32-bigmips`).

Alternatives considered:

- **Port o64 to LLVM** — months of upstream work, not a project goal.
- **Rebuild libdragon as o32** — diverges from the upstream-supported
  configuration, invalidates n64.mk, and every future libdragon update would
  need re-porting; also o64's 64-bit GPRs are part of libdragon's performance
  assumptions.
- **rust-lld with forced flags** — lld has no switch to ignore MIPS ABI
  e_flags; patching object headers on every build is fragile.

## Decision

1. Rust compiles to a **staticlib** for a custom target
   (`platforms/n64/mips-nintendo64-none.json`): `arch=mips`,
   `llvm-abiname=o32`, `+noabicalls`, big-endian, `cpu=mips3` — o32 with no
   GOT/PLT machinery, plain static addressing.
2. The final link happens **inside the toolchain container** with
   `mips64-elf-g++ -mabi=o64 -Wl,--no-warn-mismatch`, mirroring libdragon's
   own `n64.mk` `%.elf` recipe (same library order, `n64.ld`, `--gc-sections`,
   `--wrap __do_global_ctors`).
3. Every cross-language call goes through the C shim
   (`crates/platform-n64/shim/trino_shim.c`), which is restricted to the
   subset where **o32 and o64 agree**: at most 4 arguments, all
   i32/u32/f32/pointers, no by-value structs, no variadics, returns void /
   32-bit scalar / pointer. o32 and o64 pass those identically (a0–a3 / f12,
   returns in v0 / f0), so mixed-ABI calls are well-defined in practice.

`--no-warn-mismatch` silences GNU ld's e_flags compatibility check for the
o32 archive; the machine code itself is plain MIPS III, valid under either
flag set.

## Consequences

- The shim restriction is a hard rule: new FFI entries must stay in the safe
  subset (pass pointers to `#[repr(C)]` structs for anything wider). Enforced
  by review + the comment block at the top of the shim.
- `f64` and 64-bit integer arguments must never cross the boundary (they are
  where o32 and o64 differ). Inside Rust code they are fine.
- If a future binutils hard-errors on the flag mix, the fallback is patching
  `e_flags` of the staticlib members to o64 inside the container (objcopy or
  a 4-byte patch at ELF offset 0x24) before linking.
- Nightly + `-Zbuild-std=core,alloc -Zjson-target-spec` are required; the
  JSON spec fields are validated exactly by rustc, so nightly bumps may need
  spec updates (the compiler error names the field and expected value).
