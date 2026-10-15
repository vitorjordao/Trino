//! The **Animation** tab: preview and check the clips of any model with a
//! skeleton — clip list, play / pause / frame steps, a time scrubber and
//! speed, a crossfade toward a second clip with its weight (`AnimBlend`),
//! measurements per clip with warnings (stride speed and distance per
//! cycle, foot slide, foot under the floor, bounds), a ground grid, onion
//! skin, the color-slot palette and the PC / N64 / 3DS looks; and a
//! "console pose": the current pose baked into a static model.
//!
//! The measurements and the pose export are `trino_asset_pipeline::anim`,
//! the same code `cargo xtask model inspect --anim` and `model pose` run.
//! What the tab changes ends in a file: the entity's clip and speed (Use on
//! entity → the scene), or the exported pose model (glTF + manifest).

use std::time::Instant;

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Vec2};
use eframe::egui_wgpu::RenderState;
use trino_asset_pipeline::anim::{self, ClipMetrics, Limits};
use trino_core::{
    AnimRef, ClipId, Color, Material, Mesh, ModelId, ModelParams, Renderer, Shading, Transform3,
};
use trino_platform_pc::{PcRenderer, SimProfile};

use crate::app::{App, Cmd};
use crate::gizmo::OrbitCam;
use crate::theme::{self, c, icon};

/// The tab's state (what is previewed and how).
pub struct AnimPanel {
    /// Logical path of the previewed model (`models/robot`).
    pub model: Option<String>,
    pub clip: Option<String>,
    pub playing: bool,
    /// Seconds into the clip.
    pub time: f32,
    pub speed: f32,
    /// Crossfade toward this clip by this weight (0..1).
    pub blend: Option<(String, f32)>,
    /// Earlier frames drawn faded behind the pose (0 = off).
    pub ghosts: u8,
    pub grid: bool,
    /// Colors for the mesh's color slots 1..=4 (`ModelParams::palette`).
    pub palette: Option<[[u8; 3]; 4]>,
    pub profile: SimProfile,
    /// Name the console pose is exported under.
    pub pose_name: String,
    /// Measurements of the model's clips (cached per model and bake).
    pub metrics: Vec<ClipMetrics>,
    metrics_key: Option<(String, u64)>,
    preview: Option<Preview>,
    last: Instant,
}

impl Default for AnimPanel {
    fn default() -> Self {
        AnimPanel {
            model: None,
            clip: None,
            playing: true,
            time: 0.0,
            speed: 1.0,
            blend: None,
            ghosts: 0,
            grid: true,
            palette: None,
            profile: SimProfile::Native,
            pose_name: String::new(),
            metrics: Vec::new(),
            metrics_key: None,
            preview: None,
            last: Instant::now(),
        }
    }
}

impl AnimPanel {
    /// Dev hook `TRINO_EDITOR_ANIM=<model>:<clip>@<time>` (screenshots,
    /// tests): previews that clip, paused at that time.
    pub fn from_env() -> Self {
        let mut p = Self::from_hook(std::env::var("TRINO_EDITOR_ANIM").ok().as_deref());
        if let Ok(v) = std::env::var("TRINO_EDITOR_ANIM_VIEW") {
            p.apply_view(&v);
        }
        p
    }

    /// Dev hook `TRINO_EDITOR_ANIM_VIEW=ghosts=3,blend=wave:0.5,look=n64,palette`.
    pub fn apply_view(&mut self, spec: &str) {
        for part in spec.split(',').map(str::trim) {
            match part.split_once('=') {
                Some(("ghosts", n)) => self.ghosts = n.parse().unwrap_or(0).min(6),
                Some(("blend", b)) => {
                    let (clip, w) = b.split_once(':').unwrap_or((b, "0.5"));
                    self.blend = Some((clip.to_string(), w.parse().unwrap_or(0.5)));
                }
                Some(("look", l)) => {
                    self.profile = match l {
                        "n64" => SimProfile::N64,
                        "3ds" => SimProfile::N3ds,
                        _ => SimProfile::Native,
                    }
                }
                _ if part == "palette" => {
                    self.palette = Some([
                        [230, 80, 80],
                        [80, 200, 120],
                        [80, 120, 230],
                        [240, 200, 60],
                    ])
                }
                _ => {}
            }
        }
    }

