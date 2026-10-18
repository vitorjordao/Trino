//! Boot-time asset loading from the RomFS.
//!
//! `cargo xtask assets 3ds` writes `index.tsv` into the RomFS: one
//! `handle-hex \t kind \t filename` line per asset. Loading by handle keeps
//! game code identical across platforms (`SpriteId::from_path`). An
//! optional fourth column names the only hardware tier that loads the line
//! (`base` or `expanded`, issue #54; `trino_core::asset::loads_on`).

use alloc::ffi::CString;
use alloc::string::String;
use alloc::vec::Vec;

use crate::ffi;
use crate::{N3dsAudio, N3dsPlatform, N3dsRenderer, runtime};

pub struct N3dsAssets;

impl N3dsAssets {
    /// RomFS existence probe. Takes a romfs-relative path like `/test_mode`.
    pub fn exists(romfs_relative_path: &str) -> bool {
        let Ok(cpath) = CString::new(romfs_relative_path) else {
            return false;
        };
        unsafe { ffi::trino_file_exists(cpath.as_ptr()) != 0 }
    }

    /// Read `romfs:/index.tsv` and register every asset with the renderer,
    /// audio and platform (blobs) backends.
    pub fn load_all(
        renderer: &mut N3dsRenderer,
        audio: &mut N3dsAudio,
        platform: &mut N3dsPlatform,
    ) {
        if !Self::exists("/index.tsv") {
            runtime::log("TRINO_ASSETS: no index.tsv in RomFS\n");
            return;
        }
        let Some(index) = read_text("romfs:/index.tsv") else {
            runtime::log("TRINO_ASSETS: no index.tsv in RomFS\n");
            return;
        };
        let tier = runtime::hw_tier();
        for line in index.lines() {
            let mut cols = line.split('\t');
            let (Some(id_hex), Some(kind), Some(file)) = (cols.next(), cols.next(), cols.next())
            else {
                continue;
            };
            if !trino_core::asset::loads_on(cols.next(), tier) {
                continue;
            }
            let Ok(id) = u32::from_str_radix(id_hex, 16) else {
                continue;
            };
            let mut path = String::from("romfs:/");
            path.push_str(file);
            let Ok(cpath) = CString::new(path.as_str()) else {
                continue;
            };
            match kind {
                "sprite" => {
                    let ptr = unsafe { ffi::trino_sprite_load(cpath.as_ptr()) };
                    if !ptr.is_null() {
                        renderer.register(id, ptr);
                    }
                }
                "sound" => {
                    let ptr = unsafe { ffi::trino_wav_load(cpath.as_ptr()) };
                    if !ptr.is_null() {
                        audio.register_sound(id, ptr);
                    }
                }
                "music" => {
                    let ptr = unsafe { ffi::trino_wav_load(cpath.as_ptr()) };
                    if !ptr.is_null() {
                        audio.register_music(id, ptr);
                    }
                }
                // The project's mix (`assets/audio.toml`): bus volumes and
                // duck rules, applied once the sounds are in.
                "mixer" => {
                    if let Some(bytes) = read_bytes(&path) {
                        audio.apply_mixer(&bytes);
                    }
                }
                "model" => {
                    if let Some(bytes) = read_bytes(&path) {
                        renderer.register_mesh(id, bytes);
                    }
                }
                "font" => {
                    if let Some(bytes) = read_bytes(&path) {
                        renderer.register_font(id, bytes);
                    }
                }
                "blob" => {
                    if let Some(bytes) = read_bytes(&path) {
                        platform.insert_blob(id, bytes);
                    }
                }
                _ => {}
            }
        }
    }
}

fn read_bytes(romfs_path: &str) -> Option<Vec<u8>> {
    let cpath = CString::new(romfs_path).ok()?;
    let mut size: u32 = 0;
    let ptr = unsafe { ffi::trino_asset_load(cpath.as_ptr(), &mut size) };
    if ptr.is_null() {
        return None;
    }
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, size as usize) };
    let out = Vec::from(bytes);
    unsafe { ffi::trino_free(ptr) };
    Some(out)
}

fn read_text(romfs_path: &str) -> Option<String> {
    String::from_utf8(read_bytes(romfs_path)?).ok()
}
