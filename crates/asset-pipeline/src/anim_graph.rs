//! Animation graph bakes (issue #65): every `assets/anim/<name>.anim.ron`
//! becomes the blob `anim/<name>` (`TANG`), checked against the clips the
//! model bakes to on that platform. Rides the UI bake
//! ([`crate::ui::bake`]), so the PC and Web packs, the N64 and 3DS images,
//! the memory plan and live reload all carry it.

use std::path::Path;

use trino_scene::anim::{self, AnimGraphDef, ClipFacts};

use crate::manifest::{Manifest, Platform};

/// The model's clip facts on `platform` (`None`: not baked there, or not
/// in the manifest).
pub fn model_clips(
    root: &Path,
    manifest: &Manifest,
    platform: Platform,
    model: &str,
) -> Result<Option<Vec<ClipFacts>>, String> {
    let tmdl = model_tmdl(root, manifest, platform, model)?;
    tmdl.map(|t| anim::clip_facts(&t)).transpose()
}

/// The model's TMDL as `platform` bakes it.
pub fn model_tmdl(
    root: &Path,
    manifest: &Manifest,
    platform: Platform,
    model: &str,
) -> Result<Option<Vec<u8>>, String> {
    let short = model.strip_prefix("models/").unwrap_or(model);
    let Some(decl) = manifest.models.get(short) else {
        return Ok(None);
    };
    if !decl.bakes_for(platform) {
        return Ok(None);
    }
    let source = crate::resolve_source(&root.join("assets"), platform, &decl.file)?;
    crate::bake_model(&source, &decl.options()).map(Some)
}

/// Whether `sounds/<name>` is declared.
pub fn has_sound(manifest: &Manifest, name: &str) -> bool {
    manifest.sounds.contains_key(name) || manifest.music.contains_key(name)
}

/// Bake one graph for `platform`; `Ok(None)` when its model is not on that
/// platform (a PC-only rig).
pub fn bake(
    root: &Path,
    manifest: &Manifest,
    platform: Platform,
    def: &AnimGraphDef,
) -> Result<Option<Vec<u8>>, String> {
    let Some(clips) = model_clips(root, manifest, platform, &def.model_path())? else {
        if manifest
            .models
            .contains_key(def.model_path().trim_start_matches("models/"))
        {
            return Ok(None);
        }
        return Err(format!(
            "model `{}` is not in the manifest",
            def.model_path()
        ));
    };
    anim::to_tang(def, &clips, &|s| has_sound(manifest, s)).map(Some)
}

/// Every graph of the project for `platform`: (logical `anim/<name>`, blob).
pub fn bake_all(
    root: &Path,
    manifest: &Manifest,
    platform: Platform,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for (name, path) in anim::graph_files(root) {
        let r = AnimGraphDef::load(&path).and_then(|def| bake(root, manifest, platform, &def));
        match r {
            Ok(Some(bytes)) => out.push((anim::blob_path(&name), bytes)),
            Ok(None) => {}
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors.join("\n"))
    }
}

/// Plays the animation graphs of a scene's entities (`anim_graph`): the PC
/// scene player and the editor's Play, with no game code. One instance per
/// entity (by its name path), the graph baked from its RON on first use;
/// each step sets the item's pose, and moves it by the graph's root motion.
pub struct GraphPlayback {
    root: std::path::PathBuf,
    /// Move entities by root motion (the scene player); off, they play in
    /// place (the editor's Scene and Game views, where they are edited).
    pub carry: bool,
    graphs: std::collections::HashMap<String, Result<Vec<u8>, String>>,
    /// Each graph file's modification time when it was baked: an edit
    /// (the editor, an agent) rebakes it and restarts its instances.
    stamps: std::collections::HashMap<String, Option<std::time::SystemTime>>,
    playing: std::collections::HashMap<String, Playing>,
    /// The events of the last step: (entity name, event).
    pub events: Vec<(String, trino_core::AnimEvent)>,
}

struct Playing {
    graph: String,
    inst: trino_core::AnimGraphInstance,
    /// Where root motion has carried it (model space, from its placement).
    root: trino_core::anim::RootPose,
}

impl GraphPlayback {
    /// For the project at `root` (the folder with `assets/`).
    pub fn new(root: &Path) -> GraphPlayback {
        GraphPlayback {
            root: root.to_path_buf(),
            carry: true,
            graphs: Default::default(),
            stamps: Default::default(),
            playing: Default::default(),
            events: Vec::new(),
        }
    }

    /// Forget every graph and instance (a reload: they start over).
    pub fn clear(&mut self) {
        self.graphs.clear();
        self.stamps.clear();
        self.playing.clear();
    }

