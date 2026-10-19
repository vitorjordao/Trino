//! Declarations for the C shim (`shim/trino_shim_3ds.c`). Signatures MUST
//! match the shim exactly and stay inside the safe ABI subset documented
//! there: <=4 scalar/pointer args, no by-value structs, no variadics.

use core::ffi::{c_char, c_void};

/// Mirrors `trino_fill_t` in the shim (fill_rect).
#[repr(C)]
pub struct TrinoFill {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub depth: f32,
    pub color: u32,
}

/// Mirrors `trino_blit_t` in the shim.
#[repr(C)]
pub struct TrinoBlit {
    pub x: f32,
    pub y: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub theta: f32,
    pub flip_x: u32,
    pub flip_y: u32,
    pub tint: u32,
    /// citro2d z (larger = nearer): layers sprites above/below the 3D band.
    pub depth: f32,
    /// Source sub-rect in sprite pixels; `src_w == 0` = whole sprite.
    pub src_x: i32,
    pub src_y: i32,
    pub src_w: i32,
    pub src_h: i32,
}

unsafe extern "C" {
    /// The New 3DS check and its speedup (bit 0 New 3DS, bit 1 speedup
    /// on, bits 8.. application memory MiB; `crate::hw::Probe`).
    pub fn trino_hw_probe(want_speedup: u32) -> u32;
    /// Start the worker core (core 2 on a New 3DS, else core 1 at 30%);
    /// the core, or -1 (`crate::worker`).
    pub fn trino_worker_start(new3ds: u32) -> i32;
    /// Queue `job(arg)`; 0 when not queued (the caller runs it).
    pub fn trino_worker_submit(job: extern "C" fn(u32), arg: u32) -> u32;
    pub fn trino_worker_wait();
    pub fn trino_init();
    pub fn trino_log(msg: *const c_char);
    pub fn trino_panic(msg: *const c_char) -> !;
    pub fn trino_ticks_us() -> u32;
    /// 0 when the applet manager wants the app to quit (HOME / power).
    pub fn trino_app_running() -> i32;

    pub fn trino_frame_begin(rgba8888: u32);
    pub fn trino_frame_end();
    /// Later draws (2D and 3D, each screen's projection) on the touch
    /// screen (1) or the top screen (0).
    pub fn trino_set_screen(bottom: u32);

    pub fn trino_sprite_load(romfs_path: *const c_char) -> *mut c_void;
    /// A `.t3x` in memory (citro2d copies it; the caller keeps `data`).
    pub fn trino_sprite_load_mem(data: *const u8, size: u32) -> *mut c_void;
    /// Free a sheet the GPU is done with (unbinds it first).
    pub fn trino_sprite_free(sheet: *mut c_void);
    pub fn trino_fill_rect(fill: *const TrinoFill);
    /// Returns (width << 16) | height.
    pub fn trino_sprite_size(sheet: *mut c_void) -> u32;
    pub fn trino_sprite_blit(sheet: *mut c_void, params: *const TrinoBlit);

    /// Draw `count` vertices (`count / 3` triangles) through citro3d with
    /// per-vertex depth. 10 floats per vertex: x, y (screen px), depth
    /// (citro2d z, larger = nearer), w (view depth for perspective-correct
    /// UVs, 1 = screen-linear), u, v (0..1 in the image), r, g, b, a (0..1).
    /// `sheet`: texture (image 0; `clamp` != 0 = billboard addressing with
    /// a half-alpha cutout), null = vertex colors only.
    pub fn trino_3d_draw(verts: *const f32, count: u32, sheet: *mut c_void, clamp: u32);
    /// Sampling of the next `trino_3d_draw` calls: `flags` bit 1 point
    /// filter, bits 4..8 the detail layer's repeats per base repeat (log2),
    /// bit 8 water (ADR 0015: the layers blended half and half times the
    /// vertex color, blended over the frame without writing depth, the
    /// detail layer offset by `scroll` = `u << 16 | v` in 1/65536 of its
    /// repeat); `detail` (null = none) is multiplied over the texture.
    /// Bit 9: a shadow (ADR 0024: the vertex color blended by its alpha,
    /// depth tested, not written), bits 10..12 its stencil part (the
    /// shim's `trino_shadow_stencil`: 1 once, 2 mark, 3 on the mark, 4
    /// unmark), bit 13 times the blob texture's alpha (`trino_shadow_blob`).
    pub fn trino_3d_material(detail: *mut c_void, flags: u32, scroll: u32);
    /// The blob texture of shadow draws: `size`² 8-bit alpha values,
    /// row-major (`trino_core::shadow::blob_texels`), copied.
    pub fn trino_shadow_blob(texels: *const u8, size: u32);
    /// Copy a water mesh to linear memory for `trino_water_draw`: `nverts`
    /// vertices of 24 bytes (x, y, z, u, v floats, r, g, b, a bytes) and
    /// `nidx` u16 indices. Null when linear memory is short.
    pub fn trino_water_mesh(
        verts: *const u8,
        nverts: u32,
        idx: *const u16,
        nidx: u32,
    ) -> *mut c_void;
    /// Draw a water mesh on the GPU (`water.v.pica`, ADR 0015): `u` = 104
    /// floats (the world-to-screen matrix, the model rows, then the
    /// shader's vectors from `wdir` to `detailoff`), `sheet` the base
    /// texture (null = none), the material from `trino_3d_material`.
    /// `flags` bit 0: cull back faces.
    pub fn trino_water_draw(mesh: *mut c_void, u: *const f32, sheet: *mut c_void, flags: u32);
    /// Copy a mesh to linear memory for the GPU T&L path: `nverts`
    /// vertices of 28 bytes (x, y, z, nx, ny, nz as f32, then r, g, b, a)
    /// and `nidx` u16 indices. Null when linear memory is short.
    pub fn trino_gpu_mesh(verts: *const u8, nverts: u32, idx: *const u16, nidx: u32)
    -> *mut c_void;
    /// Draw a `trino_gpu_mesh`: `u` = the 4x4 screen matrix (16 floats, rows)
    /// then light, ambient, diffuse, tint, emissive, fog row, fog color (4
    /// floats each, 44 in all); flags
    /// bit 0 = cull back faces, bit 1 = a planar shadow (blended, depth
    /// tested, not written) with bits 4..6 its stencil part (as
    /// `trino_3d_material`'s bits 10..12).
    pub fn trino_gpu_draw(mesh: *mut c_void, u: *const f32, flags: u32);
    /// Free a `trino_gpu_mesh` or `trino_water_mesh` (the GPU is done with it).
    pub fn trino_gpu_mesh_free(mesh: *mut c_void);

    /// Bits 0..13 = `Button` discriminants (12/13 = ZL/ZR, New 3DS).
    pub fn trino_joypad_buttons() -> u32;
    /// (x as i16 as u16) << 16 | (y as i16 as u16).
    pub fn trino_joypad_stick() -> u32;
    /// The C-stick (New 3DS; 0 elsewhere), packed like `trino_joypad_stick`.
    pub fn trino_cstick() -> u32;
    /// Bit 31 = touching, bits 16..24 = y, bits 0..9 = x (bottom screen px).
    pub fn trino_touch() -> u32;

    /// Load a baked `T3AU` sound (DSP-ADPCM or PCM16): whole into linear
    /// memory, or just its header and chunk contexts when it streams.
    pub fn trino_wav_load(romfs_path: *const c_char) -> *mut c_void;
    /// Bits 0..8 the bus (`trino_core::Bus`), bit 8 streamed, bit 9
    /// DSP-ADPCM, bit 10 baked to loop.
    pub fn trino_wav_info(wav: *mut c_void) -> u32;
    /// Free a loaded sound, stopping the channels that play it.
    pub fn trino_wav_free(wav: *mut c_void);
    pub fn trino_wav_play(wav: *mut c_void, channel: u32, looped: u32);
    pub fn trino_channel_stop(channel: u32);
    /// Once per frame: releases the streams that played to their end.
    pub fn trino_audio_poll();
    /// 0 streams playing, 1 bytes held by loaded sounds, 2 streaming
    /// buffer bytes, 3 underruns, 4 samples streamed, 5 the audio thread's
    /// core (2 or 1, the worker's second core; 0 = the app core).
    pub fn trino_audio_stat(which: u32) -> u32;
    /// Per-channel left/right volume, fixed point (65536 = 1.0): scalar
    /// floats after an int are unsafe across the N64's o32/o64 boundary.
    pub fn trino_ch_volume(channel: u32, lvol_q16: u32, rvol_q16: u32);
    pub fn trino_ch_playing(channel: u32) -> i32;

    /// Path is romfs-relative, e.g. `/index.tsv` (no `romfs:/`).
    pub fn trino_file_exists(romfs_relative_path: *const c_char) -> i32;
    pub fn trino_asset_load(romfs_path: *const c_char, size_out: *mut u32) -> *mut c_void;
    pub fn trino_free(ptr: *mut c_void);
    /// Ranged reads (asset group bundles): `romfs:/…` path; NULL = missing.
    pub fn trino_file_open(romfs_path: *const c_char) -> *mut c_void;
    /// `len` bytes at `offset` into `dest`; returns the bytes read.
    pub fn trino_file_read_at(file: *mut c_void, offset: u32, dest: *mut u8, len: u32) -> u32;
    pub fn trino_file_close(file: *mut c_void);
    /// 0 = heap bytes in use (malloc + linear), 1 = both heaps' size.
    pub fn trino_heap_stat(which: u32) -> u32;

    /// Test hook: RGBA8888 of the top screen (`bottom` = 0) or the touch
    /// screen (1) at (x, y); 0 = unavailable.
    pub fn trino_debug_pixel(x: u32, y: u32, bottom: u32) -> u32;
    /// Test hook: print the top screen as `TRINO_FBR:` rows (RGBA5551) and
    /// a `TRINO_FB_END:` hash line — the N64 dump format.
    pub fn trino_debug_fb_dump(bottom: u32);

    /// Save file on the SD card: returns its size (0 = none, -1 = error),
    /// copying up to `cap` bytes into `dest`.
    pub fn trino_save_load(dest: *mut u8, cap: u32) -> i32;
    /// Atomically replace the save file; 0 = ok, -1 = error.
    pub fn trino_save_store(src: *const u8, len: u32) -> i32;

    // newlib, for the global allocator.
    pub fn memalign(align: usize, size: usize) -> *mut c_void;
    pub fn free(ptr: *mut c_void);
}
