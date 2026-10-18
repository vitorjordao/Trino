//! Assets browser: models (real + fake objects), prefabs, materials,
//! sprites and audio as a searchable tile grid with rendered thumbnails.
//! Click to inspect, double-click to add to the scene, drag onto a view to
//! place it where you drop.

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use crate::app::{App, AssetSel, Cmd};
use crate::dialogs::Dialog;
use crate::theme::{self, c, icon};
use crate::viewport::AssetPayload;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AssetTab {
    Models,
    Prefabs,
    Materials,
    Sprites,
    Particles,
    Audio,
}

struct Tile {
    sel: Option<AssetSel>,
    payload: Option<AssetPayload>,
    name: String,
    sub: String,
    glyph: &'static str,
    color: Color32,
    thumb: Option<egui::TextureId>,
    badge: Option<&'static str>,
}

/// Below this width the tabs wrap and the search box gets its own row.
const ONE_ROW_WIDTH: f32 = 640.0;

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    let one_row = ui.available_width() >= ONE_ROW_WIDTH;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (tab, glyph, name, n) in [
            (
                AssetTab::Models,
                icon::CUBE,
                "Models",
                app.project.models.len(),
            ),
            (
                AssetTab::Prefabs,
                icon::PACKAGE,
                "Prefabs",
                app.lib.prefabs.len(),
            ),
            (
                AssetTab::Materials,
                icon::PALETTE,
                "Materials",
                app.lib.materials.len(),
            ),
            (
                AssetTab::Sprites,
                icon::IMAGE,
                "Sprites",
                app.project.sprites.len(),
            ),
            (
                AssetTab::Particles,
                icon::SPARKLE,
                "Particles",
                app.particles.len(),
            ),
            (
                AssetTab::Audio,
                icon::SPEAKER_HIGH,
                "Audio",
                app.project.sounds.len() + app.project.music.len(),
            ),
        ] {
            let on = app.assets_tab == tab;
            let text = RichText::new(format!("{glyph} {name} {n}")).color(if on {
                Color32::WHITE
            } else {
                c::TEXT_WEAK
            });
            let mut b = egui::Button::new(text).min_size(Vec2::new(0.0, 26.0));
            b = if on {
                b.fill(c::ACCENT_SOFT)
            } else {
                b.frame_when_inactive(false)
            };
            if ui.add(b).clicked() {
                app.assets_tab = tab;
            }
        }
        if one_row {
            ui.add_space(8.0);
            search_row(app, ui);
        }
    });
    if !one_row {
        ui.horizontal(|ui| search_row(app, ui));
    }
    ui.add_space(6.0);
    grid(app, ui);
}

/// The search box and the tab's "new" button.
fn search_row(app: &mut App, ui: &mut egui::Ui) {
    {
        let w = (ui.available_width() - 40.0).max(100.0);
        ui.allocate_ui(Vec2::new(w, 26.0), |ui| {
            theme::search_box(ui, &mut app.assets_filter, "Search assets…");
        });
        let (tip, dialog) = match app.assets_tab {
            AssetTab::Models => (
                "New fake object (Shift+A)",
                Some(Dialog::fake_object(&app.project)),
            ),
            AssetTab::Materials => ("New material", Some(Dialog::new_material())),
            AssetTab::Particles => {
                if theme::named_icon_button(ui, icon::PLUS, "New particle effect").clicked() {
                    let mut n = 1;
                    while app.particles.contains_key(&format!("effect-{n}")) {
                        n += 1;
                    }
                    app.cmds.push(Cmd::NewParticles(format!("effect-{n}")));
                }
                ("", None)
            }
            _ => ("", None),
        };
        if let Some(d) = dialog
            && theme::icon_button(ui, icon::PLUS, tip).clicked()
        {
            app.cmds.push(Cmd::Open(d));
        }
    }
}

