//! Scene and Game views: the scene rendered through the **real PC backend**
//! (`PcRenderer`) into an offscreen texture on eframe's own wgpu device,
//! plus egui overlays (grid labels, selection boxes, markers, gizmo).
//!
//! - **Scene view**: free orbit camera, rendered at the panel's pixel size
//!   (crisp), with a fading ground grid and editing helpers.
//! - **Game view**: the scene's own camera at the console-simulation
//!   resolution and look (N64 dither, 3DS bilinear), sprites included.
//!
//! Views re-render only when something they show changed (content
//! revision, camera, size, assets, animation) — an idle editor costs nothing.
//! The texture is re-registered only when the framebuffer size changes
//! (re-registering per frame is the classic egui-wgpu leak).

use std::time::Instant;

use eframe::egui::{
    self, Color32, CornerRadius, Margin, Pos2, Rect, RichText, Sense, Stroke, Vec2,
};
use eframe::egui_wgpu::RenderState;
use trino_asset_pipeline::MeshData;
use trino_core::{
    Color, Fog, Light, Material, ModelId, ModelParams, Renderer, Shading, SpriteId, SpriteParams,
    Vec2 as TVec2, Vec3,
};
use trino_platform_pc::{PcRenderer, SimProfile};
use trino_scene::{Resolved, Scene, Transform};

use crate::gizmo::{self, Drag, Gizmo, GizmoMode, OrbitCam, Proj};
use crate::project::Project;
use crate::theme::{self, c, icon};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewKind {
    Scene,
    Game,
}

/// What can be dragged from the Assets panel onto a view.
#[derive(Clone, Debug)]
pub enum AssetPayload {
    Model(String),
    Prefab(String),
    Sprite(String),
}

impl AssetPayload {
    pub fn label(&self) -> &str {
        match self {
            AssetPayload::Model(s) | AssetPayload::Prefab(s) | AssetPayload::Sprite(s) => s,
        }
    }
}

pub struct ViewInput<'a> {
    pub scene: &'a Scene,
    pub resolved: &'a Resolved,
    pub project: &'a Project,
    pub selected: Option<&'a [usize]>,
    /// Additional selected entities (multi-select).
    pub extra: &'a [Vec<usize>],
    /// Bumped by the app on any content change.
    pub rev: u64,
    pub time: f32,
    pub animate: bool,
}

pub enum ViewAction {
    Select(Option<Vec<usize>>),
    /// Ctrl/Shift+click: add or remove from the selection.
    ToggleSelect(Vec<usize>),
    /// Shift+drag box select.
    SelectMany(Vec<Vec<usize>>),
    /// Live gizmo edit (the app's undo coalesces until release).
    SetTransform(Vec<usize>, Transform),
    Drop(AssetPayload, Vec3),
    ToggleAnimate,
}

pub struct Stats {
    pub items: usize,
    pub tris: usize,
    pub ms: f32,
    /// What the renderer counted for the last frame (budget overlay).
    pub frame: trino_core::FrameStats,
}

pub struct Viewport {
    pub kind: ViewKind,
    render_state: RenderState,
    renderer: PcRenderer,
    texture: egui::TextureId,
    pub profile: SimProfile,
    pub cam: OrbitCam,
    pub show_grid: bool,
    /// Draw collider wireframes (world boxes — exactly what the physics
    /// uses): green = solid, yellow = trigger.
    pub show_colliders: bool,
    /// Game view: the console budget overlay (tris / sprites / texture vs
    /// the selected profile's Caps).
    pub show_budget: bool,
    /// Game view: a scene running in the editor (Play ▸ Play here).
    pub play: Option<crate::play::PlaySession>,
    pub gizmo: GizmoMode,
    pub snap: bool,
    drag: Option<(Vec<usize>, Drag)>,
    uploaded_gen: u64,
    rendered: Option<RenderKey>,
    last_tick: Instant,
    flash: Option<(Vec<usize>, Instant)>,
    last_selected: Option<Vec<usize>>,
    grid: ModelId,
    pub stats: Stats,
    hover: Option<Vec<usize>>,
    /// Box select in progress (Shift+drag on the Scene view): start point.
    box_start: Option<Pos2>,
}

const GRID_EXTENT: i32 = 40;

/// What a rendered frame depended on: (content rev, size, asset generation,
/// camera, animation time). Same key = skip the render.
type RenderKey = (u64, (u32, u32), u64, [f32; 6], f32);

