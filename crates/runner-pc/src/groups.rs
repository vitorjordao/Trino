//! Asset groups on the PC (issue #56, ADR 0022): the host side of
//! `Platform::load_group`. The game asks [`PcPlatform`]; once per frame,
//! before the game's update, the host calls [`GroupLoader::pump`], which
//! frees what was unloaded and reads queued bundles (`groups/<name>.tbnd`
//! in the bake) for a time slice (`Caps::load_budget_us`), uploading each
//! asset as it is decoded.
//!
//! An asset in two loaded groups is uploaded once and freed with the last
//! of them. Strict mode (`TRINO_STRICT=1` on a console profile) checks every
//! load against the console's RAM ([`trino_asset_pipeline::memory`]) and
//! panics with the groups, the overrun and the biggest assets.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use trino_asset_pipeline::memory::MemoryPlan;
use trino_asset_pipeline::{BundleLoad, LoadedAssets};
use trino_core::bundle::MemoryCategory;
use trino_core::{BlobId, FontId, GroupId, HwTier, ModelId, MusicId, SoundId, SpriteId};
use trino_platform_pc::{PcAudio, PcPlatform, PcRenderer};

/// What a group holds, by backend.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Kind {
    Sprite,
    Sound,
    Music,
    Model,
    Font,
    Blob,
}

impl Kind {
    fn category(self) -> MemoryCategory {
        match self {
            Kind::Sprite => MemoryCategory::Textures,
            Kind::Sound | Kind::Music => MemoryCategory::Audio,
            Kind::Model => MemoryCategory::Meshes,
            Kind::Font | Kind::Blob => MemoryCategory::Data,
        }
    }
}

/// An uploaded asset: how many loaded groups hold it, and its bytes.
struct Held {
    refs: u32,
    bytes: u32,
}

/// The PC's group loader (see the module docs).
pub struct GroupLoader {
    dir: PathBuf,
    tier: HwTier,
    budget: Duration,
    /// Whose sprite pixels to upload ([`crate::sprite_platform`]).
    target: trino_asset_pipeline::Platform,
    /// Group handle → name (bundle file, messages).
    names: HashMap<GroupId, String>,
    current: Option<(GroupId, BundleLoad)>,
    /// Workers that decode a bundle's entries (the renderer's precompile
    /// budget: `TRINO_THREADS`, else all CPUs but one).
    threads: u32,
    /// What each loaded or loading group uploaded.
    held_by: HashMap<GroupId, Vec<(Kind, u32)>>,
    held: HashMap<(Kind, u32), Held>,
    plan: Option<MemoryPlan>,
}

impl GroupLoader {
    /// The groups baked into `baked_dir` (`groups/*.tbnd`), declared to
    /// `platform` so `load_group` knows them.
    pub fn new(baked_dir: &Path, renderer: &PcRenderer, platform: &mut PcPlatform) -> Self {
        let mut names = HashMap::new();
        if let Ok(dir) = std::fs::read_dir(baked_dir.join("groups")) {
            for e in dir.flatten() {
                let path = e.path();
                if path.extension().is_some_and(|x| x == "tbnd")
                    && let Some(name) = path.file_stem().and_then(|s| s.to_str())
                {
                    let id = GroupId::from_name(name);
                    if platform.groups_mut().add(id) {
                        names.insert(id, name.to_string());
                    }
                }
            }
        }
        let caps = trino_core::Renderer::caps(renderer);
        GroupLoader {
            dir: baked_dir.to_path_buf(),
            tier: renderer.hw_tier(),
            budget: Duration::from_micros(caps.load_budget_us as u64),
            target: crate::sprite_platform(renderer),
            names,
            current: None,
            threads: renderer.precompile_threads(),
            held_by: HashMap::new(),
            held: HashMap::new(),
            plan: None,
        }
    }

    /// Check every load against this console plan (strict mode).
    pub fn with_plan(mut self, plan: MemoryPlan) -> Self {
        self.plan = Some(plan);
        self
    }