fn grid(app: &mut App, ui: &mut egui::Ui) {
    let needle = app.assets_filter.to_lowercase();
    let mut tiles: Vec<Tile> = Vec::new();
    match app.assets_tab {
        AssetTab::Models => {
            for m in app.project.models.values() {
                tiles.push(Tile {
                    sel: Some(AssetSel::Model(m.logical.clone())),
                    payload: Some(AssetPayload::Model(m.logical.clone())),
                    name: m.name.clone(),
                    sub: format!("{} tris", m.tris),
                    glyph: if m.is_proxy() {
                        icon::GHOST
                    } else {
                        icon::CUBE
                    },
                    color: c::MODEL,
                    thumb: m.thumb.as_ref().map(|t| t.id()),
                    badge: if m.is_proxy() {
                        Some(icon::GHOST)
                    } else if m.joints > 0 {
                        Some(icon::FILM_STRIP)
                    } else {
                        None
                    },
                });
            }
        }
        AssetTab::Prefabs => {
            for (l, p) in &app.lib.prefabs {
                let mut parts = 0;
                count(&p.root, &mut parts);
                tiles.push(Tile {
                    sel: Some(AssetSel::Prefab(l.clone())),
                    payload: Some(AssetPayload::Prefab(l.clone())),
                    name: l.trim_start_matches("prefabs/").to_string(),
                    sub: format!("{parts} part(s)"),
                    glyph: icon::PACKAGE,
                    color: c::PREFAB,
                    thumb: app.prefab_thumbs.get(l).map(|(_, t)| t.id()),
                    badge: None,
                });
            }
        }
        AssetTab::Materials => {
            for (l, d) in &app.lib.materials {
                tiles.push(Tile {
                    sel: Some(AssetSel::Material(l.clone())),
                    payload: None,
                    name: l.trim_start_matches("materials/").to_string(),
                    sub: format!("{:?}", d.shading),
                    glyph: icon::PALETTE,
                    color: c::MATERIAL,
                    thumb: app.material_thumbs.get(l).map(|(_, t)| t.id()),
                    badge: None,
                });
            }
        }
        AssetTab::Sprites => {
            for s in app.project.sprites.values() {
                tiles.push(Tile {
                    sel: Some(AssetSel::Sprite(s.logical.clone())),
                    payload: Some(AssetPayload::Sprite(s.logical.clone())),
                    name: s.logical.trim_start_matches("sprites/").to_string(),
                    sub: format!("{}×{}", s.width, s.height),
                    glyph: icon::IMAGE,
                    color: c::SPRITE,
                    thumb: s.thumb.as_ref().map(|t| t.id()),
                    badge: None,
                });
            }
        }
        AssetTab::Particles => {
            for (name, d) in &app.particles {
                tiles.push(Tile {
                    sel: Some(AssetSel::Particles(name.clone())),
                    payload: None,
                    name: name.clone(),
                    sub: format!("max {}", d.max),
                    glyph: icon::SPARKLE,
                    color: c::TRIGGER,
                    thumb: None,
                    badge: None,
                });
            }
        }
        AssetTab::Audio => {
            for (l, music) in app
                .project
                .sounds
                .iter()
                .map(|s| (s, false))
                .chain(app.project.music.iter().map(|s| (s, true)))
            {
                tiles.push(Tile {
                    sel: Some(AssetSel::Audio(l.clone())),
                    payload: None,
                    name: l.rsplit('/').next().unwrap_or(l).to_string(),
                    sub: if music {
                        "music".into()
                    } else {
                        "sound".into()
                    },
                    glyph: if music {
                        icon::MUSIC_NOTES
                    } else {
                        icon::SPEAKER_HIGH
                    },
                    color: c::ACCENT_TEXT,
                    thumb: None,
                    badge: None,
                });
            }
        }
    }
    tiles.retain(|t| needle.is_empty() || t.name.to_lowercase().contains(&needle));

    if tiles.is_empty() {
        let (title, hint) = match app.assets_tab {
            AssetTab::Prefabs => (
                "No prefabs",
                "Right-click an entity › Create prefab from this",
            ),
            AssetTab::Materials => ("No materials", "Press + to create one"),
            AssetTab::Particles => ("No particle effects", "Press + to create one"),
            AssetTab::Models => (
                "No models",
                "Press + for a fake object, or add a .glb to the manifest",
            ),
            _ => ("Nothing here", "Declare assets in assets/manifest.toml"),
        };
        theme::empty_state(ui, icon::FOLDER_OPEN, title, hint);
        return;
    }

    let tile_w = 104.0;
    let tile_h = 128.0;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let cols = ((ui.available_width() + 8.0) / (tile_w + 8.0))
                .floor()
                .max(1.0) as usize;
            for row in tiles.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    for t in row {
                        tile(app, ui, t, Vec2::new(tile_w, tile_h));
                    }
                });
                ui.add_space(8.0);
            }
        });
}

fn count(e: &trino_scene::Entity, n: &mut usize) {
    *n += 1;
    for c in &e.children {
        count(c, n);
    }
}

