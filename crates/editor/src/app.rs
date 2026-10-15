//! The editor application: state, the command queue panels talk through,
//! menus / toolbar / status bar, file sync (hot reload of scenes, prefabs,
//! materials and assets), shortcuts and undo.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc::Receiver;
use std::time::Instant;

use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke, Vec2};
use trino_asset_pipeline as pipeline;
use trino_core::Vec3;
use trino_platform_pc::SimProfile;
use trino_scene::{
    Entity, Library, MaterialDef, ModelComponent, Prefab, PrefabInstance, Resolved, Scene,
    SpriteComponent, resolve,
};

use crate::dialogs::Dialog;
use crate::doc::{Doc, DocKind};
use crate::level_editor::LevelEditor;
use crate::log::{Level, Log};
use crate::project::{Project, Thumbnailer};
use crate::theme::{self, c, icon};
use crate::viewport::{AssetPayload, ViewAction, ViewInput, ViewKind, Viewport};

pub const DEFAULT_SCENE: &str = "scenes/showcase.scene.ron";
const LEVEL_PATH: &str = "examples/platformer/src/level1.txt";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Scene,
    Game,
    Level,
    Animation,
    Hierarchy,
    Inspector,
    Assets,
    Console,
}

impl Tab {
    pub fn title(self) -> String {
        let (glyph, name) = match self {
            Tab::Scene => (icon::CUBE_TRANSPARENT, "Scene"),
            Tab::Game => (icon::GAME_CONTROLLER, "Game"),
            Tab::Level => (icon::GRID_FOUR, "Level"),
            Tab::Animation => (icon::PERSON_SIMPLE_WALK, "Animation"),
            Tab::Hierarchy => (icon::TREE_STRUCTURE, "Hierarchy"),
            Tab::Inspector => (icon::SLIDERS_HORIZONTAL, "Inspector"),
            Tab::Assets => (icon::FOLDER_OPEN, "Assets"),
            Tab::Console => (icon::TERMINAL, "Console"),
        };
        format!("{glyph}  {name}")
    }
}

/// Asset selected in the Assets panel (shown in the Inspector).
#[derive(Clone, Debug, PartialEq)]
pub enum AssetSel {
    Model(String),
    Material(String),
    Prefab(String),
    Sprite(String),
    /// A particle effect, by name (`particles/<name>`).
    Particles(String),
}

/// Everything panels ask the app to do (applied after the UI pass).
pub enum Cmd {
    Select(Option<Vec<usize>>),
    SelectAsset(AssetSel),
    Create {
        parent: Option<Vec<usize>>,
        entity: Box<Entity>,
    },
    Delete(Vec<usize>),
    Duplicate(Vec<usize>),
    Reparent {
        from: Vec<usize>,
        to: Option<Vec<usize>>,
    },
    MakePrefab(Vec<usize>, String),
    Unpack(Vec<usize>),
    EditPrefab(String),
    ExitPrefab,
    Focus(Vec<usize>),
    Copy(Vec<usize>),
    Paste,
    Open(Dialog),
    AddAsset(AssetPayload, Option<Vec3>),
    /// Asset dropped on a hierarchy row: create it as that entity's child,
    /// at the parent's origin.
    AddAssetUnder(AssetPayload, Vec<usize>),
    SaveMaterial(String),
    NewMaterial(String, MaterialDef),
    CreateProxy(String, pipeline::Primitive, bool),
    SaveProxy(String, pipeline::Primitive),
    RetargetModel(String, String),
    OpenScene(PathBuf),
    NewScene(String),
    Export(PathBuf),
    Save,
    Undo,
    Redo,
    /// Ctrl/Shift+click: add to / remove from the selection.
    ToggleSelect(Vec<usize>),
    SelectMany(Vec<Vec<usize>>),
    SelectAll,
    /// Delete / duplicate everything selected.
    DeleteSelection,
    DuplicateSelection,
    Align(usize, crate::ops::AlignTo),
    Distribute(usize),
    /// Array: `count` copies of the active entity, `step` apart.
    ArrayCopies {
        count: usize,
        step: [f32; 3],
    },
    /// Drop the selection onto the surface below (End).
    DropToSurface,
    /// Push an instance's overrides into its prefab.
    ApplyOverrides(Vec<usize>),
    /// The in-memory library prefab changed (in-place editing): write it.
    SavePrefab(String),
    PlayHere,
    StopPlayHere,
    /// Files dropped on the window: models / sprites to import.
    ImportFiles(Vec<PathBuf>),
    FocusTab(Tab),
    /// Scene tabs: bring open scene `n` (index into `open_docs`) to front,
    /// or close it.
    SwitchScene(usize),
    CloseScene(usize),
    /// Write an impostor's settings (`assets/impostors/<name>.impostor.ron`).
    SaveImpostor(String, pipeline::impostor::ImpostorDef),
    /// Render it: atlases, tables and manifest entries (the same bake as
    /// `cargo xtask impostor bake <name>`).
    BakeImpostor(String),
    /// Bake a clip's pose into a static model (Animation › Console pose;
    /// the same as `cargo xtask model pose`).
    /// A new particle effect (`assets/shared/particles/<name>.particles.ron`
    /// + `[particles.<name>]`), like `cargo xtask particles new`.
    NewParticles(String),
    ExportPose {
        model: String,
        clip: Option<String>,
        time: f32,
        name: String,
    },
}

pub struct App {
    pub project: Project,
    pub lib: Library,
    pub doc: Doc,
    /// Scene document (and its camera) parked while a prefab is open.
    pub parked: Option<Doc>,
    /// Other open scenes (tabs), not in front.
    pub open_docs: Vec<Doc>,
    /// Command palette (Ctrl+K): open + query.
    pub palette: Option<String>,
    parked_cam: Option<crate::gizmo::OrbitCam>,
    pub asset_sel: Option<AssetSel>,
    /// The scene as the PC sees it (Scene view, picking, inspector).
    pub resolved: Resolved,
    /// The scene as the Game view's profile sees it: that platform's
    /// variants applied (the PC one when it simulates the PC).
    pub resolved_game: Resolved,
    pub scene_view: Viewport,
    pub game_view: Viewport,
    pub thumbs: Thumbnailer,
    pub material_thumbs: std::collections::HashMap<String, (MaterialDef, egui::TextureHandle)>,
    /// Prefab previews keyed by logical path, with the library revision and
    /// asset generation they were rendered at.
    pub prefab_thumbs: std::collections::HashMap<String, ((u64, u64), egui::TextureHandle)>,
    /// Edits waiting for the user to finish interacting before they hit the
    /// disk (dragging a color must not write a file — and rebake — per frame).
    pending_materials: HashSet<String>,
    pending_proxies: std::collections::HashMap<String, pipeline::Primitive>,
    /// Bumped when saved content settles: prefab thumbnails key on it.
    thumb_rev: u64,
    pub log: Log,
    pub cmds: Vec<Cmd>,
    pub dialog: Option<Dialog>,
    pub collapsed: HashSet<Vec<usize>>,
    /// Prefab instances expanded in the hierarchy (collapsed by default).
    pub expanded: HashSet<Vec<usize>>,
    pub renaming: Option<(Vec<usize>, String)>,
    pub hierarchy_filter: String,
    pub assets_tab: crate::panels::assets::AssetTab,
    pub assets_filter: String,
    pub level: LevelEditor,
    pub animate: bool,
    anim_start: Instant,
    rev: u64,
    rev_scene: Option<Scene>,
    lib_rev: u64,
    seen_lib_rev: u64,
    play: Option<Child>,
    asset_events: Receiver<Vec<u32>>,
    _asset_watcher: Box<dyn std::any::Any>,
    file_events: Receiver<Vec<PathBuf>>,
    _file_watcher: Box<dyn std::any::Any>,
    /// Last known text per watched file — our own writes and what we
    /// loaded (echo guard + "did the content actually change" filter).
    written: std::collections::HashMap<PathBuf, String>,
    clipboard_note: Option<String>,
    frames: u64,
    smoke_frames: Option<u64>,
    screenshot: Option<(u64, PathBuf)>,
    screenshot_requested: bool,
    last_frame: Instant,
    pub fps: f32,
    pub show_shortcuts: bool,
    dock: Option<egui_dock::DockState<Tab>>,
    /// A tab to bring to front on the next frame (Play here -> Game).
    focus_tab: Option<Tab>,
    /// Dev hook `TRINO_EDITOR_PLAY=here`: start Play here on the first frame.
    play_here_on_start: bool,
    /// Dev hook `TRINO_EDITOR_INSPECT=<section>`: the Inspector opens that
    /// section and scrolls to it (`platforms`, `lod`, `impostor`).
    pub inspect_focus: Option<String>,
    /// Inspector › LOD › Preview: (entity, distance) both views draw that
    /// entity's LOD at.
    pub lod_preview: Option<(Vec<usize>, f32)>,
    /// Impostor settings on disk, by name (`assets/impostors/`).
    pub impostor_defs: std::collections::BTreeMap<String, pipeline::impostor::ImpostorDef>,
    /// Atlas previews by impostor name, with the project generation they
    /// were made at.
    pub impostor_thumbs: std::collections::HashMap<String, (u64, egui::TextureHandle)>,
    /// The Animation tab.
    pub anim: crate::anim_panel::AnimPanel,
    /// Particle effects by name (the manifest's `[particles.*]` files).
    pub particles: std::collections::BTreeMap<String, pipeline::particles::ParticlesDef>,
    /// Effects edited in the inspector, written once the user settles.
    pub pending_particles: HashSet<String>,
    /// The particle inspector's live preview.
    pub particles_view: crate::panels::particles::ParticlesView,
    /// eframe's wgpu device (panels with their own preview renderer).
    pub render_state: eframe::egui_wgpu::RenderState,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        let render_state = cc
            .wgpu_render_state
            .clone()
            .expect("editor requires the wgpu backend");
        // Relative to the working directory, *without* a `./` prefix: watcher
        // paths are compared component-wise against these.
        let root = PathBuf::new();
        let mut log = Log::new();
        let mut project = Project::new(root.clone());
        for line in project.bake_and_load() {
            match line {
                Ok(l) => log.info(l),
                Err(e) => log.error(e),
            }
        }
        let mut lib = Library::load(&root);
        lib.model_bounds = project.model_bounds();
        lib.impostors = pipeline::impostor::baked(&project.assets_dir());
        for e in &lib.errors {
            log.error(e.clone());
        }
        log.info(format!(
            "library: {} material(s), {} prefab(s)",
            lib.materials.len(),
            lib.prefabs.len()
        ));

