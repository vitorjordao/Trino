//! The project on disk as the editor sees it: baked assets (sprites, models
//! with their stats and clip names), placeholder primitives, thumbnails, and
//! the file operations behind *Create → Fake object*, *Replace with real
//! model*, etc. Every operation writes plain text files (manifest, RON) —
//! the same files an AI agent or a human edits — and lets the hot-reload
//! watchers pick the change up.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use eframe::egui;
use trino_asset_pipeline as pipeline;
use trino_asset_pipeline::manifest::edit;
use trino_asset_pipeline::{Manifest, Primitive};
use trino_core::{
    Camera3, Color, DEFAULT_LIGHT, Mat34, Material, Mesh, ModelId, ModelParams, Renderer, Vec3,
};
use trino_platform_pc::{PcRenderer, SimProfile};

pub const BAKED: &str = "target/assets/pc";
pub const PROXY_DIR: &str = "models/proxies";

pub struct SpriteAsset {
    pub logical: String,
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// What the 3DS samples from the sprite's 3DS format (the bake's
    /// `preview_3ds`), drawn by the Game view's 3DS profile.
    pub preview_3ds: Option<Vec<u8>>,
    /// The PC's BCn blocks (the bake's `compressed`): the Game view's PC
    /// profile draws them decoded, what any BC GPU samples.
    pub compressed: Option<pipeline::CompressedSprite>,
    pub thumb: Option<egui::TextureHandle>,
}

pub struct ModelAsset {
    pub logical: String,
    /// Manifest key (`crate` for `models/crate`).
    pub name: String,
    pub id: u32,
    pub tmdl: Vec<u8>,
    pub verts: usize,
    pub tris: usize,
    pub joints: usize,
    pub clips: Vec<String>,
    /// Bind-pose AABB (model space).
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// Bounding sphere (covers animation).
    pub center: [f32; 3],
    pub radius: f32,
    /// Manifest `file` (relative to assets/shared).
    pub source: String,
    /// Parsed placeholder when `source` is a `.prim.ron`.
    pub proxy: Option<Primitive>,
    /// Ambient occlusion baked into its vertex colors (the manifest's
    /// `ao`).
    pub ao: Option<pipeline::AoBake>,
    /// The LUT the consoles' bake grades its colors through (the manifest's
    /// `grading`, issue #64), and that graded twin: what the Game view's
    /// console profiles draw.
    pub grading: Option<String>,
    pub graded: Option<Vec<u8>>,
    /// Clips whose root motion the bake extracts and each clip's timeline
    /// events (the manifest's `root_motion` and `events.<clip>`, issue #65).
    pub root_motion: Vec<String>,
    pub events: std::collections::BTreeMap<String, Vec<(f32, String)>>,
    pub thumb: Option<egui::TextureHandle>,
}

impl ModelAsset {
    /// The mesh `r`'s profile draws: the console-graded twin on the
    /// console profiles, else the model as baked.
    pub fn tmdl_for(&self, r: &PcRenderer) -> Vec<u8> {
        match (r.profile(), &self.graded) {
            (SimProfile::N64 | SimProfile::N3ds, Some(g)) => g.clone(),
            _ => self.tmdl.clone(),
        }
    }

    pub fn is_proxy(&self) -> bool {
        self.proxy.is_some()
    }
}

pub struct Project {
    pub root: PathBuf,
    /// Baked assets that did not load (issue #68), newest reload's: the
    /// Console lists each with a re-bake button.
    pub load_errors: Vec<pipeline::AssetError>,
    pub sprites: BTreeMap<String, SpriteAsset>,
    pub models: BTreeMap<String, ModelAsset>,
    pub sounds: Vec<String>,
    pub music: Vec<String>,
    /// Font glyph tables by logical path (`fonts/x` -> (id, TFNT)); the
    /// atlas is in `sprites` under the same logical path.
    pub fonts: BTreeMap<String, (u32, Vec<u8>)>,
    /// Each sprite's N64 TMEM bytes, by handle, from its size and its
    /// manifest `n64` format (`None`: a palette, which can't share TMEM).
    /// The budget HUD's water lines read it.
    pub tmem: BTreeMap<u32, Option<u32>>,
    /// The grading LUTs (`luts/x` -> (id, TLUT), issue #64), and the scene
    /// files the consoles' bake grades (`bake_grading` on `[scenes.*]`).
    pub luts: BTreeMap<String, (u32, Vec<u8>)>,
    pub bake_grading: Vec<String>,
    /// Baked collision meshes (`TCOL`, issue #66) by blob id
    /// (`collision/<name>`; `Shape::Mesh::mesh`).
    pub collision: BTreeMap<u32, Vec<u8>>,
    /// Bumped whenever asset content changes (viewports re-upload, caches
    /// invalidate).
    pub generation: u64,
}