impl Viewport {
    pub fn new(render_state: RenderState, kind: ViewKind, profile: SimProfile) -> Self {
        let device = wgpu::Device::clone(&render_state.device);
        let queue = wgpu::Queue::clone(&render_state.queue);
        let mut renderer = PcRenderer::with_device(device, queue, profile);
        if kind == ViewKind::Scene {
            renderer.set_n64_look(false);
        }
        let grid = ModelId(trino_core::asset_id("__editor/grid"));
        renderer.upload_mesh(grid, grid_tmdl());
        let texture = render_state.renderer.write().register_native_texture(
            &render_state.device,
            renderer.offscreen_view(),
            wgpu::FilterMode::Nearest,
        );
        Viewport {
            kind,
            render_state,
            renderer,
            texture,
            profile,
            cam: OrbitCam::default(),
            show_grid: true,
            show_colliders: true,
            show_budget: true,
            play: None,
            gizmo: GizmoMode::Translate,
            snap: false,
            drag: None,
            uploaded_gen: u64::MAX,
            rendered: None,
            last_tick: Instant::now(),
            flash: None,
            last_selected: None,
            grid,
            stats: Stats {
                items: 0,
                tris: 0,
                ms: 0.0,
                frame: trino_core::FrameStats::default(),
            },
            hover: None,
            box_start: None,
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

    /// Switch the Game view's console profile (new renderer, re-upload).
    pub fn set_profile(&mut self, profile: SimProfile, project: &Project) {
        if profile == self.profile {
            return;
        }
        let device = wgpu::Device::clone(&self.render_state.device);
        let queue = wgpu::Queue::clone(&self.render_state.queue);
        self.renderer = PcRenderer::with_device(device, queue, profile);
        self.renderer.upload_mesh(self.grid, grid_tmdl());
        project.upload_all(&mut self.renderer);
        self.uploaded_gen = project.generation;
        self.profile = profile;
        self.rendered = None;
        self.reregister();
    }

    /// Re-upload changed assets (live reload).
    pub fn upload(&mut self, project: &Project, changed: &[String]) {
        project.upload_some(&mut self.renderer, changed);
        self.rendered = None;
    }

    /// Internal resolution of this view's renderer.
    pub fn internal_size(&self) -> (u32, u32) {
        self.renderer.internal_size()
    }

    pub fn profile_key(&self) -> &'static str {
        match self.profile {
            SimProfile::Native => "native",
            SimProfile::N64 => "n64",
            SimProfile::N3ds => "3ds",
        }
    }

    pub fn focus_on(&mut self, input: &ViewInput, path: &[usize]) {
        if let Some((c, r)) = selection_sphere(input, path) {
            self.cam.focus(c, r);
        }
    }

    // ------------------------------------------------------------ render --

    fn render(&mut self, input: &ViewInput, size: (u32, u32)) {
        let started = Instant::now();
        if self.kind == ViewKind::Scene {
            self.renderer.set_internal_size(size.0, size.1);
        }
        let env = &input.scene.environment;
        let clear = env.clear;
        let r = &mut self.renderer;
        r.begin_frame(Color::rgb(clear[0], clear[1], clear[2]));
        let camera = match self.kind {
            ViewKind::Scene => self.cam.camera(),
            ViewKind::Game => env.core_camera(),
        };
        r.set_camera(&camera);
        let light = env.core_light();

        if self.kind == ViewKind::Scene && self.show_grid {
            // The grid fades into the background with distance.
            let d = self.cam.dist;
            r.set_light(&Light {
                fog: Fog {
                    color: Color::rgb(clear[0], clear[1], clear[2]),
                    near: d * 0.6,
                    far: d * 2.4 + 12.0,
                },
                ..light
            });
            r.draw_model_matrix(
                self.grid,
                &trino_core::Mat34::IDENTITY,
                Material::VertexLit,
                &ModelParams {
                    shading: Shading::Unlit,
                    double_sided: true,
                    ..ModelParams::DEFAULT
                },
            );
        }
        r.set_light(&light);

        let flash = self
            .flash
            .as_ref()
            .map(|(p, t)| (p.clone(), t.elapsed().as_secs_f32()));
        let mut tris = 0;
        // The Game view culls interior cells like the game does; the Scene
        // view shows everything (it is for editing).
        let visible = match self.kind {
            ViewKind::Game => {
                let (w, h) = self.renderer.internal_size();
                input
                    .resolved
                    .visible_cells(&camera, trino_core::Vec2::new(w as f32, h as f32))
            }
            ViewKind::Scene => u64::MAX,
        };
        let r = &mut self.renderer;
        for item in &input.resolved.items {
            if !trino_core::vis::in_view(item.cell, visible) {
                continue;
            }
            // LOD as the game sees it, from this view's camera.
            let Some(lod_id) = item.model_from(camera.eye) else {
                continue;
            };
            let id = &lod_id;
            let mut params = item.params;
            if !input.animate {
                params.anim = None;
            }
            if let Some((path, age)) = &flash
                && item.owner.starts_with(path)
                && *age < 0.6
            {
                // Selection flash: a soft accent glow that eases out.
                let k = (1.0 - age / 0.6).powi(2);
                let add = |v: u8, a: f32| (v as f32 + a * k).min(255.0) as u8;
                params.emissive = Color::rgb(
                    add(params.emissive.r, 70.0),
                    add(params.emissive.g, 50.0),
                    add(params.emissive.b, 120.0),
                );
            }
            if let Some(m) = input.project.model_by_id(*id) {
                tris += m.tris;
            }
            r.draw_model_matrix(*id, &item.matrix, Material::VertexLit, &params);
        }
        if self.kind == ViewKind::Game {
            for item in &input.resolved.items {
                if let Some(s) = &item.sprite {
                    let t = s.tint;
                    r.draw_sprite(
                        SpriteId(s.id.0),
                        TVec2::new(s.pos[0], s.pos[1]),
                        &SpriteParams {
                            scale: TVec2::new(s.scale[0], s.scale[1]),
                            rotation: s.rotation,
                            tint: Color::rgba(t[0], t[1], t[2], t[3]),
                            flip_x: s.flip_x,
                            flip_y: s.flip_y,
                            ..SpriteParams::DEFAULT
                        },
                    );
                }
            }
        }
        r.end_frame();
        self.stats = Stats {
            items: input.resolved.items.len(),
            tris,
            ms: started.elapsed().as_secs_f32() * 1000.0,
            frame: r.stats(),
        };
    }

    // -------------------------------------------------------------- show --

    pub fn show(&mut self, ui: &mut egui::Ui, input: &ViewInput) -> Vec<ViewAction> {
        let mut actions = Vec::new();
        let dt = self.last_tick.elapsed().as_secs_f32().min(0.1);
        self.last_tick = Instant::now();
        let cam_moving = self.cam.step(dt);

        if input.project.generation != self.uploaded_gen {
            input.project.upload_all(&mut self.renderer);
            self.uploaded_gen = input.project.generation;
            self.rendered = None;
        }
        if self.last_selected.as_deref() != input.selected {
            self.last_selected = input.selected.map(|s| s.to_vec());
            self.flash = input.selected.map(|s| (s.to_vec(), Instant::now()));
        }
        let flashing = self
            .flash
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed().as_secs_f32() < 0.6);

        // Layout: Scene fills the panel; Game letterboxes the console image.
        let avail = ui.available_rect_before_wrap();
        let ppp = ui.ctx().pixels_per_point();
        let (internal, image_rect) = match self.kind {
            ViewKind::Scene => {
                let px = (
                    ((avail.width() * ppp) as u32).max(16),
                    ((avail.height() * ppp) as u32).max(16),
                );
                (px, avail)
            }
            ViewKind::Game => {
                let (iw, ih) = self.renderer.internal_size();
                let fit = (avail.width() / iw as f32).min(avail.height() / ih as f32);
                let s = if fit * ppp >= 1.0 {
                    (fit * ppp).floor() / ppp
                } else {
                    fit.max(0.05)
                };
                let size = Vec2::new(iw as f32 * s, ih as f32 * s);
                ((iw, ih), Rect::from_center_size(avail.center(), size))
            }
        };

        // Re-render only on change.
        let cam_key = {
            let e = self.cam.eye();
            [
                e.x,
                e.y,
                e.z,
                self.cam.target.x,
                self.cam.target.y,
                self.cam.target.z,
            ]
        };
        let anim_time = if input.animate { input.time } else { 0.0 };
        let key = (
            input.rev,
            internal,
            input.project.generation,
            cam_key,
            anim_time,
        );
        let size_changed = self.rendered.as_ref().map(|r| r.1) != Some(internal);
        if let Some(session) = self.play.as_mut() {
            // Playing: the game updates and draws every frame, fed by the
            // keyboard (unless a text field has focus).
            let dt = session.tick();
            let typing = ui.ctx().egui_wants_keyboard_input();
            let input_state =
                ui.input(|i| crate::play::input_from_keys(|k| !typing && i.key_down(k)));
            use trino_core::Game as _;
            session
                .game
                .update(&input_state, &mut crate::play::NullAudio, dt);
            session.game.render(&mut self.renderer);
            self.stats = Stats {
                items: input.resolved.items.len(),
                tris: self.renderer.stats().tris as usize,
                ms: dt * 1000.0,
                frame: self.renderer.stats(),
            };
            self.rendered = None;
            ui.ctx().request_repaint();
        } else if self.rendered.as_ref() != Some(&key) || flashing || cam_moving {
            self.render(input, internal);
            if size_changed {
                self.reregister();
            }
            self.rendered = Some(key);
        }
        if flashing || cam_moving {
            ui.ctx().request_repaint();
        }

        // Background behind letterboxing.
        ui.painter()
            .rect_filled(avail, CornerRadius::same(6), c::BG0);
        let response = ui.allocate_rect(avail, Sense::click_and_drag());
        ui.painter().image(
            self.texture,
            image_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );

        let camera = match self.kind {
            ViewKind::Scene => self.cam.camera(),
            ViewKind::Game => input.scene.environment.core_camera(),
        };
        let proj = Proj::new(&camera, internal, image_rect);
        let painter = ui.painter_at(image_rect.intersect(avail));
        let mouse = response.hover_pos();

        // ---- selection + gizmo ------------------------------------------
        let sel_item = input.selected.and_then(|sel| {
            input
                .resolved
                .items
                .iter()
                .find(|i| i.owner == sel && !i.from_prefab)
        });
        let gizmo = sel_item.map(|item| Gizmo::new(&proj, gizmo::translation(&item.matrix)));
        let hot_handle = match (&gizmo, mouse, &self.drag) {
            (_, _, Some((_, d))) => Some(d.handle),
            (Some(g), Some(m), None) if self.kind == ViewKind::Scene => g.hit(&proj, self.gizmo, m),
            _ => None,
        };

        // Drag start / update / end.
        if self.kind == ViewKind::Scene {
            if response.drag_started_by(egui::PointerButton::Primary)
                && !ui.input(|i| i.modifiers.alt)
                && let (Some(h), Some(g), Some(item), Some(m)) = (
                    hot_handle,
                    &gizmo,
                    sel_item,
                    response.interact_pointer_pos(),
                )
                && let Some(local) = input.scene.get(&item.owner).map(|e| e.transform)
                && let Some(d) = Drag::begin(self.gizmo, h, g, &proj, m, &item.matrix, local)
            {
                self.drag = Some((item.owner.clone(), d));
            }
            if let Some((path, d)) = &self.drag {
                if let Some(m) = ui.input(|i| i.pointer.interact_pos()) {
                    let snap = self.snap ^ ui.input(|i| i.modifiers.command);
                    let t = d.update(&proj, m, snap);
                    actions.push(ViewAction::SetTransform(path.clone(), t));
                }
                if !ui.input(|i| i.pointer.primary_down()) {
                    self.drag = None;
                }
            }
        }

        // ---- camera controls --------------------------------------------
        if self.kind == ViewKind::Scene && self.drag.is_none() {
            let shift = ui.input(|i| i.modifiers.shift);
            let alt = ui.input(|i| i.modifiers.alt);
            let delta = response.drag_delta();
            let pan = (response.dragged_by(egui::PointerButton::Middle) && shift)
                || (response.dragged_by(egui::PointerButton::Secondary) && shift);
            // Shift + left drag on empty space: box select.
            if response.drag_started_by(egui::PointerButton::Primary)
                && shift
                && hot_handle.is_none()
            {
                self.box_start = response.interact_pointer_pos();
            }
            if let Some(start) = self.box_start {
                let now = ui.input(|i| i.pointer.interact_pos()).unwrap_or(start);
                let rect = Rect::from_two_pos(start, now);
                painter.rect_filled(rect, CornerRadius::ZERO, c::ACCENT.gamma_multiply(0.12));
                painter.rect_stroke(
                    rect,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, c::ACCENT),
                    egui::StrokeKind::Inside,
                );
                if !ui.input(|i| i.pointer.primary_down()) {
                    self.box_start = None;
                    let mut hits: Vec<Vec<usize>> = Vec::new();
                    for item in &input.resolved.items {
                        let origin = gizmo::translation(&item.matrix);
                        if let Some(p) = proj.project(origin)
                            && rect.contains(p)
                            && !hits.contains(&item.owner)
                        {
                            hits.push(item.owner.clone());
                        }
                    }
                    actions.push(ViewAction::SelectMany(hits));
                }
                ui.ctx().request_repaint();
            }
            let orbit = (response.dragged_by(egui::PointerButton::Middle) && !shift)
                || (response.dragged_by(egui::PointerButton::Secondary) && !shift)
                || (response.dragged_by(egui::PointerButton::Primary) && alt);
            if pan {
                self.cam.pan(delta, image_rect.height());
            } else if orbit {
                self.cam.orbit(delta);
            }
            if response.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    self.cam.zoom(scroll);
                    ui.ctx().request_repaint();
                }
            }
            if pan || orbit {
                ui.ctx().request_repaint();
            }
        }

        // ---- picking ------------------------------------------------------
        let pick = |pos: Pos2| -> Option<Vec<usize>> { pick(&proj, input, pos) };
        self.hover = if self.drag.is_none() && hot_handle.is_none() {
            mouse.and_then(pick)
        } else {
            None
        };
        if response.clicked_by(egui::PointerButton::Primary) && hot_handle.is_none() {
            let hit = response.interact_pointer_pos().and_then(pick);
            let additive = ui.input(|i| i.modifiers.shift || i.modifiers.command);
            match hit {
                Some(path) if additive => actions.push(ViewAction::ToggleSelect(path)),
                hit => actions.push(ViewAction::Select(hit)),
            }
        }

        // ---- overlays -------------------------------------------------------
        // While playing, the Game view shows only the game.
        let playing = self.play.is_some();
        if self.kind == ViewKind::Scene && self.show_colliders {
            colliders(&painter, &proj, input);
            vis_volumes(&painter, &proj, input);
        }
        if let Some(h) = &self.hover
            && Some(h.as_slice()) != input.selected
            && !playing
        {
            outline(
                &painter,
                &proj,
                input,
                h,
                Stroke::new(1.0, Color32::from_white_alpha(90)),
            );
        }
        if let Some(sel) = input.selected.filter(|_| !playing) {
            outline(&painter, &proj, input, sel, Stroke::new(1.6, c::ACCENT));
        }
        if !playing {
            for extra in input.extra {
                outline(
                    &painter,
                    &proj,
                    input,
                    extra,
                    Stroke::new(1.2, c::ACCENT.gamma_multiply(0.7)),
                );
            }
        }
        if !playing {
            markers(&painter, &proj, input, self.hover.as_deref());
        }
        if self.kind == ViewKind::Scene
            && let Some(g) = &gizmo
        {
            g.draw(&painter, &proj, self.gizmo, hot_handle);
        }

        // ---- drag & drop from the Assets panel ------------------------------
        if let Some(payload) = response.dnd_hover_payload::<AssetPayload>()
            && let Some(m) = mouse
        {
            let at = proj.ground_point(m, 0.0).unwrap_or_else(|| {
                let (o, d) = proj.ray(m);
                o + d * 6.0
            });
            if let Some(p) = proj.project(at) {
                painter.circle(
                    p,
                    7.0,
                    c::ACCENT.gamma_multiply(0.4),
                    Stroke::new(1.5, c::ACCENT),
                );
                painter.text(
                    p + Vec2::new(12.0, -12.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{} {}", icon::PLUS, payload.label()),
                    egui::FontId::proportional(12.0),
                    c::ACCENT_TEXT,
                );
            }
            if let Some(p) = response.dnd_release_payload::<AssetPayload>() {
                actions.push(ViewAction::Drop((*p).clone(), at));
            }
        }

        // ---- floating toolbar + widgets -------------------------------------
        self.toolbar(ui, avail, input, &mut actions);
        if self.kind == ViewKind::Scene {
            let center = Pos2::new(avail.max.x - 48.0, avail.min.y + 52.0);
            if let Some(axis) = gizmo::axis_widget(ui, &proj, center) {
                use std::f32::consts::{FRAC_PI_2, PI};
                // Click an axis to look down it (Blender-style).
                let (yaw, pitch) = match axis {
                    0 => (FRAC_PI_2, 0.0),
                    3 => (-FRAC_PI_2, 0.0),
                    1 => (self.cam.yaw, 1.54),
                    4 => (self.cam.yaw, -1.54),
                    2 => (PI, 0.0),
                    _ => (0.0, 0.0),
                };
                self.cam.view_from(yaw, pitch);
            }
        }
        let stats = format!(
            "{}  {}   {}  {}   {:.1} ms   {}×{}",
            icon::CUBE,
            self.stats.items,
            icon::TRIANGLE,
            self.stats.tris,
            self.stats.ms,
            internal.0,
            internal.1
        );
        painter.text(
            Pos2::new(avail.min.x + 10.0, avail.max.y - 10.0),
            egui::Align2::LEFT_BOTTOM,
            stats,
            egui::FontId::monospace(11.0),
            Color32::from_white_alpha(120),
        );
        if self.kind == ViewKind::Game && self.show_budget {
            // Panel-wide painter: the overlay sits over the letterbox area
            // when there is room, never clipped by the game image.
            self.budget_overlay(&ui.painter_at(avail), avail);
        }

        // ---- keyboard (when hovered) ------------------------------------------
        if response.hovered()
            && self.kind == ViewKind::Scene
            && !ui.ctx().egui_wants_keyboard_input()
        {
            use egui::Key;
            let (w, e, r, f, g, n1, n3, n7, ctrl) = ui.input(|i| {
                (
                    i.key_pressed(Key::W),
                    i.key_pressed(Key::E),
                    i.key_pressed(Key::R),
                    i.key_pressed(Key::F),
                    i.key_pressed(Key::G),
                    i.key_pressed(Key::Num1),
                    i.key_pressed(Key::Num3),
                    i.key_pressed(Key::Num7),
                    i.modifiers.command,
                )
            });
            if !ctrl {
                if w {
                    self.gizmo = GizmoMode::Translate;
                }
                if e {
                    self.gizmo = GizmoMode::Rotate;
                }
                if r {
                    self.gizmo = GizmoMode::Scale;
                }
                if g {
                    self.show_grid = !self.show_grid;
                    self.rendered = None;
                }
            }
            if f {
                match input.selected {
                    Some(sel) => self.focus_on(input, sel),
                    None => self.cam.focus(Vec3::new(0.0, 0.5, 0.0), 6.0),
                }
            }
            use std::f32::consts::{FRAC_PI_2, PI};
            if n1 {
                self.cam.view_from(if ctrl { 0.0 } else { PI }, 0.0);
            }
            if n3 {
                self.cam
                    .view_from(if ctrl { -FRAC_PI_2 } else { FRAC_PI_2 }, 0.0);
            }
            if n7 {
                self.cam
                    .view_from(self.cam.yaw, if ctrl { -1.54 } else { 1.54 });
            }
        }
        actions
    }

    /// Console budget panel (top right of the Game view).
    fn budget_overlay(&self, painter: &egui::Painter, avail: Rect) {
        let caps = *self.renderer.caps();
        let rows = crate::budget::rows(&self.stats.frame, &caps);
        let w = 210.0;
        let row_h = 18.0;
        let rect = Rect::from_min_size(
            Pos2::new(avail.max.x - w - 10.0, avail.min.y + 50.0),
            Vec2::new(w, 24.0 + rows.len() as f32 * row_h),
        );
        painter.rect_filled(rect, CornerRadius::same(6), c::BG1.gamma_multiply(0.88));
        painter.text(
            rect.min + Vec2::new(8.0, 6.0),
            egui::Align2::LEFT_TOP,
            format!(
                "{} budget",
                match self.profile {
                    SimProfile::N64 => "N64",
                    SimProfile::N3ds => "3DS",
                    SimProfile::Native => "PC",
                }
            ),
            egui::FontId::proportional(11.5),
            c::TEXT_WEAK,
        );
        for (i, row) in rows.iter().enumerate() {
            let y = rect.min.y + 24.0 + i as f32 * row_h;
            let color = match row.level {
                crate::budget::Level::Ok => c::OK,
                crate::budget::Level::Near => c::WARN,
                crate::budget::Level::Over => c::ERROR,
            };
            let value = if row.limit == u64::MAX {
                format!("{}{}", row.used, row.unit)
            } else {
                format!("{}{} / {}{}", row.used, row.unit, row.limit, row.unit)
            };
            painter.text(
                Pos2::new(rect.min.x + 8.0, y),
                egui::Align2::LEFT_TOP,
                row.label,
                egui::FontId::proportional(11.0),
                c::TEXT,
            );
            painter.text(
                Pos2::new(rect.max.x - 8.0, y),
                egui::Align2::RIGHT_TOP,
                value,
                egui::FontId::monospace(10.5),
                color,
            );
            if row.limit != u64::MAX {
                let bar = Rect::from_min_size(
                    Pos2::new(rect.min.x + 8.0, y + 13.0),
                    Vec2::new(w - 16.0, 2.0),
                );
                painter.rect_filled(bar, CornerRadius::ZERO, c::BORDER);
                let filled = Rect::from_min_size(
                    bar.min,
                    Vec2::new(bar.width() * row.fraction().min(1.0), 2.0),
                );
                painter.rect_filled(filled, CornerRadius::ZERO, color);
            }
        }
    }

    fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        avail: Rect,
        input: &ViewInput,
        actions: &mut Vec<ViewAction>,
    ) {
        let rect = Rect::from_min_size(
            avail.min + Vec2::new(8.0, 8.0),
            Vec2::new(avail.width() - 120.0, 34.0),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        egui::Frame::new()
            .fill(c::BG1.gamma_multiply(0.86))
            .stroke(Stroke::new(1.0, c::BORDER))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(6, 3))
            .show(&mut child, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    match self.kind {
                        ViewKind::Scene => {
                            for (mode, glyph, tip) in [
                                (GizmoMode::Translate, icon::ARROWS_OUT_CARDINAL, "Move (W)"),
                                (GizmoMode::Rotate, icon::ARROW_CLOCKWISE, "Rotate (E)"),
                                (GizmoMode::Scale, icon::CORNERS_OUT, "Scale (R)"),
                            ] {
                                if theme::toggle_icon(ui, self.gizmo == mode, glyph, tip).clicked()
                                {
                                    self.gizmo = mode;
                                }
                            }
                            ui.separator();
                            if theme::toggle_icon(
                                ui,
                                self.snap,
                                icon::MAGNET,
                                "Snap (hold Ctrl to invert): 0.25 m / 15° / 0.05",
                            )
                            .clicked()
                            {
                                self.snap = !self.snap;
                            }
                            if theme::toggle_icon(ui, self.show_grid, icon::GRID_FOUR, "Grid (G)")
                                .clicked()
                            {
                                self.show_grid = !self.show_grid;
                                self.rendered = None;
                            }
                            if theme::toggle_icon(
                                ui,
                                self.show_colliders,
                                icon::BOUNDING_BOX,
                                "Colliders (green = solid, yellow = trigger)",
                            )
                            .clicked()
                            {
                                self.show_colliders = !self.show_colliders;
                            }
                            if theme::toggle_icon(
                                ui,
                                input.animate,
                                icon::FILM_STRIP,
                                "Preview animations",
                            )
                            .clicked()
                            {
                                actions.push(ViewAction::ToggleAnimate);
                            }
                            ui.separator();
                            if theme::icon_button(ui, icon::CROSSHAIR, "Frame selection (F)")
                                .clicked()
                            {
                                match input.selected {
                                    Some(sel) => self.focus_on(input, sel),
                                    None => self.cam.focus(Vec3::new(0.0, 0.5, 0.0), 6.0),
                                }
                            }
                            if theme::icon_button(
                                ui,
                                icon::VIDEO_CAMERA,
                                "Align view to the game camera",
                            )
                            .clicked()
                            {
                                let cam = input.scene.environment.core_camera();
                                self.cam.look_from(cam.eye, cam.target);
                            }
                        }
                        ViewKind::Game => {
                            ui.label(RichText::new(icon::GAME_CONTROLLER).color(c::TEXT_WEAK));
                            let mut profile = self.profile;
                            for (p, name) in [
                                (SimProfile::N64, "N64"),
                                (SimProfile::N3ds, "3DS"),
                                (SimProfile::Native, "PC"),
                            ] {
                                if theme::toggle_icon(
                                    ui,
                                    profile == p,
                                    name,
                                    &format!("Simulate {name} output"),
                                )
                                .clicked()
                                {
                                    profile = p;
                                }
                            }
                            if profile != self.profile {
                                self.set_profile(profile, input.project);
                            }
                            ui.separator();
                            if theme::toggle_icon(
                                ui,
                                self.show_budget,
                                icon::GAUGE,
                                "Console budget: tris / sprites / texture vs the profile's Caps",
                            )
                            .clicked()
                            {
                                self.show_budget = !self.show_budget;
                            }
                            if self.profile == SimProfile::N64 {
                                let mut look = self.renderer.n64_look();
                                if theme::toggle_icon(
                                    ui,
                                    look,
                                    icon::SPARKLE,
                                    "N64 look: 3-point filter + RGBA5551 dither",
                                )
                                .clicked()
                                {
                                    look = !look;
                                    self.renderer.set_n64_look(look);
                                    self.rendered = None;
                                }
                            }
                            if theme::toggle_icon(
                                ui,
                                input.animate,
                                icon::FILM_STRIP,
                                "Preview animations",
                            )
                            .clicked()
                            {
                                actions.push(ViewAction::ToggleAnimate);
                            }
                        }
                    }
                });
            });
    }
}