        let scene_path = PathBuf::from(
            std::env::var("TRINO_EDITOR_SCENE").unwrap_or_else(|_| DEFAULT_SCENE.to_string()),
        );
        let (scene, text) = match std::fs::read_to_string(&scene_path) {
            Ok(text) => match Scene::from_ron(&text) {
                Ok(s) => {
                    log.info(format!("opened {}", scene_path.display()));
                    (s, text)
                }
                Err(e) => {
                    log.error(e);
                    (Scene::new("main"), String::new())
                }
            },
            Err(_) => {
                log.warn("no scene found — starting an empty one");
                (Scene::new("main"), String::new())
            }
        };
        let mut doc = Doc::scene(scene_path, scene, text);
        // Dev hook (screenshots/tests): TRINO_EDITOR_SELECT=<name>[,<name>…]
        // selects them (the first is active) and frames it in the Scene view.
        let mut cmds = Vec::new();
        if let Ok(names) = std::env::var("TRINO_EDITOR_SELECT") {
            let mut picked: Vec<Vec<usize>> = Vec::new();
            for name in names.split(',').map(str::trim) {
                doc.scene.walk(|p, e| {
                    if e.name == name && !picked.iter().any(|q| q == p) {
                        picked.push(p.to_vec());
                    }
                });
            }
            doc.select_many(picked);
            if let Some(p) = &doc.selected {
                cmds.push(Cmd::Focus(p.clone()));
            }
        }
        // Dev hook: TRINO_EDITOR_OPEN=fake-object | material:<logical> |
        // model:<logical> | prefab:<logical> (edit mode) — screenshots of
        // dialogs and asset inspectors.
        let mut assets_tab = crate::panels::assets::AssetTab::Models;
        let open = std::env::var("TRINO_EDITOR_OPEN").unwrap_or_default();
        match open.split_once(':') {
            Some(("material", l)) => {
                assets_tab = crate::panels::assets::AssetTab::Materials;
                cmds.push(Cmd::SelectAsset(AssetSel::Material(l.to_string())));
            }
            Some(("model", l)) => cmds.push(Cmd::SelectAsset(AssetSel::Model(l.to_string()))),
            Some(("particles", n)) => {
                assets_tab = crate::panels::assets::AssetTab::Particles;
                let name = n.split_once('@').map_or(n, |(n, _)| n);
                cmds.push(Cmd::SelectAsset(AssetSel::Particles(name.to_string())));
            }
            Some(("prefab", l)) => {
                assets_tab = crate::panels::assets::AssetTab::Prefabs;
                cmds.push(Cmd::EditPrefab(l.to_string()));
            }
            _ if open == "fake-object" => cmds.push(Cmd::Open(Dialog::fake_object(&project))),
            _ if open == "array" => cmds.push(Cmd::Open(Dialog::array())),
            _ => {}
        }

        let doc_selected = doc.selected.clone();
        let mut scene_view =
            Viewport::new(render_state.clone(), ViewKind::Scene, SimProfile::Native);
        // Dev hook: TRINO_EDITOR_GAME=pc|n64|3ds picks the Game view's profile.
        let game_profile = match std::env::var("TRINO_EDITOR_GAME").as_deref() {
            Ok("pc") => SimProfile::Native,
            Ok("3ds") => SimProfile::N3ds,
            _ => SimProfile::N64,
        };
        let game_view = Viewport::new(render_state.clone(), ViewKind::Game, game_profile);
        let cam = doc.scene.environment.core_camera();
        scene_view.cam.look_from(cam.eye, cam.target);
        let thumbs = Thumbnailer::new(
            wgpu::Device::clone(&render_state.device),
            wgpu::Queue::clone(&render_state.queue),
        );

        // Asset pipeline watcher: masters → rebake → changed handles.
        let (tx, asset_events) = std::sync::mpsc::channel();
        let watcher = pipeline::watch::watch(
            project.assets_dir(),
            pipeline::Platform::Pc,
            project.baked_dir(),
            move |changed| {
                let _ = tx.send(changed);
            },
        )
        .expect("failed to start asset watcher");

        // Text sources: scenes, prefabs, materials, the platformer level.
        let (ftx, file_events) = std::sync::mpsc::channel();
        let mut file_watcher = notify_debouncer_full::new_debouncer(
            std::time::Duration::from_millis(200),
            None,
            move |result: notify_debouncer_full::DebounceEventResult| {
                if let Ok(events) = result {
                    // Reading a file emits inotify *access* events; reacting to
                    // those would make every reload trigger the next one.
                    use notify_debouncer_full::notify::EventKind;
                    let paths: Vec<PathBuf> = events
                        .into_iter()
                        .filter(|e| !matches!(e.event.kind, EventKind::Access(_)))
                        .flat_map(|e| e.event.paths.clone())
                        .collect();
                    if !paths.is_empty() {
                        let _ = ftx.send(paths);
                    }
                }
            },
        )
        .expect("failed to start file watcher");
        use notify_debouncer_full::notify::RecursiveMode;
        for dir in [
            "scenes",
            "prefabs",
            "assets/materials",
            "assets/impostors",
            "assets/shared/particles",
            "examples/platformer/src",
        ] {
            let _ = std::fs::create_dir_all(dir);
            if let Err(e) = file_watcher.watch(Path::new(dir), RecursiveMode::Recursive) {
                log.warn(format!("cannot watch {dir}: {e}"));
            }
        }