impl Project {
    pub fn new(root: PathBuf) -> Self {
        Project {
            root,
            sprites: BTreeMap::new(),
            models: BTreeMap::new(),
            sounds: Vec::new(),
            music: Vec::new(),
            fonts: BTreeMap::new(),
            tmem: BTreeMap::new(),
            luts: BTreeMap::new(),
            bake_grading: Vec::new(),
            collision: BTreeMap::new(),
            generation: 0,
            load_errors: Vec::new(),
        }
    }

    /// The models whose collision mesh is baked (`Mesh` colliders).
    pub fn collision_models(&self) -> Vec<String> {
        self.models
            .keys()
            .filter(|m| {
                let id = trino_core::asset_id(&trino_scene::collision_blob_path(m));
                self.collision.contains_key(&id)
            })
            .cloned()
            .collect()
    }

    /// A baked collision mesh by its blob id.
    pub fn collision_mesh(&self, id: u32) -> Option<trino_core::physics::MeshCollider<'_>> {
        trino_core::physics::MeshCollider::parse(self.collision.get(&id)?).ok()
    }

    /// Bind-pose AABB per model logical path (`Auto` scene colliders).
    pub fn model_bounds(&self) -> BTreeMap<String, trino_scene::ModelBounds> {
        self.models
            .iter()
            .map(|(k, m)| (k.clone(), (m.min, m.max)))
            .collect()
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn baked_dir(&self) -> PathBuf {
        self.root.join(BAKED)
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.assets_dir().join("manifest.toml")
    }

    /// Bake everything and load it. Returns log lines.
    pub fn bake_and_load(&mut self) -> Vec<Result<String, String>> {
        let mut log = Vec::new();
        match pipeline::bake_all(
            &self.assets_dir(),
            pipeline::Platform::Pc,
            &self.baked_dir(),
        ) {
            Ok(report) => log.push(Ok(format!("baked {} asset(s)", report.entries.len()))),
            Err(e) => log.push(Err(format!("bake failed:\n{e}"))),
        }
        match self.reload(None) {
            Ok(n) => log.push(Ok(format!("loaded {n} asset(s)"))),
            Err(e) => log.push(Err(e)),
        }
        log
    }

    /// (Re)load baked assets — all, or only `ids`. Returns how many loaded.
    pub fn reload(&mut self, ids: Option<&[u32]>) -> Result<usize, String> {
        let (assets, errors) = pipeline::load_dir_report(&self.baked_dir(), ids)?;
        match ids {
            None => self.load_errors = errors,
            Some(ids) => {
                self.load_errors.retain(|e| !ids.contains(&e.id));
                self.load_errors.extend(errors);
            }
        }
        let manifest = Manifest::load(&self.manifest_path()).ok();
        let mut n = 0;
        for s in assets.sprites {
            n += 1;
            self.tmem.insert(
                s.id,
                n64_tmem(manifest.as_ref(), &s.logical, s.width, s.height),
            );
            self.sprites.insert(
                s.logical.clone(),
                SpriteAsset {
                    logical: s.logical,
                    id: s.id,
                    width: s.width,
                    height: s.height,
                    rgba: s.rgba,
                    preview_3ds: s.preview_3ds,
                    compressed: s.compressed,
                    thumb: None,
                },
            );
        }
        for m in assets.models {
            n += 1;
            let name = m.logical.trim_start_matches("models/").to_string();
            let decl = manifest.as_ref().and_then(|man| man.models.get(&name));
            let source = decl.map(|d| d.file.clone()).unwrap_or_default();
            let opts = decl.map(|d| d.options()).unwrap_or_default();
            let ao = opts.ao.clone();
            let grading = decl.and_then(|d| d.grading.clone());
            let src_path = self.resolve_source(&source);
            let proxy = if source.ends_with(".prim.ron") {
                src_path.as_ref().and_then(|p| Primitive::load(p).ok())
            } else {
                None
            };
            let clips = src_path
                .as_ref()
                .map(|p| pipeline::clip_names(p))
                .unwrap_or_default();
            let Ok(mesh) = Mesh::from_tmdl(&m.tmdl) else {
                continue;
            };
            let mut min = [f32::MAX; 3];
            let mut max = [f32::MIN; 3];
            for i in 0..mesh.vertex_count {
                let p = mesh.position(i);
                for (k, v) in [p.x, p.y, p.z].into_iter().enumerate() {
                    min[k] = min[k].min(v);
                    max[k] = max[k].max(v);
                }
            }
            let (center, radius) = mesh
                .bounds()
                .map(|(c, r)| ([c.x, c.y, c.z], r))
                .unwrap_or(([0.0; 3], 1.0));
            let asset = ModelAsset {
                verts: mesh.vertex_count,
                tris: mesh.index_count / 3,
                joints: mesh.joint_count(),
                logical: m.logical.clone(),
                name,
                id: m.id,
                min,
                max,
                center,
                radius,
                clips,
                source,
                proxy,
                ao,
                grading,
                graded: m.graded,
                root_motion: opts.root_motion,
                events: opts.events,
                thumb: None,
                tmdl: m.tmdl,
            };
            self.models.insert(m.logical, asset);
        }
        for f in assets.fonts {
            n += 1;
            self.fonts.insert(f.logical, (f.id, f.table));
        }
        for b in assets.blobs {
            if b.bytes.starts_with(&trino_core::post::TLUT_MAGIC) {
                n += 1;
                self.luts.insert(b.logical, (b.id, b.bytes));
            } else if b.bytes.starts_with(trino_core::physics::TCOL_MAGIC) {
                n += 1;
                self.collision.insert(b.id, b.bytes);
            }
        }
        if let Some(m) = &manifest {
            self.bake_grading = m
                .scenes
                .values()
                .filter(|d| d.bake_grading)
                .map(|d| d.file.clone())
                .collect();
        }
        if ids.is_none() {
            self.sounds = assets.sounds.into_iter().map(|s| s.logical).collect();
            self.music = assets.music.into_iter().map(|s| s.logical).collect();
        }
        self.generation += 1;
        Ok(n)
    }

    fn resolve_source(&self, file: &str) -> Option<PathBuf> {
        if file.is_empty() {
            return None;
        }
        pipeline::resolve_source(&self.assets_dir(), pipeline::Platform::Pc, file).ok()
    }

    /// Upload every asset into a renderer (new viewport / profile switch).
    pub fn upload_all(&self, r: &mut PcRenderer) {
        for s in self.sprites.values() {
            r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
        }
        for m in self.models.values() {
            r.upload_mesh(ModelId(m.id), m.tmdl_for(r));
        }
        for (id, table) in self.fonts.values() {
            r.upload_font(trino_core::FontId(*id), table.clone());
        }
        for (id, lut) in self.luts.values() {
            r.take_lut_blob(*id, lut);
        }
    }

    /// The `TLUT` blob of `logical` (`luts/x`), when the bake has it.
    pub fn lut(&self, logical: &str) -> Option<trino_core::Lut<'_>> {
        self.luts
            .get(logical)
            .and_then(|(_, b)| trino_core::Lut::parse(b).ok())
    }

    /// Sprites a console bakes from its own override file — the impostor
    /// atlases, whose N64 version has smaller tiles (and a palette) — so the
    /// Game view's N64 profile samples what the console will; on the 3DS,
    /// each sprite's 3DS format (ETC1 blocks, 4-bit banding); on the PC,
    /// the BCn blocks decoded. Every other sprite keeps the master.
    pub fn upload_platform_sprites(&self, r: &mut PcRenderer, target: trino_scene::Target) {
        if target == trino_scene::Target::Pc {
            for s in self.sprites.values() {
                if let Some(c) = &s.compressed
                    && let Some(px) = c.format.decode(s.width, s.height, &c.data)
                {
                    r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &px);
                }
            }
            return;
        }
        if target == trino_scene::Target::N3ds {
            for s in self.sprites.values() {
                if let Some(p) = &s.preview_3ds {
                    r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, p);
                }
            }
            return;
        }
        if target != trino_scene::Target::N64 {
            return;
        }
        for name in pipeline::impostor::names(&self.assets_dir()) {
            let file = self.assets_dir().join(format!("n64/impostors/{name}.png"));
            if let Some((w, h, rgba)) = decode_png_rgba(&file) {
                let id =
                    trino_core::SpriteId::from_path(&pipeline::impostor::sprite_logical(&name));
                r.upload_sprite(id, w, h, &rgba);
            }
        }
    }

    /// Upload only the given logical paths.
    pub fn upload_some(&self, r: &mut PcRenderer, logicals: &[String]) {
        for l in logicals {
            if let Some(s) = self.sprites.get(l) {
                r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
            }
            if let Some(m) = self.models.get(l) {
                r.upload_mesh(ModelId(m.id), m.tmdl_for(r));
            }
            if let Some((id, table)) = self.fonts.get(l) {
                r.upload_font(trino_core::FontId(*id), table.clone());
            }
            if let Some((id, lut)) = self.luts.get(l) {
                r.take_lut_blob(*id, lut);
            }
        }
    }

    pub fn logicals_for_ids(&self, ids: &[u32]) -> Vec<String> {
        let mut out: Vec<String> = self
            .sprites
            .values()
            .filter(|s| ids.contains(&s.id))
            .map(|s| s.logical.clone())
            .collect();
        out.extend(
            self.models
                .values()
                .filter(|m| ids.contains(&m.id))
                .map(|m| m.logical.clone()),
        );
        out.extend(
            self.luts
                .iter()
                .filter(|(_, (id, _))| ids.contains(id))
                .map(|(l, _)| l.clone()),
        );
        out
    }

    pub fn model_by_id(&self, id: ModelId) -> Option<&ModelAsset> {
        self.models.values().find(|m| m.id == id.0)
    }

    // ------------------------------------------------------ file ops ----

    /// Write a placeholder primitive and register it in the manifest.
    /// Returns the logical path (`models/<name>`); the asset watcher bakes it.
    pub fn create_proxy(&self, name: &str, prim: &Primitive) -> Result<String, String> {
        if !edit::valid_name(name) {
            return Err(format!(
                "`{name}` is not a valid asset name (use lowercase letters, digits, - and _)"
            ));
        }
        let manifest = self.manifest_path();
        let text = std::fs::read_to_string(&manifest)
            .map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;
        let rel = format!("{PROXY_DIR}/{name}.prim.ron");
        let new = edit::add_model(&text, name, &rel)?;
        let file = self.assets_dir().join("shared").join(&rel);
        std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&file, prim.to_ron()).map_err(|e| e.to_string())?;
        std::fs::write(&manifest, new).map_err(|e| e.to_string())?;
        Ok(format!("models/{name}"))
    }

    /// Rewrite a placeholder's parameters (live: the watcher rebakes).
    pub fn save_proxy(&self, model: &ModelAsset, prim: &Primitive) -> Result<(), String> {
        let path = self
            .resolve_source(&model.source)
            .ok_or_else(|| format!("cannot locate {}", model.source))?;
        std::fs::write(&path, prim.to_ron()).map_err(|e| e.to_string())
    }

    /// Point a model at another source file (e.g. placeholder → real .glb).
    pub fn retarget_model(&self, name: &str, file: &str) -> Result<(), String> {
        if self.resolve_source(file).is_none() {
            return Err(format!(
                "`{file}` not found under assets/shared/ (or a platform override)"
            ));
        }
        let manifest = self.manifest_path();
        let text = std::fs::read_to_string(&manifest).map_err(|e| e.to_string())?;
        let new = edit::set_model_file(&text, name, file)?;
        std::fs::write(&manifest, new).map_err(|e| e.to_string())
    }

    /// Bake ambient occlusion into a model's vertex colors (`Some`) or
    /// stop (`None`): its manifest `ao` line (live: the watcher rebakes).
    pub fn set_model_ao(&self, name: &str, ao: Option<&pipeline::AoBake>) -> Result<(), String> {
        let manifest = self.manifest_path();
        let text = std::fs::read_to_string(&manifest).map_err(|e| e.to_string())?;
        let value = ao.map(|a| a.to_toml());
        let new = edit::set_model_ao(&text, name, value.as_deref())?;
        std::fs::write(&manifest, new).map_err(|e| e.to_string())
    }

    /// A clip's timeline events (Animation tab): its manifest
    /// `events.<clip>` line. Live: the watcher rebakes the model.
    pub fn set_clip_events(
        &self,
        model: &str,
        clip: &str,
        events: &[(f32, String)],
    ) -> Result<(), String> {
        let name = model.trim_start_matches("models/");
        self.edit_manifest(|t| edit::set_model_events(t, name, clip, events))
    }

    /// The clips whose root motion the bake extracts (`root_motion`).
    pub fn set_root_motion(&self, model: &str, clips: &[String]) -> Result<(), String> {
        let name = model.trim_start_matches("models/");
        self.edit_manifest(|t| edit::set_model_root_motion(t, name, clips))
    }

    /// Set (`Some`, `"auto"` included) or remove (`None`: the platform's
    /// default) the `formats` entry of a sprite or font atlas
    /// (`sprites/x`, `fonts/x`) for `platform`. Live: the watcher rebakes.
    pub fn set_texture_format(
        &mut self,
        logical: &str,
        platform: pipeline::Platform,
        format: Option<&str>,
    ) -> Result<(), String> {
        let (kind, name) = crate::textures::table(logical)?;
        self.edit_manifest(|t| edit::set_texture_format(t, kind, name, platform.key(), format))?;
        // The budget's TMEM map follows the manifest at once (a format with
        // the same bytes rebakes nothing, and the watcher reports nothing).
        let manifest = Manifest::load(&self.manifest_path()).ok();
        if let Some(s) = self.sprites.get(logical) {
            let tmem = n64_tmem(manifest.as_ref(), logical, s.width, s.height);
            self.tmem.insert(s.id, tmem);
        }
        Ok(())
    }

    /// Set or remove a texture's `quality` or `min_psnr`.
    pub fn set_texture_key(
        &self,
        logical: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<(), String> {
        let (kind, name) = crate::textures::table(logical)?;
        self.edit_manifest(|t| edit::set_texture_key(t, kind, name, key, value))
    }

    /// Set or remove a sound's `quality`, `stream`, `loop` or `bus`
    /// (`sounds/x`, `music/x`). Live: the watcher rebakes.
    pub fn set_audio_key(
        &self,
        logical: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<(), String> {
        let (music, name) = crate::audio::split(logical)?;
        let kind = if music { "music" } else { "sounds" };
        self.edit_manifest(|t| edit::set_audio_key(t, kind, name, key, value))
    }

    /// Set (`Some`, `"auto"` included) or remove (`None`: the default) one
    /// platform's `codec` or `sample_rate` entry of a sound.
    pub fn set_audio_platform(
        &self,
        logical: &str,
        key: &str,
        platform: pipeline::Platform,
        value: Option<&str>,
    ) -> Result<(), String> {
        let (music, name) = crate::audio::split(logical)?;
        let kind = if music { "music" } else { "sounds" };
        self.edit_manifest(|t| edit::set_audio_platform(t, kind, name, key, platform, value))
    }

    /// Put `logical` in `groups` (issue #56): its manifest `groups` line,
    /// new groups declared. Live: the watcher rebakes.
    pub fn set_groups(&self, logical: &str, groups: &[String]) -> Result<(), String> {
        self.edit_manifest(|t| edit::set_groups(t, logical, groups))
    }

    /// Declare `[groups.<name>]`.
    pub fn add_group(&self, name: &str) -> Result<(), String> {
        self.edit_manifest(|t| edit::add_group(t, name))
    }

    fn edit_manifest(&self, f: impl FnOnce(&str) -> Result<String, String>) -> Result<(), String> {
        let manifest = self.manifest_path();
        let text = std::fs::read_to_string(&manifest).map_err(|e| e.to_string())?;
        std::fs::write(&manifest, f(&text)?).map_err(|e| e.to_string())
    }

    /// Any free `base`, `base-2`, ... model name.
    pub fn unique_model_name(&self, base: &str) -> String {
        let taken = |n: &str| self.models.contains_key(&format!("models/{n}"));
        if !taken(base) {
            return base.to_string();
        }
        (2..)
            .map(|i| format!("{base}-{i}"))
            .find(|n| !taken(n))
            .unwrap()
    }
}

/// A texture's N64 TMEM bytes from its size and its manifest `n64` format
/// (RGBA5551 when undeclared; `None` for a palette or `auto`).
fn n64_tmem(manifest: Option<&Manifest>, logical: &str, width: u32, height: u32) -> Option<u32> {
    let formats = manifest.and_then(|m| match crate::textures::table(logical) {
        Ok(("fonts", name)) => m.fonts.get(name).map(|d| &d.formats),
        Ok((_, name)) => m.sprites.get(name).map(|d| &d.formats),
        Err(_) => None,
    });
    let format = formats
        .and_then(|f| f.get("n64").cloned())
        .unwrap_or_else(|| "RGBA5551".into());
    trino_scene::budget::tmem_bytes(width, height, &format)
}

/// Renders small model/material previews with the real renderer (offscreen,
/// read back once per asset change) for the Assets panel.
pub struct Thumbnailer {
    renderer: PcRenderer,
    sphere: ModelId,
}

pub const THUMB: u32 = 88;

impl Thumbnailer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let mut renderer = PcRenderer::with_device(device, queue, SimProfile::Native);
        renderer.set_internal_size(THUMB, THUMB);
        let sphere = ModelId(trino_core::asset_id("__editor/sphere"));
        let prim = Primitive {
            shape: pipeline::Shape::Sphere { radius: 1.0 },
            segments: 28,
            color: [255, 255, 255, 255],
            ..Primitive::default()
        };
        renderer.upload_mesh(sphere, prim.build().encode().expect("sphere bakes"));
        Thumbnailer { renderer, sphere }
    }

    fn render(
        &mut self,
        model: ModelId,
        center: Vec3,
        radius: f32,
        params: &ModelParams,
    ) -> egui::ColorImage {
        let r = &mut self.renderer;
        r.begin_frame(Color::rgba(0, 0, 0, 0));
        let fov = 40f32.to_radians();
        let dist = radius / (fov * 0.5).sin() * 1.08;
        let dir = Vec3::new(0.62, 0.5, -0.6).normalized();
        r.set_camera(&Camera3 {
            eye: center + dir * dist,
            target: center,
            fov_y: fov,
        });
        r.set_light(&DEFAULT_LIGHT);
        r.draw_model_matrix(model, &Mat34::IDENTITY, Material::VertexLit, params);
        r.end_frame();
        let px = r.read_offscreen();
        egui::ColorImage::from_rgba_unmultiplied([THUMB as usize, THUMB as usize], &px)
    }

    pub fn model(&mut self, m: &ModelAsset) -> egui::ColorImage {
        self.renderer.upload_mesh(ModelId(m.id), m.tmdl.clone());
        let c = Vec3::new(m.center[0], m.center[1], m.center[2]);
        self.render(ModelId(m.id), c, m.radius.max(0.01), &ModelParams::DEFAULT)
    }

    /// Live preview of a placeholder being configured (Fake object dialog).
    pub fn primitive(&mut self, prim: &Primitive) -> egui::ColorImage {
        let id = ModelId(trino_core::asset_id("__editor/preview"));
        let Ok(blob) = prim.build().encode() else {
            return egui::ColorImage::new(
                [THUMB as usize, THUMB as usize],
                vec![egui::Color32::TRANSPARENT; (THUMB * THUMB) as usize],
            );
        };
        let (c, r) = Mesh::from_tmdl(&blob)
            .ok()
            .and_then(|m| m.bounds())
            .unwrap_or((Vec3::ZERO, 1.0));
        self.renderer.upload_mesh(id, blob);
        self.render(id, c, r.max(0.01), &ModelParams::DEFAULT)
    }

    /// A prefab (or any resolved entity tree) framed as one object.
    pub fn items(
        &mut self,
        items: &[trino_scene::DrawItem],
        project: &Project,
    ) -> egui::ColorImage {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for item in items {
            let Some(m) = item
                .model
                .as_ref()
                .and_then(|(id, _)| project.model_by_id(*id))
            else {
                continue;
            };
            for i in 0..8 {
                let c = Vec3::new(
                    if i & 1 == 0 { m.min[0] } else { m.max[0] },
                    if i & 2 == 0 { m.min[1] } else { m.max[1] },
                    if i & 4 == 0 { m.min[2] } else { m.max[2] },
                );
                let p = item.matrix.transform_point(c);
                for (k, v) in [p.x, p.y, p.z].into_iter().enumerate() {
                    lo[k] = lo[k].min(v);
                    hi[k] = hi[k].max(v);
                }
            }
            self.renderer.upload_mesh(ModelId(m.id), m.tmdl.clone());
        }
        if lo[0] > hi[0] {
            return egui::ColorImage::new(
                [THUMB as usize, THUMB as usize],
                vec![egui::Color32::TRANSPARENT; (THUMB * THUMB) as usize],
            );
        }
        let center = Vec3::new(
            (lo[0] + hi[0]) * 0.5,
            (lo[1] + hi[1]) * 0.5,
            (lo[2] + hi[2]) * 0.5,
        );
        let d = Vec3::new(hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]);
        let radius = (d.dot(d).sqrt() * 0.5).max(0.05);
        let r = &mut self.renderer;
        r.begin_frame(Color::rgba(0, 0, 0, 0));
        let fov = 40f32.to_radians();
        let dist = radius / (fov * 0.5).sin() * 1.08;
        let dir = Vec3::new(0.62, 0.5, -0.6).normalized();
        r.set_camera(&Camera3 {
            eye: center + dir * dist,
            target: center,
            fov_y: fov,
        });
        r.set_light(&DEFAULT_LIGHT);
        for item in items {
            if let Some((id, _)) = &item.model {
                r.draw_model_matrix(*id, &item.matrix, Material::VertexLit, &item.params);
            }
        }
        r.end_frame();
        let px = r.read_offscreen();
        egui::ColorImage::from_rgba_unmultiplied([THUMB as usize, THUMB as usize], &px)
    }

    pub fn material(&mut self, params: &ModelParams) -> egui::ColorImage {
        self.render(self.sphere, Vec3::ZERO, 1.0, params)
    }
}

