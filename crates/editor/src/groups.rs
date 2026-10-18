//! Asset groups and RAM in the editor (issue #56, ADR 0022): the manifest's
//! groups, which the Assets panel's chips filter by and take drops on, which
//! the scene settings' `requires` and a cell's groups pick from, and the
//! memory plans the Memory tab draws — the numbers `cargo xtask assets
//! report` prints, measured on a worker keyed on the manifest. Edits are
//! the same text lines `cargo xtask assets group` writes.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui::{self, RichText};
use trino_asset_pipeline::Manifest;
use trino_asset_pipeline::memory::{self, MemoryPlan};
use trino_core::HwTier;

/// The console tiers the Memory tab measures (the ones with a RAM ceiling),
/// in the order it shows them.
pub const TIERS: [(trino_asset_pipeline::Platform, HwTier); 4] = [
    (trino_asset_pipeline::Platform::N64, HwTier::Base),
    (trino_asset_pipeline::Platform::N64, HwTier::Expanded),
    (trino_asset_pipeline::Platform::N3ds, HwTier::Base),
    (trino_asset_pipeline::Platform::N3ds, HwTier::Expanded),
];

type Plans = Result<Vec<MemoryPlan>, String>;

pub struct Groups {
    /// Hash of the manifest text the rest was read from.
    key: u64,
    pub manifest: Option<Manifest>,
    /// `boot` first, then the declared groups.
    pub names: Vec<String>,
    /// Each asset's groups, by logical path (`["boot"]` when it names none).
    of: BTreeMap<String, Vec<String>>,
    /// The Assets panel's group chip (`None`: every asset).
    pub filter: Option<String>,
    /// The "New group" field.
    pub new_name: String,
    plans: Option<(u64, Plans)>,
    pending: Option<(u64, Receiver<Plans>)>,
    /// `assets check`'s scene problems, for (scene rev, manifest key).
    problems: Option<((u64, u64), Vec<String>)>,
}

impl Groups {
    /// `filter`: the `TRINO_EDITOR_ASSET_GROUP` hook's chip.
    pub fn new(filter: Option<String>) -> Self {
        Groups {
            key: 0,
            manifest: None,
            names: vec![trino_asset_pipeline::groups::BOOT.to_string()],
            of: BTreeMap::new(),
            filter,
            new_name: String::new(),
            plans: None,
            pending: None,
            problems: None,
        }
    }

    /// Follow the manifest (once a frame: an edit here, an agent's edit or
    /// `cargo xtask assets group` all show at once).
    pub fn sync(&mut self, manifest_path: &Path) {
        let text = std::fs::read_to_string(manifest_path).unwrap_or_default();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        let key = h.finish();
        if key == self.key {
            return;
        }
        self.key = key;
        // The last good manifest stays while a hand edit is half-typed.
        if let Ok(m) = Manifest::load(manifest_path) {
            self.names = m.group_names();
            self.of = m
                .logicals()
                .into_iter()
                .map(|l| {
                    let g = m.groups_of(&l);
                    (l, g)
                })
                .collect();
            self.manifest = Some(m);
        }
        if self
            .filter
            .as_ref()
            .is_some_and(|f| !self.names.contains(f))
        {
            self.filter = None;
        }
    }

    /// `logical`'s groups (`["boot"]` when it names none or is unknown).
    pub fn of(&self, logical: &str) -> Vec<String> {
        self.of
            .get(logical)
            .cloned()
            .unwrap_or_else(|| vec![trino_asset_pipeline::groups::BOOT.to_string()])
    }

    /// Whether `logical` shows under the current chip.
    pub fn shows(&self, logical: &str) -> bool {
        self.filter
            .as_ref()
            .is_none_or(|f| self.of(logical).contains(f))
    }

    /// How many assets `group` holds.
    pub fn count(&self, group: &str) -> usize {
        self.of
            .values()
            .filter(|g| g.iter().any(|x| x == group))
            .count()
    }