    /// The strict-mode plan for `renderer`'s profile when `TRINO_STRICT`
    /// is on and the profile is a console's, built from `assets_root`.
    pub fn strict_plan(assets_root: &Path, renderer: &PcRenderer) -> Option<MemoryPlan> {
        let strict = matches!(std::env::var("TRINO_STRICT").as_deref(), Ok("1" | "true"));
        let platform = match renderer.profile() {
            trino_platform_pc::SimProfile::N64 => trino_asset_pipeline::Platform::N64,
            trino_platform_pc::SimProfile::N3ds => trino_asset_pipeline::Platform::N3ds,
            _ => return None,
        };
        if !strict {
            return None;
        }
        Some(
            trino_asset_pipeline::memory::plan(assets_root, platform, renderer.hw_tier())
                .unwrap_or_else(|e| panic!("strict mode: memory plan: {e}")),
        )
    }

    /// The time slice a pump reads for (`Caps::load_budget_us`).
    pub fn set_budget(&mut self, budget: Duration) {
        self.budget = budget;
    }

    /// The names of the groups this bake holds.
    pub fn group_names(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.names.values().map(String::as_str).collect();
        v.sort();
        v
    }

    /// Boot's assets fit the strict plan (call once after the boot upload).
    pub fn check_boot(&self) {
        if let Some(plan) = &self.plan
            && let Err(e) = plan.check(&[])
        {
            panic!("strict mode: {e}");
        }
    }

    /// Free what was unloaded, then read queued groups for one time slice.
    pub fn pump(
        &mut self,
        renderer: &mut PcRenderer,
        audio: &mut PcAudio,
        platform: &mut PcPlatform,
    ) {
        while let Some(g) = platform.groups_mut().take_free() {
            if self.current.as_ref().is_some_and(|(c, _)| *c == g) {
                self.current = None;
            }
            self.release(g, renderer, audio, platform);
        }
        let start = Instant::now();
        loop {
            if self.current.is_none() {
                let Some(g) = platform.groups_mut().current_load() else {
                    break;
                };
                match self.open(g, platform) {
                    Ok(b) => self.current = Some((g, b)),
                    Err(e) => {
                        eprintln!("[trino] asset group: {e}");
                        platform.groups_mut().finish(g, false);
                        continue;
                    }
                }
            }
            let (g, bundle) = self.current.as_mut().expect("opened above");
            let g = *g;
            if bundle.is_done() {
                platform.groups_mut().finish(g, true);
                self.current = None;
                continue;
            }
            let mut out = LoadedAssets::default();
            // The runtime thread budget decodes several entries at once (#101).
            let step = bundle.step_many(self.threads, &mut out);
            let (done, total) = bundle.progress();
            if let Err(e) = step.and_then(|()| self.install(g, out, renderer, audio, platform)) {
                eprintln!("[trino] asset group `{}`: {e}", self.name(g));
                self.current = None;
                // A failed group frees what it had uploaded.
                platform.groups_mut().finish(g, false);
                let _ = platform.groups_mut().take_free();
                self.release(g, renderer, audio, platform);
                continue;
            }
            platform.groups_mut().set_progress(g, done, total);
            if start.elapsed() >= self.budget {
                break;
            }
        }
    }

    fn name(&self, g: GroupId) -> String {
        self.names
            .get(&g)
            .cloned()
            .unwrap_or_else(|| format!("{:08x}", g.0))
    }

    fn open(&self, g: GroupId, platform: &PcPlatform) -> Result<BundleLoad, String> {
        let name = self
            .names
            .get(&g)
            .ok_or_else(|| format!("{:08x}: not in this build", g.0))?;
        if let Some(plan) = &self.plan {
            // Every group in memory or on its way, this one included.
            let loaded: Vec<&str> = self
                .names
                .iter()
                .filter(|(id, _)| {
                    **id == g
                        || !matches!(
                            trino_core::Platform::group_state(platform, **id),
                            trino_core::LoadState::Unloaded | trino_core::LoadState::Failed
                        )
                })
                .map(|(_, n)| n.as_str())
                .collect();
            if let Err(e) = plan.check(&loaded) {
                panic!("strict mode: {e}");
            }
        }
        BundleLoad::open(&self.dir.join(format!("groups/{name}.tbnd")), self.tier)
    }

