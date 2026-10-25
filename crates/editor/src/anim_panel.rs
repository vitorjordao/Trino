//! The **Animation** tab: preview and check the clips of any model with a
//! skeleton — clip list, play / pause / frame steps, a time scrubber and
//! speed, a crossfade toward a second clip with its weight (`AnimBlend`),
//! measurements per clip with warnings (stride speed and distance per
//! cycle, foot slide, foot under the floor, bounds), a ground grid, onion
//! skin, the color-slot palette and the PC / N64 / 3DS looks; and a
//! "console pose": the current pose baked into a static model.
//!
//! Issue #65 adds a **Graph** mode (`anim_graph_panel`: state machines and
//! blend spaces, `assets/anim/<name>.anim.ron`) and the clip's **events** (a lane under the scrubber, added
//! at the playhead, renamed, moved, removed) and **root motion** (extract
//! it at bake, carry the preview by the track, the foot slide carried by
//! it beside the constant-speed one). Both are manifest lines
//! (`events.<clip>`, `root_motion`): the watcher rebakes the model.
//!
//! The measurements and the pose export are `trino_asset_pipeline::anim`,
//! the same code `cargo xtask model inspect --anim` and `model pose` run.
//! What the tab changes ends in a file: the entity's clip and speed (Use on
//! entity → the scene), or the exported pose model (glTF + manifest).

use std::time::Instant;

use eframe::egui::{self, Pos2, Rect, RichText, Sense, Vec2};
use eframe::egui_wgpu::RenderState;
use trino_asset_pipeline::anim::{self, ClipMetrics, Limits};
use trino_core::{
    AnimRef, ClipId, Color, Material, Mesh, ModelId, ModelParams, Renderer, Transform3,
};
use trino_platform_pc::SimProfile;

use trino_core::anim::RootPose;

use crate::anim_graph_panel::{self as gp, GraphEdit};
use crate::app::{App, Cmd};
use crate::preview::Preview;
use crate::theme::{self, c, icon};

/// Clips (one clip, measured) or Graph (an animation graph, played).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AnimMode {
    #[default]
    Clips,
    Graph,
}

/// `TRINO_EDITOR_ANIM_GRAPH`: (graph, parameters, selection).
pub type GraphHook = (String, Vec<(String, f32)>, Option<String>);

/// The tab's state (what is previewed and how).
pub struct AnimPanel {
    pub mode: AnimMode,
    /// The graph open in Graph mode.
    pub graph: Option<crate::anim_graph_panel::GraphEdit>,
    pub graph_error: Option<String>,
    /// `TRINO_EDITOR_ANIM_GRAPH=<name>[:param=v,..]` and
    /// `TRINO_EDITOR_ANIM_SELECT`, applied once the graph is open.
    pub graph_hook: Option<GraphHook>,
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
    /// Move the preview by the clip's root track (root motion).
    pub follow_root: bool,
    /// Name of the event "Add at playhead" adds.
    pub new_event: String,
    /// Foot IK preview (issue #65): the ground under the model — a slope
    /// (degrees, rising forward) and a step under its left foot (m) — and
    /// whether IK runs where the previewed platform has it off (the N64).
    pub ik_ground: bool,
    pub ik_slope: f32,
    pub ik_step: f32,
    pub ik_force: bool,
    /// The IK pose the preview drew last (tests read it).
    pub ik_drawn: trino_core::anim::IkPose,
    metrics_key: Option<(String, u64)>,
    preview: Option<crate::preview::Preview>,
    last: Instant,
}

