---
name: build-3ds
description: Build the Nintendo 3DS app (target/3ds/trino.3dsx) with the local devkitPro toolchain, and troubleshoot the 3DS pipeline (devkitARM, citro2d shim, tex3ds assets). Use for any "build for 3DS / 3dsx / libctru" request.
---

# build-3ds

```
cargo xtask build 3ds              # shim + assets + Rust + .3dsx (all local, no Docker)
cargo xtask build 3ds --selftest   # the self-test app `test 3ds --no-build` boots (CI artifact)
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

**All 3D goes through one citro3d path** (`trino_3d_draw`): `tri3d.v.pica` is
assembled by `picasso` into `target/3ds/tri3d_shbin.h`, which the shim includes. The
renderer sorts the batch into texture runs and hands each run over in one call (10
floats per vertex: x, y, depth, w, u, v, r, g, b, a); the shim copies it into a
per-frame vertex buffer in linear memory (16 384 triangles; `TRINO_WARN:` once when
full — `C3D_FRAME_SYNCDRAW` makes restarting it each frame safe), flushes the data
cache and issues `C3D_DrawArrays`. Then `C2D_Prepare()` hands the GPU back to
citro2d for sprites. `apps/3ds/build.rs` relinks when only the shim object changes
(`TRINO_3DS_SHIM`), and the app's stack is 256 KB (`__stacksize__`).

**Static meshes skip the CPU T&L (ADR 0012 §1).** Meshes `render3d::gpu_mesh_check`
accepts are copied to linear memory at register (`trino_gpu_mesh`, 28-byte vertices +
u16 indices); a draw `gpu_check` accepts is 44 floats of uniforms (matrix, light,
ambient, diffuse, tint, emissive, then the fog row and fog color: the shader fogs per
vertex from the view depth, a zero row = no fog) and one
`C3D_DrawElements` through `tnl3d.v.pica` (`trino_gpu_draw`), which transforms, lights
and projects on the PICA200. All shaders are assembled by `compile_shader` (`SHADERS`
in `xtask/src/n3ds.rs`: `tri3d`, `tnl3d`, `water`). **Water** (ADR 0015) has its own
program, `water.v.pica` (`trino_water_mesh`: 24-byte vertices; `trino_water_draw`: 104
floats of uniforms that `render3d::gpu_water` fills, the four ripples one per lane).
`crates/core/tests/gpu_water.rs` emulates the program on the host against the CPU
water; the shader's float24 is checked by the `gpu tnl water` frame in Azahar. Water
draws use `trino_3d_material` bit 8 (TexEnv INTERPOLATE of the two layers with a
0x80 constant), are blended and write color only (`GPU_WRITE_COLOR`), and come after
the frame's opaque 3D. A uniform can only be the first source of a PICA
instruction. Traps:
- **The tilt projection mirrors the winding:** the GPU path culls `GPU_CULL_FRONT_CCW`
  (`TRINO_TNL_CULL`). With the other winding only the boxes' inner faces drew and the
  `occlusion` step failed (their bottoms are coplanar with the floor).
- **A PICA uniform may only be an instruction's first source** (picasso).
- **GPU draws happen at call time,** not at the batch flush: they move the 2D depth to the
  HUD band themselves (`sprite_depth`).
- `N3dsRenderer::set_gpu_tnl(false)` = everything on the CPU (the `gpu tnl` self-test
  step's reference; A/B measurements). The host gate of the split is
  `tests/perf/castle64_work_3ds.toml` (`perf_budget`).
- **Why a draw stayed on the CPU:** `TRINO_FRAME` prints `off_gpu=<GpuReject>:<n>,…`
  per frame (`N3dsRenderer::gpu_rejects`; `Offscreen` ones are culled, not drawn), and
  `cargo xtask scene budget --platform 3ds` predicts the split from the host. A mesh
  whose linear-memory copy failed logs `TRINO_WARN` and counts as `NoGpuMemory`.
- **A "frozen" top screen in a self-test is usually the test, not the renderer.**
  Azahar's status bar shows `Speed` (emulation vs real time) and `App: N FPS` (frames
  the app presented). The self-test draws castle64 1 of 4 updates and the showcase 1 of
  32, and pixel probes used to wait two vblanks per pixel (`trino_debug_pixel`: 64 s of
  `App: 0 FPS` for one 960-sample check; now once per presented frame). `fps=` in
  `TRINO_FRAME` and the showcase panel count drawn frames. To check what a person sees,
  record Azahar's window (Wayland: `QT_QPA_PLATFORM=xcb`, GStreamer `ximagesrc xid=…`).
- **Azahar doesn't simulate the GPU's timing by default** ("Simulate 3DS GPU timings"
  is off), so the PICA200 side of a frame can look cheaper than on a 3DS. `run|test|watch 3ds --accurate-timing` (or
  `TRINO_AZAHAR_GPU_TIMING=1`) turns it on for one session (run-emulator skill).
- **Fog is the shader's, not the PICA fog LUT:** the LUT is indexed by the depth band,
  where a 16–42 m fog spans ~0.0007 of the range. The shader interpolates the fog
  perspective-correctly, so a fogged GPU frame is slightly closer to the true value
  than the CPU path's depth splits (the `gpu tnl showcase-fog` step: 2/960 samples
  differ by more than 24).

- **Never draw 3D with `C2D_DrawTriangle`.** It takes one depth for the whole
  triangle: big floor triangles then win the depth test over whatever stands on
  them (castle64's player and boars sank into the ground — the `occlusion`
  self-test step reproduces it). Depth must be per vertex (`band_depth(tri.z[i])`).
- Untextured triangles pass **w = 1** (screen-linear colors, like the PC and N64
  rasterizers); textured ones pass the view depth (perspective-correct UVs).
- Immediate mode (`C3D_ImmSendAttrib`) was the old textured path: ~144 bytes of
  command buffer per triangle, so a full scene could overflow the 256 KB buffer.

- **Logging:** `trino_log` goes to `svcOutputDebugString` (Azahar's log, the
  test harness). When the app was launched by 3dslink, `__3dslink_host` is set and
  `trino_link_init` also connects stdout to the host, so the log goes there too. This
  is for real hardware (`run 3ds --device network`, the `hardware` skill). The 3dslink
  header only forward-declares `struct in_addr`: reading `.s_addr` needs
  `<arpa/inet.h>`.

**Two screens.** `s_top` (400×240) and `s_bottom` (320×240) are citro2d screen
targets, and each has a 16-bit depth buffer. `trino_set_screen(1)` begins the
bottom's scene and switches `s_proj` (the tri3d projection) to the bottom's
`Mtx_OrthoTilt(0, 320, 240, 0)`. `trino_frame_begin` resets both to the top. On the
Rust side, `N3dsRenderer::set_screen` only goes 0 → 1 (the PC's order) and rebuilds
the T&L context at 320×240, and `begin_frame` goes back to 400×240. A 3D draw on the
bottom with the top's projection lands about 40 px off. The `dual screen` self-test
step probes for that.

## The shim rule

Unlike the N64 there is no ABI hazard (both sides are ARM EABI-hf), but the
shim (`trino_shim_3ds.c` + `crates/platform-3ds/src/ffi.rs`, matching pairs)
is still the only FFI surface: citro2d is mostly static-inline C that Rust
cannot call, and the discipline (≤4 scalar/pointer args, no by-value
structs, no variadics, **no scalar float after an int** — volumes/pans cross as
16.16 fixed point, like on the N64) keeps the console backends symmetrical. Never call
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
  with `any(...)` (see `examples/castle64/src/lib.rs`).
- Nightly missing: `rustup toolchain install nightly --component rust-src`.
