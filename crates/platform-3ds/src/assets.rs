//! Asset loading from the RomFS: the `boot` group at boot, the others when
//! the game asks (issue #56).
//!
//! `cargo xtask assets 3ds` writes `index.tsv` into the RomFS: one
//! `handle-hex \t kind \t filename` line per asset. Loading by handle keeps
//! game code identical across platforms (`SpriteId::from_path`). An
//! optional fourth column names the only hardware tier that loads the line
//! (`base` or `expanded`, issue #54; `trino_core::asset::loads_on`).
//!
//! **Groups**: a `group-id \t group \t groups/<name>.tbnd` line names a
//! group's bundle (`trino_core::bundle`): textures, meshes, fonts and blobs
//! inline (LZ4), sounds as the `T3AU` files beside it (some stream).
//! `Platform::load_group` queues it; [`N3dsAssets::pump`], once per frame
//! before the game's update, reads entries for `Caps::load_budget_us` of the
//! main thread's time: each inline entry's bytes go to the worker core
//! (`crate::worker`, the second core the audio thread shares) to decode,
//! a few ahead, and the main thread uploads them in order as they come
//! back, never waiting on the worker. An asset two groups hold loads once
//! and is freed with the last of them; what each takes is the heap it grew
//! (malloc and linear memory), counted in `Platform::memory`.
//!
//! **Packed boot**: with `[groups.boot] compression = { 3ds = "lz4" }` the
//! boot group's textures, meshes, fonts and blobs are one bundle too, named
//! by a `group-id \t boot \t groups/boot.tbnd` line; [`N3dsAssets::load_all`]
//! decodes it on the worker (a few entries ahead, as the pump does) and
//! registers each entry like a loose file. Sounds stay loose.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::ffi::CString;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU8, Ordering};

use trino_core::asset::{IndexKind, IndexLine};
use trino_core::bundle::{Bundle, Entry, EntryKind, HEADER_LEN};
use trino_core::group::HeapMeter;
use trino_core::{GroupId, GroupTable, MemoryReport};

use crate::{N3dsAudio, N3dsPlatform, N3dsRenderer, ffi, runtime, worker};

pub struct N3dsAssets;

/// The loader's state, kept by [`N3dsPlatform`].
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
    meter: HeapMeter,
}

/// Entries read ahead of the one being uploaded (their decodes run on the
/// worker meanwhile).
const AHEAD: usize = 4;

/// A group being read: its open bundle, its table, the next entry to read
/// and the ones read but not uploaded yet, in order.
struct Loading {
    group: GroupId,
    file: RomFile,
    table: Vec<u8>,
    entries: usize,
    next: usize,
    inflight: VecDeque<Inflight>,
    done: u64,
    total: u64,
}

/// A read entry: inline ones decoding on the worker, external ones (sounds)
/// loaded from their file when their turn comes.
struct Inflight {
    entry: Entry,
    source: Source,
}

enum Source {
    Decode(Decode),
    File(String),
}

impl Inflight {
    fn ready(&self) -> bool {
        match &self.source {
            Source::Decode(d) => d.state() != RUNNING,
            Source::File(_) => true,
        }
    }
}

const RUNNING: u8 = 0;
const DECODED: u8 = 1;
const FAILED: u8 = 2;

/// One inline entry on the worker: its stored bytes in, the asset out.
struct DecodeJob {
    entry: Entry,
    stored: Vec<u8>,
    /// Written by the worker only while `state` is [`RUNNING`].
    out: UnsafeCell<Vec<u8>>,
    state: AtomicU8,
}

/// The main thread's handle on a [`DecodeJob`]. Dropping it (a cancelled
/// load) waits for the worker to be done with it.
struct Decode(*mut DecodeJob);

