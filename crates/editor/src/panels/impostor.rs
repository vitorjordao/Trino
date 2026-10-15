//! Inspector › Model asset › Impostor: pre-render the model from N yaws
//! into a billboard atlas — create the settings, tune them, bake, and look
//! at the atlas. The settings are `assets/impostors/<name>.impostor.ron`
//! and the bake is `trino_asset_pipeline::impostor::bake`, the same file and
//! the same function `cargo xtask impostor` uses: an agent and a person
//! produce identical results, and each sees the other's changes.

use eframe::egui::{self, RichText, Vec2};
use trino_asset_pipeline::impostor::{self, ImpostorDef};
use trino_core::perf::N64_COST;

use crate::app::{App, Cmd};
use crate::theme::{self, c, icon};

/// Tile sizes offered (powers of two the bake accepts).
const TILES: [u16; 6] = [8, 16, 32, 64, 128, 256];

/// The settings fields. Returns true when one changed.
pub fn settings_editor(ui: &mut egui::Ui, def: &mut ImpostorDef) -> bool {
    let mut changed = false;
    theme::prop(ui, "Views", |ui| {
        changed |= ui
            .add(egui::Slider::new(&mut def.views, 2..=32))
            .on_hover_text("Yaws around the model; the camera's direction picks one")
            .changed();
    });
    theme::prop(ui, "Pitch", |ui| {
        changed |= ui
            .add(egui::Slider::new(&mut def.pitch, -60.0..=85.0).suffix("°"))
            .on_hover_text("Camera elevation the views are rendered from")
            .changed();
    });
    theme::prop(ui, "Tile", |ui| {
        egui::ComboBox::from_id_salt("impostor-tile")
            .selected_text(format!("{} px", def.tile))
            .show_ui(ui, |ui| {
                for t in TILES {
                    if ui
                        .selectable_label(def.tile == t, format!("{t} px"))
                        .clicked()
                    {
                        def.tile = t;
                        changed = true;
                    }
                }
            })
            .response
            .on_hover_text("Texels per view on PC and 3DS");
    });
    theme::prop(ui, "N64 tile", |ui| {
        let auto = impostor::n64_tile(def.views, def.tile);
        let shown = def
            .n64_tile
            .map_or_else(|| format!("auto · {auto} px"), |t| format!("{t} px"));
        egui::ComboBox::from_id_salt("impostor-n64-tile")
            .selected_text(shown)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(def.n64_tile.is_none(), format!("auto · {auto} px"))
                    .clicked()
                {
                    def.n64_tile = None;
                    changed = true;
                }
                for t in TILES.into_iter().filter(|t| *t <= 64) {
                    if ui
                        .selectable_label(def.n64_tile == Some(t), format!("{t} px"))
                        .clicked()
                    {
                        def.n64_tile = Some(t);
                        changed = true;
                    }
                }
            })
            .response
            .on_hover_text("Auto: the largest tile whose CI4 atlas fits the N64's TMEM");
    });
    let (cols, rows) = impostor::grid(def.views);
    let n64 = def
        .n64_tile
        .unwrap_or_else(|| impostor::n64_tile(def.views, def.tile)) as u32;
    let (w, h) = (cols as u32 * n64, rows as u32 * n64);
    let fits = w * h <= impostor::N64_CI4_TEXELS;
    ui.add(
        egui::Label::new(
            RichText::new(format!(
                "{cols}x{rows} grid · PC {}x{} · N64 {w}x{h} CI4 {}",
                cols as u32 * def.tile as u32,
                rows as u32 * def.tile as u32,
                if fits { "fits TMEM" } else { "past TMEM!" }
            ))
            .size(11.0)
            .color(if fits { c::TEXT_FAINT } else { c::ERROR }),
        )
        .truncate(),
    );
    changed
}

/// The section for model `logical`.
pub fn section(app: &mut App, ui: &mut egui::Ui, logical: &str, tris: usize, verts: usize) {
    let found = app
        .impostor_defs
        .iter()
        .find(|(_, d)| d.model == logical)
        .map(|(n, d)| (n.clone(), d.clone()));
    let focus = app.inspect_focus.as_deref() == Some("impostor");
    let top = ui.cursor().min;
    theme::component_open(
        ui,
        "c-impostor",
        icon::IMAGE_SQUARE,
        c::SPRITE,
        "Impostor",
        found.is_some() || focus,
        |ui| match found {
            None => create(app, ui, logical),
            Some((name, def)) => existing(app, ui, &name, def, tris, verts),
        },
    );
    if focus {
        ui.scroll_to_rect(
            egui::Rect::from_min_size(top, Vec2::new(1.0, 1.0)),
            Some(egui::Align::TOP),
        );
    }
}