        let screenshot = std::env::var("TRINO_EDITOR_SCREENSHOT").ok().map(|p| {
            let frames = std::env::var("TRINO_EDITOR_SCREENSHOT_FRAME")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30);
            (frames, PathBuf::from(p))
        });

        App {
            project,
            lib,
            doc,
            parked: None,
            parked_cam: None,
            asset_sel: None,
            resolved: Resolved::default(),
            resolved_game: Resolved::default(),
            scene_view,
            game_view,
            thumbs,
            material_thumbs: Default::default(),
            prefab_thumbs: Default::default(),
            pending_materials: HashSet::new(),
            pending_proxies: Default::default(),
            thumb_rev: 0,
            log,
            cmds,
            dialog: None,
            collapsed: HashSet::new(),
            expanded: HashSet::new(),
            renaming: None,
            hierarchy_filter: String::new(),
            assets_tab,
            assets_filter: String::new(),
            level: LevelEditor::load(PathBuf::from(LEVEL_PATH)),
            animate: true,
            anim_start: Instant::now(),
            rev: 0,
            rev_scene: None,
            lib_rev: 0,
            seen_lib_rev: 0,
            play: None,
            asset_events,
            _asset_watcher: Box::new(watcher),
            file_events,
            _file_watcher: Box::new(file_watcher),
            written: Default::default(),
            clipboard_note: None,
            frames: 0,
            smoke_frames: std::env::var("TRINO_EDITOR_SMOKE_FRAMES")
                .ok()
                .and_then(|v| v.parse().ok()),
            screenshot,
            screenshot_requested: false,
            last_frame: Instant::now(),
            fps: 0.0,
            show_shortcuts: false,
            // Built on the first frame, when the window size is known.
            dock: None,
            focus_tab: None,
            open_docs: Vec::new(),
            // Dev hook: TRINO_EDITOR_PALETTE=<query> opens the palette.
            palette: std::env::var("TRINO_EDITOR_PALETTE").ok(),
            play_here_on_start: std::env::var("TRINO_EDITOR_PLAY").as_deref() == Ok("here"),
            inspect_focus: std::env::var("TRINO_EDITOR_INSPECT").ok(),
            // Dev hook: TRINO_EDITOR_LOD_PREVIEW=<m> previews the selected
            // entity's LOD at that distance.
            lod_preview: std::env::var("TRINO_EDITOR_LOD_PREVIEW")
                .ok()
                .and_then(|v| v.parse().ok())
                .zip(doc_selected)
                .map(|(d, p)| (p, d)),
            impostor_defs: load_impostor_defs(&root.join("assets")),
            impostor_thumbs: Default::default(),
            anim: crate::anim_panel::AnimPanel::from_env(),
            particles: load_particles(&root.join("assets")),
            pending_particles: HashSet::new(),
            // `TRINO_EDITOR_OPEN=particles:<name>@n64` previews that platform.
            particles_view: crate::panels::particles::ParticlesView {
                platform: match open.rsplit_once('@').map(|(_, p)| p) {
                    Some("n64") => pipeline::Platform::N64,
                    Some("3ds") => pipeline::Platform::N3ds,
                    _ => pipeline::Platform::Pc,
                },
                ..Default::default()
            },
            render_state,
        }
    }

    pub fn time(&self) -> f32 {
        self.anim_start.elapsed().as_secs_f32()
    }

    pub fn view_input(&self) -> ViewInput<'_> {
        ViewInput {
            scene: &self.doc.scene,
            resolved: &self.resolved,
            project: &self.project,
            selected: self.doc.selected.as_deref(),
            extra: &self.doc.extra,
            rev: self.rev,
            time: self.time(),
            animate: self.animate,
            lod_preview: self.lod_preview.as_ref().map(|(p, d)| (p.as_slice(), *d)),
        }
    }

    /// Entities using a material (inspector "shared by N").
    pub fn material_users(&self, logical: &str) -> usize {
        let mut n = 0;
        self.doc.scene.walk(|_, e| {
            if e.model.as_ref().and_then(|m| m.material.as_deref()) == Some(logical) {
                n += 1;
            }
        });
        n
    }

    pub fn mark_lib_changed(&mut self) {
        self.lib_rev += 1;
    }

    // ------------------------------------------------------------ files --

    fn write_echo(&mut self, path: PathBuf, text: String) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, &text)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        self.written.insert(path, text);
        Ok(())
    }

    fn sync_files(&mut self) {
        // Assets (rebaked by the pipeline watcher).
        let mut changed_ids: Vec<u32> = Vec::new();
        while let Ok(ids) = self.asset_events.try_recv() {
            changed_ids.extend(ids);
        }
        if !changed_ids.is_empty() {
            match self.project.reload(Some(&changed_ids)) {
                Ok(n) => {
                    // `Auto` colliders follow model edits; impostor LOD
                    // levels follow a fresh bake (its blobs changed).
                    self.lib.model_bounds = self.project.model_bounds();
                    let impostors = pipeline::impostor::baked(&self.project.assets_dir());
                    if impostors != self.lib.impostors {
                        self.lib.impostors = impostors;
                        self.mark_lib_changed();
                    }
                    let logicals = self.project.logicals_for_ids(&changed_ids);
                    self.scene_view.upload(&self.project, &logicals);
                    self.game_view.upload(&self.project, &logicals);
                    // Brand-new assets (not in the old catalog) need a full
                    // reload so every view uploads them.
                    if logicals.len() < n {
                        let _ = self.project.reload(None);
                    }
                    self.log.toast(
                        Level::Ok,
                        format!(
                            "{} {}",
                            icon::ARROWS_CLOCKWISE,
                            if logicals.len() == 1 {
                                format!("reloaded {}", logicals[0])
                            } else {
                                format!("reloaded {} assets", logicals.len().max(n))
                            }
                        ),
                    );
                }
                Err(e) => self
                    .log
                    .toast(Level::Error, format!("asset reload failed: {e}")),
            }
        }

        // Text files.
        let mut paths: Vec<PathBuf> = Vec::new();
        while let Ok(p) = self.file_events.try_recv() {
            paths.extend(p);
        }
        if paths.is_empty() {
            return;
        }
        paths.sort();
        paths.dedup();
        let cwd = std::env::current_dir().unwrap_or_default();
        let cwd_canon = std::fs::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone());
        let mut level_checked = false;
        for abs in paths {
            let p = relative_to(&abs, &cwd, &cwd_canon);
            let text = std::fs::read_to_string(&p).ok();
            if let Some(t) = &text
                && self.written.get(&p) == Some(t)
            {
                continue; // our own write, or content we already loaded
            }
            if let Some(t) = &text {
                self.written.insert(p.clone(), t.clone());
            }
            let s = p.to_string_lossy().replace('\\', "/");
            if s.ends_with(".material.ron") || s.ends_with(".prefab.ron") {
                match self.lib.reload_file(Path::new(""), &p) {
                    Ok(Some(logical)) => {
                        self.mark_lib_changed();
                        self.material_thumbs.remove(&logical);
                        // An open prefab that changed on disk.
                        if self.doc.path == p
                            && self.doc.disk_text != text.clone().unwrap_or_default()
                        {
                            self.external_doc_change(&p, text.clone());
                        }
                        self.log.toast(
                            Level::Ok,
                            format!("{} {logical} updated", icon::ARROWS_CLOCKWISE),
                        );
                    }
                    Ok(None) => {}
                    Err(e) => self.log.toast(Level::Error, e),
                }
            } else if s.ends_with(".scene.ron") {
                if p == self.doc.path || self.parked.as_ref().is_some_and(|d| d.path == p) {
                    self.external_doc_change(&p, text);
                }
            } else if s.ends_with(pipeline::particles::DEF_EXT) {
                // An effect changed on disk (an agent, a text editor): the
                // inspector and its preview follow.
                self.particles = load_particles(&self.project.assets_dir());
                self.log.toast(
                    Level::Ok,
                    format!("{} {} reloaded", icon::ARROWS_CLOCKWISE, p.display()),
                );
            } else if s.ends_with(pipeline::impostor::DEF_EXT) {
                // An agent (or another tool) edited impostor settings: show
                // them; the atlas changes when someone bakes.
                self.impostor_defs = load_impostor_defs(&self.project.assets_dir());
                self.log.toast(
                    Level::Ok,
                    format!(
                        "{} {} changed — Bake impostor (or cargo xtask impostor bake) to update",
                        icon::ARROWS_CLOCKWISE,
                        p.display()
                    ),
                );
            } else if s.ends_with("level1.txt") && !level_checked {
                level_checked = true;
                if self.level.maybe_external_reload() {
                    self.log
                        .toast(Level::Ok, "level reloaded (external change)");
                }
            }
        }
    }

    fn external_doc_change(&mut self, path: &Path, text: Option<String>) {
        let Some(text) = text else {
            return;
        };
        let doc = if self.doc.path == path {
            &mut self.doc
        } else if let Some(d) = self.parked.as_mut().filter(|d| d.path == path) {
            d
        } else {
            return;
        };
        if text == doc.disk_text {
            return;
        }
        if doc.dirty() {
            self.log.toast(
                Level::Warn,
                format!(
                    "{} changed on disk, but you have unsaved edits — save to overwrite or reopen to load it",
                    path.display()
                ),
            );
            return;
        }
        let parsed = match &doc.kind {
            DocKind::Scene => Scene::from_ron(&text),
            DocKind::Prefab { logical } => Prefab::from_ron(&text).map(|p| {
                let mut s = Scene::new(logical.clone());
                s.entities.push(p.root);
                s
            }),
        };
        match parsed {
            Ok(scene) => {
                doc.reload_from(scene, text);
                self.log.toast(
                    Level::Ok,
                    format!("{} reloaded (external change)", path.display()),
                );
            }
            Err(e) => self
                .log
                .toast(Level::Error, format!("external change is invalid: {e}")),
        }
    }

    // --------------------------------------------------------- commands --

    fn run_cmds(&mut self, ctx: &egui::Context) {
        let cmds = std::mem::take(&mut self.cmds);
        for cmd in cmds {
            if let Err(e) = self.run(cmd, ctx) {
                self.log.toast(Level::Error, e);
            }
        }
    }

    fn run(&mut self, cmd: Cmd, ctx: &egui::Context) -> Result<(), String> {
        match cmd {
            Cmd::Select(path) => {
                self.doc.select_only(path);
                if self.doc.selected.is_some() {
                    self.asset_sel = None;
                }
            }
            Cmd::ToggleSelect(path) => {
                self.doc.toggle(path);
                self.asset_sel = None;
            }
            Cmd::SelectMany(paths) => {
                let n = paths.len();
                self.doc.select_many(paths);
                if n > 0 {
                    self.asset_sel = None;
                    self.log.info(format!("selected {n} entities"));
                }
            }
            Cmd::SelectAll => {
                let mut all = Vec::new();
                for (i, _) in self.doc.scene.entities.iter().enumerate() {
                    all.push(vec![i]);
                }
                self.doc.select_many(all);
            }
            Cmd::DeleteSelection => {
                let mut sel = self.doc.selection();
                if self.doc.is_prefab() {
                    sel.retain(|p| p != &[0]);
                }
                let gone = crate::ops::remove_many(&mut self.doc.scene, &sel);
                self.doc.select_only(None);
                self.log.info(format!("deleted {}", gone.join(", ")));
            }
            Cmd::DuplicateSelection => {
                let mut sel = self.doc.selection();
                if self.doc.is_prefab() {
                    sel.retain(|p| p != &[0]);
                }
                // Back to front so earlier paths stay valid.
                sel.sort();
                let mut new = Vec::new();
                for path in sel.iter().rev() {
                    let copies = crate::ops::array(&mut self.doc.scene, path, 1, [1.0, 0.0, 0.0]);
                    new.extend(copies);
                }
                self.doc.select_many(new);
            }
            Cmd::Align(axis, to) => {
                let sel = self.doc.selection();
                let n = crate::ops::align(&mut self.doc.scene, &sel, axis, to);
                if n == 0 {
                    return Err("align needs at least two selected entities".into());
                }
            }
            Cmd::Distribute(axis) => {
                let sel = self.doc.selection();
                let n = crate::ops::distribute(&mut self.doc.scene, &sel, axis);
                if n == 0 {
                    return Err("distribute needs at least three selected entities".into());
                }
            }
            Cmd::ArrayCopies { count, step } => {
                let sel = self
                    .doc
                    .selected
                    .clone()
                    .ok_or("select an entity to array")?;
                let new = crate::ops::array(&mut self.doc.scene, &sel, count, step);
                self.log.info(format!("array: {} copies", new.len()));
                let mut all = vec![sel];
                all.extend(new);
                self.doc.select_many(all);
            }
            Cmd::DropToSurface => {
                self.resolved = resolve(&self.doc.scene, &self.lib, self.time());
                let boxes: Vec<(Vec<usize>, trino_core::Aabb)> = self
                    .resolved
                    .items
                    .iter()
                    .filter_map(|item| {
                        let (id, _) = item.model.as_ref()?;
                        let m = self.project.model_by_id(*id)?;
                        let local = trino_core::Aabb::new(
                            trino_core::Vec3::new(m.min[0], m.min[1], m.min[2]),
                            trino_core::Vec3::new(m.max[0], m.max[1], m.max[2]),
                        );
                        Some((item.owner.clone(), local.transformed(&item.matrix)))
                    })
                    .collect();
                for path in self.doc.selection() {
                    let own = boxes
                        .iter()
                        .filter(|(o, _)| o.starts_with(&path))
                        .map(|(_, b)| *b)
                        .reduce(|a, b| a.union(&b));
                    let others: Vec<trino_core::Aabb> = boxes
                        .iter()
                        .filter(|(o, _)| !o.starts_with(&path))
                        .map(|(_, b)| *b)
                        .collect();
                    crate::ops::drop_to_surface(&mut self.doc.scene, &path, own, &others);
                }
            }
            Cmd::ApplyOverrides(path) => {
                let (logical, prefab, stale) = self
                    .lib
                    .apply_overrides_to_prefab(&mut self.doc.scene, &path)?;
                let file = Library::prefab_file(Path::new(""), &logical);
                self.write_echo(file, prefab.to_ron())?;
                self.mark_lib_changed();
                for s in stale {
                    self.log.warn(format!("dropped stale override `{s}`"));
                }
                self.log.toast(
                    Level::Ok,
                    format!(
                        "{} overrides applied to {logical} — every instance follows",
                        icon::PACKAGE
                    ),
                );
            }
            Cmd::SavePrefab(logical) => {
                let prefab = self
                    .lib
                    .prefabs
                    .get(&logical)
                    .ok_or("unknown prefab")?
                    .clone();
                let file = Library::prefab_file(Path::new(""), &logical);
                self.write_echo(file, prefab.to_ron())?;
                self.mark_lib_changed();
            }
            Cmd::PlayHere => self.start_play_here(),
            Cmd::StopPlayHere => self.stop_play_here(),
            Cmd::ImportFiles(files) => {
                for f in files {
                    match self.import_file(&f) {
                        Ok(msg) => self.log.toast(Level::Ok, msg),
                        Err(e) => self.log.toast(Level::Error, e),
                    }
                }
            }
            Cmd::FocusTab(tab) => self.focus_tab = Some(tab),
            Cmd::SwitchScene(i) => {
                if i >= self.open_docs.len() {
                    return Ok(());
                }
                if self.doc.is_prefab() {
                    self.run(Cmd::ExitPrefab, ctx)?;
                }
                std::mem::swap(&mut self.doc, &mut self.open_docs[i]);
                let cam = self.doc.scene.environment.core_camera();
                self.scene_view.cam.look_from(cam.eye, cam.target);
            }
            Cmd::CloseScene(i) => {
                if i >= self.open_docs.len() {
                    return Ok(());
                }
                if self.open_docs[i].dirty() {
                    self.open_docs[i].save()?;
                }
                let d = self.open_docs.remove(i);
                self.log.info(format!("closed {}", d.path.display()));
            }
            Cmd::SelectAsset(a) => {
                self.asset_sel = Some(a);
                self.doc.selected = None;
            }
            Cmd::Create { parent, entity } => {
                let mut entity = *entity;
                // Prefab documents have exactly one root.
                let parent = if self.doc.is_prefab() && parent.is_none() {
                    Some(vec![0])
                } else {
                    parent
                };
                entity.name = self.doc.scene.unique_name(&entity.name);
                let path = self
                    .doc
                    .scene
                    .insert(parent.as_deref(), entity)
                    .ok_or("cannot create there")?;
                if let Some(p) = &parent {
                    self.collapsed.remove(p);
                }
                self.doc.selected = Some(path);
                self.asset_sel = None;
            }
            Cmd::Delete(path) => {
                if self.doc.is_prefab() && path == [0] {
                    return Err("the prefab root cannot be deleted".into());
                }
                let e = self.doc.scene.remove(&path).ok_or("nothing to delete")?;
                self.doc.selected = None;
                self.log.info(format!("deleted {}", e.name));
            }
            Cmd::Duplicate(path) => {
                if self.doc.is_prefab() && path == [0] {
                    return Err("duplicate the prefab's children, not its root".into());
                }
                let mut e = self
                    .doc
                    .scene
                    .get(&path)
                    .cloned()
                    .ok_or("nothing to duplicate")?;
                e.name = self.doc.scene.unique_name(&e.name);
                e.transform.pos[0] += 1.0;
                let list = self.doc.scene.siblings_mut(&path).ok_or("bad path")?;
                let at = path.last().unwrap() + 1;
                list.insert(at, e);
                let mut new = path.clone();
                *new.last_mut().unwrap() = at;
                self.doc.selected = Some(new);
            }
            Cmd::Reparent { from, to } => {
                if self.doc.is_prefab() && (from == [0] || to.is_none()) {
                    return Err("a prefab has a single root".into());
                }
                let new = self
                    .doc
                    .scene
                    .reparent(&from, to.as_deref())
                    .ok_or("cannot move an entity into itself")?;
                self.doc.selected = Some(new);
            }
            Cmd::MakePrefab(path, name) => {
                if !pipeline::manifest::edit::valid_name(&name) {
                    return Err(format!("`{name}` is not a valid prefab name"));
                }
                let logical = format!("prefabs/{name}");
                if self.lib.prefabs.contains_key(&logical) {
                    return Err(format!("{logical} already exists"));
                }
                let prefab = self.lib.make_prefab(&mut self.doc.scene, &path, &logical)?;
                let file = Library::prefab_file(Path::new(""), &logical);
                self.write_echo(file, prefab.to_ron())?;
                self.mark_lib_changed();
                self.log
                    .toast(Level::Ok, format!("{} created {logical}", icon::PACKAGE));
            }
            Cmd::Unpack(path) => {
                self.lib.unpack(&mut self.doc.scene, &path)?;
                self.collapsed.remove(&path);
                self.log
                    .toast(Level::Ok, "prefab unpacked (instance made real)");
            }
            Cmd::EditPrefab(logical) => {
                if self.doc.is_prefab() {
                    self.run(Cmd::ExitPrefab, ctx)?;
                }
                let prefab = self
                    .lib
                    .prefabs
                    .get(&logical)
                    .ok_or("unknown prefab")?
                    .clone();
                let path = Library::prefab_file(Path::new(""), &logical);
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let doc = Doc::prefab(logical.clone(), path, &prefab, text);
                self.parked = Some(std::mem::replace(&mut self.doc, doc));
                self.parked_cam = Some(self.scene_view.cam);
                self.asset_sel = None;
                // Frame the prefab once it has been resolved this frame.
                self.cmds.push(Cmd::Focus(vec![0]));
                self.log.info(format!(
                    "editing {logical} — every instance updates on save"
                ));
            }
            Cmd::ExitPrefab => {
                if !self.doc.is_prefab() {
                    return Ok(());
                }
                if self.doc.dirty() {
                    self.save_doc()?;
                }
                if let Some(scene) = self.parked.take() {
                    self.doc = scene;
                }
                if let Some(cam) = self.parked_cam.take() {
                    self.scene_view.cam = cam;
                }
            }
            Cmd::Focus(path) => {
                // Resolve now: the document may have just changed (prefab mode).
                self.resolved = resolve(&self.doc.scene, &self.lib, self.time());
                let input = self.view_input();
                if let Some((c, r)) = crate::viewport::selection_sphere(&input, &path) {
                    self.scene_view.cam.focus(c, r);
                }
            }
            Cmd::Copy(path) => {
                let e = self.doc.scene.get(&path).ok_or("nothing selected")?;
                let text = ron::ser::to_string_pretty(e, ron::ser::PrettyConfig::default())
                    .map_err(|e| e.to_string())?;
                ctx.copy_text(text);
                self.log.toast(
                    Level::Ok,
                    format!("{} copied {} as RON", icon::COPY, e.name),
                );
            }
            Cmd::Paste => {
                self.clipboard_note = Some(String::new());
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            }
            Cmd::Open(d) => self.dialog = Some(d),
            Cmd::AddAssetUnder(payload, parent) => {
                self.run(Cmd::AddAsset(payload, None), ctx)?;
                // Re-target the Create just queued at the requested parent.
                if let Some(Cmd::Create { parent: p, entity }) = self.cmds.last_mut() {
                    *p = Some(parent);
                    if entity.sprite.is_none() {
                        entity.transform.pos = [0.0; 3];
                    }
                }
            }
            Cmd::AddAsset(payload, at) => {
                let at = at.unwrap_or(self.scene_view.cam.target);
                let at = [snap_half(at.x), snap_half(at.y.max(0.0)), snap_half(at.z)];
                let entity = match &payload {
                    AssetPayload::Model(l) => {
                        let name = l.rsplit('/').next().unwrap_or("model").to_string();
                        let mut e = Entity::new(name).with_model(ModelComponent::new(l.clone()));
                        e.transform.pos = at;
                        e
                    }
                    AssetPayload::Prefab(l) => {
                        let name = l.rsplit('/').next().unwrap_or("prefab").to_string();
                        let mut e = Entity::new(name);
                        e.prefab = Some(PrefabInstance::new(l.clone()));
                        e.transform.pos = at;
                        e
                    }
                    AssetPayload::Sprite(l) => {
                        let name = l.rsplit('/').next().unwrap_or("sprite").to_string();
                        let mut e = Entity::new(name);
                        e.sprite = Some(SpriteComponent::new(l.clone()));
                        e.transform.pos = [144.0, 104.0, 0.0];
                        e
                    }
                };
                if let (AssetPayload::Prefab(l), DocKind::Prefab { logical }) =
                    (&payload, &self.doc.kind)
                    && l == logical
                {
                    return Err("a prefab cannot contain itself".into());
                }
                self.cmds.push(Cmd::Create {
                    parent: None,
                    entity: Box::new(entity),
                });
            }
            Cmd::SaveMaterial(logical) => {
                // The in-memory library already has the edit (views update
                // live); the file is written once the interaction settles.
                self.pending_materials.insert(logical);
                self.mark_lib_changed();
            }
            Cmd::NewMaterial(name, def) => {
                if !pipeline::manifest::edit::valid_name(&name) {
                    return Err(format!("`{name}` is not a valid material name"));
                }
                let logical = format!("materials/{name}");
                if self.lib.materials.contains_key(&logical) {
                    return Err(format!("{logical} already exists"));
                }
                self.lib.materials.insert(logical.clone(), def);
                self.run(Cmd::SaveMaterial(logical.clone()), ctx)?;
                self.asset_sel = Some(AssetSel::Material(logical.clone()));
                self.doc.selected = None;
                self.log
                    .toast(Level::Ok, format!("{} created {logical}", icon::PALETTE));
            }
            Cmd::CreateProxy(name, prim, add) => {
                let logical = self.project.create_proxy(&name, &prim)?;
                self.log.toast(
                    Level::Ok,
                    format!("{} fake object {logical} created", icon::GHOST),
                );
                // The asset watcher bakes it (~200 ms) and every view
                // uploads it; baking here too would race that thread.
                if add {
                    self.run(Cmd::AddAsset(AssetPayload::Model(logical), None), ctx)?;
                }
            }
            Cmd::SaveProxy(logical, prim) => {
                if let Some(m) = self.project.models.get_mut(&logical) {
                    m.proxy = Some(prim.clone());
                }
                self.pending_proxies.insert(logical, prim);
            }
            Cmd::SaveImpostor(name, def) => {
                let path = pipeline::impostor::save_def(&self.project.assets_dir(), &name, &def)?;
                self.written.insert(
                    path.clone(),
                    std::fs::read_to_string(&path).unwrap_or_default(),
                );
                self.impostor_defs.insert(name, def);
            }
            Cmd::BakeImpostor(name) => {
                let r = pipeline::impostor::bake(&self.project.assets_dir(), &name)?;
                self.log.toast(
                    Level::Ok,
                    format!(
                        "{} impostor {name}: {} views, {}x{} atlas, N64 {}x{} CI4{}",
                        icon::IMAGE_SQUARE,
                        r.pc.impostor.views,
                        r.pc.width,
                        r.pc.height,
                        r.n64.width,
                        r.n64.height,
                        if r.written.is_empty() {
                            " (unchanged)"
                        } else {
                            ""
                        }
                    ),
                );
                for f in &r.written {
                    self.log.info(format!("wrote assets/{f}"));
                }
                // The asset watcher rebakes the sprite and the blob; the
                // library picks the tables up on that reload.
            }
            Cmd::NewParticles(name) => {
                let sprite = self
                    .project
                    .sprites
                    .keys()
                    .next()
                    .cloned()
                    .unwrap_or_else(|| "sprites/particle".into());
                let def = pipeline::particles::ParticlesDef::new(sprite);
                let assets = self.project.assets_dir();
                pipeline::particles::save(&assets, &name, &def)?;
                let file = pipeline::particles::def_file(&assets, &name);
                self.written.insert(file, def.to_ron());
                self.particles.insert(name.clone(), def);
                self.asset_sel = Some(AssetSel::Particles(name.clone()));
                self.log.toast(
                    Level::Ok,
                    format!("{} particles/{name} created", icon::SPARKLE),
                );
            }
            Cmd::ExportPose {
                model,
                clip,
                time,
                name,
            } => {
                let e = trino_asset_pipeline::anim::export_pose(
                    &self.project.assets_dir(),
                    &model,
                    clip.as_deref(),
                    time,
                    &name,
                )?;
                self.log.toast(
                    Level::Ok,
                    format!(
                        "{} {} exported: {} tris, N64 ≈{:.1} ms per draw",
                        icon::CUBE,
                        e.logical,
                        e.tris,
                        e.n64_us as f32 / 1000.0
                    ),
                );
                self.log.info(format!("wrote assets/{}", e.file));
            }
            Cmd::RetargetModel(name, file) => {
                self.project.retarget_model(&name, &file)?;
                self.log.toast(
                    Level::Ok,
                    format!("models/{name} now uses {file} — same handle, every reference kept"),
                );
            }
            Cmd::OpenScene(path) => {
                if self.doc.is_prefab() {
                    self.run(Cmd::ExitPrefab, ctx)?;
                }
                // Already open in a tab: bring it to front.
                if let Some(i) = self.open_docs.iter().position(|d| d.path == path) {
                    return self.run(Cmd::SwitchScene(i), ctx);
                }
                if self.doc.path == path {
                    return Ok(());
                }
                let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
                let scene = Scene::from_ron(&text)?;
                let old = std::mem::replace(&mut self.doc, Doc::scene(path.clone(), scene, text));
                self.open_docs.push(old);
                let cam = self.doc.scene.environment.core_camera();
                self.scene_view.cam.look_from(cam.eye, cam.target);
                self.log
                    .toast(Level::Ok, format!("opened {}", path.display()));
            }
            Cmd::NewScene(name) => {
                if !pipeline::manifest::edit::valid_name(&name) {
                    return Err(format!("`{name}` is not a valid scene name"));
                }
                let path = PathBuf::from(format!("scenes/{name}.scene.ron"));
                if path.exists() {
                    return Err(format!("{} already exists", path.display()));
                }
                if self.doc.dirty() {
                    self.save_doc()?;
                }
                let mut scene = Scene::new(name);
                let mut ground = Entity::new("spawn");
                ground.tags.push("spawn".into());
                scene.entities.push(ground);
                self.doc = Doc::scene(path, scene, String::new());
                self.save_doc()?;
            }
            Cmd::Export(out) => {
                let (code, warnings) = trino_scene::codegen::to_rust(&self.doc.scene, &self.lib);
                for w in warnings {
                    self.log.warn(w);
                }
                std::fs::write(&out, code).map_err(|e| e.to_string())?;
                self.log.toast(
                    Level::Ok,
                    format!("{} exported {}", icon::CODE, out.display()),
                );
            }
            Cmd::Save => self.save_doc()?,
            Cmd::Undo => {
                if self.doc.undo() {
                    self.log.info("undo");
                }
            }
            Cmd::Redo => {
                if self.doc.redo() {
                    self.log.info("redo");
                }
            }
        }
        Ok(())
    }

    /// Write pending material / fake-object edits (called once input settles).
    fn flush_pending(&mut self) {
        let mats: Vec<String> = self.pending_materials.drain().collect();
        for logical in &mats {
            if let Some(def) = self.lib.materials.get(logical).cloned() {
                let file = Library::material_file(Path::new(""), logical);
                if let Err(e) = self.write_echo(file, def.to_ron()) {
                    self.log.toast(Level::Error, e);
                }
            }
        }
        let proxies: Vec<(String, pipeline::Primitive)> = self.pending_proxies.drain().collect();
        for (logical, prim) in &proxies {
            let saved = self
                .project
                .models
                .get(logical)
                .map(|m| self.project.save_proxy(m, prim));
            if let Some(Err(e)) = saved {
                self.log.toast(Level::Error, e);
            }
        }
        if !mats.is_empty() || !proxies.is_empty() {
            self.thumb_rev += 1;
        }
        let effects: Vec<String> = self.pending_particles.drain().collect();
        for name in effects {
            if let Some(def) = self.particles.get(&name).cloned() {
                let assets = self.project.assets_dir();
                let file = pipeline::particles::def_file(&assets, &name);
                match pipeline::particles::save(&assets, &name, &def) {
                    Ok(_) => {
                        self.written.insert(file, def.to_ron());
                    }
                    Err(e) => self.log.toast(Level::Error, e),
                }
            }
        }
    }

    pub fn save_doc(&mut self) -> Result<(), String> {
        let path = self.doc.path.clone();
        self.doc.save()?;
        self.written
            .insert(path.clone(), self.doc.disk_text.clone());
        if let DocKind::Prefab { logical } = &self.doc.kind {
            if let Some(root) = self.doc.scene.entities.first() {
                self.lib
                    .prefabs
                    .insert(logical.clone(), Prefab::new(root.clone()));
            }
            self.mark_lib_changed();
        }
        self.log.toast(
            Level::Ok,
            format!("{} saved {}", icon::FLOPPY_DISK, path.display()),
        );
        Ok(())
    }

    fn handle_paste(&mut self, ctx: &egui::Context) {
        if self.clipboard_note.is_none() {
            return;
        }
        let pasted = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Paste(t) => Some(t.clone()),
                _ => None,
            })
        });
        if let Some(text) = pasted {
            self.clipboard_note = None;
            match ron::from_str::<Entity>(&text) {
                Ok(e) => {
                    let parent = self.doc.selected.as_ref().and_then(|p| {
                        let mut p = p.clone();
                        p.pop();
                        (!p.is_empty()).then_some(p)
                    });
                    self.cmds.push(Cmd::Create {
                        parent,
                        entity: Box::new(e),
                    });
                }
                Err(e) => self.log.toast(
                    Level::Warn,
                    format!("clipboard is not an entity (RON): {e}"),
                ),
            }
        }
    }

    // ------------------------------------------------------------- chrome --

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.label(
                RichText::new(format!("{} Trino", icon::CUBE))
                    .family(theme::bold())
                    .color(c::ACCENT_TEXT),
            );
            ui.add_space(6.0);
            ui.menu_button("File", |ui| {
                if ui
                    .button(format!("{}  New scene…", icon::FILE_PLUS))
                    .clicked()
                {
                    self.cmds.push(Cmd::Open(Dialog::new_scene()));
                }
                ui.menu_button(format!("{}  Open scene", icon::FOLDER_OPEN), |ui| {
                    let mut scenes: Vec<PathBuf> = std::fs::read_dir("scenes")
                        .map(|d| {
                            d.flatten()
                                .map(|e| e.path())
                                .filter(|p| p.to_string_lossy().ends_with(".scene.ron"))
                                .collect()
                        })
                        .unwrap_or_default();
                    scenes.sort();
                    for s in scenes {
                        if ui.button(s.display().to_string()).clicked() {
                            self.cmds.push(Cmd::OpenScene(s));
                        }
                    }
                });
                ui.separator();
                if ui
                    .add(
                        egui::Button::new(format!("{}  Save", icon::FLOPPY_DISK))
                            .shortcut_text("Ctrl+S"),
                    )
                    .clicked()
                {
                    self.cmds.push(Cmd::Save);
                }
                if ui
                    .button(format!("{}  Export scene to game code…", icon::CODE))
                    .clicked()
                {
                    self.cmds
                        .push(Cmd::Open(Dialog::export(&self.doc.scene.name)));
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui
                    .add_enabled(
                        self.doc.can_undo(),
                        egui::Button::new(format!("{}  Undo", icon::ARROW_U_UP_LEFT))
                            .shortcut_text("Ctrl+Z"),
                    )
                    .clicked()
                {
                    self.cmds.push(Cmd::Undo);
                }
                if ui
                    .add_enabled(
                        self.doc.can_redo(),
                        egui::Button::new(format!("{}  Redo", icon::ARROW_U_UP_RIGHT))
                            .shortcut_text("Ctrl+Y"),
                    )
                    .clicked()
                {
                    self.cmds.push(Cmd::Redo);
                }
                ui.separator();
                let sel = self.doc.selected.clone();
                if ui
                    .add_enabled(
                        sel.is_some(),
                        egui::Button::new(format!("{}  Duplicate", icon::COPY))
                            .shortcut_text("Ctrl+D"),
                    )
                    .clicked()
                {
                    self.cmds.push(Cmd::DuplicateSelection);
                }
                if ui
                    .add_enabled(
                        sel.is_some(),
                        egui::Button::new(format!("{}  Copy as RON", icon::CODE))
                            .shortcut_text("Ctrl+C"),
                    )
                    .clicked()
                {
                    self.cmds.push(Cmd::Copy(sel.clone().unwrap()));
                }
                if ui
                    .add(
                        egui::Button::new(format!("{}  Paste entity", icon::DOWNLOAD_SIMPLE))
                            .shortcut_text("Ctrl+V"),
                    )
                    .clicked()
                {
                    self.cmds.push(Cmd::Paste);
                }
                if ui
                    .add_enabled(
                        sel.is_some(),
                        egui::Button::new(format!("{}  Delete", icon::TRASH)).shortcut_text("Del"),
                    )
                    .clicked()
                {
                    self.cmds.push(Cmd::DeleteSelection);
                }
                ui.separator();
                if ui
                    .add(
                        egui::Button::new(format!("{}  Select all", icon::SELECTION_ALL))
                            .shortcut_text("Ctrl+A"),
                    )
                    .clicked()
                {
                    self.cmds.push(Cmd::SelectAll);
                }
                if ui
                    .add(
                        egui::Button::new(format!("{}  Command palette", icon::COMMAND))
                            .shortcut_text("Ctrl+K"),
                    )
                    .clicked()
                {
                    self.palette = Some(String::new());
                }
            });
            ui.menu_button("Arrange", |ui| self.arrange_menu(ui));
            ui.menu_button("Create", |ui| {
                crate::panels::hierarchy::create_menu(self, ui, None);
            });
            ui.menu_button("View", |ui| {
                ui.checkbox(&mut self.scene_view.show_grid, "Grid");
                ui.checkbox(&mut self.animate, "Preview animations");
                ui.checkbox(&mut self.scene_view.snap, "Snap");
                ui.separator();
                if ui
                    .button(format!("{}  Keyboard shortcuts", icon::INFO))
                    .clicked()
                {
                    self.show_shortcuts = true;
                }
            });
            ui.menu_button("Help", |ui| {
                if ui
                    .button(format!("{}  Keyboard shortcuts", icon::INFO))
                    .clicked()
                {
                    self.show_shortcuts = true;
                }
                ui.label(
                    RichText::new("AGENTS.md documents every file format.").color(c::TEXT_WEAK),
                );
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            // Play / Stop.
            let playing = self.play.is_some() || self.game_view.play.is_some();
            let t = ui
                .ctx()
                .animate_bool_with_time(egui::Id::new("play-anim"), playing, 0.25);
            let (glyph, label, color) = if playing {
                (icon::STOP, "Stop", c::ERROR)
            } else {
                (icon::PLAY, "Play", c::PLAY)
            };
            let fill = theme::mix(c::BG2, color.gamma_multiply(0.25), t.max(0.35));
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(format!("{glyph}  {label}"))
                            .color(color)
                            .family(theme::bold()),
                    )
                    .fill(fill)
                    .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
                    .min_size(Vec2::new(84.0, 28.0)),
                )
                .on_hover_text(if self.doc.is_prefab() {
                    "Run the game in a separate process (Ctrl+P)"
                } else {
                    "Play this scene in a separate process — it live-reloads as you edit (Ctrl+P)"
                })
                .clicked()
            {
                if self.game_view.play.is_some() {
                    self.stop_play_here();
                } else {
                    self.toggle_play(!self.doc.is_prefab());
                }
            }
            if !playing {
                ui.menu_button(RichText::new(icon::CARET_DOWN).color(c::TEXT_WEAK), |ui| {
                    if ui
                        .button(format!(
                            "{}  Play here (in the Game view)",
                            icon::PLAY_CIRCLE
                        ))
                        .on_hover_text(
                            "Run this scene inside the editor — unsaved edits included. \
                             Tags drive it: `player` walks (WASD/arrows, Space jumps), \
                             `coin` collects, `goal` wins (Ctrl+Shift+P)",
                        )
                        .clicked()
                    {
                        self.start_play_here();
                    }
                    if ui
                        .button(format!("{}  Play this scene", icon::CUBE_TRANSPARENT))
                        .clicked()
                    {
                        self.toggle_play(true);
                    }
                    if ui
                        .button(format!(
                            "{}  Play the game (castle64)",
                            icon::GAME_CONTROLLER
                        ))
                        .clicked()
                    {
                        self.toggle_play(false);
                    }
                });
            }
            if playing {
                let pulse = (self.time() * 3.0).sin() * 0.5 + 0.5;
                theme::pill(
                    ui,
                    &format!("{} running", icon::LIGHTNING),
                    c::PLAY,
                    c::PLAY.gamma_multiply(0.12 + 0.1 * pulse),
                );
                ui.ctx().request_repaint();
            }
            ui.separator();
            if theme::icon_button(ui, icon::ARROW_U_UP_LEFT, "Undo (Ctrl+Z)").clicked() {
                self.cmds.push(Cmd::Undo);
            }
            if theme::icon_button(ui, icon::ARROW_U_UP_RIGHT, "Redo (Ctrl+Y)").clicked() {
                self.cmds.push(Cmd::Redo);
            }
            if theme::icon_button(ui, icon::FLOPPY_DISK, "Save (Ctrl+S)").clicked() {
                self.cmds.push(Cmd::Save);
            }
            ui.separator();
            // Breadcrumb: Scene › prefab.
            match &self.doc.kind {
                DocKind::Scene => {
                    ui.label(RichText::new(icon::CUBE_TRANSPARENT).color(c::TEXT_WEAK));
                    ui.label(RichText::new(&self.doc.scene.name).family(theme::bold()));
                }
                DocKind::Prefab { logical } => {
                    let parked = self
                        .parked
                        .as_ref()
                        .map(|d| d.scene.name.clone())
                        .unwrap_or_default();
                    if ui
                        .add(
                            egui::Button::new(format!("{}  {parked}", icon::CARET_LEFT))
                                .frame_when_inactive(false),
                        )
                        .on_hover_text("Back to the scene (saves the prefab)")
                        .clicked()
                    {
                        self.cmds.push(Cmd::ExitPrefab);
                    }
                    ui.label(RichText::new(icon::CARET_RIGHT).color(c::TEXT_FAINT));
                    theme::pill(
                        ui,
                        &format!("{} {logical}", icon::PACKAGE),
                        c::PREFAB,
                        c::PREFAB.gamma_multiply(0.15),
                    );
                }
            }
            let dirty = self.doc.dirty();
            let t = ui
                .ctx()
                .animate_bool_with_time(egui::Id::new("dirty-dot"), dirty, 0.3);
            if t > 0.01 {
                ui.label(RichText::new("●").color(c::WARN.gamma_multiply(t)))
                    .on_hover_text("Unsaved changes");
            }
            // Other open scenes (tabs): click to switch, × to close.
            for (i, d) in self.open_docs.iter().enumerate() {
                ui.add_space(6.0);
                let label = format!(
                    "{} {}{}",
                    icon::CUBE_TRANSPARENT,
                    d.scene.name,
                    if d.dirty() { " ●" } else { "" }
                );
                if ui
                    .add(egui::Button::new(RichText::new(label).color(c::TEXT_WEAK)).small())
                    .on_hover_text(format!("{} — switch to this scene", d.path.display()))
                    .clicked()
                {
                    self.cmds.push(Cmd::SwitchScene(i));
                }
                if ui
                    .add(
                        egui::Button::new(RichText::new(icon::X).size(10.0))
                            .small()
                            .frame(false),
                    )
                    .on_hover_text("Close (saves it first)")
                    .clicked()
                {
                    self.cmds.push(Cmd::CloseScene(i));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // The file name matters most: elide the path's start to fit.
                let full = self.doc.path.display().to_string();
                let fits = ((ui.available_width() - 8.0) / 6.6).max(0.0) as usize;
                let shown = elide_start(&full, fits);
                if !shown.is_empty() {
                    ui.label(RichText::new(shown).color(c::TEXT_FAINT).size(12.0))
                        .on_hover_text(full);
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            // Counters on the right first; the last log line takes what is
            // left and truncates (a long path must not run under them).
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (w, e) = self.log.counts();
                ui.label(
                    RichText::new(format!("{:.0} fps", self.fps))
                        .monospace()
                        .color(c::TEXT_FAINT),
                );
                ui.label(
                    RichText::new(format!("{} {}", icon::TRIANGLE, self.scene_view.stats.tris))
                        .color(c::TEXT_FAINT),
                )
                .on_hover_text(
                    "Triangles in the scene view. The N64 budget is CPU time, not a \
                     triangle count: the Game view's budget HUD shows the N64 3D time \
                     (trino_core::perf::N64_COST); strict mode stops above \
                     Caps::max_3d_us_per_frame.",
                );
                let mut n = 0;
                self.doc.scene.walk(|_, _| n += 1);
                ui.label(
                    RichText::new(format!("{} {n}", icon::TREE_STRUCTURE)).color(c::TEXT_FAINT),
                );
                if e > 0 {
                    ui.label(RichText::new(format!("{} {e}", icon::X_CIRCLE)).color(c::ERROR));
                }
                if w > 0 {
                    ui.label(RichText::new(format!("{} {w}", icon::WARNING)).color(c::WARN));
                }
                if !self.resolved.warnings.is_empty() {
                    ui.label(
                        RichText::new(format!(
                            "{} {} scene issue(s)",
                            icon::WARNING,
                            self.resolved.warnings.len()
                        ))
                        .color(c::WARN),
                    )
                    .on_hover_text(self.resolved.warnings.join("\n"));
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    if let Some(last) = self.log.last() {
                        ui.label(RichText::new(last.level.glyph()).color(last.level.color()));
                        let text = last.text.lines().next().unwrap_or("");
                        ui.add(
                            egui::Label::new(RichText::new(text).color(c::TEXT_WEAK).size(12.0))
                                .truncate(),
                        );
                    }
                });
            });
        });
    }

    /// The *Arrange* menu (also reachable from the command palette).
    fn arrange_menu(&mut self, ui: &mut egui::Ui) {
        use crate::ops::AlignTo;
        let n = self.doc.selection().len();
        ui.label(
            RichText::new(format!("{n} selected (Ctrl/Shift+click, Shift+drag)"))
                .size(11.5)
                .color(c::TEXT_FAINT),
        );
        for (axis, name) in [(0, "X"), (1, "Y"), (2, "Z")] {
            ui.menu_button(format!("{}  Align {name}", icon::ALIGN_LEFT), |ui| {
                for (to, label) in [
                    (AlignTo::Min, "min"),
                    (AlignTo::Center, "center"),
                    (AlignTo::Max, "max"),
                ] {
                    if ui.add_enabled(n >= 2, egui::Button::new(label)).clicked() {
                        self.cmds.push(Cmd::Align(axis, to));
                    }
                }
            });
        }
        for (axis, name) in [(0, "X"), (1, "Y"), (2, "Z")] {
            if ui
                .add_enabled(
                    n >= 3,
                    egui::Button::new(format!("{}  Distribute {name}", icon::DOTS_THREE)),
                )
                .clicked()
            {
                self.cmds.push(Cmd::Distribute(axis));
            }
        }
        ui.separator();
        if ui
            .add_enabled(n >= 1, egui::Button::new(format!("{}  Array…", icon::COPY)))
            .clicked()
        {
            self.cmds.push(Cmd::Open(Dialog::array()));
        }
        if ui
            .add_enabled(
                n >= 1,
                egui::Button::new(format!("{}  Drop to surface", icon::ARROW_LINE_DOWN))
                    .shortcut_text("End"),
            )
            .clicked()
        {
            self.cmds.push(Cmd::DropToSurface);
        }
    }

    /// Every action, for the command palette: (label, hint, command).
    fn palette_items(&self) -> Vec<crate::palette::Item<Cmd>> {
        use crate::ops::AlignTo;
        let mut v: Vec<crate::palette::Item<Cmd>> = Vec::new();
        let mut add = |label: String, hint: &str, cmd: Cmd| {
            v.push(crate::palette::Item {
                label,
                hint: hint.to_string(),
                payload: cmd,
            })
        };
        add("Save".into(), "Ctrl+S", Cmd::Save);
        add("Undo".into(), "Ctrl+Z", Cmd::Undo);
        add("Redo".into(), "Ctrl+Y", Cmd::Redo);
        add(
            "Play here (in the Game view)".into(),
            "Ctrl+Shift+P",
            Cmd::PlayHere,
        );
        add("Stop playing here".into(), "Esc", Cmd::StopPlayHere);
        add("New scene…".into(), "File", Cmd::Open(Dialog::new_scene()));
        add(
            "Create fake object…".into(),
            "Shift+A",
            Cmd::Open(Dialog::fake_object(&self.project)),
        );
        add(
            "New material…".into(),
            "Create",
            Cmd::Open(Dialog::new_material()),
        );
        add(
            "Export scene to game code…".into(),
            "File",
            Cmd::Open(Dialog::export(&self.doc.scene.name)),
        );
        add("Select all".into(), "Ctrl+A", Cmd::SelectAll);
        add("Deselect".into(), "Esc", Cmd::Select(None));
        add(
            "Duplicate selection".into(),
            "Ctrl+D",
            Cmd::DuplicateSelection,
        );
        add("Delete selection".into(), "Del", Cmd::DeleteSelection);
        add("Drop to surface".into(), "End", Cmd::DropToSurface);
        add("Array…".into(), "Arrange", Cmd::Open(Dialog::array()));
        for (axis, name) in [(0, "X"), (1, "Y"), (2, "Z")] {
            for (to, t) in [
                (AlignTo::Min, "min"),
                (AlignTo::Center, "center"),
                (AlignTo::Max, "max"),
            ] {
                add(
                    format!("Align {name} — {t}"),
                    "Arrange",
                    Cmd::Align(axis, to),
                );
            }
            add(
                format!("Distribute {name}"),
                "Arrange",
                Cmd::Distribute(axis),
            );
        }
        if let Some(sel) = &self.doc.selected
            && let Some(e) = self.doc.scene.get(sel)
        {
            if let Some(inst) = &e.prefab {
                if !inst.overrides.is_empty() {
                    add(
                        format!("Apply overrides to {}", inst.path),
                        "Prefab",
                        Cmd::ApplyOverrides(sel.clone()),
                    );
                }
                add(
                    format!("Edit prefab {}", inst.path),
                    "Prefab",
                    Cmd::EditPrefab(inst.path.clone()),
                );
                add(
                    "Unpack prefab instance".into(),
                    "Prefab",
                    Cmd::Unpack(sel.clone()),
                );
            } else {
                add(
                    "Make prefab from selection…".into(),
                    "Prefab",
                    Cmd::Open(Dialog::make_prefab(sel.clone(), &e.name)),
                );
            }
            add("Frame selection".into(), "F", Cmd::Focus(sel.clone()));
        }
        for (tab, name) in [
            (Tab::Scene, "Scene"),
            (Tab::Game, "Game"),
            (Tab::Level, "Level"),
            (Tab::Assets, "Assets"),
            (Tab::Console, "Console"),
        ] {
            add(format!("Go to {name} view"), "View", Cmd::FocusTab(tab));
        }
        let mut scenes: Vec<PathBuf> = std::fs::read_dir("scenes")
            .map(|d| {
                d.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.to_string_lossy().ends_with(".scene.ron"))
                    .collect()
            })
            .unwrap_or_default();
        scenes.sort();
        for s in scenes {
            add(
                format!("Open scene {}", s.display()),
                "File",
                Cmd::OpenScene(s),
            );
        }
        v
    }

    /// Ctrl+K: type to filter every action, Enter runs the best match.
    fn palette_window(&mut self, ctx: &egui::Context) {
        let Some(mut query) = self.palette.take() else {
            return;
        };
        let items = self.palette_items();
        let hits = crate::palette::filter(&items, &query);
        let mut run: Option<usize> = None;
        let mut keep = true;
        egui::Window::new(format!("{}  Command palette", icon::COMMAND))
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 80.0])
            .default_width(460.0)
            .min_height(60.0)
            .show(ctx, |ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut query)
                        .hint_text(format!("{}  Type a command…", icon::MAGNIFYING_GLASS))
                        .desired_width(f32::INFINITY),
                );
                r.request_focus();
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    keep = false;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    run = hits.first().copied();
                }
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        for &i in hits.iter().take(40) {
                            let it = &items[i];
                            ui.horizontal(|ui| {
                                if ui.selectable_label(false, &it.label).clicked() {
                                    run = Some(i);
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(&it.hint).size(11.0).color(c::TEXT_FAINT),
                                        );
                                    },
                                );
                            });
                        }
                        if hits.is_empty() {
                            ui.label(RichText::new("no matching command").color(c::TEXT_FAINT));
                        }
                    });
            });
        if let Some(i) = run {
            let mut items = items;
            self.cmds.push(items.swap_remove(i).payload);
        } else if keep {
            self.palette = Some(query);
        }
    }

    /// Import a file dropped on the window: `.glb`/`.gltf` become models,
    /// `.png` sprites — copied under `assets/shared/<kind>/imported/` and
    /// declared in the manifest; the asset watcher bakes them.
    fn import_file(&mut self, file: &Path) -> Result<String, String> {
        use pipeline::manifest::edit;
        let ext = file
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let stem = file
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("file has no name")?;
        let name = edit::name_from_stem(stem);
        if !edit::valid_name(&name) {
            return Err(format!("cannot derive an asset name from `{stem}`"));
        }
        let (kind, sub) = match ext.as_str() {
            "glb" | "gltf" => ("models", "models/imported"),
            "png" => ("sprites", "sprites/imported"),
            other => {
                return Err(format!(
                    "cannot import `.{other}` (models: .glb/.gltf, sprites: .png)"
                ));
            }
        };
        let rel = format!("{sub}/{name}.{ext}");
        let dest = self.project.assets_dir().join("shared").join(&rel);
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::copy(file, &dest).map_err(|e| format!("copy {}: {e}", file.display()))?;
        let manifest_path = self.project.manifest_path();
        let text = std::fs::read_to_string(&manifest_path).map_err(|e| e.to_string())?;
        let new = if kind == "models" {
            edit::add_model(&text, &name, &rel)?
        } else {
            // CI8: a safe N64 format for imported art (≤256 colors).
            edit::add_sprite(&text, &name, &rel, "CI8")?
        };
        std::fs::write(&manifest_path, new).map_err(|e| e.to_string())?;
        Ok(format!(
            "{} imported {kind}/{name} ({rel}) — baking",
            icon::DOWNLOAD_SIMPLE
        ))
    }

    /// Play the open scene inside the Game view (same process).
    fn start_play_here(&mut self) {
        let screen = self.game_view.internal_size();
        let target = crate::viewport::target_of(self.game_view.profile);
        match crate::play::PlaySession::start(&self.doc.scene, &self.lib, target, screen) {
            Ok(session) => {
                self.game_view.play = Some(session);
                self.focus_tab = Some(Tab::Game);
                self.log.toast(
                    Level::Ok,
                    format!(
                        "{} playing in the Game view — Stop or Esc ends it",
                        icon::PLAY
                    ),
                );
            }
            Err(e) => self.log.toast(Level::Error, e),
        }
    }

    fn stop_play_here(&mut self) {
        if let Some(s) = self.game_view.play.take() {
            self.log.info(format!(
                "play here stopped after {} frames ({}/{} coins)",
                s.frames, s.game.coins, s.game.total_coins
            ));
        }
    }

    /// Start/stop play mode. `scene`: play the open scene (`TRINO_SCENE`,
    /// live-reloading) instead of the game crate linked into the PC app.
    fn toggle_play(&mut self, scene: bool) {
        if let Some(mut child) = self.play.take() {
            let _ = child.kill();
            self.log.info("play stopped");
            return;
        }
        if self.doc.dirty()
            && let Err(e) = self.save_doc()
        {
            self.log.error(e);
        }
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let mut cmd = Command::new(cargo);
        cmd.args(["run", "-p", "trino-app-pc"])
            .env("TRINO_SIM", self.game_view.profile_key());
        if scene {
            cmd.env("TRINO_SCENE", &self.doc.path);
        }
        match cmd.spawn() {
            Ok(child) => {
                self.play = Some(child);
                self.log.toast(
                    Level::Ok,
                    format!("{} game started (separate process)", icon::PLAY),
                );
            }
            Err(e) => self.log.toast(Level::Error, format!("play failed: {e}")),
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        use egui::{Key, KeyboardShortcut, Modifiers};
        let sc = |m: Modifiers, k: Key| {
            ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)))
        };
        if sc(Modifiers::COMMAND, Key::S) {
            self.cmds.push(Cmd::Save);
        }
        if sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || sc(Modifiers::COMMAND, Key::Y) {
            self.cmds.push(Cmd::Redo);
        }
        if sc(Modifiers::COMMAND, Key::Z) {
            self.cmds.push(Cmd::Undo);
        }
        if sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::P) {
            if self.game_view.play.is_some() {
                self.stop_play_here();
            } else {
                self.start_play_here();
            }
        }
        if self.game_view.play.is_some() && sc(Modifiers::NONE, Key::Escape) {
            self.stop_play_here();
        }
        if sc(Modifiers::COMMAND, Key::P) {
            let scene = !self.doc.is_prefab();
            self.toggle_play(scene);
        }
        if sc(Modifiers::COMMAND, Key::V) {
            self.cmds.push(Cmd::Paste);
        }
        if sc(Modifiers::COMMAND, Key::K) {
            self.palette = Some(String::new());
        }
        if sc(Modifiers::COMMAND, Key::A) {
            self.cmds.push(Cmd::SelectAll);
        }
        if let Some(sel) = self.doc.selected.clone() {
            if sc(Modifiers::COMMAND, Key::D) {
                self.cmds.push(Cmd::DuplicateSelection);
            }
            if sc(Modifiers::NONE, Key::End) {
                self.cmds.push(Cmd::DropToSurface);
            }
            if sc(Modifiers::COMMAND, Key::C) {
                self.cmds.push(Cmd::Copy(sel.clone()));
            }
            if sc(Modifiers::NONE, Key::Delete) {
                self.cmds.push(Cmd::DeleteSelection);
            }
            if sc(Modifiers::NONE, Key::F2) {
                let name = self
                    .doc
                    .scene
                    .get(&sel)
                    .map(|e| e.name.clone())
                    .unwrap_or_default();
                self.renaming = Some((sel.clone(), name));
            }
            if sc(Modifiers::NONE, Key::Escape) {
                self.cmds.push(Cmd::Select(None));
            }
        }
        if sc(Modifiers::SHIFT, Key::A) {
            self.cmds
                .push(Cmd::Open(Dialog::fake_object(&self.project)));
        }
        if sc(Modifiers::NONE, Key::F1) {
            self.show_shortcuts = !self.show_shortcuts;
        }
    }

    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_shortcuts;
        egui::Window::new(format!("{}  Keyboard shortcuts", icon::INFO))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                egui::Grid::new("shortcuts")
                    .num_columns(2)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        for (k, what) in [
                            ("Ctrl+S", "Save scene / prefab"),
                            ("Ctrl+Z / Ctrl+Y", "Undo / redo"),
                            ("Ctrl+D", "Duplicate"),
                            (
                                "Ctrl+C / Ctrl+V",
                                "Copy / paste entity as RON text (works with AI chats)",
                            ),
                            ("Del", "Delete"),
                            ("F2", "Rename"),
                            ("Shift+A", "Create a fake object"),
                            ("Ctrl+K", "Command palette (every action, searchable)"),
                            ("Ctrl/Shift+click", "Add to / remove from the selection"),
                            ("Shift+drag", "Box select (Scene view)"),
                            ("Ctrl+A", "Select all"),
                            ("End", "Drop the selection onto the surface below"),
                            ("Ctrl+P", "Play / stop (separate process)"),
                            ("Ctrl+Shift+P", "Play here (in the Game view) / stop"),
                            ("Esc", "Stop playing here"),
                            ("W / E / R", "Move / rotate / scale gizmo (scene view)"),
                            ("Ctrl (while dragging)", "Toggle snapping"),
                            ("F", "Frame selection"),
                            ("G", "Toggle grid"),
                            ("1 / 3 / 7 (+Ctrl)", "Front / right / top view (opposite)"),
                            ("MMB / RMB drag", "Orbit (Alt+LMB too)"),
                            ("Shift + MMB/RMB drag", "Pan"),
                            ("Wheel", "Zoom"),
                            ("F1", "This window"),
                        ] {
                            ui.label(RichText::new(k).monospace().color(c::ACCENT_TEXT));
                            ui.label(what);
                            ui.end_row();
                        }
                    });
            });
        self.show_shortcuts = open;
    }

    fn update_thumbnails(&mut self, ctx: &egui::Context) {
        // A few per frame keeps startup smooth.
        let mut budget = 3;
        let missing: Vec<String> = self
            .project
            .models
            .values()
            .filter(|m| m.thumb.is_none())
            .map(|m| m.logical.clone())
            .take(budget)
            .collect();
        for l in missing {
            let img = self.thumbs.model(&self.project.models[&l]);
            let tex = ctx.load_texture(format!("thumb:{l}"), img, egui::TextureOptions::LINEAR);
            self.project.models.get_mut(&l).unwrap().thumb = Some(tex);
            budget -= 1;
        }
        for s in self
            .project
            .sprites
            .values_mut()
            .filter(|s| s.thumb.is_none())
        {
            let img = egui::ColorImage::from_rgba_unmultiplied(
                [s.width as usize, s.height as usize],
                &s.rgba,
            );
            s.thumb = Some(ctx.load_texture(
                format!("sprite:{}", s.logical),
                img,
                egui::TextureOptions::NEAREST,
            ));
        }
        let stale: Vec<(String, MaterialDef)> = self
            .lib
            .materials
            .iter()
            .filter(|(l, d)| {
                self.material_thumbs
                    .get(*l)
                    .is_none_or(|(old, _)| old != *d)
            })
            .map(|(l, d)| (l.clone(), d.clone()))
            .take(budget.max(1))
            .collect();
        for (l, d) in stale {
            let img = self.thumbs.material(&d.params(None));
            let tex = ctx.load_texture(format!("mat:{l}"), img, egui::TextureOptions::LINEAR);
            self.material_thumbs.insert(l, (d, tex));
        }
        let stamp = (self.thumb_rev, self.project.generation);
        let stale: Vec<String> = self
            .lib
            .prefabs
            .keys()
            .filter(|l| self.prefab_thumbs.get(*l).is_none_or(|(s, _)| *s != stamp))
            .take(2)
            .cloned()
            .collect();
        for l in stale {
            let mut scene = Scene::new("thumb");
            let mut e = Entity::new("p");
            e.prefab = Some(PrefabInstance::new(l.clone()));
            scene.entities.push(e);
            let items = resolve(&scene, &self.lib, 0.0).items;
            let img = self.thumbs.items(&items, &self.project);
            let tex = ctx.load_texture(format!("prefab:{l}"), img, egui::TextureOptions::LINEAR);
            self.prefab_thumbs.insert(l, (stamp, tex));
        }
        if self.project.models.values().any(|m| m.thumb.is_none())
            || self.prefab_thumbs.len() < self.lib.prefabs.len()
        {
            ctx.request_repaint();
        }
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some((frame, path)) = self.screenshot.clone() else {
            return;
        };
        if self.frames >= frame && !self.screenshot_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            self.screenshot_requested = true;
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = shot {
            let [w, h] = image.size;
            let file = std::fs::File::create(&path).expect("screenshot file");
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().expect("png header");
            writer.write_image_data(image.as_raw()).expect("png data");
            eprintln!("screenshot saved to {}", path.display());
            self.screenshot = None;
            if self.smoke_frames.is_none() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        ctx.request_repaint();
    }
}