/// Nearest entity under `pos`: model boxes (ray vs OBB) and marker dots.
fn pick(proj: &Proj, input: &ViewInput, pos: Pos2) -> Option<Vec<usize>> {
    if !proj.rect.contains(pos) {
        return None;
    }
    let (o, d) = proj.ray(pos);
    let mut best: Option<(f32, &Vec<usize>)> = None;
    for item in &input.resolved.items {
        let t = match &item.model {
            Some((id, _)) => input
                .project
                .model_by_id(*id)
                .and_then(|m| gizmo::ray_obb(o, d, &item.matrix, m.min, m.max)),
            None if is_marker(input, item) => {
                let p = gizmo::translation(&item.matrix);
                proj.project(p)
                    .filter(|s| (*s - pos).length() < 10.0)
                    .map(|_| proj.depth(p))
            }
            None => None,
        };
        if let Some(t) = t
            && best.is_none_or(|(bt, _)| t < bt)
        {
            best = Some((t, &item.owner));
        }
    }
    best.map(|(_, p)| p.clone())
}

fn outline(
    painter: &egui::Painter,
    proj: &Proj,
    input: &ViewInput,
    path: &[usize],
    stroke: Stroke,
) {
    for item in input
        .resolved
        .items
        .iter()
        .filter(|i| i.owner.starts_with(path))
    {
        if let Some((id, _)) = &item.model
            && let Some(m) = input.project.model_by_id(*id)
        {
            for [a, b] in gizmo::box_lines(proj, &item.matrix, m.min, m.max) {
                painter.line_segment([a, b], stroke);
            }
        }
    }
}