impl Decode {
    fn start(entry: Entry, stored: Vec<u8>) -> Decode {
        let job = Box::into_raw(Box::new(DecodeJob {
            entry,
            stored,
            out: UnsafeCell::new(vec![0; entry.raw as usize]),
            state: AtomicU8::new(RUNNING),
        }));
        // Runs inline when there is no worker or its ring is full.
        worker::submit(decode_job, job as usize as u32);
        Decode(job)
    }

    fn state(&self) -> u8 {
        // SAFETY: the job lives until this handle goes; `state` is atomic.
        unsafe { (*self.0).state.load(Ordering::Acquire) }
    }

    /// The decoded bytes (`None`: corrupt). Only once not [`RUNNING`].
    fn take(self) -> Option<Vec<u8>> {
        debug_assert_ne!(self.state(), RUNNING);
        let ptr = self.0;
        core::mem::forget(self);
        // SAFETY: the worker stored the final state last and is done.
        let job = unsafe { Box::from_raw(ptr) };
        (job.state.load(Ordering::Acquire) == DECODED).then(|| job.out.into_inner())
    }
}

impl Drop for Decode {
    fn drop(&mut self) {
        while self.state() == RUNNING {
            worker::wait();
        }
        // SAFETY: as in `take`.
        drop(unsafe { Box::from_raw(self.0) });
    }
}

/// The worker's half: decode one entry and check its hash.
extern "C" fn decode_job(arg: u32) {
    // SAFETY: `Decode::start`'s box, alive until the state below is read;
    // `out` is the worker's until then.
    let job = unsafe { &*(arg as usize as *const DecodeJob) };
    let out = unsafe { &mut *job.out.get() };
    let ok = trino_core::bundle::decode(&job.entry, &job.stored, out).is_ok()
        && (job.entry.hash == 0 || trino_core::bundle::hash(out) == job.entry.hash);
    job.state
        .store(if ok { DECODED } else { FAILED }, Ordering::Release);
}

/// An open RomFS file, read in ranges.
struct RomFile(*mut c_void);

impl RomFile {
    fn open(path: &str) -> Option<RomFile> {
        let c = CString::new(path).ok()?;
        let f = unsafe { ffi::trino_file_open(c.as_ptr()) };
        (!f.is_null()).then_some(RomFile(f))
    }

    fn read_at(&self, offset: u64, len: usize) -> Option<Vec<u8>> {
        let offset = u32::try_from(offset).ok()?;
        let mut buf = vec![0u8; len];
        let n = unsafe { ffi::trino_file_read_at(self.0, offset, buf.as_mut_ptr(), len as u32) };
        (n as usize == len).then_some(buf)
    }
}

impl Drop for RomFile {
    fn drop(&mut self) {
        unsafe { ffi::trino_file_close(self.0) }
    }
}

/// The heap in use now: malloc and the linear heap.
pub(crate) fn heap_used() -> u32 {
    unsafe { ffi::trino_heap_stat(0) }
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
        let used = heap_used();
        MemoryReport {
            ram_bytes,
            heap_used: used,
            heap_peak: self.meter.peak().max(used),
            heap_total: unsafe { ffi::trino_heap_stat(1) },
            groups_resident: self.table.resident(),
            groups_loading: self.table.loading(),
            ..self.memory
        }
    }

    /// Note the heap's use for the peak (frame ends, load steps).
    pub(crate) fn sample(&mut self) {
        self.meter.sample(heap_used());
    }
}

/// Where an asset's bytes come from.
enum Bytes<'a> {
    /// A RomFS file (`romfs:/…`).
    File(&'a str),
    /// Decoded from a bundle.
    Owned(Vec<u8>),
}

/// What one asset load left behind.
enum Loaded {
    /// Registered with a backend; the heap it took.
    Bytes(u32),
    /// A blob (its owner keeps it).
    Blob(Vec<u8>),
    Failed,
}

impl N3dsAssets {
    /// RomFS existence probe. Takes a romfs-relative path like `/test_mode`.
    pub fn exists(romfs_relative_path: &str) -> bool {
        let Ok(cpath) = CString::new(romfs_relative_path) else {
            return false;
        };
        unsafe { ffi::trino_file_exists(cpath.as_ptr()) != 0 }
    }

