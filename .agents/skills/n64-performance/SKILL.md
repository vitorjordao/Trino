---
name: n64-performance
description: Measure and improve Trino's frame cost on the N64 — the TRINO_PERF line (emulated N64 time from ares), what it measures, how to compare before/after, and which host numbers not to trust. Use for "make it faster on N64", "frame rate", "T&L optimization", BACKLOG P0 #1, or any change to render3d/tessellate/backends that could cost CPU.
---

# n64-performance

## The number that counts

```
cargo xtask test n64        # prints, near the end:
TRINO_PERF:castle64 hub frame update=<us> render=<us> total=<us> (emulated N64 time; ...)
```

- **Where it comes from:** the self-test (`apps/n64/src/lib.rs`, after the bot's
  run) renders 8 castle64 hub frames. The `frame` closure times update and render
  with `runtime::now_us()`, which reads the CPU count register through the shim
  (`trino_ticks_us`).
- **Why it is real N64 time:** ares emulates the count register, so the figure is
  the console's clock, not the host's. It came out the same on a host GPU and on
  lavapipe.
- **What render covers:** CPU T&L, clipping, sorting and the RDP command submit. It
  does not cover the RDP's own fill time.
- **Baseline** (2026-10-12, branch `backlog/full-pass`): update ≈ 0.3 ms, render
  ≈ **670 ms** per frame. That is ~1.5 fps; the target is 33.3 ms (30 fps). Update
  this line whenever the number moves.

## Not the number

- `bench_tnl` (`cargo test --release -p xtask --test bench_tnl -- --ignored
  --nocapture`) measures the **host** CPU. Use it to guard against regressions and
  to see relative cost (for example, smooth skinning adds 3%). Never use it as
  evidence of console speed.
- PC frame times say nothing about the N64.

## Workflow for a perf change

1. **Baseline first:** record `TRINO_PERF` on the base commit, in a `git worktree`
   (never `git stash`).
2. **Break the frame down before optimizing** (BACKLOG P0 #1 (a)): add temporary
   `now_us()` spans around transform, light, clip, sort and submit, and log them as
   `TRINO_PERF_STAGE:` lines. Optimize the biggest stage.
3. **Hold the pixels:** the change must leave the PC goldens and
   `tests/golden/n64/showcase.png` byte-identical. A change that moves pixels on
   purpose (fixed point, for example) is a renderer change: re-bless it, review it
   and explain it.
4. **Record it:** quote before and after `TRINO_PERF` in the commit message and in
   BACKLOG, and update the baseline line above.

## Known levers (from BACKLOG P0 #1)

- **RSP T&L:** a small ucode, or tiny3d's, behind the same `draw_model` contract
  (ADR 0003 allows it). This is the biggest lever.
- **Static geometry:** cache transformed static geometry while the camera is still.
- **Fixed-point paths** for the VR4300. Floats are slow there, and the core's f32
  path is deterministic by design, so keep results bit-compatible or re-bless on
  purpose.
- **Draw less:** frustum culling, LOD (`model.lod`) and portals (`core::vis`) already
  exist. Check that castle64 actually uses them before writing new code.
