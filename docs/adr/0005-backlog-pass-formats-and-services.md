# ADR 0005 — Engine services, data formats and console rules from the backlog pass

- Status: accepted
- Date: 2026-10-12
- Context: the 18-item backlog of `docs/BACKLOG.md` (October 2026), implemented on
  branch `backlog/full-pass`, one commit per item. This ADR records every format,
  ABI and convention decision those items introduced. The format sections are
  normative, and the code doc comments quote them.

## 1. Platform services and the game API (v2)

- `Platform` gained `blob(id) -> Option<&'static [u8]>`, plus `save_capacity`,
  `save_read` and `save_write`. Blobs are `'static` so that parsed views over them
  (`BakedScene`, fonts) can live in game state with no lifetimes. Each backend owns
  the bytes for the program's lifetime: on PC that is a leaked `Box`, on the consoles
  it is a file read once at load.
- `Game::update_with(platform, input, audio, dt)` is the entry point the apps call.
  `update` without a platform stays available for tests.
- `GAME_API_VERSION` = **2**. The hot-reload host refuses a dylib built against v1.
- **Save framing** (`core::save`): the medium holds `header ‖ data`. The header is
  `"TS" u16 len u32 fnv1a(data)`, little-endian. A torn or foreign medium reads back
  as "no save".
  - N64: EEPROM 16 Kbit, declared with `ed64romconfig --savetype eeprom16k`.
  - 3DS: `sdmc:/3ds/trino/save.bin`.
  - PC: a file next to the build output.
  - The PC save capacity follows the simulated console's `Caps.save_bytes`.

## 2. Bitmap text — `TFNT` v1

- A font is two pieces under one `FontId`:
  - an **atlas sprite**, which gets each console's native texture format (CI4 on N64);
  - a **glyph table** blob in TFNT format.
- TFNT layout (little-endian): `"TFNT"`, `u16 version=1`, `u16 line_height`,
  `u16 ascent`, `u16 glyph_count`, then 12-byte records sorted by codepoint. Each
  record is `u32 codepoint`, `u16 x`, `u16 y`, `u8 w`, `u8 h`, `i8 x_offset`,
  `u8 advance`.
- Layout and drawing live in core and use `draw_sprite` with a source `region`, so
  backends only keep the table.
- `SpriteParams.region` (a source rectangle) is new. Strict mode now checks the
  *drawn region* against TMEM, not the whole atlas.

## 3. Baked scenes — `TSCN` v1

The runtime twin of scene codegen. The bake (`trino_scene::bake::to_tscn`) and the
parser (`core::scene::BakedScene`, zero-copy, validated up front) agree byte for
byte. The layout:

- **Header** (12 bytes): `"TSCN"`, `u16 version=1`, `u16 reserved`, `u32 node_count`.
- **Environment** (68 bytes): clear color, camera, light, fog.
- **Node records**, 152 bytes each:
  - name and tags as (offset, len) pairs into the data table, with tags `\n`-separated;
  - the matrix as 3×4 f32;
  - flags, model, tint and emissive, clip and speed;
  - collider shape and offset, texture;
  - LOD (offset, count) into the data table, using 8-byte `f32 distance, u32 model`
    records;
  - sound (`u32 sound, f32 radius, f32 volume`);
  - visibility (`u8 cell, u8 kind, u8 a, u8 b, f32×3 half`).
- **Data table**: strings and LOD records.

The format is still **v1** because it has never shipped. The LOD (+8 bytes), sound
(+12) and visibility (+16) fields were added before any release. **The first release
freezes v1**; after that, any layout change bumps the version and adds a migration in
`BakedScene::parse`.

Manifest kind `[scenes.*]`: paths are relative to the project root, and the blob id is
`fnv1a("scenes/<name>")`.

## 4. TMDL sections added

TMDL stays extensible by section; unknown tags are skipped, so old readers keep working.

- `TXUV`: `f32 u, f32 v` per vertex.
- `TXID`: `u32` sprite handle of the default texture.
- `SKW2` (after `SKEL`, opt-in with `skinning = "smooth"`): `u8 joint2, u8 weight2` per
  vertex. The vertex blends from its SKEL joint toward `joint2` by `weight2 / 255`
  (a two-bone linear blend).
- `SLOT` (color slots): `u8 slot` per vertex, 0..=4, any value above 4 is a
  `BadSection`. Written only when some vertex has a non-zero slot, so models without
  slots bake byte-identical to before. Source: the glTF custom attribute `_SLOT`
  (SCALAR, float or integer; floats round to the nearest slot). The bake carries it
  through the vertex-cache reorder like every per-vertex array.

  A draw's `ModelParams::palette: Option<Palette>` (`Palette = [Color; 4]`) multiplies
  the RGB of a vertex in slot `s ≥ 1` by `palette[s - 1]` with `(a·b + 127) / 255`
  (the material tint-stacking rounding: white keeps the vertex color exactly), before
  tint and lighting; slot 0 keeps the baked color, and the palette's alpha is
  ignored. One mesh is drawn many times with different skin, hair or clothing colors.
  The software T&L applies it on every path (static, flat, rigid, smooth), so all
  three targets get it with no backend code. The palette is resolved once per draw,
  and a draw without one reads `mesh.color` exactly as before, so goldens are
  unchanged. The N64 RSP overlay draws baked colors, so `rsp_eligible` refuses a
  palette on a slotted mesh (CPU path), and `MemoKey::new` refuses any palette (its
  16 words are full).

