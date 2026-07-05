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

    pub fn trino_joypad_buttons() -> u32;
    /// (x as i16 as u16) << 16 | (y as i16 as u16).
    pub fn trino_joypad_stick() -> u32;

    pub fn trino_wav_load(dfs_path: *const c_char) -> *mut c_void;
    pub fn trino_wav_play(wav: *mut c_void, channel: u32);
    pub fn trino_channel_stop(channel: u32);
    pub fn trino_audio_poll();

    /// Path is DFS-relative, e.g. `/index.tsv` (no `rom:/`).
    pub fn trino_file_exists(dfs_relative_path: *const c_char) -> i32;
    pub fn trino_asset_load(dfs_path: *const c_char, size_out: *mut u32) -> *mut c_void;
    pub fn trino_free(ptr: *mut c_void);

    // newlib, for the global allocator.
    pub fn memalign(align: usize, size: usize) -> *mut c_void;
    pub fn free(ptr: *mut c_void);
}
