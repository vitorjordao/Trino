//! Trino Editor: Unity-style shell over the real engine backend.
//!
//! - **Viewport** renders the scene through `PcRenderer` (render-to-texture
//!   on eframe's wgpu device) with console-simulation profiles.
//! - **Play** launches the game as a **separate process** (Fyrox model): a
//!   crashing game never takes the editor down. Stop kills it.
//! - Assets rebake + re-upload live while the editor runs.
//!
//! Smoke-test hook: `TRINO_EDITOR_SMOKE_FRAMES=N` closes after N frames.

mod viewport;

use std::path::PathBuf;
use std::process::{Child, Command};

use eframe::egui;
use trino_asset_pipeline as pipeline;
use trino_platform_pc::SimProfile;
use trino_scene::{Entity, Scene, SpriteComponent};

use viewport::Viewport;

const SCENE_PATH: &str = "scenes/main.scene.ron";

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("Trino Editor")
            .with_inner_size([1440.0, 810.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Trino Editor",
        options,
        Box::new(|cc| Ok(Box::new(EditorApp::new(cc)))),
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Viewport,
    Hierarchy,
    Inspector,
    Assets,
    Console,
}

struct EditorApp {
    dock: egui_dock::DockState<Tab>,
    viewport: Viewport,
    scene: Scene,
    scene_path: PathBuf,
    selected: Option<usize>,
    sprite_paths: Vec<String>,
    log: Vec<String>,
    play: Option<Child>,
    dirty: bool,
    asset_events: std::sync::mpsc::Receiver<Vec<u32>>,
    _asset_watcher: Box<dyn std::any::Any>,
    frames: u64,
    smoke_frames: Option<u64>,
}

impl EditorApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let render_state = cc
            .wgpu_render_state
            .clone()
            .expect("editor requires the wgpu backend");

        let mut dock = egui_dock::DockState::new(vec![Tab::Viewport]);
        let surface = dock.main_surface_mut();
        let [viewport_node, right] =
            surface.split_right(egui_dock::NodeIndex::root(), 0.78, vec![Tab::Hierarchy]);
        surface.split_below(right, 0.4, vec![Tab::Inspector]);
        let [_, bottom] = surface.split_below(viewport_node, 0.75, vec![Tab::Assets]);
        surface.split_right(bottom, 0.5, vec![Tab::Console]);

        let mut viewport = Viewport::new(render_state, SimProfile::N64);
        let mut log = Vec::new();

        // Bake + upload assets; start the live watcher.
        let (assets_root, baked) = (PathBuf::from("assets"), PathBuf::from("target/assets/pc"));
        match pipeline::bake_all(&assets_root, pipeline::Platform::Pc, &baked) {
            Ok(report) => log.push(format!("baked {} asset(s)", report.entries.len())),
            Err(e) => log.push(format!("BAKE FAILED:\n{e}")),
        }
        let mut sprite_paths = Vec::new();
        if let Ok(assets) = pipeline::load_dir(&baked, None) {
            for sprite in &assets.sprites {
                sprite_paths.push(sprite.logical.clone());
            }
            upload(viewport.renderer_mut(), assets);
        }

        let (tx, asset_events) = std::sync::mpsc::channel();
        let watcher =
            pipeline::watch::watch(assets_root, pipeline::Platform::Pc, baked, move |changed| {
                let _ = tx.send(changed);
            })
            .expect("failed to start asset watcher");

        let scene_path = PathBuf::from(SCENE_PATH);
        let scene = match Scene::load(&scene_path) {
            Ok(scene) => {
                log.push(format!("loaded {}", scene_path.display()));
                scene
            }
            Err(_) => {
                log.push("no scene found, starting a default one".into());
                default_scene()
            }
        };

        EditorApp {
            dock,
            viewport,
            scene,
            scene_path,
            selected: None,
            sprite_paths,
            log,
            play: None,
            dirty: false,
            asset_events,
            _asset_watcher: Box::new(watcher),
            frames: 0,
            smoke_frames: std::env::var("TRINO_EDITOR_SMOKE_FRAMES")
                .ok()
                .and_then(|v| v.parse().ok()),
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let playing = self.play.is_some();
            if playing {
                if ui.button("⏹ Stop").clicked()
                    && let Some(mut child) = self.play.take()
                {
                    let _ = child.kill();
                    self.log.push("play stopped".into());
                }
            } else if ui.button("▶ Play").clicked() {
                self.scene_save();
                let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
                match Command::new(cargo)
                    .args(["run", "-p", "trino-app-pc"])
                    .env("TRINO_SIM", self.viewport.profile_key())
                    .spawn()
                {
                    Ok(child) => {
                        self.play = Some(child);
                        self.log.push("play started (separate process)".into());
                    }
                    Err(e) => self.log.push(format!("play failed: {e}")),
                }
            }

            ui.separator();
            ui.label("Simular:");
            let mut profile = self.viewport.profile;
            egui::ComboBox::from_id_salt("sim-profile")
                .selected_text(profile_name(profile))
                .show_ui(ui, |ui| {
                    for p in [SimProfile::Native, SimProfile::N64, SimProfile::N3ds] {
                        ui.selectable_value(&mut profile, p, profile_name(p));
                    }
                });
            if profile != self.viewport.profile {
                self.viewport.set_profile(profile);
                // New renderer = empty texture set; re-upload everything.
                if let Ok(assets) = pipeline::load_dir(&PathBuf::from("target/assets/pc"), None) {
                    upload(self.viewport.renderer_mut(), assets);
                }
                self.log
                    .push(format!("simulation: {}", profile_name(profile)));
            }

            ui.separator();
            if ui.button("💾 Save").clicked() {
                self.scene_save();
            }
            if self.dirty {
                ui.label(egui::RichText::new("● unsaved").color(egui::Color32::YELLOW));
            }
            ui.label(self.scene_path.display().to_string());
        });
    }

    fn scene_save(&mut self) {
        match self.scene.save(&self.scene_path) {
            Ok(()) => {
                self.dirty = false;
                self.log
                    .push(format!("saved {}", self.scene_path.display()));
            }
            Err(e) => self.log.push(format!("save failed: {e}")),
        }
    }
}

