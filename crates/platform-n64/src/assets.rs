//! Asset loading from the DFS: the `boot` group at boot, the others when
//! the game asks (issue #56).
//!
//! `cargo xtask assets n64` writes `index.tsv` into the filesystem:
//! one `handle-hex \t kind \t filename` line per asset. Loading by handle
//! keeps game code identical across platforms (`SpriteId::from_path`).
//! An optional fourth column names the only hardware tier that loads the
//! line (`base` or `expanded`, issue #54): a scene or a particle effect
//! whose tier variants differ is baked twice under one handle, a model
//! listed for `n64_expanded` only once, and each N64 loads its own.
//! Sounds and music add `\t codec \t bus` after an empty tier column
//! (every tier loads them, issue #53). The project's mix (`audio/mixer`,
//! kind `mixer`) is applied to the audio once all loaded.
//!
//! **Groups**: a `group-id \t group \t groups/<name>.tbnd` line names a
//! group's bundle (`trino_core::bundle`): a table of the group's files,
//! each an index line's file and columns. `Platform::load_group` queues
//! it; [`N64Assets::pump`], once per frame before the game's update, reads
//! entries for `Caps::load_budget_us` (libdragon's `asset_load` and
//! `sprite_load` decompress what `mkasset` compressed). An asset two
//! groups hold loads once and is freed with the last of them; what each
//! takes is the heap it grew, counted in `Platform::memory`.

use alloc::collections::BTreeMap;
use alloc::ffi::CString;
use alloc::string::String;
use alloc::vec::Vec;

use trino_core::bundle::{Bundle, EntryKind};
use trino_core::group::HeapMeter;
use trino_core::mem::FrameArena;
use trino_core::{GroupId, GroupTable, MemoryReport};

use crate::ffi;
use crate::{N64Audio, N64Platform, N64Renderer, runtime};

/// Bytes for one bundle entry's locator (`<file>\t<codec>\t<bus>`, about
/// 30 baked).
const LOCATOR_SCRATCH: usize = 128;

pub struct N64Assets;

/// The loader's state, kept by [`N64Platform`].
pub(crate) struct Groups {
    pub(crate) table: GroupTable,
    /// Bundle path per group (`index.tsv`'s `group` lines).
    files: BTreeMap<u32, String>,
    current: Option<Loading>,
    /// What each loaded or loading group holds, and every held asset's
    /// (references, heap bytes).
    held_by: BTreeMap<u32, Vec<(EntryKind, u32)>>,
    held: BTreeMap<(u8, u32), (u32, u32)>,
    /// Blobs of loaded non-boot groups (`Platform::group_blob`).
    pub(crate) blobs: BTreeMap<u32, Vec<u8>>,
    pub(crate) memory: MemoryReport,
    pub(crate) meter: HeapMeter,
}

/// A group being read: its bundle's table and the next entry.
struct Loading {
    group: GroupId,
    table: Vec<u8>,
    next: usize,
    done: u64,
    total: u64,
}

impl Groups {
    pub(crate) fn new() -> Self {
        Groups {
            table: GroupTable::new(),
            files: BTreeMap::new(),
            current: None,
            held_by: BTreeMap::new(),
            held: BTreeMap::new(),
            blobs: BTreeMap::new(),
            memory: MemoryReport::default(),
            meter: HeapMeter::default(),
        }
    }

    /// What `Platform::memory` reports: the heap now and its peak, the
    /// resident assets, the groups.
    pub(crate) fn report(&self, ram_bytes: u32) -> MemoryReport {
        let (used, total) = runtime::heap();
        MemoryReport {
            ram_bytes,
            heap_used: used,
            heap_peak: self.meter.peak().max(used),
            heap_total: total,
            groups_resident: self.table.resident(),
            groups_loading: self.table.loading(),
            ..self.memory
        }
    }