/// Collider wireframes: the world box each collider resolves to (what
/// `trino_core::physics` collides against).
fn colliders(painter: &egui::Painter, proj: &Proj, input: &ViewInput) {
    for item in &input.resolved.items {
        let Some(col) = item.collider else {
            continue;
        };
        let w = col.world_aabb(&item.matrix);
        let color = if col.trigger { c::TRIGGER } else { c::COLLIDER };
        let stroke = Stroke::new(1.0, color.gamma_multiply(0.85));
        for [a, b] in gizmo::box_lines(
            proj,
            &trino_core::Mat34::IDENTITY,
            [w.min.x, w.min.y, w.min.z],
            [w.max.x, w.max.y, w.max.z],
        ) {
            painter.line_segment([a, b], stroke);
        }
    }
}

/// Cell and portal wireframes: the world boxes `trino_core::vis` uses.
fn vis_volumes(painter: &egui::Painter, proj: &Proj, input: &ViewInput) {
    for item in &input.resolved.items {
        let (half, color) = match item.vis {
            Some(trino_core::VisVolume::Cell { half }) => (half, c::CELL),
            Some(trino_core::VisVolume::Portal { half, .. }) => (half, c::PORTAL),
            None => continue,
        };
        let p = gizmo::translation(&item.matrix);
        let stroke = Stroke::new(1.0, color.gamma_multiply(0.7));
        for [a, b] in gizmo::box_lines(
            proj,
            &trino_core::Mat34::IDENTITY,
            [p.x - half.x, p.y - half.y, p.z - half.z],
            [p.x + half.x, p.y + half.y, p.z + half.z],
        ) {
            painter.line_segment([a, b], stroke);
        }
    }
}