    /// Upload one step's assets for `g` (an asset another group already
    /// holds only gains a reference).
    fn install(
        &mut self,
        g: GroupId,
        assets: LoadedAssets,
        renderer: &mut PcRenderer,
        audio: &mut PcAudio,
        platform: &mut PcPlatform,
    ) -> Result<(), String> {
        let clamp = |v: usize| v.min(u32::MAX as usize) as u32;
        let target = self.target;
        for s in &assets.sprites {
            let key = (Kind::Sprite, s.id);
            if self.hold(g, key) {
                crate::upload_sprite(renderer, s, target);
                let bytes = renderer.sprite_bytes(SpriteId(s.id)).min(u32::MAX as u64);
                self.uploaded(key, bytes as u32, platform);
            }
        }
        for (list, music) in [(&assets.sounds, false), (&assets.music, true)] {
            for s in list {
                let kind = if music { Kind::Music } else { Kind::Sound };
                let key = (kind, s.id);
                if self.hold(g, key) {
                    if music {
                        audio.upload_music_blob(MusicId(s.id), &s.blob)?;
                    } else {
                        audio.upload_sound_blob(SoundId(s.id), &s.blob)?;
                    }
                    let bytes = clamp(audio.clip_bytes(s.id, music));
                    self.uploaded(key, bytes, platform);
                }
            }
        }
        for m in assets.models {
            let key = (Kind::Model, m.id);
            if self.hold(g, key) {
                trino_core::render3d::Mesh::from_tmdl(&m.tmdl)
                    .map_err(|e| format!("model {:08x}: {e:?}", m.id))?;
                let bytes = clamp(m.tmdl.len());
                renderer.upload_mesh(ModelId(m.id), m.tmdl);
                self.uploaded(key, bytes, platform);
            }
        }
        for f in assets.fonts {
            let key = (Kind::Font, f.id);
            if self.hold(g, key) {
                trino_core::text::BitmapFont::parse(&f.table)
                    .map_err(|e| format!("font {:08x}: {e:?}", f.id))?;
                let bytes = clamp(f.table.len());
                renderer.upload_font(FontId(f.id), f.table);
                self.uploaded(key, bytes, platform);
            }
        }
        for b in assets.blobs {
            let key = (Kind::Blob, b.id);
            if self.hold(g, key) {
                let bytes = clamp(b.bytes.len());
                platform.insert_group_blob(BlobId(b.id), b.bytes);
                self.uploaded(key, bytes, platform);
            }
        }
        Ok(())
    }

    /// `g` holds `key`; `true` when nothing uploaded it yet.
    fn hold(&mut self, g: GroupId, key: (Kind, u32)) -> bool {
        self.held_by.entry(g).or_default().push(key);
        match self.held.get_mut(&key) {
            Some(h) => {
                h.refs += 1;
                false
            }
            None => true,
        }
    }

    fn uploaded(&mut self, key: (Kind, u32), bytes: u32, platform: &mut PcPlatform) {
        platform.memory_mut().add(key.0.category(), bytes);
        self.held.insert(key, Held { refs: 1, bytes });
    }

    /// Drop `g`'s references; free what no other group holds.
    fn release(
        &mut self,
        g: GroupId,
        renderer: &mut PcRenderer,
        audio: &mut PcAudio,
        platform: &mut PcPlatform,
    ) {
        for key in self.held_by.remove(&g).unwrap_or_default() {
            let Some(h) = self.held.get_mut(&key) else {
                continue;
            };
            h.refs -= 1;
            if h.refs > 0 {
                continue;
            }
            let bytes = self.held.remove(&key).map_or(0, |h| h.bytes);
            platform.memory_mut().sub(key.0.category(), bytes);
            let (kind, id) = key;
            match kind {
                Kind::Sprite => {
                    renderer.remove_sprite(SpriteId(id));
                }
                Kind::Sound => {
                    audio.remove_sound(SoundId(id));
                }
                Kind::Music => {
                    audio.remove_music(MusicId(id));
                }
                Kind::Model => {
                    renderer.remove_mesh(ModelId(id));
                }
                Kind::Font => {
                    renderer.remove_font(FontId(id));
                }
                Kind::Blob => {
                    platform.remove_group_blob(BlobId(id));
                }
            }
        }
    }

    /// Assets held by loaded or loading groups (each once).
    pub fn resident_assets(&self) -> usize {
        self.held.len()
    }
}