    /// The memory plans of [`TIERS`], measured on a worker when the
    /// manifest moved (`generation`: a rebake); the last ones stay shown
    /// meanwhile. `None` until the first measurement lands.
    pub fn plans(
        &mut self,
        ctx: &egui::Context,
        assets: &Path,
        generation: u64,
    ) -> Option<&Result<Vec<MemoryPlan>, String>> {
        let key = self.key ^ generation.rotate_left(32);
        let stale = self.plans.as_ref().is_none_or(|(k, _)| *k != key);
        let running = self.pending.as_ref().is_some_and(|(k, _)| *k == key);
        if stale && !running {
            let (tx, rx) = channel();
            let (assets, ctx2) = (assets.to_path_buf(), ctx.clone());
            std::thread::spawn(move || {
                let plans = TIERS
                    .iter()
                    .map(|(p, t)| memory::plan(&assets, *p, *t))
                    .collect();
                let _ = tx.send(plans);
                ctx2.request_repaint();
            });
            self.pending = Some((key, rx));
        }
        if let Some((k, rx)) = &self.pending {
            match rx.try_recv() {
                Ok(p) => {
                    self.plans = Some((*k, p));
                    self.pending = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.pending = None,
            }
        }
        self.plans.as_ref().map(|(_, p)| p)
    }

    /// What `cargo xtask assets check` says about `scene`'s groups
    /// (`trino_asset_pipeline::groups::scene_problems`), again only when
    /// the scene (`rev`) or the manifest moved.
    pub fn scene_problems(
        &mut self,
        scene: &trino_scene::Scene,
        lib: &trino_scene::Library,
        rev: u64,
    ) -> Vec<String> {
        let key = (rev, self.key);
        if self.problems.as_ref().is_none_or(|(k, _)| *k != key) {
            let p = self.manifest.as_ref().map_or_else(Vec::new, |m| {
                trino_asset_pipeline::groups::scene_problems(scene, lib, m)
            });
            self.problems = Some((key, p));
        }
        self.problems
            .as_ref()
            .map(|(_, p)| p.clone())
            .unwrap_or_default()
    }

    /// The last measured plans, when they measured (the Game view's HUD
    /// line; [`plans`](Self::plans) keeps them fresh).
    pub fn ready_plans(&self) -> Option<&[MemoryPlan]> {
        self.plans
            .as_ref()
            .and_then(|(_, p)| p.as_ref().ok())
            .map(Vec::as_slice)
    }

    /// Whether plans have landed and none is being measured (tests wait on
    /// it after the tab drew once).
    #[cfg(test)]
    pub fn measured(&self) -> bool {
        self.pending.is_none() && self.plans.is_some()
    }

    /// Whether a measurement is running (the tab says so).
    pub fn measuring(&self) -> bool {
        self.pending.is_some()
    }
}

/// The logical path an Assets tile edits groups of, for the payloads a
/// chip takes (prefabs have no groups: their parts' assets do).
pub fn payload_logical(p: &crate::viewport::AssetPayload) -> Option<&str> {
    match p {
        crate::viewport::AssetPayload::Model(l) | crate::viewport::AssetPayload::Sprite(l) => {
            Some(l)
        }
        crate::viewport::AssetPayload::Prefab(_) => None,
    }
}

/// What a tile without a scene payload (a sound, a track, a particle
/// effect) carries while dragged onto a group chip.
#[derive(Clone, Debug)]
pub struct GroupDrag(pub String);

/// One toggle chip per group of `names`, those in `picked` on, `fixed` ones
/// on and locked (boot in a scene's `requires`). The new list (in `names`'
/// order) when one was clicked.
pub fn toggles(
    ui: &mut egui::Ui,
    id: &str,
    names: &[String],
    picked: &[String],
    fixed: &[&str],
) -> Option<Vec<String>> {
    use crate::theme::{c, icon};
    let mut out = None;
    ui.horizontal_wrapped(|ui| {
        // Whole chips on the next line, never cut (a `theme::prop` row
        // truncates).
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
        for g in names {
            let locked = fixed.contains(&g.as_str());
            let on = locked || picked.contains(g);
            let text = RichText::new(if on {
                format!("{} {g}", icon::CHECK)
            } else {
                g.clone()
            })
            .size(11.5)
            .color(if on {
                egui::Color32::WHITE
            } else {
                c::TEXT_WEAK
            });
            let b = egui::Button::new(text)
                .corner_radius(egui::CornerRadius::same(99))
                .fill(if on { c::ACCENT_SOFT } else { c::BG2 });
            let resp = ui
                .push_id((id, g), |ui| ui.add_enabled(!locked, b))
                .inner
                .on_disabled_hover_text("always loaded");
            if resp.clicked() {
                let next = names
                    .iter()
                    .filter(|n| {
                        let was = fixed.contains(&n.as_str()) || picked.contains(n);
                        if *n == g { !was } else { was }
                    })
                    .cloned()
                    .collect();
                out = Some(next);
            }
        }
        // Named in the file but not declared: shown, to take off.
        for g in picked.iter().filter(|g| !names.contains(g)) {
            let resp = ui
                .push_id((id, g, "unknown"), |ui| {
                    ui.add(
                        egui::Button::new(
                            RichText::new(format!("{} {g}", icon::WARNING))
                                .size(11.5)
                                .color(c::WARN),
                        )
                        .corner_radius(egui::CornerRadius::same(99)),
                    )
                })
                .inner
                .on_hover_text("not declared in the manifest: click to remove");
            if resp.clicked() {
                out = Some(picked.iter().filter(|x| *x != g).cloned().collect());
            }
        }
    });
    out
}