fn upload(renderer: &mut trino_platform_pc::PcRenderer, assets: pipeline::LoadedAssets) {
    for sprite in assets.sprites {
        renderer.upload_sprite(
            trino_core::SpriteId(sprite.id),
            sprite.width,
            sprite.height,
            &sprite.rgba,
        );
    }
    // Sounds are ignored by the editor viewport (no audio preview yet).
}

fn default_scene() -> Scene {
    let mut scene = Scene::new("main");
    scene.entities.push(Entity {
        name: "player".into(),
        transform: trino_scene::Transform2D {
            pos: [144.0, 104.0],
            ..Default::default()
        },
        sprite: Some(SpriteComponent::new("sprites/player")),
    });
    scene
}

fn profile_name(p: SimProfile) -> &'static str {
    match p {
        SimProfile::Native => "PC (nativo)",
        SimProfile::N64 => "N64 look",
        SimProfile::N3ds => "3DS look",
    }
}

impl eframe::App for EditorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Live asset reload.
        let mut changed_batches = Vec::new();
        while let Ok(changed) = self.asset_events.try_recv() {
            changed_batches.push(changed);
        }
        for changed in changed_batches {
            match pipeline::load_dir(&PathBuf::from("target/assets/pc"), Some(&changed)) {
                Ok(assets) => {
                    self.log
                        .push(format!("assets reloaded: {} item(s)", changed.len()));
                    upload(self.viewport.renderer_mut(), assets);
                }
                Err(e) => self.log.push(format!("asset reload failed: {e}")),
            }
        }

        // Reap a finished play process.
        if let Some(child) = &mut self.play
            && let Ok(Some(status)) = child.try_wait()
        {
            self.log.push(format!("game exited: {status}"));
            self.play = None;
        }

        self.toolbar(ui);
        ui.separator();

        // Render the scene into the offscreen framebuffer, then lay out the
        // dock (whose Viewport tab shows that texture).
        self.viewport.render_scene(&self.scene, self.selected);

        let mut dock = std::mem::replace(&mut self.dock, egui_dock::DockState::new(vec![]));
        egui_dock::DockArea::new(&mut dock).show_inside(ui, &mut TabView { app: self });
        self.dock = dock;

        // Keep animating (asset reloads land without user input).
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));

        self.frames += 1;
        if let Some(max) = self.smoke_frames
            && self.frames >= max
        {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn on_exit(&mut self) {
        if let Some(mut child) = self.play.take() {
            let _ = child.kill();
        }
    }
}