    /// A RomFS file's bytes (`/replay.trec`). `None` when it isn't there.
    pub fn read(romfs_relative_path: &str) -> Option<Vec<u8>> {
        let mut path = alloc::string::String::from("romfs:");
        path.push_str(romfs_relative_path);
        read_bytes(&path)
    }

    /// Read `romfs:/index.tsv` and register every boot asset with the
    /// renderer, audio and platform (blobs) backends, and the groups the
    /// build holds.
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
        let mut mixer = None;
        // What the engine holds before any asset (the planner's reserve).
        let before = heap_used();
        for line in index.lines() {
            // `trino_core::asset::IndexLine`: the parse the N64 shares and
            // the `asset_index` fuzz target covers on the host (#121).
            let Some(line) = IndexLine::parse(line) else {
                continue;
            };
            if !line.loads_on(tier) {
                continue;
            }
            let id = line.id;
            let path = romfs(line.file);
            let kind = match line.kind {
                IndexKind::Group => {
                    if platform.groups.table.add(GroupId(id)) {
                        platform.groups.files.insert(id, path);
                    }
                    continue;
                }
                IndexKind::Boot => {
                    load_boot_bundle(&path, tier, renderer, audio, platform);
                    continue;
                }
                // The project's mix (`assets/audio.toml`): bus volumes and
                // duck rules, applied once the sounds are in.
                IndexKind::Mixer => {
                    mixer = read_bytes(&path);
                    continue;
                }
                IndexKind::Entry(kind) => kind,
                IndexKind::Compression | IndexKind::Unknown(_) => continue,
            };
            let loaded = load_one(kind, id, Bytes::File(&path), renderer, audio);
            boot_loaded(kind, id, loaded, platform);
        }
        if let Some(bytes) = mixer {
            audio.apply_mixer(&bytes);
        }
        platform.groups.sample();
        let total = unsafe { ffi::trino_heap_stat(1) };
        runtime::log(&alloc::format!(
            "TRINO_MEM: ram={} heap_total={total} before_assets={before} boot_assets={}\n",
            runtime::hw().caps().ram_bytes,
            heap_used().saturating_sub(before)
        ));
    }

    /// The loader's frame (see the module docs): free what was unloaded,
    /// then read queued groups for `Caps::load_budget_us`. Call once per
    /// frame, before the game's update.
    pub fn pump(renderer: &mut N3dsRenderer, audio: &mut N3dsAudio, platform: &mut N3dsPlatform) {
        while let Some(g) = platform.groups.table.take_free() {
            if platform
                .groups
                .current
                .as_ref()
                .is_some_and(|c| c.group == g)
            {
                // Waits for its decodes still on the worker.
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
            if cur.inflight.front().is_some_and(Inflight::ready) {
                // Upload the oldest entry the worker is done with.
                let item = cur.inflight.pop_front().expect("front");
                cur.done += item.entry.read_bytes() as u64;
                let (done, total) = (cur.done, cur.total);
                if !hold(g, item, renderer, audio, platform) {
                    fail(g, renderer, audio, platform);
                    continue;
                }
                platform.groups.table.set_progress(g, done, total);
                platform.groups.sample();
            } else if cur.inflight.len() < AHEAD && cur.next < cur.entries {
                // Read the next entry; an inline one starts decoding.
                let b = Bundle::parse(&cur.table).expect("checked by open");
                let e = b.entry(cur.next);
                cur.next += 1;
                if !e.loads_on(tier) {
                    continue;
                }
                let source = match (b.path(&e), b.payload_range(&e)) {
                    (Some(file), _) => Some(Source::File(romfs(file))),
                    (None, Some(r)) => cur
                        .file
                        .read_at(r.start, (r.end - r.start) as usize)
                        .map(|stored| Source::Decode(Decode::start(e, stored))),
                    (None, None) => None,
                };
                match source {
                    Some(source) => cur.inflight.push_back(Inflight { entry: e, source }),
                    None => {
                        fail(g, renderer, audio, platform);
                        continue;
                    }
                }
            } else if cur.inflight.is_empty() {
                platform.groups.table.finish(g, true);
                platform.groups.current = None;
                continue;
            } else {
                // The worker is on the oldest entry: the game's frame goes on.
                break;
            }
            if runtime::now_us().wrapping_sub(start) >= budget {
                break;
            }
        }
    }
}