    /// `<model>:<clip>@<time>` (each part optional after the model).
    pub fn from_hook(hook: Option<&str>) -> Self {
        let mut p = AnimPanel::default();
        if let Some(v) = hook {
            let (model, rest) = v.split_once(':').unwrap_or((v, ""));
            let (clip, time) = rest.split_once('@').unwrap_or((rest, ""));
            p.model = Some(model.to_string());
            if !clip.is_empty() {
                p.clip = Some(clip.to_string());
            }
            if let Ok(t) = time.parse() {
                p.time = t;
                p.playing = false;
            }
        }
        p
    }

    /// The pose drawn: the clip at `time`, crossfaded when set.
    pub fn anim_ref(&self) -> Option<AnimRef> {
        let clip = ClipId::from_name(self.clip.as_deref()?);
        let a = AnimRef::new(clip, self.time);
        Some(match &self.blend {
            Some((b, w)) => a.blend_to(ClipId::from_name(b), self.time, *w),
            None => a,
        })
    }

    /// The current clip's measurements.
    pub fn current_metrics(&self) -> Option<&ClipMetrics> {
        let clip = self.clip.as_deref()?;
        self.metrics.iter().find(|m| m.name == clip)
    }

    /// Advance a playing clip by `dt` seconds (looping clips wrap).
    pub fn tick(&mut self, dt: f32) {
        if !self.playing {
            return;
        }
        let Some(m) = self.current_metrics() else {
            return;
        };
        let (d, looping) = (m.duration.max(1e-3), m.looping);
        self.time += dt * self.speed;
        if looping {
            self.time = self.time.rem_euclid(d);
        } else if self.time > d {
            self.time = d;
            self.playing = false;
        }
    }

    /// One frame forward (`+1`) or back (`-1`), paused.
    pub fn step(&mut self, dir: f32, fps: f32) {
        self.playing = false;
        let d = self.current_metrics().map_or(0.0, |m| m.duration);
        let f = (self.time * fps).round() + dir;
        self.time = (f / fps).clamp(0.0, d.max(0.0));
    }
}

/// The tab's own renderer: the model alone, orbited by mouse.
struct Preview {
    render_state: RenderState,
    renderer: PcRenderer,
    texture: egui::TextureId,
    profile: SimProfile,
    size: (u32, u32),
    cam: OrbitCam,
    /// (model, project generation) uploaded.
    uploaded: Option<(String, u64)>,
}

const GRID: ModelId = ModelId(trino_core::asset_id("__editor/anim-grid"));

impl Preview {
    fn new(render_state: RenderState, profile: SimProfile) -> Self {
        let device = wgpu::Device::clone(&render_state.device);
        let queue = wgpu::Queue::clone(&render_state.queue);
        let mut renderer = PcRenderer::with_device(device, queue, profile);
        renderer.upload_mesh(GRID, crate::viewport::grid_tmdl());
        let texture = render_state.renderer.write().register_native_texture(
            &render_state.device,
            renderer.offscreen_view(),
            wgpu::FilterMode::Nearest,
        );
        Preview {
            render_state,
            renderer,
            texture,
            profile,
            size: (0, 0),
            cam: OrbitCam::default(),
            uploaded: None,
        }
    }

    fn reregister(&mut self) {
        let mut r = self.render_state.renderer.write();
        r.free_texture(&self.texture);
        self.texture = r.register_native_texture(
            &self.render_state.device,
            self.renderer.offscreen_view(),
            wgpu::FilterMode::Nearest,
        );
    }
}

/// An icon button whose accessible name is `name` (screen readers and the
/// UI tests find it by that, not by the glyph).
fn named_icon(ui: &mut egui::Ui, glyph: &str, name: &str) -> egui::Response {
    let r = theme::icon_button(ui, glyph, name);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    r
}