struct TabView<'a> {
    app: &'a mut EditorApp,
}

impl egui_dock::TabViewer for TabView<'_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        match tab {
            Tab::Viewport => "Viewport".into(),
            Tab::Hierarchy => "Hierarquia".into(),
            Tab::Inspector => "Inspector".into(),
            Tab::Assets => "Assets".into(),
            Tab::Console => "Console".into(),
        }
    }

    fn closeable(&mut self, _tab: &mut Tab) -> bool {
        false
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        let app = &mut *self.app;
        match tab {
            Tab::Viewport => {
                if let Some(clicked) = app.viewport.show(ui, &app.scene) {
                    app.selected = Some(clicked);
                }
            }
            Tab::Hierarchy => {
                if ui.button("+ Entidade").clicked() {
                    app.scene.entities.push(Entity {
                        name: format!("entity-{}", app.scene.entities.len()),
                        transform: Default::default(),
                        sprite: None,
                    });
                    app.selected = Some(app.scene.entities.len() - 1);
                    app.dirty = true;
                }
                ui.separator();
                for (i, entity) in app.scene.entities.iter().enumerate() {
                    let selected = app.selected == Some(i);
                    if ui.selectable_label(selected, &entity.name).clicked() {
                        app.selected = Some(i);
                    }
                }
            }
            Tab::Inspector => {
                let Some(i) = app.selected else {
                    ui.label("Nada selecionado.");
                    return;
                };
                let Some(entity) = app.scene.entities.get_mut(i) else {
                    app.selected = None;
                    return;
                };
                let mut dirty = false;

                dirty |= ui.text_edit_singleline(&mut entity.name).changed();
                ui.separator();

                ui.label("Transform");
                ui.horizontal(|ui| {
                    ui.label("pos");
                    dirty |= ui
                        .add(egui::DragValue::new(&mut entity.transform.pos[0]).speed(1.0))
                        .changed();
                    dirty |= ui
                        .add(egui::DragValue::new(&mut entity.transform.pos[1]).speed(1.0))
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.label("scale");
                    dirty |= ui
                        .add(egui::DragValue::new(&mut entity.transform.scale[0]).speed(0.05))
                        .changed();
                    dirty |= ui
                        .add(egui::DragValue::new(&mut entity.transform.scale[1]).speed(0.05))
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.label("rotation");
                    dirty |= ui
                        .add(egui::DragValue::new(&mut entity.transform.rotation).speed(0.02))
                        .changed();
                });
                ui.separator();

                match &mut entity.sprite {
                    Some(sprite) => {
                        ui.label("Sprite");
                        egui::ComboBox::from_id_salt("sprite-path")
                            .selected_text(&sprite.path)
                            .show_ui(ui, |ui| {
                                for path in &app.sprite_paths {
                                    if ui.selectable_label(sprite.path == *path, path).clicked() {
                                        sprite.path = path.clone();
                                        dirty = true;
                                    }
                                }
                            });
                        dirty |= ui.checkbox(&mut sprite.flip_x, "flip X").changed();
                        dirty |= ui.checkbox(&mut sprite.flip_y, "flip Y").changed();
                        if ui.button("Remover sprite").clicked() {
                            entity.sprite = None;
                            dirty = true;
                        }
                    }
                    None => {
                        if ui.button("+ Sprite").clicked() {
                            entity.sprite = Some(SpriteComponent::new(
                                app.sprite_paths
                                    .first()
                                    .cloned()
                                    .unwrap_or_else(|| "sprites/player".into()),
                            ));
                            dirty = true;
                        }
                    }
                }
                ui.separator();
                if ui.button("🗑 Excluir entidade").clicked() {
                    app.scene.entities.remove(i);
                    app.selected = None;
                    dirty = true;
                }
                if dirty {
                    app.dirty = true;
                }
            }
            Tab::Assets => {
                ui.label("assets/manifest.toml");
                ui.separator();
                for path in &app.sprite_paths {
                    ui.horizontal(|ui| {
                        ui.label("🖼");
                        ui.monospace(path);
                    });
                }
                ui.separator();
                ui.label("Edite um master em assets/ e ele recarrega ao vivo.");
            }
            Tab::Console => {
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &app.log {
                            ui.monospace(line);
                        }
                    });
            }
        }
    }
}