impl Default for AnimPanel {
    fn default() -> Self {
        AnimPanel {
            mode: AnimMode::Clips,
            graph: None,
            graph_error: None,
            graph_hook: None,
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
            follow_root: true,
            new_event: "foot_l".into(),
            ik_ground: false,
            ik_slope: 12.0,
            ik_step: 0.0,
            ik_force: false,
            ik_drawn: trino_core::anim::IkPose::NONE,
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
        if let Ok(v) = std::env::var("TRINO_EDITOR_ANIM_GRAPH") {
            p.graph_hook = Some(Self::graph_spec(
                &v,
                std::env::var("TRINO_EDITOR_ANIM_SELECT").ok(),
            ));
            p.mode = AnimMode::Graph;
        }
        p
    }

    /// `<graph>[:param=v,param=v]` → (graph, params, selection).
    pub fn graph_spec(
        v: &str,
        select: Option<String>,
    ) -> (String, Vec<(String, f32)>, Option<String>) {
        let (name, rest) = v.split_once(':').unwrap_or((v, ""));
        let params = rest
            .split(',')
            .filter_map(|kv| kv.split_once('='))
            .filter_map(|(k, v)| Some((k.trim().to_string(), v.trim().parse().ok()?)))
            .collect();
        (name.to_string(), params, select)
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
                Some(("ik", v)) => {
                    // ik=<slope deg>[:<step m>][:force]
                    let mut it = v.split(':');
                    self.ik_ground = true;
                    self.ik_slope = it.next().and_then(|x| x.parse().ok()).unwrap_or(12.0);
                    self.ik_step = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
                    self.ik_force = it.next() == Some("force");
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

    /// The pose drawn: the graph's in Graph mode, else the clip at `time`,
    /// crossfaded when set.
    pub fn anim_ref(&self) -> Option<AnimRef> {
        if self.mode == AnimMode::Graph {
            return self.graph.as_ref().and_then(|g| g.pose());
        }
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
    let clip_events = asset.events.clone();
    let root_motion = asset.root_motion.clone();
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

    // ---- top: mode, model, clips, transport ----------------------------
    ui.horizontal(|ui| {
        for (m, label, glyph) in [
            (AnimMode::Clips, "Clips", icon::FILM_STRIP),
            (AnimMode::Graph, "Graph", icon::TREE_STRUCTURE),
        ] {
            if ui
                .selectable_label(a.mode == m, format!("{glyph} {label}"))
                .clicked()
            {
                a.mode = m;
            }
        }
    });
    if a.mode == AnimMode::Graph {
        let project_root = app.project.root.clone();
        let sounds = app.project.sounds.clone();
        graph_mode(
            app,
            ui,
            &model,
            &clips,
            &tmdl,
            &project_root,
            &sounds,
            generation,
            dt,
            center,
            radius,
        );
        return;
    }
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
        if theme::named_icon_button(ui, icon::SKIP_BACK, "Previous frame").clicked() {
            a.step(-1.0, fps);
        }
        let (glyph, tip) = if a.playing {
            (icon::PAUSE, "Pause")
        } else {
            (icon::PLAY, "Play")
        };
        if theme::named_icon_button(ui, glyph, tip).clicked() {
            a.playing = !a.playing;
        }
        if theme::named_icon_button(ui, icon::SKIP_FORWARD, "Next frame").clicked() {
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

    let events_now = a
        .clip
        .as_ref()
        .and_then(|c| clip_events.get(c))
        .cloned()
        .unwrap_or_default();
    if let Some(t) = event_lane(ui, a.time, duration, &events_now) {
        a.time = t;
        a.playing = false;
    }

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
            clip_meta(app, ui, &model, &clip_events, &root_motion);
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
    // Root motion: the pose stands where its track has carried it.
    // Graph mode: no onion skin (an instance has no past), the root the
    // graph's motion carried. Clips mode: the clip's track at each ghost.
    let graph_mode = a.mode == AnimMode::Graph;
    let ghosts = if graph_mode { 0 } else { ghosts };
    let carry: Option<(RootPose, Vec<RootPose>)> = if graph_mode {
        a.graph.as_ref().map(|g| (g.carried, vec![g.carried]))
    } else if a.follow_root {
        let clip = a.clip.as_deref().map(ClipId::from_name);
        app.project.models.get(model).and_then(|m| {
            let meta = trino_core::anim::ClipMeta::from_tmdl(&m.tmdl)?;
            let g = meta.roots.root_of(clip?)?;
            let past = (0..=ghosts as i32)
                .map(|k| g.at((time - k as f32 * step * 2.0).max(0.0)))
                .collect();
            Some((g.at(time), past))
        })
    } else {
        None
    };
    let at = |k: usize| -> Transform3 {
        match &carry {
            Some((now, past)) => {
                let g = if k == 0 { *now } else { past[k] };
                Transform3 {
                    position: trino_core::Vec3::new(g.x, 0.0, g.z),
                    rotation: trino_core::Vec3::new(0.0, g.yaw, 0.0),
                    scale: trino_core::Vec3::ONE,
                }
            }
            None => Transform3::default(),
        }
    };
    let palette = a.palette.map(|p| p.map(|c| Color::rgb(c[0], c[1], c[2])));
    let p = Preview::ensure(&mut a.preview, &render_state, profile);
    let generation = app.project.generation;
    if p.uploaded.as_ref() != Some(&(model.to_string(), generation)) {
        app.project.upload_all(&mut p.renderer);
        let c = trino_core::Vec3::new(center[0], center[1], center[2]);
        p.cam.focus(c, radius.max(0.3));
        p.uploaded = Some((model.to_string(), generation));
    }
    let id = ModelId(trino_core::asset_id(model));
    // Foot IK onto the preview's ground, by the previewed platform's tier.
    let ik = (a.ik_ground, a.ik_slope, a.ik_step, a.ik_force);
    let tmdl = app.project.models.get(model).map(|m| m.tmdl.clone());
    let ground = move |q: trino_core::Vec3| {
        // The model's left is +X: the step is under its left foot.
        q.z * ik.1.to_radians().tan() + if q.x > 0.0 { ik.2 } else { 0.0 }
    };
    let mut drawn = trino_core::anim::IkPose::NONE;
    p.show(
        ui,
        rect,
        grid,
        |r: &mut trino_platform_pc::PcRenderer, _| {
            let anim = match (
                anim,
                ik.0,
                tmdl.as_deref().and_then(|t| Mesh::from_tmdl(t).ok()),
            ) {
                (Some(a0), true, Some(mesh)) if mesh.ik_chains().is_some() => {
                    let chains = mesh.ik_chains().unwrap();
                    let mut w = [trino_core::Mat34::IDENTITY; trino_core::MAX_JOINTS];
                    mesh.pose_world(Some(a0), &mut w);
                    let foot = |l: usize| {
                        let m = w[chains.chain[l.min(chains.legs as usize - 1)][2] as usize];
                        trino_core::Vec3::new(m.m[0][3], m.m[1][3], m.m[2][3])
                    };
                    let t = at(0);
                    let mm = trino_core::Mat34::from_rotation_scale_translation(
                        t.rotation, t.scale, t.position,
                    );
                    let caps = *r.caps();
                    let pose = trino_core::anim::FootIk {
                        force: ik.3,
                        ..Default::default()
                    }
                    .pose(&caps, &mm, [foot(0), foot(1)], |q| Some(ground(q)));
                    drawn = pose;
                    // The ground it stands on: a slab along the slope, the
                    // step's under the left foot.
                    let slab = |r: &mut trino_platform_pc::PcRenderer,
                                x: f32,
                                w: f32,
                                h: f32,
                                c: Color| {
                        r.draw_model(
                            ModelId(trino_core::asset_id("models/wall")),
                            &Transform3 {
                                position: trino_core::Vec3::new(
                                    x + t.position.x,
                                    h - 0.01,
                                    t.position.z,
                                ),
                                rotation: trino_core::Vec3::new(-ik.1.to_radians(), 0.0, 0.0),
                                scale: trino_core::Vec3::new(w, 0.02, 3.0),
                            },
                            Material::VertexLit,
                            &ModelParams::tinted(c),
                        );
                    };
                    slab(r, -0.75, 1.5, 0.0, Color::rgb(110, 150, 100));
                    slab(r, 0.75, 1.5, ik.2, Color::rgb(150, 130, 100));
                    Some(a0.with_ik(pose))
                }
                (a0, ..) => a0,
            };
            // Onion skin: earlier frames, fainter the older.
            for k in (1..=ghosts as i32).rev() {
                let t = (time - k as f32 * step * 2.0).max(0.0);
                let alpha = (110.0 * (1.0 - k as f32 / (ghosts as f32 + 1.0))) as u8;
                r.draw_model(
                    id,
                    &at(k as usize),
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
                &at(0),
                Material::VertexLit,
                &ModelParams {
                    anim,
                    palette,
                    ..ModelParams::DEFAULT
                },
            );
        },
    );
    app.anim.ik_drawn = drawn;
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(250));
}

/// The Foot IK preview's ground and tier (both modes).
fn ik_ui(ui: &mut egui::Ui, a: &mut AnimPanel, has_chains: bool) {
    theme::component(ui, "anim-ik", icon::FOOTPRINTS, c::MODEL, "Foot IK", |ui| {
        if !has_chains {
            ui.label(
                RichText::new("No IK chains: the manifest's ik = { left = [hip, knee, foot], .. }")
                    .size(11.0)
                    .color(c::TEXT_FAINT),
            );
            return;
        }
        ui.checkbox(&mut a.ik_ground, "Stand on uneven ground");
        theme::prop(ui, "Slope", |ui| {
            ui.add(egui::Slider::new(&mut a.ik_slope, -30.0..=30.0).suffix("°"));
        });
        theme::prop(ui, "Step", |ui| {
            ui.add(egui::Slider::new(&mut a.ik_step, -0.3..=0.3).suffix(" m"))
                .on_hover_text("Under the left foot");
        });
        ui.checkbox(&mut a.ik_force, "Force on the N64")
            .on_hover_text("Caps::anim_ik is off on the N64 (FootIk::force turns it on, at N64_COST.us_per_ik_draw)");
        let p = a.ik_drawn;
        let tier = if a.profile == SimProfile::N64 && !a.ik_force {
            "N64 look: IK off (the console's tier)".to_string()
        } else {
            format!(
                "feet {:+.3} / {:+.3} m",
                p.left as f32 / 1000.0,
                p.right as f32 / 1000.0
            )
        };
        ui.label(RichText::new(tier).monospace().color(c::TEXT_WEAK));
    });
}

fn controls(
    app: &mut App,
    ui: &mut egui::Ui,
    model: &str,
    clips: &[String],
    tris: usize,
    verts: usize,
) {
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
            let rows: Vec<(&str, String)> = rows
                .into_iter()
                .chain(m.root_foot_slide.map(|s| {
                    (
                        "Root motion",
                        format!("{:.2} m/cycle · slide {:.1} cm", m.root_distance, s * 100.0),
                    )
                }))
                .collect();
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
            let advice = if m.root_motion {
                "Root motion: move the character by the clip's track (AnimGraphInstance::root_motion) \
                 and its planted feet stay put. Same numbers: cargo xtask model inspect <model> --anim"
                    .to_string()
            } else {
                format!(
                    "Move the character at {:.2} m/s so its planted feet stay put. \
                 Same numbers: cargo xtask model inspect <file> --anim",
                    m.speed
                )
            };
            ui.label(RichText::new(advice).size(11.0).color(c::TEXT_FAINT));
        },
    );

    let chains = app
        .project
        .models
        .get(model)
        .and_then(|m| Mesh::from_tmdl(&m.tmdl).ok())
        .is_some_and(|m| m.ik_chains().is_some());
    let a = &mut app.anim;
    ik_ui(ui, a, chains);
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
}

/// Graph mode: the graph picker, the canvas over the preview, the
/// parameters and the selection beside them.
#[allow(clippy::too_many_arguments)]
fn graph_mode(
    app: &mut App,
    ui: &mut egui::Ui,
    model: &str,
    clips: &[String],
    tmdl: &[u8],
    root: &std::path::Path,
    sounds: &[String],
    generation: u64,
    dt: f32,
    center: [f32; 3],
    radius: f32,
) {
    let graphs = gp::graphs_for(root, model);
    let a = &mut app.anim;
    let open = |a: &mut AnimPanel, name: &str| match GraphEdit::open(root, name) {
        Ok(g) => {
            a.graph = Some(g);
            a.graph_error = None;
        }
        Err(e) => a.graph_error = Some(e),
    };
    if let Some((name, ..)) = a.graph_hook.clone()
        && a.graph.as_ref().is_none_or(|g| g.name != name)
    {
        open(a, &name);
    }
    if a.graph
        .as_ref()
        .is_some_and(|g| g.def.model_path() != model)
    {
        a.graph = None;
    }
    if a.graph.is_none()
        && a.graph_error.is_none()
        && let Some(first) = graphs.first()
    {
        open(a, first);
    }
    ui.horizontal(|ui| {
        let current = a
            .graph
            .as_ref()
            .map_or("none".to_string(), |g| format!("anim/{}", g.name));
        let w = 180.0f32.min(ui.available_width() - 8.0);
        let mut pick = None;
        theme::fixed_width(ui, w, |ui| {
            egui::ComboBox::from_id_salt("anim-graph")
                .selected_text(&current)
                .width(w)
                .truncate()
                .show_ui(ui, |ui| {
                    for n in &graphs {
                        if ui
                            .selectable_label(current == format!("anim/{n}"), format!("anim/{n}"))
                            .clicked()
                        {
                            pick = Some(n.clone());
                        }
                    }
                })
                .response
                .widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Animation graph")
                });
        });
        if let Some(n) = pick {
            open(a, &n);
        }
        if ui
            .button(format!("{} New graph", icon::PLUS))
            .on_hover_text(format!(
                "Same as: cargo xtask anim new <name> --model {model}"
            ))
            .clicked()
        {
            let base = model.trim_start_matches("models/").replace('_', "-");
            let mut name = base.clone();
            let mut k = 2;
            while trino_scene::anim::graph_file(root, &name).exists() {
                name = format!("{base}-{k}");
                k += 1;
            }
            let def = trino_scene::anim::AnimGraphDef::starter(model, clips);
            match def.save(&trino_scene::anim::graph_file(root, &name)) {
                Ok(()) => open(a, &name),
                Err(e) => a.graph_error = Some(e),
            }
        }
        let (glyph, tip) = if a.playing {
            (icon::PAUSE, "Pause")
        } else {
            (icon::PLAY, "Play")
        };
        if theme::named_icon_button(ui, glyph, tip).clicked() {
            a.playing = !a.playing;
        }
        if let Some(g) = &a.graph
            && let Some(inst) = &g.inst
        {
            let state = g.def.states.get(inst.state()).map_or("?", |s| s.0.as_str());
            let fade = inst
                .transition()
                .map(|(f, k)| {
                    format!(
                        " ← {} {:.0}%",
                        g.def.states.get(f).map_or("?", |s| s.0.as_str()),
                        k * 100.0
                    )
                })
                .unwrap_or_default();
            ui.label(
                RichText::new(format!("{state}{fade} · {:.2}", inst.clock()))
                    .monospace()
                    .color(c::TEXT_WEAK),
            );
        }
    });
    if let Some(e) = &a.graph_error {
        ui.label(RichText::new(format!("{} {e}", icon::WARNING)).color(c::WARN));
    }
    let Some(g) = a.graph.as_mut() else {
        theme::empty_state(
            ui,
            icon::TREE_STRUCTURE,
            "No graph for this model",
            "New graph writes assets/anim/<name>.anim.ron: one state per clip",
        );
        return;
    };
    let facts = trino_scene::anim::clip_facts(tmdl).ok();
    g.poll_file();
    g.rebake(facts.as_deref(), sounds, generation);
    if let Some((name, params, select)) = a.graph_hook.clone()
        && g.name == name
        && g.inst.is_some()
    {
        for (k, v) in &params {
            g.set_param(k, *v);
        }
        if let Some(s) = &select {
            gp::select_spec(g, s);
        }
        a.graph_hook = None;
    }
    if a.playing {
        g.tick(dt * a.speed);
        ui.ctx().request_repaint();
    }

    let avail = ui.available_rect_before_wrap();
    let wide = avail.width() >= 760.0;
    let side = 330.0f32.min(avail.width());
    let (view_rect, controls_rect) = if wide {
        (
            Rect::from_min_max(avail.min, Pos2::new(avail.max.x - side - 8.0, avail.max.y)),
            Rect::from_min_max(Pos2::new(avail.max.x - side, avail.min.y), avail.max),
        )
    } else {
        let h = (avail.height() * 0.5).max(160.0);
        (
            Rect::from_min_size(avail.min, Vec2::new(avail.width(), h)),
            Rect::from_min_max(Pos2::new(avail.min.x, avail.min.y + h + 8.0), avail.max),
        )
    };
    let split = view_rect.min.y + view_rect.height() * 0.5;
    let canvas_rect = Rect::from_min_max(view_rect.min, Pos2::new(view_rect.max.x, split - 4.0));
    let preview_rect = Rect::from_min_max(Pos2::new(view_rect.min.x, split + 4.0), view_rect.max);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(canvas_rect));
    if gp::canvas(&mut child, canvas_rect, g)
        && let Err(e) = gp::layout_moved(g)
    {
        app.log.toast(crate::log::Level::Error, e);
    }
    let render_state = app.render_state.clone();
    preview(app, ui, preview_rect, render_state, model, center, radius);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(controls_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let mut changed = false;
    egui::ScrollArea::vertical()
        .id_salt("graph-controls")
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            ui.set_max_width(ui.available_width() - 10.0);
            if let Some(g) = app.anim.graph.as_mut() {
                changed = gp::controls(ui, g, clips);
            }
            let chains = app
                .project
                .models
                .get(model)
                .and_then(|m| Mesh::from_tmdl(&m.tmdl).ok())
                .is_some_and(|m| m.ik_chains().is_some());
            let a = &mut app.anim;
            ik_ui(ui, a, chains);
            theme::component(ui, "graph-play", icon::TIMER, c::TEXT, "Playback", |ui| {
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
                theme::prop(ui, "Speed", |ui| {
                    ui.add(egui::Slider::new(&mut a.speed, 0.0..=3.0).suffix("×"));
                });
                if ui.button(format!("{} Restart", icon::ARROW_COUNTER_CLOCKWISE)).clicked()
                    && let Some(g) = a.graph.as_mut()
                {
                    g.inst = None;
                    g.carried = RootPose::IDENTITY;
                    g.rebake(None, &[], u64::MAX);
                }
                ui.label(
                    RichText::new("Play a script without the editor: cargo xtask anim simulate <graph> --script s.ron")
                        .size(11.0)
                        .color(c::TEXT_FAINT),
                );
            });
        });
    if changed
        && let Some(g) = app.anim.graph.as_mut()
        && let Err(e) = g.save()
    {
        app.log.toast(crate::log::Level::Error, e);
    }
    ui.allocate_rect(avail, Sense::hover());
}

