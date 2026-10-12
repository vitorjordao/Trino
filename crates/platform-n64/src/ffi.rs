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
    pub fn trino_panic(msg: *const c_char) -> !;
    pub fn trino_ticks_us() -> u32;

    pub fn trino_frame_begin(rgba8888: u32);
    pub fn trino_frame_end();

    pub fn trino_sprite_load(dfs_path: *const c_char) -> *mut c_void;
    /// Returns (width << 16) | height.
    pub fn trino_sprite_size(sprite: *mut c_void) -> u32;
    pub fn trino_sprite_blit(sprite: *mut c_void, params: *const TrinoBlit);

    /// Set the RDP shade combiner + z-test; call once before a `trino_tri`
    /// batch.
    pub fn trino_3d_begin();
    /// pts: 6 floats (x/y per vertex); colors: 12 bytes (rgba per vertex);
    /// zs: 3 floats (normalized depth 0..1, 0 = near) for the RDP z-buffer.
    pub fn trino_tri(pts: *const f32, colors: *const u8, zs: *const f32);
    /// Select a `sprite_t` as the texture for `trino_tri_tex` (TMEM upload,
    /// TEX0 * SHADE, perspective). `clamp` != 0: billboard mode (no tiling,
    /// half-alpha cutout); 0: tiled.
    pub fn trino_3d_texture(sprite: *mut c_void, clamp: u32);
    /// 30 floats: per vertex x, y, z, u, v, w, r, g, b, a (colors 0..1,
    /// u/v normalized, w = view depth).
    pub fn trino_tri_tex(verts: *const f32);
    /// Test hook: RGBA8888 of the last frame at (x, y); 0 = unavailable.
    pub fn trino_debug_pixel(x: u32, y: u32) -> u32;

    pub fn trino_joypad_buttons() -> u32;
    /// (x as i16 as u16) << 16 | (y as i16 as u16).
    pub fn trino_joypad_stick() -> u32;

    pub fn trino_wav_load(dfs_path: *const c_char) -> *mut c_void;
    /// `looped` is a no-op on N64 (loop lives in the wav64 file).
    pub fn trino_wav_play(wav: *mut c_void, channel: u32, looped: u32);
    pub fn trino_channel_stop(channel: u32);
    pub fn trino_audio_poll();

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