    /// Note the heap's use for the peak (frame ends, load steps).
    pub(crate) fn sample(&mut self) {
        self.meter.sample(runtime::heap().0);
    }
}

/// What one asset load left behind.
enum Loaded {
    /// Registered with a backend; the heap it took.
    Bytes(u32),
    /// A blob (the platform keeps it).
    Blob(Vec<u8>),
    Failed,
}

impl N64Assets {
    /// DFS existence probe. Takes a DFS-relative path like `/test_mode`.
    pub fn exists(dfs_relative_path: &str) -> bool {
        let Ok(cpath) = CString::new(dfs_relative_path) else {
            return false;
        };
        unsafe { ffi::trino_file_exists(cpath.as_ptr()) != 0 }
    }

    /// A DFS file's bytes (`/replay.trec`), decompressed if the bake
    /// compressed it. `None` when it isn't there.
    pub fn read(dfs_relative_path: &str) -> Option<Vec<u8>> {
        let mut path = String::from("rom:");
        path.push_str(dfs_relative_path);
        read_bytes(&path)
    }

    /// Read `rom:/index.tsv` and register every boot asset with the
    /// renderer, audio and platform (blobs) backends, and the groups the
    /// build holds.
    pub fn load_all(renderer: &mut N64Renderer, audio: &mut N64Audio, platform: &mut N64Platform) {
        if !Self::exists("/index.tsv") {
            runtime::log("TRINO_ASSETS: no index.tsv in DFS\n");
            return;
        }
        let Some(index) = read_text("rom:/index.tsv") else {
            runtime::log("TRINO_ASSETS: no index.tsv in DFS\n");
            return;
        };
        let tier = runtime::hw_tier();
        let mut mixer = None;
        // What the engine holds before any asset: `ram - heap_total +
        // before` is the reserve `trino_asset_pipeline::memory` plans with.
        let (before, heap_total) = runtime::heap();
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
            match kind {
                "group" => {
                    if platform.groups.table.add(GroupId(id)) {
                        platform.groups.files.insert(id, String::from(file));
                    }
                    continue;
                }
                "compression" => {
                    // SAFETY: plain C, called before the assets it decodes.
                    unsafe { crate::ffi::trino_asset_compression(id as i32) };
                    continue;
                }
                "mixer" => {
                    let mut path = String::from("rom:/");
                    path.push_str(file);
                    mixer = read_bytes(&path);
                    continue;
                }
                _ => {}
            }
            let Some(kind) = EntryKind::parse(kind) else {
                continue;
            };
            match load_one(kind, id, file, &mut cols, renderer, audio) {
                Loaded::Bytes(b) => platform.groups.memory.add(kind.category(), b),
                Loaded::Blob(bytes) => {
                    platform
                        .groups
                        .memory
                        .add(kind.category(), bytes.len() as u32);
                    platform.insert_blob(id, bytes);
                }
                Loaded::Failed => {}
            }
        }
        if let Some(bytes) = mixer {
            match trino_core::MixerConfig::parse(&bytes) {
                Ok(cfg) => cfg.apply(audio),
                Err(e) => runtime::log(&alloc::format!("TRINO_ASSETS: audio/mixer: {e}\n")),
            }
        }
        platform.groups.sample();
        let after = runtime::heap().0;
        runtime::log(&alloc::format!(
            "TRINO_MEM: ram={} heap_total={heap_total} before_assets={before} boot_assets={}\n",
            runtime::memory_size(),
            after.saturating_sub(before)
        ));
    }

    /// The loader's frame (see the module docs): free what was unloaded,
    /// then read queued groups for `Caps::load_budget_us`. Call once per
    /// frame, before the game's update.
    pub fn pump(renderer: &mut N64Renderer, audio: &mut N64Audio, platform: &mut N64Platform) {
        while let Some(g) = platform.groups.table.take_free() {
            if platform
                .groups
                .current
                .as_ref()
                .is_some_and(|c| c.group == g)
            {
                platform.groups.current = None;
            }
            release(g, renderer, audio, platform);
        }
        if platform.groups.table.loading() == 0 {
            return;
        }
        let budget = trino_core::Renderer::caps(renderer).load_budget_us;
        let tier = runtime::hw_tier();
        let start = runtime::now_us();
        // Each entry's file name, copied out of the bundle table (which
        // `hold` cannot borrow): stack scratch, not a heap string freed
        // between two resident assets (issue #56 phase 4: no small holes
        // in the 4 MB heap a group load leaves behind).
        let mut scratch = FrameArena::<LOCATOR_SCRATCH>::new();
        loop {
            if platform.groups.current.is_none() {
                let Some(g) = platform.groups.table.current_load() else {
                    break;
                };
                match open(&platform.groups, g, tier) {
                    Some(l) => platform.groups.current = Some(l),
                    None => {
                        runtime::log(&alloc::format!(
                            "TRINO_ASSETS: group {:08x}: no bundle\n",
                            g.0
                        ));
                        platform.groups.table.finish(g, false);
                        continue;
                    }
                }
            }
            let Some(cur) = platform.groups.current.as_mut() else {
                break;
            };
            let g = cur.group;
            let b = Bundle::parse(&cur.table).expect("checked by open");
            if cur.next >= b.len() {
                platform.groups.table.finish(g, true);
                platform.groups.current = None;
                continue;
            }
            let e = b.entry(cur.next);
            cur.next += 1;
            if !e.loads_on(tier) {
                continue;
            }
            cur.done += e.read_bytes() as u64;
            let (done, total) = (cur.done, cur.total);
            scratch.reset();
            let heap_copy;
            let locator = match b.path(&e) {
                Some(p) => match scratch.alloc_bytes(p.as_bytes()) {
                    Some(bytes) => core::str::from_utf8(bytes).ok(),
                    // Longer than any baked name: the heap, once.
                    None => {
                        heap_copy = String::from(p);
                        Some(heap_copy.as_str())
                    }
                },
                None => None,
            };
            let ok =
                locator.is_some_and(|loc| hold(g, e.kind, e.id, loc, renderer, audio, platform));
            if !ok {
                runtime::log(&alloc::format!(
                    "TRINO_ASSETS: group {:08x}: entry {:08x} failed\n",
                    g.0,
                    e.id
                ));
                platform.groups.current = None;
                // A failed group frees what it had loaded.
                platform.groups.table.finish(g, false);
                let _ = platform.groups.table.take_free();
                release(g, renderer, audio, platform);
                continue;
            }
            platform.groups.table.set_progress(g, done, total);
            platform.groups.sample();
            if runtime::now_us().wrapping_sub(start) >= budget {
                break;
            }
        }
    }
}

