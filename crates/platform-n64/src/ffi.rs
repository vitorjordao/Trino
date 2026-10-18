//! Declarations for the C shim (`shim/trino_shim.c`). Signatures MUST match
//! the shim exactly and stay inside the safe ABI subset documented there:
//! <=4 scalar/pointer args, no by-value structs, no variadics.

use core::ffi::{c_char, c_void};

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
    /// Source sub-rect in sprite pixels; `src_w == 0` = whole sprite.
    pub src_x: i32,
    pub src_y: i32,
    pub src_w: i32,
    pub src_h: i32,
}

unsafe extern "C" {
    pub fn trino_init();
    pub fn trino_log(msg: *const c_char);
    /// Self-test only: print `TRINO_STALL` (where the CPU waits, RSP/RDP
    /// status) when no frame begins and nothing is logged for 4 s.
    pub fn trino_watchdog_start();
    pub fn trino_panic(msg: *const c_char) -> !;
    pub fn trino_ticks_us() -> u32;
    /// Vertical blanks since boot (VI interrupt counter; clock cross-check).
    pub fn trino_vblank_count() -> u32;

    pub fn trino_frame_begin(rgba8888: u32);
    pub fn trino_frame_end();

    pub fn trino_sprite_load(dfs_path: *const c_char) -> *mut c_void;
    /// Returns (width << 16) | height.
    pub fn trino_sprite_size(sprite: *mut c_void) -> u32;
    pub fn trino_sprite_blit(sprite: *mut c_void, params: *const TrinoBlit);
    /// Solid rectangle: xy / wh packed 16:16 (whole pixels), RGBA8888.
    pub fn trino_fill_rect(xy: u32, wh: u32, color: u32);
    /// Upload a whole font atlas to TMEM if it fits; 1 = resident.
    pub fn trino_text_begin(sprite: *mut c_void, tint: u32) -> u32;
    /// `n` glyphs of the resident atlas as (xy, st, wh) triples (screen
    /// x << 16 | y, atlas s << 16 | t, w << 16 | h), one call per run.
    pub fn trino_text_run(glyphs: *const u32, n: u32, scale: u32);

    /// Set the RDP shade combiner + z-test; call once before a `trino_tri`
    /// batch.
    pub fn trino_3d_begin();
    /// RSP overlay check: one fixed triangle drawn by `rsp_trino.S`.
    pub fn trino_rsp_test_tri();
    /// RSP T&L of one static mesh draw: 27 words in the overlay's DRAW
    /// layout (`rsp_trino.S`).
    pub fn trino_rsp_draw(words: *const u32);
    /// The colored draw command (`TrinoCmd_DrawColored`): the 27 words of
    /// `trino_rsp_draw` and the 7 of `rsp::color_words`.
    pub fn trino_rsp_draw_colored(words: *const u32, color: *const u32);
    /// A textured draw (rsp_trino_tex), after `trino_3d_texture_ex` bound its
    /// texture: the 27 words of `trino_rsp_draw` and the 17 of
    /// `rsp::tex_words` (color, then texture scale and offset).
    pub fn trino_rsp_draw_textured(words: *const u32, color: *const u32);
    /// A water draw (rsp_trino_water), after `trino_3d_water` bound its
    /// texture and blend: the 27 words of `trino_rsp_draw` and the 19 of
    /// `rsp::water_entry`.
    pub fn trino_rsp_draw_water(words: *const u32, color: *const u32);
    /// Uncached, 16-byte aligned RDRAM (the RSP reads it by DMA).
    pub fn trino_alloc_uncached(size: u32) -> *mut c_void;
    /// Physical address of a pointer (what the RSP's DMA takes).
    pub fn trino_phys_addr(p: *const c_void) -> u32;
    /// Write `len` bytes at cached address `p` back to RDRAM (data the RSP
    /// DMAs next).
    pub fn trino_cache_writeback(p: *const c_void, len: u32);
    /// Start the RSP on what is queued (`rspq_flush`): an idle RSP sleeps
    /// until then, so commands streamed during a frame would wait for its
    /// end.
    pub fn trino_rsp_flush();
    /// Claim the cache lines at cached `p` (16-aligned, `len` a multiple of
    /// 16) for writing, without reading RDRAM (every byte read later must be
    /// written first).
    pub fn trino_cache_create(p: *mut c_void, len: u32);
    /// Heap bytes in use / in total (libdragon `sys_get_heap_stats`).
    pub fn trino_heap_used() -> u32;
    pub fn trino_heap_total() -> u32;
    /// Profiling: wait for the RSP and RDP to finish everything queued.
    pub fn trino_drain();
    /// pts: 6 floats (x/y per vertex); colors: 12 bytes (rgba per vertex);
    /// zs: 3 floats (normalized depth 0..1, 0 = near) for the RDP z-buffer.
    pub fn trino_tri(pts: *const f32, colors: *const u8, zs: *const f32);
    /// Select a `sprite_t` as the texture for `trino_tri_tex` (TMEM upload,
    /// TEX0 * SHADE, perspective). `clamp` != 0: billboard mode (no tiling,
    /// half-alpha cutout); 0: tiled.
    pub fn trino_3d_texture(sprite: *mut c_void, clamp: u32);
    /// `trino_3d_texture` with options: `flags` bit 0 clamp, bit 1 point
    /// filter, bits 4..8 the detail layer's scale (log2). `detail` (may be
    /// null) is multiplied over the texture by the 2-cycle combiner from
    /// TILE1; a pair that doesn't fit TMEM draws without it. Returns 1 when
    /// the detail is on.
    pub fn trino_3d_texture_ex(sprite: *mut c_void, detail: *mut c_void, flags: u32) -> u32;
    /// Water (ADR 0015): the next `trino_tri_tex` (or, with a null
    /// `sprite`, `trino_tri`) calls blend over the frame, depth-tested but
    /// not written. With a texture and a `detail` the 2-cycle combiner
    /// blends the two half and half, then multiplies by the shade:
    /// `((TEX1 - TEX0) * ENV_ALPHA + TEX0) * SHADE`, alpha from the shade.
    /// `flags` as `trino_3d_texture_ex`'s; `scroll` = the detail tile's
    /// offset, `u << 16 | v` in 1/65536 of its repeat. Returns 1 when the
    /// detail is on (both fit TMEM and neither is a palette).
    pub fn trino_3d_water(sprite: *mut c_void, detail: *mut c_void, flags: u32, scroll: u32)
    -> u32;
    /// 30 floats: per vertex x, y, z, u, v, w, r, g, b, a (colors 0..1,
    /// u/v normalized, w = view depth).
    pub fn trino_tri_tex(verts: *const f32);
    /// Test hook: RGBA8888 of the last frame at (x, y); 0 = unavailable.
    pub fn trino_debug_pixel(x: u32, y: u32) -> u32;
    /// Test hook: print the last framebuffer over ISViewer (golden images).
    pub fn trino_debug_fb_dump();

    /// Bits 0..11 = `Button` discriminants; 16..19 = C-up, C-down, C-left, C-right.
    pub fn trino_joypad_buttons() -> u32;
    /// (x as i16 as u16) << 16 | (y as i16 as u16).
    pub fn trino_joypad_stick() -> u32;

    pub fn trino_wav_load(dfs_path: *const c_char) -> *mut c_void;
    /// `looped` loops the whole sound (the bake's lengths are multiples of
    /// 16, as libdragon wants of a VADPCM loop).
    pub fn trino_wav_play(wav: *mut c_void, channel: u32, looped: u32);
    pub fn trino_channel_stop(channel: u32);
    pub fn trino_audio_poll();
    /// Per-channel left/right volume, fixed point (65536 = 1.0): scalar
    /// floats after an int are unsafe across the N64's o32/o64 boundary.
    pub fn trino_ch_volume(channel: u32, lvol_q16: u32, rvol_q16: u32);
    pub fn trino_ch_playing(channel: u32) -> i32;
    pub fn trino_mixer_set_vol(vol_q16: u32);
    /// Bit n set = mixer channel n plays.
    pub fn trino_audio_playing_mask() -> u32;
    /// Enable Opus wav64s before the first one loads; 0 when the ROM was
    /// linked without the decoder (no Opus asset in the bake).
    pub fn trino_audio_opus_init() -> u32;
    /// Writes 10 counters (see the shim): resident bytes, stream bytes,
    /// underruns, streamed frames, poll µs, polls, RSP mix µs, buffers,
    /// samples per buffer, output rate.
    pub fn trino_audio_stats(out: *mut u32);

    /// Path is DFS-relative, e.g. `/index.tsv` (no `rom:/`).
    pub fn trino_file_exists(dfs_relative_path: *const c_char) -> i32;
    pub fn trino_asset_load(dfs_path: *const c_char, size_out: *mut u32) -> *mut c_void;
    pub fn trino_free(ptr: *mut c_void);

    /// Bytes of cartridge EEPROM (0 = none).
    pub fn trino_save_capacity() -> u32;
    pub fn trino_save_read(dest: *mut u8, start: u32, len: u32);
    pub fn trino_save_write(src: *const u8, start: u32, len: u32);

    /// Deepest main-stack use since boot, in bytes (64 KB total).
    pub fn trino_stack_used() -> u32;

    // newlib, for the global allocator.
    pub fn memalign(align: usize, size: usize) -> *mut c_void;
    pub fn free(ptr: *mut c_void);
}
