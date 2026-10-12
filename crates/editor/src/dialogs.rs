//! Modal dialogs. Each returns a [`Cmd`] for the app to run, so dialogs
//! never mutate state directly (and stay trivially undoable).

use std::path::PathBuf;

use eframe::egui::{self, RichText, Vec2};
use trino_asset_pipeline::Primitive;
use trino_scene::MaterialDef;

use crate::app::{App, Cmd};
use crate::panels::hierarchy::PROXY_COLORS;
use crate::project::Project;
use crate::theme::{self, c, icon};

pub enum Dialog {
    FakeObject {
        name: String,
        prim: Primitive,
        add: bool,
        preview: Option<(Primitive, egui::TextureHandle)>,
    },
    NewMaterial {
        name: String,
        def: MaterialDef,
    },
    MakePrefab {
        path: Vec<usize>,
        name: String,
    },
    NewScene {
        name: String,
    },
    Export {
        out: String,
    },
}

pub enum Outcome {
    Keep,
    Close,
    Run(Box<Cmd>),
}

impl Dialog {
    pub fn fake_object(project: &Project) -> Self {
        Dialog::FakeObject {
            name: project.unique_model_name("box"),
            prim: Primitive {
                color: PROXY_COLORS[project.models.len() % PROXY_COLORS.len()],
                ..Primitive::default()
            },
            add: true,
            preview: None,
        }
    }

    pub fn new_material() -> Self {
        Self::new_material_from(MaterialDef::default())
    }

    pub fn new_material_from(def: MaterialDef) -> Self {
        Dialog::NewMaterial {
            name: String::new(),
            def,
        }
    }

    pub fn make_prefab(path: Vec<usize>, entity_name: &str) -> Self {
        let name: String = entity_name
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        Dialog::MakePrefab { path, name }
    }

    pub fn new_scene() -> Self {
        Dialog::NewScene {
            name: String::new(),
        }
    }

    pub fn export(scene: &str) -> Self {
        let slug: String = scene
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        Dialog::Export {
            out: format!("examples/castle64/src/{slug}_gen.rs"),
        }
    }

    fn title(&self) -> String {
        match self {
            Dialog::FakeObject { .. } => format!("{}  New fake object", icon::GHOST),
            Dialog::NewMaterial { .. } => format!("{}  New material", icon::PALETTE),
            Dialog::MakePrefab { .. } => format!("{}  Create prefab", icon::PACKAGE),
            Dialog::NewScene { .. } => format!("{}  New scene", icon::FILE_PLUS),
            Dialog::Export { .. } => format!("{}  Export scene to game code", icon::CODE),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, app: &mut App) -> Outcome {
        let sprite_names: Vec<String> = app.project.sprites.keys().cloned().collect();
        let mut outcome = Outcome::Keep;
        let title = self.title();
        let modal = egui::Modal::new(egui::Id::new("trino-dialog")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new(title).heading());
            ui.add_space(8.0);
            let valid = |s: &str| trino_asset_pipeline::manifest::edit::valid_name(s);
            let name_field = |ui: &mut egui::Ui, name: &mut String, hint: &str| {
                theme::prop(ui, "Name", |ui| {
                    let r = ui.add(egui::TextEdit::singleline(name).hint_text(hint).desired_width(f32::INFINITY));
                    r.request_focus();
                });
                if !name.is_empty() && !valid(name) {
                    ui.label(RichText::new("lowercase letters, digits, - and _ only").size(11.5).color(c::ERROR));
                }
            };
            let confirm = match self {
                Dialog::FakeObject { name, prim, add, preview } => {
                    ui.label(RichText::new("A placeholder model with real dimensions, on every target. Replace it with art later — the handle stays.").size(12.0).color(c::TEXT_WEAK));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        // Live preview, re-rendered only when parameters change.
                        if preview.as_ref().is_none_or(|(p, _)| p != prim) {
                            let img = app.thumbs.primitive(prim);
                            *preview = Some((prim.clone(), ctx.load_texture("fake-preview", img, egui::TextureOptions::LINEAR)));
                        }
                        egui::Frame::new().fill(c::BG0).corner_radius(egui::CornerRadius::same(10)).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                            if let Some((_, tex)) = preview {
                                ui.add(egui::Image::new((tex.id(), Vec2::splat(120.0))));
                            }
                        });
                        ui.vertical(|ui| {
                            name_field(ui, name, "crate");
                            ui.checkbox(add, "Add to the scene");
                            ui.label(RichText::new(format!("assets/shared/models/proxies/{name}.prim.ron")).monospace().size(11.0).color(c::TEXT_FAINT));
                        });
                    });
                    ui.add_space(6.0);
                    crate::panels::inspector::primitive_editor(ui, prim);
                    valid(name)
                }
                Dialog::NewMaterial { name, def } => {
                    name_field(ui, name, "stone");
                    ui.add_space(4.0);
                    crate::panels::inspector::material_editor(ui, def, &sprite_names);
                    valid(name)
                }
                Dialog::MakePrefab { name, .. } => {
                    ui.label(RichText::new("The entity becomes a linked instance of a new prefab. Edit the prefab once, every instance follows.").size(12.0).color(c::TEXT_WEAK));
                    name_field(ui, name, "tree");
                    ui.label(RichText::new(format!("prefabs/{name}.prefab.ron")).monospace().size(11.0).color(c::TEXT_FAINT));
                    valid(name)
                }
                Dialog::NewScene { name } => {
                    name_field(ui, name, "level-2");
                    valid(name)
                }
                Dialog::Export { out } => {
                    ui.label(RichText::new("Writes the scene as no_std Rust data (trino_core::SceneNode) that runs on PC, N64 and 3DS. Include it from a game crate.").size(12.0).color(c::TEXT_WEAK));
                    theme::prop(ui, "File", |ui| {
                        ui.add(egui::TextEdit::singleline(out).desired_width(f32::INFINITY));
                    });
                    !out.trim().is_empty()
                }
            };
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let ok = ui.add_enabled_ui(confirm, |ui| theme::primary_button(ui, "Create")).inner;
                    if confirm && (ok.clicked() || enter) {
                        outcome = Outcome::Run(Box::new(self.command()));
                    }
                    if ui.button("Cancel").clicked() {
                        outcome = Outcome::Close;
                    }
                });
            });
        });
        if modal.should_close() && matches!(outcome, Outcome::Keep) {
            outcome = Outcome::Close;
        }
        outcome
    }

    fn command(&self) -> Cmd {
        match self {
            Dialog::FakeObject {
                name, prim, add, ..
            } => Cmd::CreateProxy(name.clone(), prim.clone(), *add),
            Dialog::NewMaterial { name, def } => Cmd::NewMaterial(name.clone(), def.clone()),
            Dialog::MakePrefab { path, name } => Cmd::MakePrefab(path.clone(), name.clone()),
            Dialog::NewScene { name } => Cmd::NewScene(name.clone()),
            Dialog::Export { out } => Cmd::Export(PathBuf::from(out.trim())),
        }
    }
}