/// The current clip's events on a lane as wide as the scrubber: a tick per
/// event, the playhead, a click on an event jumps there. Returns the time
/// clicked.
fn event_lane(
    ui: &mut egui::Ui,
    time: f32,
    duration: f32,
    events: &[(f32, String)],
) -> Option<f32> {
    let w = ui.available_width().max(40.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 16.0), Sense::click());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, c::BG0);
    let x_of = |t: f32| {
        rect.min.x + 6.0 + (rect.width() - 12.0) * (t / duration.max(1e-3)).clamp(0.0, 1.0)
    };
    for (t, name) in events {
        let x = x_of(*t);
        let col = if name.starts_with("sfx:") {
            c::PLAY
        } else {
            c::ACCENT
        };
        p.line_segment(
            [
                Pos2::new(x, rect.min.y + 2.0),
                Pos2::new(x, rect.max.y - 2.0),
            ],
            (3.0, col),
        );
    }
    let x = x_of(time);
    p.line_segment(
        [Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)],
        (1.0, c::TEXT),
    );
    let mut picked = None;
    if let Some(pos) = resp.hover_pos()
        && let Some((t, name)) = events.iter().find(|(t, _)| (x_of(*t) - pos.x).abs() < 5.0)
    {
        resp.clone().on_hover_text(format!("{name} at {t:.2} s"));
        if resp.clicked() {
            picked = Some(*t);
        }
    }
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Other,
            true,
            format!("{} events", events.len()),
        )
    });
    picked
}