/// True empties (no model, sprite, prefab or children) get a pin marker.
fn is_marker(input: &ViewInput, item: &trino_scene::DrawItem) -> bool {
    item.model.is_none()
        && item.sprite.is_none()
        && !item.from_prefab
        && input
            .scene
            .get(&item.owner)
            .is_some_and(|e| e.prefab.is_none() && e.children.is_empty())
}

fn markers(painter: &egui::Painter, proj: &Proj, input: &ViewInput, hover: Option<&[usize]>) {
    for item in &input.resolved.items {
        if !is_marker(input, item) {
            continue;
        }
        let Some(p) = proj.project(gizmo::translation(&item.matrix)) else {
            continue;
        };
        let selected = input.selected == Some(item.owner.as_slice());
        let hovered = hover == Some(item.owner.as_slice());
        let col = if selected { c::ACCENT } else { c::TEXT_WEAK };
        painter.circle(p, 9.0, c::BG0.gamma_multiply(0.8), Stroke::new(1.5, col));
        painter.text(
            p,
            egui::Align2::CENTER_CENTER,
            icon::MAP_PIN,
            egui::FontId::proportional(11.0),
            col,
        );
        if selected || hovered {
            painter.text(
                p + Vec2::new(13.0, 0.0),
                egui::Align2::LEFT_CENTER,
                item.name.rsplit('/').next().unwrap_or(&item.name),
                egui::FontId::proportional(12.0),
                Color32::from_white_alpha(200),
            );
        }
    }
}

