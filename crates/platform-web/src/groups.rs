//! Asset groups on the Web (issue #56): the pack holds the `boot` group and
//! names the others ([`crate::pack::Kind::Group`]); each of those is a
//! bundle beside the pack ([`crate::pack::bundle_url`]).
//!
//! `Platform::load_group` starts the bundle's download (once: the page
//! keeps the downloaded bytes, so a level loaded again needs no network);
//! [`pump`], once per frame before the game's update, decodes its entries
//! (LZ4, `trino_core::bundle`) and uploads them for `Caps::load_budget_us`,
//! the progress counting the bundle's bytes. An asset two groups hold
//! loads once and goes with the last of them; what each takes is what its
//! backend holds (`PcRenderer::sprite_bytes`, `WebAudio::clip_bytes`, …),
//! counted in `Platform::memory`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use trino_core::bundle::{Bundle, EntryKind as K};
use trino_core::group::HeapMeter;
use trino_core::mem::HeapStats;
use trino_core::{
    FontId, GroupId, GroupTable, HwTier, MemoryReport, ModelId, MusicId, Renderer, SoundId,
    SpriteId,
};
use trino_render_wgpu::PcRenderer;

use crate::assets::TextureTarget;
use crate::{WebAudio, WebPlatform};

/// A download in flight: its bytes (or error) once the browser has them.
type Slot = Rc<RefCell<Option<Result<Vec<u8>, String>>>>;

enum Download {
    Pending(Slot),
    Ready(Rc<[u8]>),
    Failed(String),
}

/// The groups' state, kept by [`WebPlatform`].
pub(crate) struct Groups {
    pub(crate) table: GroupTable,
    names: HashMap<u32, String>,
    /// The pack the groups came with: their bundles sit beside it.
    pack_url: Option<String>,
    downloads: HashMap<u32, Download>,
    current: Option<Cursor>,
    /// What each loaded or loading group holds, and every held asset's
    /// (references, bytes).
    held_by: HashMap<u32, Vec<(K, u32)>>,
    held: HashMap<(u8, u32), (u32, u32)>,
    /// Blobs of loaded non-boot groups (`Platform::group_blob`).
    pub(crate) blobs: HashMap<u32, Vec<u8>>,
    pub(crate) memory: MemoryReport,
    /// The page's allocator, when the app counts it (`trino_core::mem::Counting`).
    pub(crate) heap: Option<&'static dyn HeapStats>,
    meter: HeapMeter,
}

/// A group being decoded: its bundle and the next entry.
struct Cursor {
    group: GroupId,
    bytes: Rc<[u8]>,
    next: usize,
    done: u64,
    total: u64,
}

/// One decoded asset, a sprite with its KTX2 file when the bake made one.
struct Asset {
    kind: K,
    id: u32,
    bytes: Vec<u8>,
    ktx2: Option<Vec<u8>>,
}

impl Groups {
    pub(crate) fn new(ram_bytes: u32) -> Self {
        Groups {
            table: GroupTable::new(),
            names: HashMap::new(),
            pack_url: None,
            downloads: HashMap::new(),
            current: None,
            held_by: HashMap::new(),
            held: HashMap::new(),
            blobs: HashMap::new(),
            memory: MemoryReport {
                ram_bytes,
                ..MemoryReport::default()
            },
            heap: None,
            meter: HeapMeter::default(),
        }
    }

    /// A group the pack at `pack_url` names (its `Kind::Group` entry).
    pub(crate) fn add(&mut self, id: u32, name: &str, pack_url: Option<&str>) {
        if self.table.add(GroupId(id)) {
            self.names.insert(id, name.to_string());
        }
        if let Some(url) = pack_url {
            self.pack_url = Some(url.to_string());
        }
    }

    pub(crate) fn set_pack_url(&mut self, url: &str) {
        self.pack_url = Some(url.to_string());
    }