/// The clip's metadata: its events (added at the playhead, moved,
/// renamed, removed) and root motion (extracted at bake; the preview
/// carried by the track). Each change writes the manifest.
fn clip_meta(
    app: &mut App,
    ui: &mut egui::Ui,
    model: &str,
    events: &std::collections::BTreeMap<String, Vec<(f32, String)>>,
    root_motion: &[String],
) {
    let a = &mut app.anim;
    let Some(clip) = a.clip.clone() else {
        return;
    };
    let duration = a.current_metrics().map_or(0.0, |m| m.duration);
    let mut list = events.get(&clip).cloned().unwrap_or_default();
    let mut changed = false;
    theme::component(ui, "anim-events", icon::FLAG, c::ACCENT, "Events", |ui| {
        let mut remove = None;
        for (i, (t, name)) in list.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                changed |= ui
                    .add(
                        egui::DragValue::new(t)
                            .range(0.0..=duration)
                            .speed(0.01)
                            .suffix(" s"),
                    )
                    .on_hover_text("Seconds into the clip")
                    .changed();
                let w = (ui.available_width() - 30.0).max(40.0);
                changed |= ui
                    .add(egui::TextEdit::singleline(name).desired_width(w))
                    .on_hover_text("Event name; sfx:<sound> plays sounds/<sound>")
                    .lost_focus();
                if theme::named_icon_button(ui, icon::TRASH, "Remove event").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            list.remove(i);
            changed = true;
        }
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 120.0).max(40.0);
            ui.add(egui::TextEdit::singleline(&mut a.new_event).desired_width(w));
            if ui
                .button(format!("{} Add at {:.2} s", icon::PLUS, a.time))
                .on_hover_text("A timeline event at the playhead (the manifest's events.<clip>)")
                .clicked()
                && !a.new_event.trim().is_empty()
            {
                list.push((a.time, a.new_event.trim().to_string()));
                changed = true;
            }
        });
        ui.label(
            RichText::new(
                "Events fire through the animation graph (AnimGraphInstance::events). \
                 sfx:<name> carries sounds/<name>.",
            )
            .size(11.0)
            .color(c::TEXT_FAINT),
        );
    });
    if changed {
        list.sort_by(|x, y| x.0.total_cmp(&y.0));
        app.cmds.push(Cmd::SetClipEvents {
            model: model.to_string(),
            clip: clip.clone(),
            events: list,
        });
    }
    let a = &mut app.anim;
    let mut extracted = root_motion.contains(&clip);
    let mut toggled = false;
    theme::component(
        ui,
        "anim-root",
        icon::FOOTPRINTS,
        c::OK,
        "Root motion",
        |ui| {
            toggled = ui
                .checkbox(&mut extracted, "Extract at bake")
                .on_hover_text(
                    "The root's ground motion leaves the clip for a track (manifest root_motion)",
                )
                .changed();
            ui.add_enabled(
                extracted,
                egui::Checkbox::new(&mut a.follow_root, "Carry the preview by the track"),
            );
        },
    );
    if toggled {
        let mut clips: Vec<String> = root_motion
            .iter()
            .filter(|c| **c != clip)
            .cloned()
            .collect();
        if extracted {
            clips.push(clip);
        }
        app.cmds.push(Cmd::SetRootMotion {
            model: model.to_string(),
            clips,
        });
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
            root_motion: false,
            root_distance: 0.0,
            root_foot_slide: None,
            events: Vec::new(),
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