Textured 3D:
- **PC**: perspective-correct, using the `w` trick in the triangle shader.
- **N64**: `rdpq` TEX_SHADE with perspective correction.
- **3DS**: a small citro3d path, `tri3d.v.pica`, assembled by `picasso` in xtask.

Textures must be power-of-two and fit TMEM; strict mode enforces both.

## 5. Scene components added (scenes v2, no version bump — all optional)

- `collider`: `Auto` (the model's bounds), `Box`, `Sphere` or `Capsule`, plus offset
  and trigger flag. It feeds `core::physics`.
- `model.lod`: a list of `(distance, path | None)`.
- `sound`: path, radius, volume. Looping sources are played by `core::audio::Ambience`.
- `vis`: `Cell(size)` or `Portal(size, a, b)` (`core::vis`).
  - Descendants of a cell belong to it.
  - Portal cell names resolve after the scene walk.
  - The format allows up to 64 cells per scene, held in a `u64` mask.
  - **Culling is conservative.** If `visible()` runs out of its fixed stack or step
    budget, it returns "everything visible".

## 6. Console ABI and backend rules

- **FFI float rule (N64):** never pass a scalar `float` after an integer argument
  across the shim. Rust is o32, libdragon is o64, and the float lands in the wrong
  register. Pass 16.16 fixed point (`u32`) or a pointer to floats instead. Found by
  the audio self-test; both shims follow the rule.
- **Audio channel plan (N64 and 3DS):**

  | Channels | Use |
  |---|---|
  | 0–9 | one-shot sounds |
  | 10–13 | steerable voices |
  | 14 | music |
  | 15 | stinger (music ducks to 30%) |

  - A wav64 plays on one channel at a time, because it has a shared stream cursor.
  - Looped wav64 lengths must be multiples of 16 frames; xtask trims them.
  - `loop` in the manifest defaults to false for sounds and true for music.
- **16-bit output, matched on both sides:**
  - **N64**: the shim renders with magic-square RGB dither (libdragon's standard mode
    disables dithering) and enables `FILTERS_RESAMPLE_ANTIALIAS_DEDITHER`. Standard
    mode zaps coverage to full, so the VI AA/divot stages leave pixels untouched, and
    dedither is the stage that changes the picture.
  - **PC**: the N64 look reproduces this. The quantizer follows the RDP rule from
    angrylion `rdp/dither.c`: round up when the low 3 bits exceed the dither value,
    then keep the top 5 bits. Fills truncate. A VI pass then applies the restore
    filter from angrylion `vi/restore.c`.
  - `n64_parity` bounds the PC/ares difference.
- **N64 z precision:** depth is `1 - NEAR/d` with `NEAR = 0.05` in a 16-bit z-buffer.
  Coplanar decals need several centimetres of separation at gameplay distances. The
  emulator golden caught the showcase pond z-fighting at 2 cm.
- **Two screens (added 2026-10-15, GitHub #17):**
  - `trino_core::Screen`: `Top` is the main screen on every target, `Bottom` the
    3DS touch screen (simulated by the PC's 3DS profile).
  - **Order contract:** a frame draws the top first, then the bottom, once. Every
    backend refuses a switch back to the top, and `Renderer::on_screen` reports the
    refusal as `false`. On a single-screen target `on_screen(Bottom)` is `false` and
    the caller skips those draws. Backends don't discard them, so the N64 renderer
    needs no screen code at all.
  - **Per-screen projection:** a 3D draw projects to the current screen's size. The
    T&L context is built from it, and the 3DS shim switches its citro3d matrix in
    `trino_set_screen`. Each screen has its own render target and depth buffer, which
    also clips its 2D.
  - **Budgets are per frame:** both 3DS screens share one citro2d object pool
    (`C2D_Init` once) and the shim's one per-frame vertex buffer. The PC's strict mode
    and `FrameStats::tris` therefore count both screens together.
  - **ABI:** `trino_debug_pixel(x, y, bottom)` (3 arguments) reads either LCD for
    `Renderer::read_screen_pixel`. `GAME_API_VERSION` stays 3, because the host and
    the game dylib build against the same `trino-core`.

## 7. Verification conventions

- Every engine feature has a step in `castle64::selftest`. The same code runs in the
  PC suite, in ares and in Azahar.
- **N64 emulator golden:** `tests/golden/n64/showcase.png`.
  - The ROM dumps its framebuffer over ISViewer as hex rows with an FNV hash, and
    `cargo xtask test n64 [--bless]` compares it pixel-exactly.
  - ares's parallel-RDP is bit-exact: the frame matches on a GPU and on Mesa lavapipe.
  - mupen64plus is **not** used. ares is the reference emulator.
- **3DS:** there is no emulator golden. Azahar renders on the host GPU, so pixels are
  not stable across machines. The self-test probes pixels with tolerances instead.
- The **TSCN** format is checked in three ways:
  - against codegen (`check_baked`);
  - against the editor's resolve (`showcase_render`);
  - with hand-built blobs in core tests.