/// Read `g`'s bundle table (`None`: not in this build, or unreadable).
fn open(groups: &Groups, g: GroupId, tier: trino_core::HwTier) -> Option<Loading> {
    let file = groups.files.get(&g.0)?;
    let mut path = String::from("rom:/");
    path.push_str(file);
    let table = read_bytes(&path)?;
    let b = Bundle::parse(&table).ok()?;
    if b.group() != g.0 {
        return None;
    }
    let total = b.read_total(tier);
    Some(Loading {
        group: g,
        table,
        next: 0,
        done: 0,
        total,
    })
}

/// `g` holds the asset `(kind, id)` at `locator` (file, then its columns):
/// loaded now, or one more reference to what another group loaded.
fn hold(
    g: GroupId,
    kind: EntryKind,
    id: u32,
    locator: &str,
    renderer: &mut N64Renderer,
    audio: &mut N64Audio,
    platform: &mut N64Platform,
) -> bool {
    let groups = &mut platform.groups;
    groups.held_by.entry(g.0).or_default().push((kind, id));
    if let Some(h) = groups.held.get_mut(&(kind as u8, id)) {
        h.0 += 1;
        return true;
    }
    let mut cols = locator.split('\t');
    let Some(file) = cols.next() else {
        return false;
    };
    let bytes = match load_one(kind, id, file, &mut cols, renderer, audio) {
        Loaded::Bytes(b) => b,
        Loaded::Blob(bytes) => {
            let n = bytes.len() as u32;
            groups.blobs.insert(id, bytes);
            n
        }
        Loaded::Failed => return false,
    };
    groups.memory.add(kind.category(), bytes);
    groups.held.insert((kind as u8, id), (1, bytes));
    true
}