/// Any PNG (indexed, gray, RGB, RGBA) as RGBA8.
pub fn decode_png_rgba(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let file = std::fs::File::open(path).ok()?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((w, h, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp project with a sprite, a sound, a music track, a real model and
    /// a fake object, all copied from the engine's sample masters.
    fn sample_project() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let engine = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/shared");
        let shared = root.join("assets/shared");
        for (from, to) in [
            ("sprites/brick.png", "sprites/brick.png"),
            ("sounds/beep.wav", "sounds/beep.wav"),
            ("sounds/win.wav", "music/win.wav"),
            ("models/cube.glb", "models/cube.glb"),
        ] {
            std::fs::create_dir_all(shared.join(to).parent().unwrap()).unwrap();
            std::fs::copy(engine.join(from), shared.join(to)).unwrap();
        }
        std::fs::write(
            root.join("assets/manifest.toml"),
            "version = 1\n\n[sprites.brick]\nfile = \"sprites/brick.png\"\n\n\
             [sounds.beep]\nfile = \"sounds/beep.wav\"\n\n[music.win]\nfile = \"music/win.wav\"\n\n\
             [models.cube]\nfile = \"models/cube.glb\"\n",
        )
        .unwrap();
        let project = Project::new(root);
        project
            .create_proxy(
                "block",
                &Primitive {
                    shape: trino_asset_pipeline::Shape::Box {
                        size: [2.0, 1.0, 4.0],
                    },
                    ..Primitive::default()
                },
            )
            .unwrap();
        (dir, project)
    }

    #[test]
    fn bake_and_load_reads_every_asset_with_its_stats() {
        let (_dir, mut project) = sample_project();
        let log = project.bake_and_load();
        assert_eq!(
            log,
            vec![
                Ok("baked 5 asset(s)".to_string()),
                Ok("loaded 3 asset(s)".to_string())
            ]
        );
        assert_eq!(project.generation, 1);
        assert_eq!(project.sounds, vec!["sounds/beep".to_string()]);
        assert_eq!(project.music, vec!["music/win".to_string()]);
        let brick = &project.sprites["sprites/brick"];
        assert_eq!(brick.rgba.len() as u32, brick.width * brick.height * 4);

        // The fake object knows its parameters; its bounds match its size.
        let block = &project.models["models/block"];
        assert!(block.is_proxy());
        assert_eq!(block.source, "models/proxies/block.prim.ron");
        let size: Vec<f32> = (0..3).map(|k| block.max[k] - block.min[k]).collect();
        assert!(
            (size[0] - 2.0).abs() < 1e-4
                && (size[1] - 1.0).abs() < 1e-4
                && (size[2] - 4.0).abs() < 1e-4,
            "{size:?}"
        );
        assert_eq!(block.tris, 12, "a box is 12 triangles");
        let cube = &project.models["models/cube"];
        assert!(!cube.is_proxy());
        assert!(cube.verts > 0 && cube.tris > 0 && cube.radius > 0.0);
        assert_eq!(
            project.model_bounds()["models/block"],
            (block.min, block.max)
        );

        // Lookups by handle.
        assert_eq!(project.model_by_id(ModelId(cube.id)).unwrap().name, "cube");
        assert!(project.model_by_id(ModelId(1)).is_none());
        let mut found = project.logicals_for_ids(&[brick.id, block.id]);
        found.sort();
        assert_eq!(found, vec!["models/block", "sprites/brick"]);

        // Free names.
        assert_eq!(project.unique_model_name("tree"), "tree");
        assert_eq!(project.unique_model_name("block"), "block-2");
    }

    #[test]
    fn saving_a_fake_object_rebakes_it_in_place() {
        let (_dir, mut project) = sample_project();
        project.bake_and_load();
        let id = project.models["models/block"].id;
        let tall = Primitive {
            shape: trino_asset_pipeline::Shape::Box {
                size: [1.0, 6.0, 1.0],
            },
            ..Primitive::default()
        };
        let model = &project.models["models/block"];
        project.save_proxy(model, &tall).unwrap();
        let report = pipeline::bake_all(
            &project.assets_dir(),
            pipeline::Platform::Pc,
            &project.baked_dir(),
        )
        .unwrap();
        assert_eq!(
            report.changed_ids(),
            vec![id],
            "only the edited model rebakes"
        );
        assert_eq!(project.reload(Some(&[id])).unwrap(), 1);
        let block = &project.models["models/block"];
        assert_eq!(block.id, id, "same handle after the edit");
        assert!((block.max[1] - block.min[1] - 6.0).abs() < 1e-4);
        assert_eq!(block.proxy.as_ref(), Some(&tall));
        assert_eq!(
            project.sounds.len(),
            1,
            "a partial reload keeps the sound list"
        );

        // A model whose source vanished cannot be saved.
        let mut ghost = Project::new(project.root.clone());
        ghost.reload(None).unwrap();
        let mut m = ghost.models.remove("models/block").unwrap();
        m.source = "models/proxies/gone.prim.ron".into();
        assert!(ghost.save_proxy(&m, &tall).is_err());
        // An empty project fails to load instead of panicking.
        let empty = tempfile::tempdir().unwrap();
        let mut none = Project::new(empty.path().to_path_buf());
        let log = none.bake_and_load();
        assert!(log.iter().all(Result::is_err), "{log:?}");
    }

    #[test]
    fn create_proxy_writes_the_prim_and_the_manifest_entry_then_retargets() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("assets/shared/models")).unwrap();
        std::fs::write(
            root.join("assets/manifest.toml"),
            "# keep me\nversion = 1\n",
        )
        .unwrap();
        let project = Project::new(root.clone());
        let prim = Primitive::default();
        assert!(project.create_proxy("Bad Name", &prim).is_err());
        let logical = project.create_proxy("crate", &prim).unwrap();
        assert_eq!(logical, "models/crate");
        let file = root.join("assets/shared/models/proxies/crate.prim.ron");
        assert_eq!(Primitive::load(&file).unwrap(), prim);
        let manifest = std::fs::read_to_string(root.join("assets/manifest.toml")).unwrap();
        assert!(manifest.starts_with("# keep me"), "comments preserved");
        assert!(
            project.create_proxy("crate", &prim).is_err(),
            "no duplicates"
        );

        // The whole loop the editor relies on: it bakes like any model...
        let report = pipeline::bake_all(
            &root.join("assets"),
            pipeline::Platform::Pc,
            &root.join("baked"),
        )
        .unwrap();
        assert_eq!(report.entries.len(), 1);

        // ...and swapping in real art keeps the logical path.
        assert!(
            project
                .retarget_model("crate", "models/missing.glb")
                .is_err()
        );
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/shared/models/cube.glb"),
            root.join("assets/shared/models/crate.glb"),
        )
        .unwrap();
        project.retarget_model("crate", "models/crate.glb").unwrap();
        let m = Manifest::load(&root.join("assets/manifest.toml")).unwrap();
        assert_eq!(m.models["crate"].file, "models/crate.glb");
    }
}