    fn graph(&mut self, path: &str) -> Result<&[u8], String> {
        let root = self.root.clone();
        let name = path.strip_prefix("anim/").unwrap_or(path);
        let stamp = std::fs::metadata(anim::graph_file(&root, name))
            .and_then(|m| m.modified())
            .ok();
        if self.stamps.get(path).is_some_and(|s| *s != stamp) {
            self.graphs.remove(path);
            self.playing.retain(|_, p| p.graph != path);
        }
        self.stamps.insert(path.to_string(), stamp);
        self.graphs
            .entry(path.to_string())
            .or_insert_with(|| {
                let name = path.strip_prefix("anim/").unwrap_or(path);
                let manifest = Manifest::load(&root.join("assets/manifest.toml"))?;
                let def = AnimGraphDef::load(&anim::graph_file(&root, name))?;
                bake(&root, &manifest, Platform::Pc, &def)?
                    .ok_or_else(|| format!("{path}: its model is not baked for the PC"))
            })
            .as_deref()
            .map_err(|e| e.clone())
    }

    /// Advance every graph by `dt` and pose its items. Errors (a missing
    /// graph, a broken file) are returned once per step, items unposed.
    pub fn step(&mut self, items: &mut [trino_scene::DrawItem], dt: f32) -> Vec<String> {
        use trino_core::anim::RootPose;
        self.events.clear();
        let mut errors = Vec::new();
        for item in items.iter_mut() {
            let Some(c) = item.anim_graph.clone() else {
                continue;
            };
            let bytes = match self.graph(&c.graph) {
                Ok(b) => b.to_vec(),
                Err(e) => {
                    if !errors.contains(&e) {
                        errors.push(e);
                    }
                    continue;
                }
            };
            let Ok(g) = trino_core::AnimGraph::parse(&bytes) else {
                continue;
            };
            let p = self.playing.entry(item.name.clone()).or_insert_with(|| {
                let state = c
                    .state
                    .as_deref()
                    .and_then(|s| g.state_index(trino_core::anim::StateId::from_name(s)))
                    .unwrap_or(g.start());
                let mut inst = trino_core::AnimGraphInstance::new_in(&g, state);
                for (k, v) in &c.params {
                    inst.set(&g, k, *v);
                }
                Playing {
                    graph: c.graph.clone(),
                    inst,
                    root: RootPose::IDENTITY,
                }
            });
            if p.graph != c.graph {
                // Another graph on the same entity: start it over.
                *p = Playing {
                    graph: c.graph.clone(),
                    inst: trino_core::AnimGraphInstance::new(&g),
                    root: RootPose::IDENTITY,
                };
            }
            p.inst.update(&g, dt);
            for e in p.inst.events().iter() {
                self.events.push((item.name.clone(), *e));
            }
            if c.root_motion && self.carry {
                let d = p.inst.root_motion();
                p.root = p.root.then(RootPose {
                    x: d.translation.x,
                    z: d.translation.z,
                    yaw: d.yaw,
                });
            }
            item.params.anim = Some(p.inst.anim(&g));
            if p.root != RootPose::IDENTITY {
                let carry = trino_core::Mat34::from_rotation_scale_translation(
                    trino_core::Vec3::new(0.0, p.root.yaw, 0.0),
                    trino_core::Vec3::ONE,
                    trino_core::Vec3::new(p.root.x, 0.0, p.root.z),
                );
                item.matrix = item.matrix.mul(&carry);
            }
        }
        errors
    }

    /// Pose `items` as the last step left their instances (no time passes):
    /// a second resolve of the same frame (another platform's view).
    pub fn apply(&mut self, items: &mut [trino_scene::DrawItem]) {
        for item in items.iter_mut() {
            let Some(c) = item.anim_graph.clone() else {
                continue;
            };
            let Some(bytes) = self
                .graphs
                .get(&c.graph)
                .and_then(|r| r.as_ref().ok())
                .cloned()
            else {
                continue;
            };
            let (Ok(g), Some(p)) = (
                trino_core::AnimGraph::parse(&bytes),
                self.playing.get(&item.name),
            ) else {
                continue;
            };
            item.params.anim = Some(p.inst.anim(&g));
        }
    }

    /// The instance playing an entity (tests, the editor's graph view).
    pub fn instance(&self, name: &str) -> Option<&trino_core::AnimGraphInstance> {
        self.playing.get(name).map(|p| &p.inst)
    }

    /// Set a parameter of every instance of `graph` (the editor's params
    /// panel driving Play live).
    pub fn set_param(&mut self, graph: &str, param: &str, v: f32) {
        let Ok(bytes) = self.graph(graph).map(<[u8]>::to_vec) else {
            return;
        };
        let Ok(g) = trino_core::AnimGraph::parse(&bytes) else {
            return;
        };
        for p in self.playing.values_mut().filter(|p| p.graph == graph) {
            p.inst.set(&g, param, v);
        }
    }
}
