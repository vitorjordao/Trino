# ADR 0006 — Performance and visual gates, 3DS per-vertex depth, renderer state

- Status: accepted
- Date: 2026-10-13
- Context: two problems were reported on the emulators. On the N64, castle64 ran at
  a "deplorable" frame rate. On the 3DS, model vertices overlapped the floor. Both
  are simple scenes, so the cause was either the game or something latent in the
  engine. The pass measured before changing anything, fixed what the numbers
  pointed at, and made the evidence permanent. Record of the work:
  `docs/BACKLOG.md` › Done (performance & stability pass). Procedures:
  `n64-performance`, `run-emulator`, `build-3ds`.

## 1. Console profiling is part of the engine (`trino_core::perf`)

- `tessellate` returns `TnlStats`: vertex transforms, source triangles, clips, depth
  splits, back faces, emitted triangles and culled draws. They are deterministic
  (same code, same f32 on every target), and backends sum them into
  `FrameStats::tnl`.
- `RenderProfile` holds stage times (begin, T&L, sort, submit, sprites, end) plus a
  cost per model. It has fixed capacity and no allocator. Backends own the clock,
  because the core has none. The N64 renderer implements
  `start_profile`/`take_profile` with the count register, converted exactly with
  `TIMER_MICROS_LL` (the old integer divisor read 1.9% high), and a VI-interrupt
  counter cross-checks it.
- The report has a fixed line format, which tools parse:
  - `TRINO_PERF_SCENE:<scene> key=value…` for each scene;
  - `TRINO_PERF_MODEL:<scene> model=<hex> …` for each model.
- Measured states are code (`castle64::perf::SCENES`). Each scene is a **fresh**
  game in a level after a fixed number of idle updates. Game progress changes what a
  level draws, and the self-tests measure after the bot has played.

## 2. Two kinds of baseline, owned by two tools

- **Work** (`tests/perf/castle64_work.toml`) holds exact per-frame T&L counters.
  `xtask/tests/perf_budget.rs` runs the real `tessellate` on the PC and owns the
  file (`TRINO_BLESS=1`). `cargo xtask test n64` requires the ROM's counters to be
  **equal**: a difference is a determinism bug, never noise.
- **Time** (`tests/perf/castle64_n64.toml`) holds ceilings in emulated N64 µs, owned
  by `cargo xtask test n64 --bless`. The gated fields are `frame_us` (update +
  render − begin), `tnl_us`, `sort_us` and `submit_us`. `begin` is left out because
  it mostly waits for vsync or the RDP.
- Headroom: +20% on the frame and +35% on stages, plus 0.5 ms. ares is
  bit-reproducible for a given ROM, but **code layout** is not neutral. An unrelated
  1 KB function moved the hub's T&L by −10% and its submit by +20%, while the frame
  moved 5% (16 KB direct-mapped I-cache). The time gate is a net against real
  slowdowns. The exact work gate catches content and algorithm regressions
  deterministically.

## 3. A calibrated cost model instead of a triangle count

- `trino_core::perf::N64_COST` (`CostModel`) is a linear fit of measured emulated
  time:
  - 9.0 µs per vertex;
  - 44.2 µs per source triangle;
  - 419 µs per draw;
  - 14.7 µs per split;
  - plus 30 µs per emitted triangle for sort and submit.
- It fits 13 per-model points within ±11% and four of the five scene totals within
  ±2%.
- Tools that only see deterministic counters use it:
  - `model inspect` prints the per-draw estimate and warns above 25% of a 30 fps
    frame;
  - the editor budget HUD shows an "N64 3D time" row;
  - `test n64` prints the estimate and notes any drift above 25%.
- `Caps::N64.max_tris_per_frame` (4000) stays for the RDP and strict mode. It is
  not the CPU-T&L budget, which is ~200–300 triangles per 30 fps frame today.
  Replacing it with a cost budget is BACKLOG P0 #1 (d).

## 4. 3DS: all 3D through citro3d with per-vertex depth

- Untextured triangles used to go through citro2d's `C2D_DrawTriangle`, which takes
  **one depth per triangle**. The renderer passed the average, so large floor
  triangles won the depth test over characters standing on them.
- The shim API is now one entry: `trino_3d_draw(verts, count, sheet, clamp)`. It
  takes 10 floats per vertex (x, y, depth, w, u, v, r, g, b, a) and draws one texture
  run per call (`sheet` = NULL for vertex colors). Vertices go into a per-frame
  vertex buffer in linear memory (16 384 triangles; `C3D_FRAME_SYNCDRAW` makes the
  per-frame restart safe), followed by `C3D_DrawArrays`.
- This replaces `trino_3d_begin`, `trino_tri`, `trino_3d_tex_begin`,
  `trino_tri_tex` and `trino_3d_tex_end`. Immediate mode cost ~144 bytes of command
  buffer per triangle.
- Untextured triangles pass w = 1 (screen-linear colors, like the PC and N64), and
  textured ones pass the view depth.
- `castle64::selftest::occlusion` reads back pixels of boxes standing on a large
  floor. It failed in Azahar before this change and passes on PC, ares and Azahar
  after it. Any backend that resolves depth per triangle fails it.

## 5. Emulator golden frames on both consoles

- The N64 golden stays pixel-exact (`tests/golden/n64/showcase.png`).
- The 3DS dumps its top screen in the same `TRINO_FBR` format (RGBA5551) of a fixed
  castle64 state, compared with `tests/golden/3ds/castle64_hub.png` **within a
  tolerance** (`fbgolden::check_close`: mean ≤ 3, ≤ 2% far pixels). Azahar's GPU
  output depends on the host OpenGL.
- `xtask/tests/n3ds_parity.rs` holds the PC's N3ds look to that frame (mean 0.60,
  1.0% far pixels when written), as `n64_parity` does for the N64.

## 6. Renderer state belongs to the game, every frame

- Camera and light are renderer state, and they outlive a frame and the game that
  set them. castle64 never set a light and rendered with whatever ran before. On
  consoles that was the self-test showcase's colored, fogged light. On the 3DS this
  showed as a mean diff of 24.5 against the PC; on the N64 it inflated the
  measurements.
- Convention: a game sets its camera **and** light every frame. `set_light` never
  flushes a batch, so it is free.

## 7. N64 stack budget is checked

- libdragon's main stack is 64 KB, and an overflow is silent: it corrupts memory,
  and interrupts push onto the same stack, so the symptom moves between runs.
- The self-test's baked-scene step overflowed it. main, `run_self_test` (21 KB
  frame), `check_baked` (21 KB) and `ShowcaseGame::from_baked` (22 KB) were live at
  once. It had done so since that step existed. The new end-of-run stack report
  exposed it as "65280 bytes used", and random timeouts followed.
- Each showcase step now has its own `#[inline(never)]` function, and the peak is
  47.6 KB. `check_stack` reports `TRINO_STACK` after the showcase steps and at the
  end, and fails the self-test above 60 KB.
- Boxing doesn't fix this: `Box::new(big())` still materializes `big()` in the
  caller's frame.

## 8. N64 link: newlib's `mem*`

- Rust's `compiler_builtins` defines `memcpy`/`memset`/`memmove`/`memcmp` as weak
  generic loops. Once its object is linked, the linker never pulls newlib's
  optimized MIPS versions.
- The link passes `-Wl,-u,<sym> … -Wl,-lc` ahead of the staticlib. It must be
  `-Wl,-lc`, because the gcc driver drops a repeated plain `-lc`.
- `check_mem_symbols` fails the build if the map's memory section shows the Rust
  copies.
- Effect, A/B on identical code: game update −10%, T&L −0.4…−1.6%.