/// Bounding sphere of an entity subtree (focus / frame selection).
pub fn selection_sphere(input: &ViewInput, path: &[usize]) -> Option<(Vec3, f32)> {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    let mut any = false;
    for item in input
        .resolved
        .items
        .iter()
        .filter(|i| i.owner.starts_with(path))
    {
        let mut add = |p: Vec3| {
            any = true;
            for (k, v) in [p.x, p.y, p.z].into_iter().enumerate() {
                lo[k] = lo[k].min(v);
                hi[k] = hi[k].max(v);
            }
        };
        match item
            .model
            .as_ref()
            .and_then(|(id, _)| input.project.model_by_id(*id))
        {
            Some(m) => {
                for i in 0..8 {
                    let corner = Vec3::new(
                        if i & 1 == 0 { m.min[0] } else { m.max[0] },
                        if i & 2 == 0 { m.min[1] } else { m.max[1] },
                        if i & 4 == 0 { m.min[2] } else { m.max[2] },
                    );
                    add(item.matrix.transform_point(corner));
                }
            }
            None => add(gizmo::translation(&item.matrix)),
        }
    }
    any.then(|| {
        let c = Vec3::new(
            (lo[0] + hi[0]) * 0.5,
            (lo[1] + hi[1]) * 0.5,
            (lo[2] + hi[2]) * 0.5,
        );
        let d = Vec3::new(hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]);
        (c, (d.dot(d).sqrt() * 0.5).max(0.5))
    })
}