/// Models with a skeleton and clips, by logical path.
fn animated(app: &App) -> Vec<String> {
    app.project
        .models
        .values()
        .filter(|m| m.joints > 0 && !m.clips.is_empty())
        .map(|m| m.logical.clone())
        .collect()
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    let models = animated(app);
    if models.is_empty() {
        theme::empty_state(
            ui,
            icon::PERSON_SIMPLE_WALK,
            "No animated models",
            "Models with a skeleton and clips show here (glTF skins or node animation)",
        );
        return;
    }
    // Follow the selected entity's model when it is animated.
    let selected_model = app
        .doc
        .selected
        .as_ref()
        .and_then(|p| app.doc.scene.get(p))
        .and_then(|e| e.model.as_ref())
        .map(|m| m.path.clone())
        .filter(|m| models.contains(m));
    let a = &mut app.anim;
    if a.model.as_ref().is_none_or(|m| !models.contains(m)) {
        a.model = selected_model.clone().or_else(|| models.first().cloned());
        a.clip = None;
    }
    let model = a.model.clone().unwrap_or_default();
    let Some(asset) = app.project.models.get(&model) else {
        return;
    };
    let clips = asset.clips.clone();
    let (tris, verts) = (asset.tris, asset.verts);
    let tmdl = asset.tmdl.clone();
    let (center, radius) = (asset.center, asset.radius);
    let generation = app.project.generation;
    let a = &mut app.anim;
    if a.clip.as_ref().is_none_or(|c| !clips.contains(c)) {
        a.clip = clips.first().cloned();
        a.time = 0.0;
    }
    if a.metrics_key != Some((model.clone(), generation)) {
        a.metrics = anim::all_metrics(&tmdl, &clips, Limits::default());
        a.metrics_key = Some((model.clone(), generation));
    }
    if a.pose_name.is_empty() || !a.pose_name.starts_with(model.trim_start_matches("models/")) {
        a.pose_name = format!(
            "{}-pose",
            model.trim_start_matches("models/").replace('_', "-")
        );
    }
    let mesh = Mesh::from_tmdl(&tmdl).ok();
    let fps = mesh
        .as_ref()
        .and_then(|m| {
            a.clip
                .as_ref()
                .and_then(|c| m.find_clip(ClipId::from_name(c)))
        })
        .map_or(15.0, |c| c.fps);
    let dt = a.last.elapsed().as_secs_f32().min(0.1);
    a.last = Instant::now();
    a.tick(dt);
    if a.playing {
        ui.ctx().request_repaint();
    }

    // ---- top: model, clips, transport ---------------------------------
    ui.horizontal_wrapped(|ui| {
        let w = 180.0f32.min(ui.available_width() - 8.0);
        theme::fixed_width(ui, w, |ui| {
            egui::ComboBox::from_id_salt("anim-model")
                .selected_text(&model)
                .width(w)
                .truncate()
                .show_ui(ui, |ui| {
                    for m in &models {
                        if ui.selectable_label(*m == model, m).clicked() {
                            a.model = Some(m.clone());
                            a.clip = None;
                            a.blend = None;
                        }
                    }
                })
                .response
                .widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Animated model")
                });
        });
        for clip in &clips {
            let on = a.clip.as_deref() == Some(clip.as_str());
            if ui
                .selectable_label(on, format!("{} {clip}", icon::FILM_STRIP))
                .clicked()
            {
                a.clip = Some(clip.clone());
                a.time = 0.0;
            }
        }
    });
    let duration = a.current_metrics().map_or(0.0, |m| m.duration);
    ui.horizontal(|ui| {
        if named_icon(ui, icon::SKIP_BACK, "Previous frame").clicked() {
            a.step(-1.0, fps);
        }
        let (glyph, tip) = if a.playing {
            (icon::PAUSE, "Pause")
        } else {
            (icon::PLAY, "Play")
        };
        if named_icon(ui, glyph, tip).clicked() {
            a.playing = !a.playing;
        }
        if named_icon(ui, icon::SKIP_FORWARD, "Next frame").clicked() {
            a.step(1.0, fps);
        }
        ui.spacing_mut().slider_width = (ui.available_width() - 160.0).clamp(40.0, 360.0);
        if ui
            .add(egui::Slider::new(&mut a.time, 0.0..=duration.max(1e-3)).suffix(" s"))
            .on_hover_text("Time in the clip")
            .changed()
        {
            a.playing = false;
        }
        ui.label(
            RichText::new(format!("f {}", (a.time * fps).round() as i32))
                .monospace()
                .color(c::TEXT_WEAK),
        );
    });

    // ---- body: preview + controls -------------------------------------
    let avail = ui.available_rect_before_wrap();
    let wide = avail.width() >= 760.0;
    let side = 330.0f32.min(avail.width());
    let (view_rect, controls_rect) = if wide {
        (
            Rect::from_min_max(avail.min, Pos2::new(avail.max.x - side - 8.0, avail.max.y)),
            Rect::from_min_max(Pos2::new(avail.max.x - side, avail.min.y), avail.max),
        )
    } else {
        let h = (avail.height() * 0.45).max(120.0);
        (
            Rect::from_min_size(avail.min, Vec2::new(avail.width(), h)),
            Rect::from_min_max(Pos2::new(avail.min.x, avail.min.y + h + 8.0), avail.max),
        )
    };
    let render_state = app.render_state.clone();
    preview(app, ui, view_rect, render_state, &model, center, radius);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(controls_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    egui::ScrollArea::vertical()
        .id_salt("anim-controls")
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            ui.set_max_width(ui.available_width() - 10.0);
            controls(app, ui, &model, &clips, tris, verts);
        });
    ui.allocate_rect(avail, Sense::hover());
}