/// A boot asset in: counted in boot's memory, a blob handed to the platform.
fn boot_loaded(kind: EntryKind, id: u32, loaded: Loaded, platform: &mut N3dsPlatform) {
    match loaded {
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

/// The packed boot bundle (see the module docs): read whole, its entries
/// decoded [`AHEAD`] at a time on the worker and registered in order.
fn load_boot_bundle(
    path: &str,
    tier: trino_core::HwTier,
    renderer: &mut N3dsRenderer,
    audio: &mut N3dsAudio,
    platform: &mut N3dsPlatform,
) {
    let Some(bytes) = read_bytes(path) else {
        runtime::log("TRINO_ASSETS: boot bundle unreadable\n");
        return;
    };
    let bundle = match Bundle::parse(&bytes) {
        Ok(b) => b,
        Err(e) => {
            runtime::log(&alloc::format!(
                "TRINO_ASSETS: boot bundle: {}\n",
                e.label()
            ));
            return;
        }
    };
    let mut entries = bundle
        .entries()
        .filter(|e| !e.is_external() && e.loads_on(tier));
    let mut inflight: VecDeque<(Entry, Decode)> = VecDeque::new();
    loop {
        while inflight.len() < AHEAD {
            let Some(e) = entries.next() else { break };
            let Some(stored) = bundle.stored(&e) else {
                continue;
            };
            inflight.push_back((e, Decode::start(e, Vec::from(stored))));
        }
        let Some((e, job)) = inflight.pop_front() else {
            break;
        };
        while job.state() == RUNNING {
            worker::wait();
        }
        let Some(raw) = job.take() else {
            runtime::log(&alloc::format!(
                "TRINO_ASSETS: boot bundle: {:08x} corrupt\n",
                e.id
            ));
            continue;
        };
        let loaded = load_one(e.kind, e.id, Bytes::Owned(raw), renderer, audio);
        boot_loaded(e.kind, e.id, loaded, platform);
    }
}

/// `g` failed (an entry unreadable or corrupt): it frees what it had loaded.
fn fail(
    g: GroupId,
    renderer: &mut N3dsRenderer,
    audio: &mut N3dsAudio,
    platform: &mut N3dsPlatform,
) {
    runtime::log(&alloc::format!(
        "TRINO_ASSETS: group {:08x}: an entry failed to load\n",
        g.0
    ));
    platform.groups.current = None;
    platform.groups.table.finish(g, false);
    let _ = platform.groups.table.take_free();
    release(g, renderer, audio, platform);
}

/// Open `g`'s bundle and read its table (`None`: not in this build, or
/// unreadable).
fn open(groups: &Groups, g: GroupId, tier: trino_core::HwTier) -> Option<Loading> {
    let file = RomFile::open(groups.files.get(&g.0)?)?;
    let header = file.read_at(0, HEADER_LEN)?;
    let table = file.read_at(0, Bundle::prefix_len(&header).ok()?)?;
    let b = Bundle::parse(&table).ok()?;
    if b.group() != g.0 {
        return None;
    }
    let (entries, total) = (b.len(), b.read_total(tier));
    Some(Loading {
        group: g,
        file,
        table,
        entries,
        next: 0,
        inflight: VecDeque::new(),
        done: 0,
        total,
    })
}

/// `g` holds `item`'s asset: loaded now, or one more reference to what
/// another group loaded.
fn hold(
    g: GroupId,
    item: Inflight,
    renderer: &mut N3dsRenderer,
    audio: &mut N3dsAudio,
    platform: &mut N3dsPlatform,
) -> bool {
    let (kind, id) = (item.entry.kind, item.entry.id);
    let groups = &mut platform.groups;
    groups.held_by.entry(g.0).or_default().push((kind, id));
    if let Some(h) = groups.held.get_mut(&(kind as u8, id)) {
        h.0 += 1;
        return true;
    }
    let loaded = match item.source {
        Source::Decode(d) => match d.take() {
            Some(bytes) => load_one(kind, id, Bytes::Owned(bytes), renderer, audio),
            None => Loaded::Failed,
        },
        Source::File(path) => load_one(kind, id, Bytes::File(&path), renderer, audio),
    };
    let bytes = match loaded {
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
    renderer: &mut N3dsRenderer,
    audio: &mut N3dsAudio,
    platform: &mut N3dsPlatform,
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

/// Load one asset into its backend: what boot and the group loader share.
/// The heap it grew is its size (a texture's linear copy, a sound's
/// samples, a mesh's TMDL and GPU copy).
fn load_one(
    kind: EntryKind,
    id: u32,
    bytes: Bytes,
    renderer: &mut N3dsRenderer,
    audio: &mut N3dsAudio,
) -> Loaded {
    let before = heap_used();
    // Decoded bytes were on the heap before `before`: what a backend keeps
    // of them counts on top.
    let grew = |kept: usize| (heap_used() + kept as u32).saturating_sub(before);
    let owned = |bytes: Bytes| match bytes {
        Bytes::File(path) => read_bytes(path).map(|b| (b, 0)),
        Bytes::Owned(b) => {
            let n = b.len();
            Some((b, n))
        }
    };
    match kind {
        EntryKind::Sprite => {
            let ptr = match bytes {
                Bytes::File(path) => match CString::new(path) {
                    Ok(c) => unsafe { ffi::trino_sprite_load(c.as_ptr()) },
                    Err(_) => return Loaded::Failed,
                },
                Bytes::Owned(t3x) => unsafe {
                    ffi::trino_sprite_load_mem(t3x.as_ptr(), t3x.len() as u32)
                },
            };
            if ptr.is_null() {
                return Loaded::Failed;
            }
            renderer.register(id, ptr);
            Loaded::Bytes(grew(0))
        }
        EntryKind::Sound | EntryKind::Music => {
            let Bytes::File(path) = bytes else {
                return Loaded::Failed;
            };
            let Ok(c) = CString::new(path) else {
                return Loaded::Failed;
            };
            let ptr = unsafe { ffi::trino_wav_load(c.as_ptr()) };
            if ptr.is_null() {
                return Loaded::Failed;
            }
            if kind == EntryKind::Music {
                audio.register_music(id, ptr);
            } else {
                audio.register_sound(id, ptr);
            }
            Loaded::Bytes(grew(0))
        }
        EntryKind::Model => match owned(bytes) {
            Some((tmdl, kept)) => {
                renderer.register_mesh(id, tmdl);
                Loaded::Bytes(grew(kept))
            }
            None => Loaded::Failed,
        },
        EntryKind::Font => match owned(bytes) {
            Some((table, kept)) => {
                renderer.register_font(id, table);
                Loaded::Bytes(grew(kept))
            }
            None => Loaded::Failed,
        },
        _ => match owned(bytes) {
            Some((blob, _)) => Loaded::Blob(blob),
            None => Loaded::Failed,
        },
    }
}

/// `romfs:/` + a RomFS-relative file.
fn romfs(file: &str) -> String {
    let mut path = String::from("romfs:/");
    path.push_str(file);
    path
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

pub(crate) fn read_text(romfs_path: &str) -> Option<String> {
    String::from_utf8(read_bytes(romfs_path)?).ok()
}