/// `abs` relative to the working directory. Watchers may report
/// canonicalized paths (`/private/var/...` on macOS, `\\?\C:\...` on
/// Windows), so fall back to comparing canonical forms — for deleted files,
/// canonicalize the parent and re-append the name.
/// Every `[particles.*]` effect of the manifest that parses, by name.
pub fn load_particles(
    assets: &Path,
) -> std::collections::BTreeMap<String, pipeline::particles::ParticlesDef> {
    let Ok(manifest) = pipeline::Manifest::load(&assets.join("manifest.toml")) else {
        return Default::default();
    };
    manifest
        .particles
        .iter()
        .filter_map(|(name, decl)| {
            let path = pipeline::resolve_source(assets, pipeline::Platform::Pc, &decl.file).ok()?;
            let text = std::fs::read_to_string(path).ok()?;
            pipeline::particles::ParticlesDef::from_ron(&text)
                .ok()
                .map(|d| (name.clone(), d))
        })
        .collect()
}

/// Every impostor settings file under `assets/impostors/`, by name.
fn load_impostor_defs(
    assets: &Path,
) -> std::collections::BTreeMap<String, pipeline::impostor::ImpostorDef> {
    pipeline::impostor::names(assets)
        .into_iter()
        .filter_map(|n| {
            pipeline::impostor::load_def(assets, &n)
                .ok()
                .map(|d| (n, d))
        })
        .collect()
}

