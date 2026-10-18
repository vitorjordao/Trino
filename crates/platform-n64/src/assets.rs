//! Boot-time asset loading from the DFS.
//!
//! `cargo xtask assets n64` writes `index.tsv` into the filesystem:
//! one `handle-hex \t kind \t filename` line per asset (sounds and music
//! add `\t codec \t bus`). Loading by handle keeps game code identical
//! across platforms (`SpriteId::from_path`). The project's mix
//! (`audio/mixer`, kind `mixer`) is applied to the audio once all loaded.

use alloc::ffi::CString;
use alloc::string::String;
use alloc::vec::Vec;

use crate::ffi;
use crate::{N64Audio, N64Platform, N64Renderer, runtime};

pub struct N64Assets;

impl N64Assets {
    /// DFS existence probe. Takes a DFS-relative path like `/test_mode`.
    pub fn exists(dfs_relative_path: &str) -> bool {
        let Ok(cpath) = CString::new(dfs_relative_path) else {
            return false;
        };
        unsafe { ffi::trino_file_exists(cpath.as_ptr()) != 0 }
    }

    /// Read `rom:/index.tsv` and register every asset with the renderer,
    /// audio and platform (blobs) backends.
    pub fn load_all(renderer: &mut N64Renderer, audio: &mut N64Audio, platform: &mut N64Platform) {
        if !Self::exists("/index.tsv") {
            runtime::log("TRINO_ASSETS: no index.tsv in DFS\n");
            return;
        }
        let Some(index) = read_text("rom:/index.tsv") else {
            runtime::log("TRINO_ASSETS: no index.tsv in DFS\n");
            return;
        };
        let mut mixer = None;
        for line in index.lines() {
            let mut cols = line.split('\t');
            let (Some(id_hex), Some(kind), Some(file)) = (cols.next(), cols.next(), cols.next())
            else {
                continue;
            };
            let Ok(id) = u32::from_str_radix(id_hex, 16) else {
                continue;
            };
            let mut path = String::from("rom:/");
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
                "sound" | "music" => {
                    let codec = cols.next().unwrap_or("vadpcm");
                    let music = kind == "music";
                    let bus =
                        cols.next()
                            .and_then(trino_core::Bus::from_label)
                            .unwrap_or(if music {
                                trino_core::Bus::Music
                            } else {
                                trino_core::Bus::Sfx
                            });
                    // libdragon asserts on an Opus wav64 it can't decode.
                    if codec == "opus" && !audio.enable_opus() {
                        runtime::log(&alloc::format!(
                            "TRINO_ASSETS: {file} is Opus but the ROM has no Opus decoder\n"
                        ));
                        continue;
                    }
                    let ptr = unsafe { ffi::trino_wav_load(cpath.as_ptr()) };
                    if ptr.is_null() {
                        continue;
                    }
                    if music {
                        audio.register_music(id, ptr, bus);
                    } else {
                        audio.register_sound(id, ptr, bus);
                    }
                }
                "mixer" => mixer = read_bytes(&path),
                "model" => {
                    if let Some(bytes) = read_bytes(&path) {
                        let _ = renderer.register_mesh(id, bytes);
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
        if let Some(bytes) = mixer {
            match trino_core::MixerConfig::parse(&bytes) {
                Ok(cfg) => cfg.apply(audio),
                Err(e) => runtime::log(&alloc::format!("TRINO_ASSETS: audio/mixer: {e}\n")),
            }
        }
    }
}

fn read_bytes(dfs_path: &str) -> Option<Vec<u8>> {
    let cpath = CString::new(dfs_path).ok()?;
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

fn read_text(dfs_path: &str) -> Option<String> {
    String::from_utf8(read_bytes(dfs_path)?).ok()
}
