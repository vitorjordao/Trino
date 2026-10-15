//! Assets › Particles › Inspector: a particle effect (`*.particles.ron`)
//! with a live preview — rate, life, speed, direction and cone, gravity,
//! size, color, flipbook, the cap per platform (or off there), the cost in
//! triangles and estimated N64 time, and a burst on demand. Edits write the
//! file (hot-reloaded everywhere); `cargo xtask particles` reads the same
//! file and runs the same simulation.

use std::time::Instant;

use eframe::egui::{self, RichText, Vec2};
use trino_asset_pipeline::Platform;
use trino_asset_pipeline::particles::{MAX_PARTICLES, ParticlesDef, PlatformCap};
use trino_core::perf::N64_COST;
use trino_core::{ParticleSystem, Vec3};
use trino_platform_pc::SimProfile;

use crate::app::App;
use crate::preview::Preview;
use crate::theme::{self, c, icon};

/// The live preview: one system at the largest capacity, rebuilt when the
/// effect or the previewed platform changes.
pub struct ParticlesView {
    pub preview: Option<Preview>,
    pub system: Box<ParticleSystem<{ MAX_PARTICLES as usize }>>,
    /// (effect name, its settings, platform) the system was built from.
    pub built: Option<(String, ParticlesDef, Platform)>,
    pub platform: Platform,
    pub seed: u32,
    pub burst: usize,
    pub last: Instant,
}

impl Default for ParticlesView {
    fn default() -> Self {
        ParticlesView {
            preview: None,
            system: Box::new(ParticleSystem::new(trino_core::Emitter::DEFAULT, 1)),
            built: None,
            platform: Platform::Pc,
            seed: 1,
            burst: 20,
            last: Instant::now(),
        }
    }
}

impl ParticlesView {
    /// Make the system match `def` on the previewed platform (a restart
    /// when either changed). Returns true when it was rebuilt.
    pub fn sync(&mut self, name: &str, def: &ParticlesDef) -> bool {
        let key = (name.to_string(), def.clone(), self.platform);
        if self.built.as_ref() == Some(&key) {
            return false;
        }
        *self.system = ParticleSystem::from_effect(&def.effect(self.platform), self.seed);
        self.built = Some(key);
        true
    }

    /// Advance the live system by `dt`.
    pub fn step(&mut self, dt: f32) {
        self.system.update(dt);
    }
}

fn profile_of(p: Platform) -> SimProfile {
    match p {
        Platform::Pc => SimProfile::Native,
        Platform::N64 => SimProfile::N64,
        Platform::N3ds => SimProfile::N3ds,
    }
}

/// Two drag values side by side for a (min, max) range.
fn range(ui: &mut egui::Ui, label: &str, v: &mut (f32, f32), speed: f64, suffix: &str) -> bool {
    theme::prop(ui, label, |ui| {
        let a = ui
            .add(
                egui::DragValue::new(&mut v.0)
                    .speed(speed)
                    .suffix(suffix)
                    .max_decimals(2),
            )
            .on_hover_text("min")
            .changed();
        let b = ui
            .add(
                egui::DragValue::new(&mut v.1)
                    .speed(speed)
                    .suffix(suffix)
                    .max_decimals(2),
            )
            .on_hover_text("max")
            .changed();
        if v.1 < v.0 {
            v.1 = v.0;
        }
        a || b
    })
}