    /// Start `g`'s download unless it has one.
    pub(crate) fn fetch(&mut self, g: GroupId) {
        if self.downloads.contains_key(&g.0) {
            return;
        }
        let (Some(name), Some(pack)) = (self.names.get(&g.0), self.pack_url.as_deref()) else {
            self.downloads
                .insert(g.0, Download::Failed("no pack URL for its bundle".into()));
            return;
        };
        let url = crate::pack::bundle_url(pack, name);
        let slot: Slot = Rc::new(RefCell::new(None));
        let fill = slot.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let r = crate::runtime::fetch(&url).await;
            *fill.borrow_mut() = Some(r);
        });
        self.downloads.insert(g.0, Download::Pending(slot));
    }

    /// `g`'s bundle once downloaded (`None`: still on its way).
    fn download(&mut self, g: GroupId) -> Option<Result<Rc<[u8]>, String>> {
        let d = self.downloads.get_mut(&g.0)?;
        if let Download::Pending(slot) = d {
            let done = slot.borrow_mut().take()?;
            *d = match done {
                Ok(bytes) => Download::Ready(bytes.into()),
                Err(e) => Download::Failed(e),
            };
        }
        match d {
            Download::Ready(b) => Some(Ok(b.clone())),
            Download::Failed(e) => Some(Err(e.clone())),
            Download::Pending(_) => None,
        }
    }

    /// What `Platform::memory` reports: the heap (when counted), the
    /// resident assets, the groups.
    pub(crate) fn report(&self) -> MemoryReport {
        let (used, peak) = self.heap.map_or((0, 0), |h| (h.used(), h.peak()));
        MemoryReport {
            heap_used: used as u32,
            heap_peak: (peak as u32).max(self.meter.peak()),
            groups_resident: self.table.resident(),
            groups_loading: self.table.loading(),
            ..self.memory
        }
    }

    pub(crate) fn sample(&mut self) {
        if let Some(h) = self.heap {
            self.meter.sample(h.used() as u32);
        }
    }
}

/// The loader's frame (see the module docs): free what was unloaded, then
/// decode the downloaded groups for `Caps::load_budget_us`.
pub(crate) fn pump(
    renderer: &mut PcRenderer,
    audio: &mut WebAudio,
    platform: &mut WebPlatform,
    target: TextureTarget,
) {
    let tier = trino_core::Platform::hw_tier(platform);
    let now = || {
        web_sys::window()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.now() * 1000.0)
    };
    let groups = &mut platform.groups;
    while let Some(g) = groups.table.take_free() {
        if groups.current.as_ref().is_some_and(|c| c.group == g) {
            groups.current = None;
        }
        release(g, renderer, audio, groups);
    }
    if groups.table.loading() == 0 {
        return;
    }
    let budget = renderer.caps().load_budget_us as f64;
    let start = now();
    loop {
        if groups.current.is_none() {
            let Some(g) = groups.table.current_load() else {
                break;
            };
            let opened = match groups.download(g) {
                None => break,
                Some(r) => r.and_then(|bytes| {
                    let b = Bundle::parse(&bytes).map_err(|e| e.label().to_string())?;
                    if b.group() != g.0 {
                        return Err("the bundle is another group's".into());
                    }
                    let total = b.read_total(tier);
                    Ok(Cursor {
                        group: g,
                        bytes,
                        next: 0,
                        done: 0,
                        total,
                    })
                }),
            };
            match opened {
                Ok(c) => groups.current = Some(c),
                Err(e) => {
                    crate::js::log(&format!("TRINO_ASSETS: group {:08x}: {e}", g.0));
                    groups.table.finish(g, false);
                    continue;
                }
            }
        }
        let Some(cur) = groups.current.as_mut() else {
            break;
        };
        let g = cur.group;
        match step(cur, tier) {
            Ok(Some(asset)) => {
                let (done, total) = (cur.done, cur.total);
                if let Err(e) = hold(g, asset, renderer, audio, groups, target) {
                    fail(g, &e, renderer, audio, groups);
                    continue;
                }
                groups.table.set_progress(g, done, total);
                groups.sample();
            }
            Ok(None) => {
                groups.table.finish(g, true);
                groups.current = None;
                continue;
            }
            Err(e) => {
                fail(g, &e, renderer, audio, groups);
                continue;
            }
        }
        if now() - start >= budget {
            break;
        }
    }
}

/// `g` failed: it frees what it had loaded.
fn fail(
    g: GroupId,
    why: &str,
    renderer: &mut PcRenderer,
    audio: &mut WebAudio,
    groups: &mut Groups,
) {
    crate::js::log(&format!("TRINO_ASSETS: group {:08x}: {why}", g.0));
    groups.current = None;
    groups.table.finish(g, false);
    let _ = groups.table.take_free();
    release(g, renderer, audio, groups);
}