fn create(app: &mut App, ui: &mut egui::Ui, logical: &str) {
    ui.label(
        RichText::new(
            "A billboard pre-rendered from several yaws: 2 triangles far away, as a \
             LOD level. Fast on 3DS and PC; on the N64 measure first (textured \
             triangles are CPU-submitted).",
        )
        .size(11.5)
        .color(c::TEXT_FAINT),
    );
    let id = ui.id().with(("impostor-name", logical));
    let default = logical.trim_start_matches("models/").replace('_', "-");
    let mut name = ui.data_mut(|d| d.get_temp::<String>(id).unwrap_or(default));
    let valid = trino_asset_pipeline::manifest::edit::valid_name(&name);
    theme::prop(ui, "Name", |ui| {
        ui.add(egui::TextEdit::singleline(&mut name).desired_width(ui.available_width()));
    });
    if theme::primary_button(ui, format!("{}  Create and bake", icon::IMAGE_SQUARE))
        .on_hover_text(format!(
            "Writes assets/impostors/{name}.impostor.ron and bakes it \
             (cargo xtask impostor new {name} --model {logical})"
        ))
        .clicked()
        && valid
    {
        app.cmds
            .push(Cmd::SaveImpostor(name.clone(), ImpostorDef::new(logical)));
        app.cmds.push(Cmd::BakeImpostor(name.clone()));
    }
    if !valid {
        ui.label(
            RichText::new("lowercase letters, digits, - and _")
                .size(11.0)
                .color(c::WARN),
        );
    }
    ui.data_mut(|d| d.insert_temp(id, name));
}

fn existing(
    app: &mut App,
    ui: &mut egui::Ui,
    name: &str,
    def: ImpostorDef,
    tris: usize,
    verts: usize,
) {
    let assets = app.project.assets_dir();
    let state = match impostor::check(&assets, name) {
        Ok(()) => (format!("{} baked, up to date", icon::CHECK_CIRCLE), c::OK),
        Err(e) if e.contains("stale") => (
            format!("{} stale: the model changed — bake again", icon::WARNING),
            c::WARN,
        ),
        Err(e) if e.contains("not baked") => (format!("{} not baked yet", icon::WARNING), c::WARN),
        Err(e) => (format!("{} {e}", icon::X_CIRCLE), c::ERROR),
    };
    ui.add(egui::Label::new(RichText::new(state.0).color(state.1)).truncate());
    ui.add(
        egui::Label::new(
            RichText::new(format!(
                "impostors/{name} · assets/impostors/{name}.impostor.ron"
            ))
            .size(11.0)
            .monospace()
            .color(c::TEXT_FAINT),
        )
        .truncate(),
    );
    // Edits stay in the panel until Bake (no file write per slider step);
    // a change on disk (an agent's) replaces an untouched edit.
    let id = ui.id().with(("impostor-edit", name));
    let (base, mut edit) = ui
        .data_mut(|d| d.get_temp::<(ImpostorDef, ImpostorDef)>(id))
        .filter(|(b, _)| *b == def)
        .unwrap_or_else(|| (def.clone(), def.clone()));
    settings_editor(ui, &mut edit);
    let dirty = edit != base;
    ui.horizontal_wrapped(|ui| {
        let label = if dirty {
            "Save and bake"
        } else {
            "Bake impostor"
        };
        if theme::primary_button(ui, format!("{}  {label}", icon::IMAGE_SQUARE))
            .on_hover_text(format!(
                "Writes assets/impostors/{name}.impostor.ron and runs the bake: \
                 cargo xtask impostor bake {name}"
            ))
            .clicked()
        {
            if dirty {
                app.cmds
                    .push(Cmd::SaveImpostor(name.to_string(), edit.clone()));
            }
            app.cmds.push(Cmd::BakeImpostor(name.to_string()));
        }
        if dirty && ui.button("Revert").clicked() {
            edit = base.clone();
        }
        if ui
            .button(format!("{} Copy LOD line", icon::COPY))
            .on_hover_text("A scene LOD level that draws it")
            .clicked()
        {
            ui.ctx().copy_text(format!(
                "(distance: 30.0, impostor: Some(\"impostors/{name}\"))"
            ));
        }
    });
    ui.data_mut(|d| d.insert_temp(id, (base, edit)));
    // Cost: one billboard against one draw of the mesh.
    ui.add(
        egui::Label::new(
            RichText::new(format!(
                "2 tris (mesh {tris}) · N64 ≈{:.2} ms (mesh ≈{:.2} ms on the CPU path)",
                N64_COST.billboards_us(1) as f32 / 1000.0,
                N64_COST.mesh_draw_us(verts as u32, tris as u32) as f32 / 1000.0
            ))
            .size(11.0)
            .color(c::TEXT_FAINT),
        )
        .truncate(),
    )
    .on_hover_text(
        "Most static N64 meshes run on the RSP and cost far less than the CPU-path \
         estimate, while a billboard's textured triangles are CPU-submitted: an \
         impostor can make the N64 slower. Measure (cargo xtask test n64) and give \
         the N64 its own LOD distances.",
    );
    // The atlas as baked for PC/3DS.
    let sprite = impostor::sprite_logical(name);
    let generation = app.project.generation;
    let tex = match app.impostor_thumbs.get(name) {
        Some((g, t)) if *g == generation => Some(t.clone()),
        _ => app.project.sprites.get(&sprite).map(|s| {
            let img = egui::ColorImage::from_rgba_unmultiplied(
                [s.width as usize, s.height as usize],
                &s.rgba,
            );
            let t = ui.ctx().load_texture(
                format!("impostor-{name}"),
                img,
                egui::TextureOptions::NEAREST,
            );
            app.impostor_thumbs
                .insert(name.to_string(), (generation, t.clone()));
            t
        }),
    };
    if let Some(t) = tex {
        let size = t.size_vec2();
        let w = ui.available_width().min(size.x * 4.0);
        let img = ui.add(egui::Image::new((
            t.id(),
            Vec2::new(w, w * size.y / size.x),
        )));
        img.on_hover_text(format!(
            "{} views, {}x{} texels — view i was rendered from yaw i·360°/{}",
            def.views, size.x, size.y, def.views
        ));
    }
}