/// The effect's fields. Returns true when one changed.
pub fn editor(ui: &mut egui::Ui, def: &mut ParticlesDef, sprites: &[String]) -> bool {
    let mut changed = false;
    theme::prop(ui, "Sprite", |ui| {
        let w = ui.available_width();
        theme::fixed_width(ui, w, |ui| {
            egui::ComboBox::from_id_salt("pfx-sprite")
                .selected_text(&def.sprite)
                .width(w)
                .truncate()
                .show_ui(ui, |ui| {
                    for s in sprites {
                        if ui.selectable_label(def.sprite == *s, s).clicked() {
                            def.sprite = s.clone();
                            changed = true;
                        }
                    }
                })
        });
    });
    theme::prop(ui, "Rate", |ui| {
        changed |= ui
            .add(egui::Slider::new(&mut def.rate, 0.0..=200.0).suffix("/s"))
            .changed();
    });
    changed |= range(ui, "Life", &mut def.life, 0.02, " s");
    def.life.0 = def.life.0.max(0.01);
    changed |= range(ui, "Speed", &mut def.speed, 0.05, "");
    changed |= theme::vec3_row(ui, "Direction", &mut def.direction, 0.02, "");
    theme::prop(ui, "Cone", |ui| {
        changed |= ui
            .add(egui::Slider::new(&mut def.spread, 0.0..=180.0).suffix("°"))
            .changed();
    });
    changed |= theme::vec3_row(ui, "Gravity", &mut def.gravity, 0.05, "");
    changed |= range(ui, "Size", &mut def.size, 0.01, " m");
    theme::prop(ui, "Color", |ui| {
        changed |= theme::color_rgba(ui, &mut def.color.0);
        ui.label(RichText::new("→").color(c::TEXT_FAINT));
        changed |= theme::color_rgba(ui, &mut def.color.1);
    });
    theme::prop(ui, "Flipbook", |ui| {
        changed |= ui
            .add(
                egui::DragValue::new(&mut def.frames)
                    .range(0..=64)
                    .suffix(" frames"),
            )
            .changed();
        if def.frames > 0 {
            for v in def.frame_size.iter_mut() {
                changed |= ui
                    .add(egui::DragValue::new(v).range(1.0..=256.0).suffix(" px"))
                    .changed();
            }
        }
    });
    if def.frames > 0 && def.frame_size.iter().any(|v| *v <= 0.0) {
        def.frame_size = [8.0, 8.0];
    }
    theme::prop(ui, "Max", |ui| {
        changed |= ui
            .add(egui::DragValue::new(&mut def.max).range(1..=MAX_PARTICLES))
            .on_hover_text("The most particles alive at once (every platform unless capped)")
            .changed();
    });
    changed |= caps(ui, def);
    changed
}

/// The cap per platform, as a row each: its own max, or off there, and
/// what its peak costs.
fn caps(ui: &mut egui::Ui, def: &mut ParticlesDef) -> bool {
    let mut changed = false;
    theme::section_title(ui, icon::DEVICES, "Per platform");
    for p in Platform::ALL {
        let label = p.target().label();
        let base = def.max;
        let slot = def.platforms.get_mut(p);
        let mut cap = slot.unwrap_or_default();
        ui.horizontal(|ui| {
            ui.add_sized(
                [34.0, 20.0],
                egui::Label::new(RichText::new(label).color(c::TEXT_WEAK)),
            );
            let mut own = cap.max.is_some();
            let r = ui.add(egui::Checkbox::without_text(&mut own));
            r.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Checkbox,
                    true,
                    own,
                    format!("Own cap on {label}"),
                )
            });
            if r.changed() {
                cap.max = own.then_some(base);
            }
            if let Some(m) = &mut cap.max {
                ui.add(egui::DragValue::new(m).range(0..=MAX_PARTICLES).speed(0.5));
            } else {
                ui.label(RichText::new(format!("= {base}")).color(c::TEXT_FAINT));
            }
            let r = ui.add(egui::Checkbox::without_text(&mut cap.disabled));
            let off = cap.disabled;
            r.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Checkbox,
                    true,
                    off,
                    format!("Off on {label}"),
                )
            });
            ui.label(RichText::new("off").size(11.0).color(c::TEXT_FAINT));
        });
        let new = (cap != PlatformCap::default()).then_some(cap);
        if new != *slot {
            *slot = new;
            changed = true;
        }
        let n = def.cap(p) as u32;
        ui.add(
            egui::Label::new(
                RichText::new(if n == 0 {
                    format!("{label}: off — nothing drawn")
                } else {
                    format!(
                        "{label}: up to {n} · {} tris · N64 ≈{:.2} ms",
                        n * 2,
                        N64_COST.billboards_us(n) as f32 / 1000.0
                    )
                })
                .size(11.0)
                .color(c::TEXT_FAINT),
            )
            .truncate(),
        );
    }
    changed
}