#[allow(clippy::too_many_arguments)]
fn preview(
    app: &mut App,
    ui: &mut egui::Ui,
    rect: Rect,
    render_state: RenderState,
    model: &str,
    center: [f32; 3],
    radius: f32,
) {
    let a = &mut app.anim;
    // What to draw, before the preview borrows the panel.
    let anim = a.anim_ref();
    let step = a
        .current_metrics()
        .map_or(0.1, |m| m.duration / m.frames.max(1) as f32);
    let (ghosts, grid, profile, time) = (a.ghosts, a.grid, a.profile, a.time);
    let palette = a.palette.map(|p| p.map(|c| Color::rgb(c[0], c[1], c[2])));
    let p = a
        .preview
        .get_or_insert_with(|| Preview::new(render_state.clone(), profile));
    if p.profile != profile {
        *p = Preview::new(render_state, profile);
    }
    let generation = app.project.generation;
    if p.uploaded.as_ref() != Some(&(model.to_string(), generation)) {
        app.project.upload_all(&mut p.renderer);
        let c = trino_core::Vec3::new(center[0], center[1], center[2]);
        p.cam.focus(c, radius.max(0.3));
        p.uploaded = Some((model.to_string(), generation));
    }
    let ppp = ui.ctx().pixels_per_point();
    let size = if profile == SimProfile::Native {
        let s = (
            ((rect.width() * ppp) as u32).max(16),
            ((rect.height() * ppp) as u32).max(16),
        );
        p.renderer.set_internal_size(s.0, s.1);
        s
    } else {
        p.renderer.internal_size()
    };
    let resp = ui.allocate_rect(rect, Sense::click_and_drag());
    if resp.dragged() {
        p.cam.orbit(resp.drag_delta());
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            p.cam.zoom(scroll);
        }
    }
    if p.cam.step(ui.input(|i| i.stable_dt).min(0.1)) {
        ui.ctx().request_repaint();
    }
    let id = ModelId(trino_core::asset_id(model));
    let r = &mut p.renderer;
    r.begin_frame(Color::rgb(20, 22, 30));
    r.set_camera(&p.cam.camera());
    r.set_light(&trino_core::DEFAULT_LIGHT);
    if grid {
        r.draw_model(
            GRID,
            &Transform3::default(),
            Material::VertexLit,
            &ModelParams {
                shading: Shading::Unlit,
                double_sided: true,
                ..ModelParams::DEFAULT
            },
        );
    }
    // Onion skin: earlier frames, fainter the older.
    for k in (1..=ghosts as i32).rev() {
        let t = (time - k as f32 * step * 2.0).max(0.0);
        let alpha = (110.0 * (1.0 - k as f32 / (ghosts as f32 + 1.0))) as u8;
        r.draw_model(
            id,
            &Transform3::default(),
            Material::VertexLit,
            &ModelParams {
                tint: Color::rgba(160, 190, 255, alpha.max(20)),
                anim: anim.map(|x| AnimRef { time: t, ..x }),
                palette,
                ..ModelParams::DEFAULT
            },
        );
    }
    r.draw_model(
        id,
        &Transform3::default(),
        Material::VertexLit,
        &ModelParams {
            anim,
            palette,
            ..ModelParams::DEFAULT
        },
    );
    r.end_frame();
    if p.size != size {
        p.reregister();
        p.size = size;
    }
    // Letterbox console looks; fill the rect on PC.
    let image = if profile == SimProfile::Native {
        rect
    } else {
        let fit = (rect.width() / size.0 as f32).min(rect.height() / size.1 as f32);
        Rect::from_center_size(
            rect.center(),
            Vec2::new(size.0 as f32 * fit, size.1 as f32 * fit),
        )
    };
    ui.painter().rect_filled(rect, 6.0, c::BG0);
    ui.painter().image(
        p.texture,
        image,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(250));
}