/// Ground grid: 1 m lines, brighter every 5 m, colored X (red) / Z (blue)
/// axes. Thin quads, so it is z-buffered against the scene like Unity's.
fn grid_tmdl() -> Vec<u8> {
    let mut d = MeshData::default();
    let mut quad = |a: [f32; 3], b: [f32; 3], w: f32, col: [u8; 4], along_x: bool| {
        let base = d.positions.len() as u32;
        let (ox, oz) = if along_x { (0.0, w) } else { (w, 0.0) };
        // A hair above y = 0 so it sits on ground planes without z-fighting.
        let y = 0.006;
        for p in [
            [a[0] - ox, y, a[2] - oz],
            [b[0] - ox, y, b[2] - oz],
            [b[0] + ox, y, b[2] + oz],
            [a[0] + ox, y, a[2] + oz],
        ] {
            d.positions.push(p);
            d.normals.push([0.0, 1.0, 0.0]);
            d.colors.push(col);
        }
        d.indices
            .extend([base, base + 2, base + 1, base, base + 3, base + 2]);
    };
    let e = GRID_EXTENT as f32;
    for i in -GRID_EXTENT..=GRID_EXTENT {
        let v = i as f32;
        let major = i % 5 == 0;
        let (w, col) = if major {
            (0.014, [92, 98, 120, 255])
        } else {
            (0.008, [58, 62, 78, 255])
        };
        // The z = 0 line runs along X (red axis); x = 0 runs along Z (blue).
        let (along_x, along_z) = if i == 0 {
            ((0.02, [240, 86, 107, 255]), (0.02, [80, 142, 255, 255]))
        } else {
            ((w, col), (w, col))
        };
        quad([-e, 0.0, v], [e, 0.0, v], along_x.0, along_x.1, true);
        quad([v, 0.0, -e], [v, 0.0, e], along_z.0, along_z.1, false);
    }
    d.encode().expect("grid mesh")
}