/// The inspector page of effect `name`.
pub fn asset(app: &mut App, ui: &mut egui::Ui, name: &str) {
    let Some(mut def) = app.particles.get(name).cloned() else {
        theme::empty_state(ui, icon::SPARKLE, "Effect not found", name);
        return;
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::SPARKLE).size(22.0).color(c::TRIGGER));
        ui.vertical(|ui| {
            ui.label(RichText::new(name).family(theme::bold()).size(16.0));
            ui.label(
                RichText::new(format!("particles/{name} · Particle effect"))
                    .size(11.5)
                    .color(c::TEXT_FAINT),
            );
        });
    });
    ui.add_space(6.0);

    // Live preview.
    let v = &mut app.particles_view;
    v.sync(name, &def);
    let dt = v.last.elapsed().as_secs_f32().min(0.1);
    v.last = Instant::now();
    v.step(dt);
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(w, (w * 0.6).clamp(140.0, 260.0)),
        egui::Sense::hover(),
    );
    let rs = app.render_state.clone();
    let profile = profile_of(v.platform);
    let sys = &v.system;
    let p = Preview::ensure(&mut v.preview, &rs, profile);
    let generation = app.project.generation;
    if p.uploaded.as_ref() != Some(&("particles".to_string(), generation)) {
        app.project.upload_all(&mut p.renderer);
        p.cam.focus(Vec3::new(0.0, 1.0, 0.0), 0.9);
        p.uploaded = Some(("particles".to_string(), generation));
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    p.show(
        &mut child,
        rect,
        true,
        |r: &mut trino_platform_pc::PcRenderer, _| {
            sys.draw(r);
        },
    );
    ui.ctx().request_repaint();
    let v = &mut app.particles_view;
    ui.horizontal_wrapped(|ui| {
        for p in Platform::ALL {
            if ui
                .selectable_label(v.platform == p, format!("{} preview", p.target().label()))
                .clicked()
            {
                v.platform = p;
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        if ui.button(format!("{} Burst", icon::SPARKLE)).clicked() {
            v.system.burst(v.burst);
        }
        ui.add(egui::DragValue::new(&mut v.burst).range(1..=MAX_PARTICLES as usize))
            .on_hover_text("Particles per burst");
        ui.checkbox(&mut v.system.emitting, "Emitting");
        if ui
            .button(format!("{} Restart", icon::ARROW_COUNTER_CLOCKWISE))
            .clicked()
        {
            v.built = None;
        }
    });
    let live = v.system.len();
    ui.add(
        egui::Label::new(
            RichText::new(format!(
                "{live} / {} live on {} · {} tris · N64 ≈{:.2} ms",
                v.system.limit,
                v.platform.target().label(),
                live * 2,
                N64_COST.billboards_us(live as u32) as f32 / 1000.0
            ))
            .color(c::ACCENT_TEXT),
        )
        .truncate(),
    );
    ui.add_space(6.0);

    let sprites: Vec<String> = app.project.sprites.keys().cloned().collect();
    let changed = theme::component(
        ui,
        "c-particles",
        icon::SLIDERS_HORIZONTAL,
        c::TEXT,
        "Effect",
        |ui| editor(ui, &mut def, &sprites),
    )
    .unwrap_or(false);
    if changed && def.validate().is_ok() {
        app.particles.insert(name.to_string(), def);
        app.pending_particles.insert(name.to_string());
    }
    ui.label(
        RichText::new(format!(
            "assets/shared/particles/{name}.particles.ron · cargo xtask particles inspect {name}"
        ))
        .size(11.0)
        .monospace()
        .color(c::TEXT_FAINT),
    );
}