fn tile(app: &mut App, ui: &mut egui::Ui, t: &Tile, size: Vec2) {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let selected = t.sel.is_some() && app.asset_sel == t.sel;
    let id = ui.id().with(("tile", &t.name, t.glyph));
    let hov = ui.ctx().animate_bool_with_time(id, resp.hovered(), 0.12);
    let sel = ui
        .ctx()
        .animate_bool_with_time(id.with("s"), selected, 0.15);
    let painter = ui.painter().clone();
    let lift = hov * 2.0;
    let r = rect.translate(Vec2::new(0.0, -lift));
    painter.rect(
        r,
        CornerRadius::same(10),
        theme::mix(
            theme::mix(c::BG2.gamma_multiply(0.6), c::BG3, hov),
            c::ACCENT_SOFT,
            sel,
        ),
        Stroke::new(1.0, theme::mix(c::BORDER, c::ACCENT, sel.max(hov * 0.4))),
        egui::StrokeKind::Inside,
    );
    let img = Rect::from_min_size(
        r.min + Vec2::new(8.0, 8.0),
        Vec2::new(size.x - 16.0, size.x - 16.0),
    );
    painter.rect_filled(img, CornerRadius::same(7), c::BG0);
    match t.thumb {
        Some(tex) => {
            painter.image(
                tex,
                img.shrink(2.0),
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => {
            painter.text(
                img.center(),
                egui::Align2::CENTER_CENTER,
                t.glyph,
                egui::FontId::proportional(30.0),
                t.color.gamma_multiply(0.8),
            );
        }
    }
    if let Some(b) = t.badge {
        let p = img.right_top() + Vec2::new(-12.0, 12.0);
        painter.circle_filled(p, 10.0, c::BG1);
        painter.text(
            p,
            egui::Align2::CENTER_CENTER,
            b,
            egui::FontId::proportional(12.0),
            c::ACCENT_TEXT,
        );
    }
    let name_rect = Rect::from_min_size(
        Pos2::new(r.min.x + 6.0, img.max.y + 4.0),
        Vec2::new(size.x - 12.0, 18.0),
    );
    ui.put(
        name_rect,
        egui::Label::new(RichText::new(&t.name).size(12.5).color(c::TEXT))
            .truncate()
            .selectable(false),
    );
    painter.text(
        Pos2::new(r.center().x, name_rect.max.y + 6.0),
        egui::Align2::CENTER_CENTER,
        &t.sub,
        egui::FontId::proportional(10.5),
        c::TEXT_FAINT,
    );

    let resp = resp.on_hover_text(match &t.payload {
        Some(_) => format!(
            "{}\nDouble-click to add · drag onto a view to place",
            t.name
        ),
        None => t.name.clone(),
    });
    if resp.clicked()
        && let Some(s) = &t.sel
    {
        app.cmds.push(Cmd::SelectAsset(s.clone()));
    }
    if resp.double_clicked()
        && let Some(p) = &t.payload
    {
        app.cmds.push(Cmd::AddAsset(p.clone(), None));
    }
    if resp.drag_started()
        && let Some(p) = &t.payload
    {
        resp.dnd_set_drag_payload(p.clone());
    }
    if let Some(p) = &t.payload
        && resp.dragged()
    {
        // Floating label follows the cursor while dragging.
        if let Some(pos) = ui.ctx().pointer_interact_pos() {
            egui::Area::new(id.with("drag"))
                .order(egui::Order::Tooltip)
                .fixed_pos(pos + Vec2::new(12.0, 8.0))
                .interactable(false)
                .show(ui.ctx(), |ui| {
                    theme::pill(
                        ui,
                        &format!("{} {}", t.glyph, p.label()),
                        Color32::WHITE,
                        c::ACCENT,
                    );
                });
        }
    }
    let payload = t.payload.clone();
    let sel = t.sel.clone();
    resp.context_menu(|ui| {
        if let Some(p) = &payload
            && ui.button(format!("{}  Add to scene", icon::PLUS)).clicked()
        {
            app.cmds.push(Cmd::AddAsset(p.clone(), None));
        }
        if let Some(AssetSel::Prefab(l)) = &sel
            && ui
                .button(format!("{}  Edit prefab", icon::PENCIL_SIMPLE))
                .clicked()
        {
            app.cmds.push(Cmd::EditPrefab(l.clone()));
        }
        if let Some(AssetSel::Material(l)) = &sel
            && ui.button(format!("{}  Duplicate", icon::COPY)).clicked()
        {
            let def = app.lib.materials.get(l).cloned().unwrap_or_default();
            app.cmds.push(Cmd::Open(Dialog::new_material_from(def)));
        }
        if let Some(s) = &sel {
            let path = match s {
                AssetSel::Model(l)
                | AssetSel::Material(l)
                | AssetSel::Prefab(l)
                | AssetSel::Sprite(l)
                | AssetSel::Audio(l) => l.clone(),
                AssetSel::Particles(n) => format!("particles/{n}"),
            };
            if ui
                .button(format!("{}  Copy logical path", icon::COPY))
                .clicked()
            {
                ui.ctx().copy_text(path);
            }
        }
    });
}