/// `text` cut to its last `max` characters, starting with "…" when cut
/// (nothing when even "…" + a few characters wouldn't fit).
fn elide_start(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        text.to_string()
    } else if max < 6 {
        String::new()
    } else {
        let tail: String = text.chars().skip(n - (max - 1)).collect();
        format!("…{tail}")
    }
}

fn relative_to(abs: &Path, cwd: &Path, cwd_canon: &Path) -> PathBuf {
    if let Ok(r) = abs.strip_prefix(cwd) {
        return r.to_path_buf();
    }
    let canon = std::fs::canonicalize(abs).ok().or_else(|| {
        let parent = std::fs::canonicalize(abs.parent()?).ok()?;
        Some(parent.join(abs.file_name()?))
    });
    canon
        .and_then(|c| c.strip_prefix(cwd_canon).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| abs.to_path_buf())
}

fn snap_half(v: f32) -> f32 {
    (v * 2.0).round() / 2.0
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let dt = self.last_frame.elapsed().as_secs_f32();
        self.last_frame = Instant::now();
        if dt > 0.0 {
            self.fps = self.fps * 0.9 + (1.0 / dt) * 0.1;
        }

        self.sync_files();
        self.handle_paste(&ctx);
        // Files dropped on the window: import (models / sprites).
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            self.cmds.push(Cmd::ImportFiles(dropped));
        }
        if std::mem::take(&mut self.play_here_on_start) {
            self.start_play_here();
        }
        if let Some(child) = &mut self.play
            && let Ok(Some(status)) = child.try_wait()
        {
            self.log.info(format!("game exited: {status}"));
            self.play = None;
        }
        self.update_thumbnails(&ctx);

        // A LOD preview belongs to the entity it was started on.
        if self
            .lod_preview
            .as_ref()
            .is_some_and(|(p, _)| self.doc.selected.as_ref() != Some(p))
        {
            self.lod_preview = None;
        }
        // Resolve once per frame; bump the revision when content changed.
        let time = self.time();
        self.resolved = resolve(&self.doc.scene, &self.lib, time);
        self.resolved_game = match crate::viewport::target_of(self.game_view.profile) {
            trino_scene::Target::Pc => self.resolved.clone(),
            t => trino_scene::resolve_for(&self.doc.scene, &self.lib, time, t),
        };
        if self.rev_scene.as_ref() != Some(&self.doc.scene) || self.lib_rev != self.seen_lib_rev {
            self.rev += 1;
            self.rev_scene = Some(self.doc.scene.clone());
            self.seen_lib_rev = self.lib_rev;
        }
        let has_anim = self.resolved.items.iter().any(|i| i.params.anim.is_some());
        if self.lod_preview.is_some() {
            self.rev += 1; // the preview distance moves without a scene edit
        }
        if self.animate && has_anim {
            self.rev += 1; // animated content re-renders every frame
            ctx.request_repaint();
        }

        egui::Panel::top("menu")
            .frame(
                egui::Frame::new()
                    .fill(c::BG0)
                    .inner_margin(Margin::symmetric(8, 4)),
            )
            .show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(c::BG0)
                    .inner_margin(Margin::symmetric(10, 4)),
            )
            .show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(c::BG0)
                    .inner_margin(Margin::symmetric(10, 3))
                    .stroke(Stroke::new(1.0, c::BG1)),
            )
            .show(ui, |ui| self.status_bar(ui));

        let width = ui.available_width();
        let mut dock = self.dock.take().unwrap_or_else(|| default_dock(width));
        if let Some(tab) = self.focus_tab.take()
            && let Some(path) = dock.find_tab(&tab)
        {
            let _ = dock.set_active_tab(path);
        }
        let style = theme::dock_style(ui.style());
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(c::BG0))
            .show(ui, |ui| {
                egui_dock::DockArea::new(&mut dock)
                    .style(style)
                    .show_close_buttons(false)
                    .show_leaf_close_all_buttons(false)
                    .show_leaf_collapse_buttons(false)
                    .show_secondary_button_hint(false)
                    .show_inside(ui, &mut TabView { app: self });
            });
        self.dock = Some(dock);

        // Dialogs and floating windows.
        if let Some(mut d) = self.dialog.take() {
            match d.show(&ctx, self) {
                crate::dialogs::Outcome::Keep => self.dialog = Some(d),
                crate::dialogs::Outcome::Close => {}
                crate::dialogs::Outcome::Run(cmd) => self.cmds.push(*cmd),
            }
        }
        self.shortcuts_window(&ctx);
        self.palette_window(&ctx);
        self.shortcuts(&ctx);
        self.run_cmds(&ctx);

        // Undo + pending file writes: once the user is done interacting.
        let settled = !ctx.input(|i| i.pointer.any_down()) && !ctx.egui_wants_keyboard_input();
        self.doc.commit(settled);
        if settled {
            self.flush_pending();
        }

        if self.log.show_toasts(&ctx) {
            ctx.request_repaint();
        }
        self.handle_screenshot(&ctx);

        self.frames += 1;
        if let Some(max) = self.smoke_frames {
            ctx.request_repaint();
            if self.frames >= max {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn on_exit(&mut self) {
        self.flush_pending();
        if let Some(mut child) = self.play.take() {
            let _ = child.kill();
        }
    }
}

/// The panel layout for a window `width` points wide: the Hierarchy and
/// the Inspector keep a usable width (200 / 300 px) at the editor's
/// minimum window size, and their usual share of a big one.
fn default_dock(width: f32) -> egui_dock::DockState<Tab> {
    // Dev hook (screenshots): TRINO_EDITOR_TAB=scene|game|level picks the
    // focused center tab.
    let tabs = match std::env::var("TRINO_EDITOR_TAB").as_deref() {
        Ok("level") => vec![Tab::Level, Tab::Scene, Tab::Game, Tab::Animation],
        Ok("game") => vec![Tab::Game, Tab::Scene, Tab::Level, Tab::Animation],
        Ok("animation") => vec![Tab::Animation, Tab::Scene, Tab::Game, Tab::Level],
        _ => vec![Tab::Scene, Tab::Game, Tab::Level, Tab::Animation],
    };
    let mut dock = egui_dock::DockState::new(tabs);
    let surface = dock.main_surface_mut();
    let width = width.max(1.0);
    let left = (200.0 / width).clamp(0.17, 0.3);
    let rest = width * (1.0 - left);
    let right = (300.0 / rest).clamp(0.28, 0.45);
    let [center, _left] =
        surface.split_left(egui_dock::NodeIndex::root(), left, vec![Tab::Hierarchy]);
    let [center, _right] = surface.split_right(center, 1.0 - right, vec![Tab::Inspector]);
    let [_, _bottom] = surface.split_below(center, 0.70, vec![Tab::Assets, Tab::Console]);
    dock
}

struct TabView<'a> {
    app: &'a mut App,
}

