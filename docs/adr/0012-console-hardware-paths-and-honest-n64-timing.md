# ADR 0012: GPU vertex T&L on the 3DS, a faster N64 text path, and N64 timings that survive code layout

Status: accepted (October 2026). Part of GitHub #31 (the hybrid hardware pipeline); the
PC's GPU vertex path is not in this round.

## Context

- **The 3DS was the only target with no hardware T&L.** The N64 sends most static draws
  to the RSP (ADR 0007). The 3DS transformed and lit every vertex on its 268 MHz ARM11
  (`tessellate`), then copied ten floats per vertex into a per-frame buffer for the
  PICA200, whose vertex shader only mapped pixels to the screen (`tri3d.v.pica`).
- **An external project's N64 frames got 1–2 ms slower** after an engine update that
  added four small, unused methods (344 bytes) to the N64 renderer. Every other function
  in the ROM had the same size; only their addresses moved. The stage that grew was the
  2D one (`sprites`: text, rectangles, blits).
- **The same project saw one emulator run "hang"** (`timeout: no TRINO_TEST_* output
  within 300s`), and the next run of the same ROM pass.

## Decisions

### 1. GPU vertex T&L on the 3DS (`render3d::gpu_check`, `tnl3d.v.pica`)

- A mesh that `gpu_mesh_check` accepts (static, untextured, opaque, at most 65 535
  vertices) is copied once, at register, to linear memory: position and normal as floats,
  color as bytes, 28 bytes a vertex, plus its u16 indices (`trino_gpu_mesh`).
- Per draw, `gpu_check` decides. It accepts what the CPU path would draw without fog,
  flat shading, translucency, textures, palettes, skeletons or local lights. The sphere
  must also be wholly past the near plane, so the CPU path would have clipped nothing.
  Unlike the RSP rules, it takes colored lights, emissive and double-sided draws, and any
  size and position: the PICA lights in floats and clips in hardware.
- An accepted draw becomes 36 floats of uniforms (`trino_gpu_draw`) and one
  `C3D_DrawElements`:
  - the model → screen matrix, the CPU path's projection and per-vertex depth band
    multiplied by the view depth (`gpu_screen_matrix`, then the 3DS's band and the
    screen's tilt projection);
  - the model-space light, the ambient and diffuse factors, the tint, the emissive.

  `tnl3d.v.pica` evaluates the CPU path's formula in floats.
- **The rules live next to `rsp_check`, not inside it.** The RSP rules sit on the N64's
  hot path and are what tools and external projects predict the N64 with; they did not
  change.
- **Gates:**
  - `crates/core/tests/gpu_tnl.rs`: for every accepted draw, the shader's arithmetic puts
    every vertex where `tessellate` does, with the same depth and color (±1);
  - the refusals;
  - the `gpu tnl` self-test step in Azahar: the castle64 hub drawn with the GPU path and
    with the CPU path, 960 samples compared;
  - the existing `occlusion` step, which caught the mirrored cull winding;
  - `tests/perf/castle64_work_3ds.toml` (`perf_budget`): the 3DS's GPU/CPU work split
    per scene, deterministic, pinned on the host.
- Result on castle64's hub: all 26 draws (603 triangles) on the GPU; the frame matches the
  CPU path within 6 of 960 samples (mean 0.56/255), and the 3DS golden stays within its
  tolerance (mean 0.58).
- `N3dsRenderer::set_gpu_tnl(false)` puts every draw back on the CPU: the reference, and
  a switch for A/B measurements.
- An external project's isometric scenes (~110–130 draws), the same build with the path
  off and on (Azahar, `cpu_us` = the ARM11's own time per frame, now in the 3DS
  `TRINO_FRAME` line):

  | Scene | GPU path off | on |
  |---|---|---|
  | busy scene | 19.6 ms CPU, 30 fps | 5.3 ms, 60 fps |
  | the same with a UI panel open | 20.0 ms, 30 fps | 5.3 ms, 60 fps |
  | panning | 17.3 ms, 30 fps | 4.5 ms, 60 fps |
  | its largest scene | 50.2 ms frame (20 fps) | 16.7 ms (60 fps) |

  Every 3D draw there took the GPU path (no CPU T&L triangles left); its 3DS goldens
  moved by mean 0.5–0.95/255.
- The first version stalled that largest scene: every GPU draw re-emitted the program,
  attributes, texenv and citro2d's state (`C2D_Prepare`), and a few hundred draws
  overran citro3d's command buffer. The shim now enters the T&L state once per run of
  GPU draws (`trino_tnl_leave` before any other draw) and allocates 4× the default
  command buffer.

### 2. Text on the N64: one call per run of glyphs, and O(1) glyph lookup everywhere

- `text::GlyphIndex`: the table record of every codepoint below 256, built once per font
  at register. `BitmapFont::with_index` uses it. A lookup is one load instead of a
  binary search per character, and left-aligned text no longer measures its lines first.
  The glyphs found are the same; a test pins it.
- N64: the glyphs of a string go to the shim in runs of up to 32 (`trino_text_run`), not
  one FFI call each. On-screen glyphs at an integer scale skip libdragon's general
  rectangle path, which costs two divisions and four clip tests a glyph. The texture
  steps are the constants `1024 / scale` that the general path computes, so the RDP gets
  the same words.

### 3. N64 timings: measure over several code layouts

- `TRINO_N64_TEXT_PAD=<bytes>` links that much padding into `.text` ahead of the Rust
  code. The code is unchanged, and every function moves.
- An external project's scene at one engine commit, over seven paddings, measured
  25.1 to 28.1 ms. The two commits it compared, 25.8 and 27.7 ms, both fall inside that
  spread.
- So a perf claim on the N64 quotes the mean and the spread over a few paddings (e.g.
  0, 352, 2048, 6144), never one build.
- The knob moves everything after the shim together. It samples layouts; it does not
  explore every one.

### 4. Telling a hang from a slow run

- The emulator harnesses (`n64`, `3ds`) add an **idle limit** to the total timeout:
  `TRINO_EMU_IDLE` seconds without any output line, default 120.
  - A silent emulator fails as `stalled: … after step X`.
  - A run still printing at the deadline fails as `timeout: … still printing`.
- The N64 self-test arms a **stall watchdog** (`trino_watchdog_start`, a libdragon timer).
  If no frame begins and nothing is logged for 4 s of emulated time, it prints
  `TRINO_STALL` with where the CPU waits (`display_get`, `rspq_wait`,
  `rdpq_detach_show`) and the RSP/RDP status registers.
  - A console-side hang now names itself.
  - A stall with no `TRINO_STALL` line is the emulator.
- What this round saw:
  - One 300 s timeout in the external project's N64 runs. It came while the machine was
    under heavy memory pressure (load 35, 7 GB of swap).
  - A rerun of the same ROM under that load was caught still printing, about three times
    slower than usual.
  - With the idle limit in place, a real stall did show up on the 3DS (§1's command
    buffer). It was reported as `stalled: … after step X` within two minutes.

## Consequences

- 3DS frames spend far less ARM11 time on static props. Azahar's frame times overstate
  the gain, because it doesn't time the GPU, so they are quoted as an upper bound. Real
  hardware is the open measurement.
- The 3DS goldens move slightly (float24 vs f32): re-bless after looking.
- Next for the GPU path: textured meshes (alpha-tested, with painter order for the
  blended ones), fog in the shader, rigid skeletons (joint matrices as uniforms), local
  lights per vertex in the shader, and the PC native profile's own GPU vertex path with
  instancing (#31).