/// Drop `g`'s references; free what no other group holds.
fn release(
    g: GroupId,
    renderer: &mut N64Renderer,
    audio: &mut N64Audio,
    platform: &mut N64Platform,
) {
    let groups = &mut platform.groups;
    for (kind, id) in groups.held_by.remove(&g.0).unwrap_or_default() {
        let key = (kind as u8, id);
        let Some(h) = groups.held.get_mut(&key) else {
            continue;
        };
        h.0 -= 1;
        if h.0 > 0 {
            continue;
        }
        let bytes = groups.held.remove(&key).map_or(0, |h| h.1);
        groups.memory.sub(kind.category(), bytes);
        match kind {
            EntryKind::Sprite => {
                renderer.unregister(id);
            }
            EntryKind::Sound => {
                audio.unregister_sound(id);
            }
            EntryKind::Music => {
                audio.unregister_music(id);
            }
            EntryKind::Model => {
                renderer.unregister_mesh(id);
            }
            EntryKind::Font => {
                renderer.unregister_font(id);
            }
            _ => {
                groups.blobs.remove(&id);
            }
        }
    }
}

/// Load one asset (`file` in the DFS, then its index columns) into its
/// backend: what boot and the group loader share. The heap it grew is its
/// size (a sprite's texels and header, an open wav64's state, a mesh's
/// TMDL and RSP copies).
fn load_one<'a>(
    kind: EntryKind,
    id: u32,
    file: &str,
    cols: &mut impl Iterator<Item = &'a str>,
    renderer: &mut N64Renderer,
    audio: &mut N64Audio,
) -> Loaded {
    let mut path = String::from("rom:/");
    path.push_str(file);
    let Ok(cpath) = CString::new(path.as_str()) else {
        return Loaded::Failed;
    };
    let before = runtime::heap().0;
    let grew = || runtime::heap().0.saturating_sub(before);
    match kind {
        EntryKind::Sprite => {
            let ptr = unsafe { ffi::trino_sprite_load(cpath.as_ptr()) };
            if ptr.is_null() {
                return Loaded::Failed;
            }
            renderer.register(id, ptr);
            Loaded::Bytes(grew())
        }
        EntryKind::Sound | EntryKind::Music => {
            let codec = cols.next().unwrap_or("vadpcm");
            let music = kind == EntryKind::Music;
            let bus = cols
                .next()
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
                return Loaded::Failed;
            }
            let ptr = unsafe { ffi::trino_wav_load(cpath.as_ptr()) };
            if ptr.is_null() {
                return Loaded::Failed;
            }
            if music {
                audio.register_music(id, ptr, bus);
            } else {
                audio.register_sound(id, ptr, bus);
            }
            Loaded::Bytes(grew())
        }
        EntryKind::Model => match read_bytes(&path).map(|b| renderer.register_mesh(id, b)) {
            Some(true) => Loaded::Bytes(grew()),
            _ => Loaded::Failed,
        },
        EntryKind::Font => match read_bytes(&path) {
            Some(bytes) => {
                renderer.register_font(id, bytes);
                Loaded::Bytes(grew())
            }
            None => Loaded::Failed,
        },
        EntryKind::Blob => match read_bytes(&path) {
            Some(bytes) => Loaded::Blob(bytes),
            None => Loaded::Failed,
        },
        _ => Loaded::Failed,
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