/// Decode the cursor's next asset (a sprite with its variant entries);
/// `None` when the bundle is through.
fn step(cur: &mut Cursor, tier: HwTier) -> Result<Option<Asset>, String> {
    let b = Bundle::parse(&cur.bytes).map_err(|e| e.label().to_string())?;
    let decode = |e: &trino_core::bundle::Entry| -> Result<Vec<u8>, String> {
        let stored = b.stored(e).ok_or("an external entry")?;
        let mut raw = vec![0u8; e.raw as usize];
        trino_core::bundle::decode(e, stored, &mut raw)
            .map_err(|err| format!("entry {:08x}: {}", e.id, err.label()))?;
        if trino_core::bundle::hash(&raw) != e.hash {
            return Err(format!("entry {:08x}: hash mismatch", e.id));
        }
        Ok(raw)
    };
    while cur.next < b.len() {
        let first = b.entry(cur.next);
        cur.next += 1;
        if !first.loads_on(tier) {
            continue;
        }
        let bytes = decode(&first)?;
        cur.done += first.read_bytes() as u64;
        let mut ktx2 = None;
        while cur.next < b.len() {
            let e = b.entry(cur.next);
            if !matches!(
                e.kind,
                K::SpritePreview3ds | K::SpriteBc | K::SpriteKtx2 | K::ModelGraded
            ) {
                break;
            }
            cur.next += 1;
            if e.id != first.id || !e.loads_on(tier) {
                continue;
            }
            cur.done += e.read_bytes() as u64;
            if e.kind == K::SpriteKtx2 {
                ktx2 = Some(decode(&e)?);
            }
        }
        return Ok(Some(Asset {
            kind: first.kind,
            id: first.id,
            bytes,
            ktx2,
        }));
    }
    Ok(None)
}

/// `g` holds `asset`: uploaded now, or one more reference to what another
/// group uploaded.
fn hold(
    g: GroupId,
    asset: Asset,
    renderer: &mut PcRenderer,
    audio: &mut WebAudio,
    groups: &mut Groups,
    target: TextureTarget,
) -> Result<(), String> {
    let (kind, id) = (asset.kind, asset.id);
    groups.held_by.entry(g.0).or_default().push((kind, id));
    if let Some(h) = groups.held.get_mut(&(kind as u8, id)) {
        h.0 += 1;
        return Ok(());
    }
    let bytes = match kind {
        K::Sprite => {
            match &asset.ktx2 {
                Some(k) => {
                    crate::assets::upload_ktx2(renderer, SpriteId(id), k, target)?;
                }
                None => {
                    let (w, h, px) = crate::pack::read_sprite(&asset.bytes)?;
                    renderer.upload_sprite(SpriteId(id), w, h, px);
                }
            }
            renderer.sprite_bytes(SpriteId(id)) as u32
        }
        K::Sound => {
            audio.upload_sound_blob(SoundId(id), &asset.bytes)?;
            audio.clip_bytes(id, false) as u32
        }
        K::Music => {
            audio.upload_music_blob(MusicId(id), &asset.bytes)?;
            audio.clip_bytes(id, true) as u32
        }
        K::Model => {
            renderer.upload_mesh(ModelId(id), asset.bytes);
            renderer.mesh_bytes(ModelId(id)) as u32
        }
        K::Font => {
            renderer.upload_font(FontId(id), asset.bytes);
            renderer.font_bytes(FontId(id)) as u32
        }
        _ => {
            let n = asset.bytes.len() as u32;
            renderer.take_lut_blob(id, &asset.bytes);
            groups.blobs.insert(id, asset.bytes);
            n
        }
    };
    groups.memory.add(kind.category(), bytes);
    groups.held.insert((kind as u8, id), (1, bytes));
    Ok(())
}

/// Drop `g`'s references; free what no other group holds.
fn release(g: GroupId, renderer: &mut PcRenderer, audio: &mut WebAudio, groups: &mut Groups) {
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
            K::Sprite => {
                renderer.remove_sprite(SpriteId(id));
            }
            K::Sound => {
                audio.remove_sound(SoundId(id));
            }
            K::Music => {
                audio.remove_music(MusicId(id));
            }
            K::Model => {
                renderer.remove_mesh(ModelId(id));
            }
            K::Font => {
                renderer.remove_font(FontId(id));
            }
            _ => {
                renderer.remove_lut(trino_core::LutId(id));
                groups.blobs.remove(&id);
            }
        }
    }
}