impl egui_dock::TabViewer for TabView<'_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        let mut t = tab.title();
        if *tab == Tab::Console {
            let (w, e) = self.app.log.counts();
            if e > 0 {
                t = format!("{t}  {} {e}", icon::X_CIRCLE);
            } else if w > 0 {
                t = format!("{t}  {} {w}", icon::WARNING);
            }
        }
        t.into()
    }

    fn closeable(&mut self, _tab: &mut Tab) -> bool {
        false
    }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        match tab {
            Tab::Scene | Tab::Game | Tab::Console | Tab::Assets | Tab::Level | Tab::Animation => {
                [false, false]
            }
            _ => [false, true],
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        let app = &mut *self.app;
        match tab {
            Tab::Scene | Tab::Game => {
                let kind = if *tab == Tab::Scene {
                    ViewKind::Scene
                } else {
                    ViewKind::Game
                };
                let input = ViewInput {
                    scene: &app.doc.scene,
                    resolved: if kind == ViewKind::Game {
                        &app.resolved_game
                    } else {
                        &app.resolved
                    },
                    project: &app.project,
                    selected: app.doc.selected.as_deref(),
                    extra: &app.doc.extra,
                    rev: app.rev,
                    time: app.anim_start.elapsed().as_secs_f32(),
                    animate: app.animate,
                    lod_preview: app.lod_preview.as_ref().map(|(p, d)| (p.as_slice(), *d)),
                };
                let view = if kind == ViewKind::Scene {
                    &mut app.scene_view
                } else {
                    &mut app.game_view
                };
                let actions = view.show(ui, &input);
                for a in actions {
                    match a {
                        ViewAction::Select(p) => app.cmds.push(Cmd::Select(p)),
                        ViewAction::ToggleSelect(p) => app.cmds.push(Cmd::ToggleSelect(p)),
                        ViewAction::SelectMany(ps) => app.cmds.push(Cmd::SelectMany(ps)),
                        ViewAction::SetTransform(path, t) => {
                            // Multi-select: the others follow the active
                            // entity's world move (position only).
                            let before = crate::ops::world_position(&app.doc.scene, &path);
                            if let Some(e) = app.doc.scene.get_mut(&path) {
                                e.transform = t;
                            }
                            let after = crate::ops::world_position(&app.doc.scene, &path);
                            if let (Some(b), Some(a)) = (before, after) {
                                let d = a - b;
                                if d.dot(d) > 0.0 {
                                    for p in app.doc.extra.clone() {
                                        if p.starts_with(&path) {
                                            continue; // a child moves with it
                                        }
                                        if let Some(w) =
                                            crate::ops::world_position(&app.doc.scene, &p)
                                        {
                                            crate::ops::set_world_position(
                                                &mut app.doc.scene,
                                                &p,
                                                w + d,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        ViewAction::Drop(payload, at) => {
                            app.cmds.push(Cmd::AddAsset(payload, Some(at)))
                        }
                        ViewAction::ToggleAnimate => app.animate = !app.animate,
                    }
                }
            }
            Tab::Level => {
                let lines = app.level.ui(ui);
                for l in lines {
                    app.log.info(l);
                }
            }
            Tab::Animation => crate::anim_panel::ui(app, ui),
            Tab::Hierarchy => crate::panels::hierarchy::ui(app, ui),
            Tab::Inspector => crate::panels::inspector::ui(app, ui),
            Tab::Assets => crate::panels::assets::ui(app, ui),
            Tab::Console => app.log.ui(ui),
        }
    }
}

/// Card frame used by panels.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(c::BG2.gamma_multiply(0.5))
        .stroke(Stroke::new(1.0, c::BORDER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_paths_keep_their_end() {
        assert_eq!(elide_start("scenes/a.scene.ron", 40), "scenes/a.scene.ron");
        assert_eq!(
            elide_start("/very/long/dir/scenes/a.scene.ron", 12),
            "…a.scene.ron"
        );
        assert_eq!(elide_start("/x/y/z.ron", 3), "");
    }

    #[test]
    fn watcher_paths_resolve_relative_even_when_canonicalized() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("prefabs")).unwrap();
        std::fs::write(root.join("prefabs/a.prefab.ron"), "x").unwrap();
        let canon = std::fs::canonicalize(&root).unwrap();
        // Plain absolute path.
        assert_eq!(
            relative_to(&root.join("prefabs/a.prefab.ron"), &root, &canon),
            PathBuf::from("prefabs/a.prefab.ron")
        );
        // Canonical form, and a deleted file (only its parent exists).
        assert_eq!(
            relative_to(&canon.join("prefabs/a.prefab.ron"), &root, &canon),
            PathBuf::from("prefabs/a.prefab.ron")
        );
        assert_eq!(
            relative_to(&canon.join("prefabs/gone.prefab.ron"), &root, &canon),
            PathBuf::from("prefabs/gone.prefab.ron")
        );
    }
}