fn controls(
    app: &mut App,
    ui: &mut egui::Ui,
    model: &str,
    clips: &[String],
    tris: usize,
    verts: usize,
) {
    let a = &mut app.anim;
    theme::component(ui, "anim-play", icon::TIMER, c::TEXT, "Playback", |ui| {
        theme::prop(ui, "Speed", |ui| {
            ui.add(egui::Slider::new(&mut a.speed, 0.0..=3.0).suffix("×"));
        });
        theme::prop(ui, "Crossfade", |ui| {
            let shown = a
                .blend
                .as_ref()
                .map_or("off".to_string(), |(b, _)| b.clone());
            let w = ui.available_width();
            theme::fixed_width(ui, w, |ui| {
                egui::ComboBox::from_id_salt("anim-blend")
                    .selected_text(shown)
                    .width(w)
                    .truncate()
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(a.blend.is_none(), "off").clicked() {
                            a.blend = None;
                        }
                        for c in clips {
                            let on = a.blend.as_ref().is_some_and(|(b, _)| b == c);
                            if ui.selectable_label(on, c).clicked() {
                                let w = a.blend.as_ref().map_or(0.5, |(_, w)| *w);
                                a.blend = Some((c.clone(), w));
                            }
                        }
                    })
                    .response
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Crossfade to")
                    });
            });
        });
        if let Some((_, w)) = &mut a.blend {
            theme::prop(ui, "Weight", |ui| {
                ui.add(egui::Slider::new(w, 0.0..=1.0))
                    .on_hover_text("0 = only the clip, 1 = only the crossfade target");
            });
        }
    });

    // Measurements of the current clip.
    let current = a.current_metrics().cloned();
    theme::component(
        ui,
        "anim-metrics",
        icon::FOOTPRINTS,
        c::OK,
        "Measurements",
        |ui| {
            let Some(m) = current else {
                ui.label(RichText::new("no clip").color(c::TEXT_FAINT));
                return;
            };
            let rows = [
                (
                    "Duration",
                    format!("{:.2} s · {} frames", m.duration, m.frames),
                ),
                ("Stride speed", format!("{:.2} m/s", m.speed)),
                ("Per cycle", format!("{:.2} m", m.distance_per_cycle)),
                ("Foot slide", format!("{:.1} cm", m.foot_slide * 100.0)),
                ("Lowest point", format!("{:.3} m", m.lowest)),
            ];
            for (k, v) in rows {
                theme::prop(ui, k, |ui| {
                    ui.add(egui::Label::new(RichText::new(v).monospace()).truncate());
                });
            }
            let f = ((a.time / m.duration.max(1e-3)) * (m.frames - 1) as f32).round() as usize;
            if let Some(b) = m.bounds.get(f.min(m.bounds.len().saturating_sub(1))) {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "bounds now {:.2}×{:.2}×{:.2} m",
                            b.max[0] - b.min[0],
                            b.max[1] - b.min[1],
                            b.max[2] - b.min[2]
                        ))
                        .size(11.0)
                        .color(c::TEXT_FAINT),
                    )
                    .truncate(),
                );
            }
            if m.warnings.is_empty() {
                ui.label(
                    RichText::new(format!("{} within the limits", icon::CHECK_CIRCLE)).color(c::OK),
                );
            }
            for w in &m.warnings {
                ui.label(RichText::new(format!("{} {w}", icon::WARNING)).color(c::WARN));
            }
            ui.label(
                RichText::new(format!(
                    "Move the character at {:.2} m/s so its planted feet stay put. \
                 Same numbers: cargo xtask model inspect <file> --anim",
                    m.speed
                ))
                .size(11.0)
                .color(c::TEXT_FAINT),
            );
        },
    );

    theme::component(ui, "anim-view", icon::EYE, c::TEXT, "View", |ui| {
        ui.checkbox(&mut a.grid, "Ground grid");
        theme::prop(ui, "Onion skin", |ui| {
            ui.add(egui::Slider::new(&mut a.ghosts, 0..=6))
                .on_hover_text("Earlier frames drawn faded behind the pose");
        });
        let mut on = a.palette.is_some();
        if ui
            .checkbox(&mut on, "Palette (color slots)")
            .on_hover_text("Recolor the mesh's slots 1-4 (ModelParams::palette)")
            .changed()
        {
            a.palette = on.then_some([
                [230, 80, 80],
                [80, 200, 120],
                [80, 120, 230],
                [240, 200, 60],
            ]);
        }
        if let Some(pal) = &mut a.palette {
            ui.horizontal(|ui| {
                for c in pal.iter_mut() {
                    theme::color_rgb(ui, c);
                }
            });
        }
        ui.horizontal(|ui| {
            for (p, label) in [
                (SimProfile::Native, "PC look"),
                (SimProfile::N64, "N64 look"),
                (SimProfile::N3ds, "3DS look"),
            ] {
                if ui.selectable_label(a.profile == p, label).clicked() {
                    a.profile = p;
                }
            }
        });
    });

    theme::component(
        ui,
        "anim-pose",
        icon::CUBE,
        c::MODEL,
        "Console pose",
        |ui| {
            ui.label(
                RichText::new(
                    "Bake this pose into a static model: a console without skinning draws it \
                 (point the entity's N64 or 3DS variant at it).",
                )
                .size(11.5)
                .color(c::TEXT_FAINT),
            );
            theme::prop(ui, "Name", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut a.pose_name)
                        .desired_width(ui.available_width()),
                );
            });
            // The pose's budget: the same mesh, without its skeleton.
            let us = trino_core::perf::N64_COST.mesh_draw_us(verts as u32, tris as u32);
            crate::panels::inspector::budget_bar(
                ui,
                "N64 µs",
                us as f32,
                trino_core::perf::FRAME_30FPS_US as f32,
            );
            ui.label(
                RichText::new(format!("{tris} tris · {verts} vertices"))
                    .size(11.0)
                    .color(c::TEXT_FAINT),
            );
            let clip = a.clip.clone();
            if theme::primary_button(ui, format!("{}  Export pose", icon::DOWNLOAD_SIMPLE))
                .on_hover_text(format!(
                    "Same as: cargo xtask model pose {model} --clip {} --time {:.3} --name {}",
                    clip.as_deref().unwrap_or("-"),
                    a.time,
                    a.pose_name
                ))
                .clicked()
            {
                app.cmds.push(Cmd::ExportPose {
                    model: model.to_string(),
                    clip,
                    time: a.time,
                    name: a.pose_name.clone(),
                });
            }
        },
    );

    // Write the previewed clip into the selected entity (the scene file).
    let a = &app.anim;
    let target = app.doc.selected.clone().filter(|p| {
        app.doc
            .scene
            .get(p)
            .and_then(|e| e.model.as_ref())
            .is_some_and(|m| m.path == model)
    });
    if let (Some(path), Some(clip)) = (target, a.clip.clone()) {
        let speed = a.speed;
        if ui
            .button(format!("{}  Use on the selected entity", icon::CHECK))
            .on_hover_text("Sets its clip and speed in the scene")
            .clicked()
            && let Some(m) = app.doc.scene.get_mut(&path).and_then(|e| e.model.as_mut())
        {
            m.anim = Some(trino_scene::AnimComponent { clip, speed });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(looping: bool) -> AnimPanel {
        let mut p = AnimPanel {
            clip: Some("walk".into()),
            ..AnimPanel::default()
        };
        p.metrics = vec![ClipMetrics {
            name: "walk".into(),
            duration: 1.0,
            frames: 9,
            looping,
            speed: 0.4,
            distance_per_cycle: 0.4,
            foot_slide: 0.0,
            lowest: 0.0,
            feet: vec![1, 2],
            bounds: Vec::new(),
            warnings: Vec::new(),
        }];
        p
    }

    #[test]
    fn playback_wraps_loops_clamps_one_shots_and_steps_frames() {
        let mut p = panel(true);
        p.speed = 2.0;
        p.tick(0.75);
        assert!(
            (p.time - 0.5).abs() < 1e-6,
            "1.5 s wraps to 0.5: {}",
            p.time
        );
        let mut once = panel(false);
        once.tick(3.0);
        assert_eq!(once.time, 1.0);
        assert!(!once.playing, "a one-shot stops at its end");
        // Frame steps pause and land on frames (8 fps).
        let mut s = panel(true);
        s.step(1.0, 8.0);
        assert!(!s.playing);
        assert_eq!(s.time, 0.125);
        s.step(-1.0, 8.0);
        s.step(-1.0, 8.0);
        assert_eq!(s.time, 0.0, "never before the start");
        s.time = 0.99;
        s.step(1.0, 8.0);
        assert_eq!(s.time, 1.0, "never past the end");
        // Paused, time stays.
        s.tick(1.0);
        assert_eq!(s.time, 1.0);
    }

    #[test]
    fn the_pose_drawn_carries_the_crossfade() {
        let mut p = panel(true);
        p.time = 0.25;
        let a = p.anim_ref().unwrap();
        assert_eq!(a.clip, ClipId::from_name("walk"));
        assert!(a.blend.is_none());
        p.blend = Some(("run".into(), 0.3));
        let b = p.anim_ref().unwrap().blend.unwrap();
        assert_eq!(
            (b.clip, b.time, b.weight),
            (ClipId::from_name("run"), 0.25, 0.3)
        );
        p.clip = None;
        assert!(p.anim_ref().is_none());
    }

    #[test]
    fn the_hook_picks_a_model_clip_and_time() {
        let p = AnimPanel::from_hook(Some("models/robot:walk@0.4"));
        assert_eq!(p.model.as_deref(), Some("models/robot"));
        assert_eq!(p.clip.as_deref(), Some("walk"));
        assert_eq!(p.time, 0.4);
        assert!(!p.playing, "a given time pauses there");
        let mut v = AnimPanel::default();
        v.apply_view("ghosts=3, blend=wave:0.25, look=n64, palette, bogus");
        assert_eq!(v.ghosts, 3);
        assert_eq!(v.blend, Some(("wave".to_string(), 0.25)));
        assert_eq!(v.profile, SimProfile::N64);
        assert!(v.palette.is_some());
        let q = AnimPanel::from_hook(Some("models/robot"));
        assert!(q.clip.is_none() && q.playing);
        assert!(AnimPanel::from_hook(None).model.is_none());
    }
}
