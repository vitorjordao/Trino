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
    Color, Fog, HwTier, Light, LodPick, Material, ModelId, ModelParams, QualityPreset,
    QualitySettings, Renderer, Shading, SpriteId, SpriteParams, Vec2 as TVec2, Vec3,
};
use trino_platform_pc::{AoView, PcRenderer, ShadowView, SimProfile};
use trino_scene::{Resolved, Scene, Target, Transform};

use crate::gizmo::{self, AxisEvent, Drag, Gizmo, GizmoMode, OrbitCam, Proj, RenderCam};
use crate::project::Project;
use crate::theme::{self, c, icon};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewKind {
    Scene,
    Game,
}

/// The Game view's canvas when it simulates a browser (`SimProfile::Web`,
/// issue #47): a page of a fixed size, or the panel itself like a canvas
/// that fills its page. The picture is what the browser renders there:
/// the canvas over the auto pixel scale (`sim::native_internal`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum WebCanvas {
    /// The panel's own size (a canvas that fills a resizable page).
    #[default]
    Responsive,
    Hd720,
    Hd1080,
}

impl WebCanvas {
    pub const ALL: [WebCanvas; 3] = [WebCanvas::Responsive, WebCanvas::Hd720, WebCanvas::Hd1080];

    pub fn label(self) -> &'static str {
        match self {
            WebCanvas::Responsive => "Responsive",
            WebCanvas::Hd720 => "1280×720",
            WebCanvas::Hd1080 => "1920×1080",
        }
    }

    /// `responsive`, `720`, `1080` (`TRINO_EDITOR_WEB_CANVAS`).
    pub fn parse(s: &str) -> Option<WebCanvas> {
        match s {
            "responsive" => Some(WebCanvas::Responsive),
            "720" | "720p" => Some(WebCanvas::Hd720),
            "1080" | "1080p" => Some(WebCanvas::Hd1080),
            _ => None,
        }
    }

    /// The picture a browser renders for this canvas (`panel`: the
    /// responsive canvas's device pixels).
    pub fn picture(self, panel: (u32, u32)) -> (u32, u32) {
        let output = match self {
            WebCanvas::Responsive => panel,
            WebCanvas::Hd720 => (1280, 720),
            WebCanvas::Hd1080 => (1920, 1080),
        };
        let (w, h, _) = trino_platform_pc::sim::native_internal(output, 0);
        (w, h)
    }
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
    /// Inspector › LOD › Preview: draw this entity's LOD as if the camera
    /// were this far away (both views), to see each switch live.
    pub lod_preview: Option<(&'a [usize], f32)>,
    /// The scene's frame on every target, the tiers too (Game view HUD).
    pub budgets: Option<&'a crate::budget::Budgets>,
    /// Each sprite's N64 TMEM bytes (the HUD's water lines).
    pub tmem: Option<&'a std::collections::BTreeMap<u32, Option<u32>>>,
    /// The water clock and ripples (`Renderer::set_water`).
    pub water: (f32, &'a [trino_core::Ripple]),
    /// The project's audio per platform (the Game view HUD's audio line).
    pub audio: Option<&'a crate::audio::Totals>,
    /// The project's quality presets (the Game view's preset picker).
    pub quality: Option<&'a trino_core::PresetTable>,
    /// The asset memory plans per console tier (the HUD's assets RAM line,
    /// issue #56).
    pub memory: Option<&'a [trino_asset_pipeline::memory::MemoryPlan]>,
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
    /// The budget HUD's platform lines were clicked: show the Inspector's
    /// platform budget, where each reason lists its entities.
    OpenBudget,
}

pub struct Stats {
    pub items: usize,
    pub tris: usize,
    pub ms: f32,
    /// What the renderer counted for the last frame (budget overlay).
    pub frame: trino_core::FrameStats,
    /// What entities with LOD levels drew (full / light / impostor / cull).
    pub lod: crate::budget::LodCounts,
    /// The screen-space AO ran on the last frame.
    pub ao: bool,
    /// The tier the last frame's shadows drew at (`frame.shadows` counts
    /// them).
    pub shadow_tier: trino_core::ShadowTier,
}

pub struct Viewport {
    pub kind: ViewKind,
    render_state: RenderState,
    renderer: PcRenderer,
    texture: egui::TextureId,
    pub profile: SimProfile,
    /// Game view: the console's hardware tier (issue #54, ADR 0020) — the
    /// N64 with the Expansion Pak, the New 3DS. The look stays the
    /// console's; the renderer's caps and the resolved variants are the
    /// tier's.
    pub tier: HwTier,
    /// Game view, PC and Web profiles: the quality preset it previews
    /// (issue #55; the Quality tab's values) and whether its dynamic
    /// resolution runs. The consoles' presets are fixed.
    pub quality: QualityPreset,
    pub dynres: bool,
    /// The settings the renderer has (`None`: none applied yet).
    quality_applied: Option<QualitySettings>,
    pub cam: OrbitCam,
    pub show_grid: bool,
    /// Draw collider wireframes (world boxes — exactly what the physics
    /// uses): green = solid, yellow = trigger.
    pub show_colliders: bool,
    /// Scene view: the screen-space AO as the scene sets it, forced on
    /// (defaults when the scene has none), off, or the occlusion alone.
    pub ao_view: AoView,
    /// The mode the last render used (a change from the View menu
    /// re-renders).
    ao_rendered: AoView,
    /// Scene view: the shadows at the profile's tier, off, or in the
    /// debug color (every caster's, even with the scene's off).
    pub shadow_view: ShadowView,
    shadow_rendered: ShadowView,
    /// Game view: the console budget overlay (tris / sprites / texture vs
    /// the selected profile's Caps).
    pub show_budget: bool,
    /// Game view, Web profile: the simulated canvas.
    pub web_canvas: WebCanvas,
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
    /// Fly mode: RMB held on the Scene view (mouselook + WASD/QE).
    fly: bool,
}

const GRID_EXTENT: i32 = 40;

/// The platform whose variants a console-simulation profile shows (its
/// base tier: [`Viewport::target`] adds the Game view's tier).
pub fn target_of(profile: SimProfile) -> trino_scene::Target {
    match profile {
        SimProfile::Native => trino_scene::Target::Pc,
        SimProfile::N64 => trino_scene::Target::N64,
        SimProfile::N3ds => trino_scene::Target::N3ds,
        SimProfile::Web => trino_scene::Target::Web,
    }
}

/// What a rendered frame depended on: (content rev, size, asset generation,
/// camera, animation time). Same key = skip the render.
type RenderKey = (u64, (u32, u32), u64, [f32; 7], f32);

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
            tier: HwTier::Base,
            quality: QualityPreset::High,
            dynres: false,
            quality_applied: None,
            cam: OrbitCam::default(),
            show_grid: true,
            show_colliders: true,
            ao_view: AoView::Game,
            ao_rendered: AoView::Game,
            shadow_view: ShadowView::Game,
            shadow_rendered: ShadowView::Game,
            show_budget: true,
            web_canvas: std::env::var("TRINO_EDITOR_WEB_CANVAS")
                .ok()
                .and_then(|v| WebCanvas::parse(&v))
                .unwrap_or_default(),
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
                lod: crate::budget::LodCounts::default(),
                ao: false,
                shadow_tier: trino_core::ShadowTier::None,
            },
            hover: None,
            box_start: None,
            fly: false,
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

    /// Switch the Game view's console profile (new renderer, re-upload)
    /// and hardware tier: `N64` + `Expanded` is the N64 with the Expansion
    /// Pak, `N3ds` + `Expanded` the New 3DS (the PC and the Web have one
    /// tier). A new tier on the same console keeps the renderer and
    /// changes its caps.
    pub fn set_profile_tier(&mut self, profile: SimProfile, tier: HwTier, project: &Project) {
        let tier = if profile.has_tiers() {
            tier
        } else {
            HwTier::Base
        };
        if profile != self.profile {
            let device = wgpu::Device::clone(&self.render_state.device);
            let queue = wgpu::Queue::clone(&self.render_state.queue);
            self.renderer = PcRenderer::with_device(device, queue, profile);
            self.renderer.upload_mesh(self.grid, grid_tmdl());
            project.upload_all(&mut self.renderer);
            project.upload_platform_sprites(&mut self.renderer, target_of(profile));
            self.uploaded_gen = project.generation;
            self.profile = profile;
            self.quality_applied = None;
            self.reregister();
        } else if tier == self.tier {
            return;
        }
        self.set_tier(tier);
    }

    /// Game view, PC and Web profiles: draw with `settings` (the previewed
    /// preset's, from the Quality tab), dynamic resolution as the toolbar
    /// says and no frame cap (the editor paints when it needs to). The
    /// console profiles keep their fixed look.
    pub fn preview_quality(&mut self, settings: QualitySettings) {
        if self.kind != ViewKind::Game || !self.profile.is_native() {
            return;
        }
        let q = QualitySettings {
            dynamic_resolution: match (self.dynres, settings.dynamic_resolution) {
                (false, _) => None,
                (true, Some(d)) => Some(d),
                (true, None) => Some(trino_core::DynRes::DEFAULT),
            },
            frame_limit: trino_core::FrameLimit::VSync,
            ..settings
        };
        if self.quality_applied != Some(q) {
            self.renderer.apply_quality(&q);
            self.quality_applied = Some(q);
            self.rendered = None;
        }
    }

    /// The 3D's render scale now (1 on the consoles; dynamic resolution
    /// moves it).
    pub fn render_scale(&self) -> f32 {
        self.renderer.render_scale()
    }

    /// The settings the Game view draws with (`None`: a console's fixed
    /// look).
    pub fn quality_settings(&self) -> Option<QualitySettings> {
        self.quality_applied.filter(|_| self.profile.is_native())
    }

    /// Put the Game view on `tier` hardware of its console (nothing to
    /// re-upload: the renderer keeps its profile and changes its caps).
    pub fn set_tier(&mut self, tier: HwTier) {
        self.tier = if self.profile.has_tiers() {
            tier
        } else {
            HwTier::Base
        };
        self.renderer.set_hw_tier(self.tier);
        self.rendered = None;
    }

    /// The platform whose variants this view shows: its profile's, on its
    /// hardware tier (`n64_expanded`, `n3ds_new`).
    pub fn target(&self) -> trino_scene::Target {
        target_of(self.profile).with_tier(self.tier)
    }

    /// The Game view's name for what it simulates: `N64`, `N64+Pak`,
    /// `3DS`, `New 3DS`, `PC`, `Web`.
    pub fn target_label(&self) -> &'static str {
        self.target().label()
    }

    /// Re-upload changed assets (live reload).
    pub fn upload(&mut self, project: &Project, changed: &[String]) {
        project.upload_some(&mut self.renderer, changed);
        project.upload_platform_sprites(&mut self.renderer, target_of(self.profile));
        self.rendered = None;
    }

    /// Internal resolution of this view's renderer.
    pub fn internal_size(&self) -> (u32, u32) {
        self.renderer.internal_size()
    }

    /// `TRINO_SIM` for a game started from the editor: the profile on its
    /// tier (`n64x`, `3dsnew`).
    pub fn profile_key(&self) -> &'static str {
        self.profile.key(self.tier)
    }

    /// Fly mode is on (RMB held on the Scene view): WASD/QE/Shift move the
    /// camera, so the app's own key shortcuts stand down.
    pub fn flying(&self) -> bool {
        self.fly
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
        // The Scene view may draw near-orthographic: a far camera over a
        // scaled-down world (`RenderCam`); LOD still measures from the
        // orbit eye.
        let (rc, lod_eye) = match self.kind {
            ViewKind::Scene => (self.cam.render_cam(), self.cam.eye()),
            ViewKind::Game => {
                let camera = env.core_camera();
                (RenderCam::plain(camera), camera.eye)
            }
        };
        let camera = rc.camera;
        let ortho = rc.scale != 1.0;
        let place = |m: &trino_core::Mat34| if ortho { rc.world.mul(m) } else { *m };
        r.set_camera(&camera);
        let light = env.core_light();
        let fog = match self.kind {
            ViewKind::Scene => scene_view_fog(light.fog, self.cam.dist),
            ViewKind::Game => light.fog,
        };
        let light = Light {
            fog: rc.fog(fog),
            ..light
        };

        if self.kind == ViewKind::Scene && self.show_grid {
            // The grid fades into the background with distance.
            let d = self.cam.dist;
            r.set_light(&Light {
                fog: rc.fog(Fog {
                    color: Color::rgb(clear[0], clear[1], clear[2]),
                    near: d * 0.6,
                    far: d * 2.4 + 12.0,
                }),
                ..light
            });
            r.draw_model_matrix(
                self.grid,
                &place(&trino_core::Mat34::IDENTITY),
                Material::VertexLit,
                &ModelParams {
                    shading: Shading::Unlit,
                    double_sided: true,
                    ..ModelParams::DEFAULT
                },
            );
        }
        r.set_light(&light);
        // The scene's screen-space AO (the native profile runs it; the
        // console profiles show the AO baked into the models).
        r.set_ao_view(match self.kind {
            ViewKind::Scene => self.ao_view,
            ViewKind::Game => AoView::Game,
        });
        r.set_ambient_occlusion(env.core_ao().map(|a| rc.ao(a)).as_ref());
        // The scene's shadows (ADR 0024) at the previewed profile's tier,
        // onto its receivers; the Scene view's menu turns them off or
        // paints them in the debug color.
        r.set_shadow_view(match self.kind {
            ViewKind::Scene => self.shadow_view,
            ViewKind::Game => ShadowView::Game,
        });
        r.set_shadows(env.core_shadows().as_ref());
        r.set_shadow_receivers(&input.resolved.receivers());
        // Water surfaces flow with the preview clock; ripples dropped from
        // the material inspector ring out.
        r.set_water(input.water.0, input.water.1);

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
        let mut lod = crate::budget::LodCounts::default();
        // The previewed quality preset's LOD bias (#55; 1 on the consoles).
        let bias = trino_core::Renderer::lod_bias(&*r);
        for item in &input.resolved.items {
            if !trino_core::vis::in_view(item.cell, visible) {
                continue;
            }
            // LOD as the game sees it, from this view's camera (or at the
            // Inspector's preview distance for the entity being tuned).
            let pick = match input.lod_preview {
                Some((owner, d)) if item.owner == owner && !item.from_prefab => item.lod_at(d),
                _ => item.lod_from_biased(lod_eye, bias),
            };
            if !item.lod.is_empty() {
                match pick {
                    LodPick::Model(id) if item.model.as_ref().map(|m| m.0) == Some(id) => {
                        lod.full += 1
                    }
                    LodPick::Model(_) => lod.light += 1,
                    LodPick::Impostor(_) => lod.impostor += 1,
                    LodPick::None => lod.culled += 1,
                }
            }
            let lod_id = match pick {
                LodPick::Model(id) => id,
                LodPick::Impostor(imp) => {
                    imp.draw(r, &place(&item.matrix), camera.eye, item.params.tint);
                    tris += 2;
                    continue;
                }
                LodPick::None => continue,
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
            r.draw_model_matrix(*id, &place(&item.matrix), Material::VertexLit, &params);
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
            lod,
            ao: r.frame_times().ao_passes > 0,
            shadow_tier: r.shadow_tier(),
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
            input
                .project
                .upload_platform_sprites(&mut self.renderer, target_of(self.profile));
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
                // A browser's picture follows its canvas (issue #47).
                if self.profile == SimProfile::Web && self.play.is_none() {
                    let panel = (
                        ((avail.width() * ppp) as u32).max(16),
                        ((avail.height() * ppp) as u32).max(16),
                    );
                    let (w, h) = self.web_canvas.picture(panel);
                    if (w, h) != self.renderer.internal_size() {
                        self.renderer.set_internal_size(w, h);
                    }
                }
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
            // The overlay camera: its eye and field of view also change
            // with the projection (and with zoom when orthographic).
            let cam = self.cam.camera();
            let (e, t) = (cam.eye, cam.target);
            [e.x, e.y, e.z, t.x, t.y, t.z, cam.fov_y]
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
        let dynres_live = self
            .quality_settings()
            .is_some_and(|q| q.dynamic_resolution.is_some());
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
                lod: crate::budget::LodCounts::default(),
                ao: self.renderer.frame_times().ao_passes > 0,
                shadow_tier: self.renderer.shadow_tier(),
            };
            if dynres_live {
                self.renderer.report_frame_time((dt * 1e6) as u32);
            }
            self.rendered = None;
            ui.ctx().request_repaint();
        } else if self.rendered.as_ref() != Some(&key)
            || flashing
            || cam_moving
            || self.ao_rendered != self.ao_view
            || self.shadow_rendered != self.shadow_view
            || dynres_live
        {
            self.ao_rendered = self.ao_view;
            self.shadow_rendered = self.shadow_view;
            self.render(input, internal);
            if size_changed {
                self.reregister();
            }
            self.rendered = Some(key);
            if dynres_live {
                // Dynamic resolution follows this view's frames (the
                // editor's device times none on the GPU): keep drawing.
                self.renderer
                    .report_frame_time((self.stats.ms * 1000.0) as u32);
                ui.ctx().request_repaint();
            }
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

        // Drag start / update / end. A drag starts once the pointer has
        // moved past egui's threshold: the handle (and the drag's start) is
        // the one under where the button went down, not where the pointer is
        // by then (a quick drag off a thin handle grabbed its neighbour).
        let press = ui.input(|i| i.pointer.press_origin());
        let press_handle = match (&gizmo, press) {
            (Some(g), Some(p)) if self.kind == ViewKind::Scene => g.hit(&proj, self.gizmo, p),
            _ => None,
        };
        if self.kind == ViewKind::Scene {
            if response.drag_started_by(egui::PointerButton::Primary)
                && !ui.input(|i| i.modifiers.alt)
                && let (Some(h), Some(g), Some(item), Some(m)) =
                    (press_handle, &gizmo, sel_item, press)
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
            let (shift, alt) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt));
            // Hold RMB on the view to fly: mouselook, WASD along the view,
            // Q/E down/up, Shift boosts, the wheel sets the speed. (Shift+RMB
            // still pans and Alt+RMB dollies.)
            if !self.fly
                && response.hovered()
                && !shift
                && !alt
                && ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary))
            {
                self.fly = true;
                // Moving along the view shows nothing in a parallel
                // projection: flying is perspective (as Blender's walk is).
                self.cam.ortho = false;
                ui.memory_mut(|m| m.stop_text_input());
            }
            if self.fly && !ui.input(|i| i.pointer.button_down(egui::PointerButton::Secondary)) {
                self.fly = false;
            }
            if self.fly {
                use egui::Key;
                let (look, dir, boost, scroll) = ui.input(|i| {
                    let k = |key: Key| if i.key_down(key) { 1.0 } else { 0.0 };
                    (
                        i.pointer.delta(),
                        Vec3::new(
                            k(Key::D) - k(Key::A),
                            k(Key::E) - k(Key::Q),
                            k(Key::W) - k(Key::S),
                        ),
                        i.modifiers.shift,
                        i.smooth_scroll_delta.y,
                    )
                });
                if look != Vec2::ZERO {
                    self.cam.look(look);
                }
                if dir.dot(dir) > 0.0 {
                    self.cam.fly(dir, dt, boost);
                }
                if scroll != 0.0 {
                    self.cam.fly_speed_scroll(scroll);
                }
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                ui.ctx().request_repaint();
            }
            let delta = response.drag_delta();
            let (lmb, mmb, rmb) = (
                response.dragged_by(egui::PointerButton::Primary),
                response.dragged_by(egui::PointerButton::Middle),
                response.dragged_by(egui::PointerButton::Secondary) && !self.fly,
            );
            // MMB pans, with or without Ctrl/Cmd or Shift; Shift+RMB too.
            let pan = mmb || (rmb && shift && !alt);
            let dolly = rmb && alt;
            // Shift + left drag on empty space: box select.
            if response.drag_started_by(egui::PointerButton::Primary)
                && shift
                && press_handle.is_none()
            {
                self.box_start = press.or(response.interact_pointer_pos());
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
            let orbit = lmb && alt;
            if pan {
                self.cam.pan(delta, image_rect.height());
            } else if dolly {
                self.cam.dolly(delta.y);
            } else if orbit {
                self.cam.orbit(delta);
            }
            if response.hovered() && !self.fly {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    self.cam.zoom(scroll);
                    ui.ctx().request_repaint();
                }
            }
            if pan || dolly || orbit {
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
        if self.kind == ViewKind::Scene && self.shadow_view == ShadowView::Debug && !playing {
            shadow_debug(&painter, &proj, input);
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
            // The drag's angle / distance / factor, and the snap notches.
            if let Some((_, d)) = &self.drag
                && let Some(m) = ui.input(|i| i.pointer.interact_pos())
            {
                let snap = self.snap ^ ui.input(|i| i.modifiers.command);
                d.feedback(&painter, &proj, m, snap);
            }
        }

        // ---- drag & drop from the Assets panel ------------------------------
        if let Some(payload) = response.dnd_hover_payload::<AssetPayload>()
            && let Some(m) = mouse
        {
            let at = gizmo::drop_point(&proj, m, &self.cam);
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
            if let Some(event) = gizmo::axis_widget(ui, &proj, center, self.cam.ortho) {
                match event {
                    // Click an axis to look from it, again for the other side.
                    AxisEvent::Snap(axis) => {
                        self.cam.snap_to_axis(axis);
                    }
                    AxisEvent::Orbit(delta) => self.cam.orbit(delta),
                    AxisEvent::ToggleOrtho => self.cam.ortho = !self.cam.ortho,
                }
                ui.ctx().request_repaint();
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
            let lines = self.budget_overlay(
                &ui.painter_at(avail),
                avail,
                input.budgets,
                input.tmem,
                input.audio,
                input.memory.map(|m| (m, input.scene)),
            );
            if let Some(lines) = lines {
                let hud = ui
                    .interact(lines, ui.id().with("budget-hud"), egui::Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Open the platform budget: each reason lists its entities");
                if hud.clicked() {
                    actions.push(ViewAction::OpenBudget);
                }
            }
        }

        // ---- keyboard (when hovered) ------------------------------------------
        // (Not while flying: WASD/QE steer the camera then.)
        if response.hovered()
            && self.kind == ViewKind::Scene
            && !self.fly
            && !ui.ctx().egui_wants_keyboard_input()
        {
            use egui::Key;
            let (w, e, r, f, g, n1, n3, n5, n7, ctrl) = ui.input(|i| {
                (
                    i.key_pressed(Key::W),
                    i.key_pressed(Key::E),
                    i.key_pressed(Key::R),
                    i.key_pressed(Key::F),
                    i.key_pressed(Key::G),
                    i.key_pressed(Key::Num1),
                    i.key_pressed(Key::Num3),
                    i.key_pressed(Key::Num5),
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
            // Numpad-style views (Ctrl: the opposite side); 5 switches the
            // projection. egui reads the numpad digits as the top-row ones.
            for (pressed, front, back) in [(n1, 2, 5), (n3, 0, 3), (n7, 1, 4)] {
                if pressed {
                    let (yaw, pitch) =
                        gizmo::axis_view(if ctrl { back } else { front }, self.cam.yaw);
                    self.cam.view_from(yaw, pitch);
                }
            }
            if n5 {
                self.cam.ortho = !self.cam.ortho;
            }
        }
        actions
    }

    /// Console budget panel (top right of the Game view).
    fn budget_overlay(
        &self,
        painter: &egui::Painter,
        avail: Rect,
        budgets: Option<&crate::budget::Budgets>,
        tmem: Option<&std::collections::BTreeMap<u32, Option<u32>>>,
        audio: Option<&crate::audio::Totals>,
        memory: Option<(&[trino_asset_pipeline::memory::MemoryPlan], &Scene)>,
    ) -> Option<Rect> {
        let caps = *self.renderer.caps();
        let mut rows = crate::budget::rows(&self.stats.frame, &caps, &self.stats.lod);
        // The scene budget is the scene camera's: not while playing.
        let budgets = budgets.filter(|_| self.play.is_none());
        let target = self.target();
        if let Some(b) = budgets {
            let n64 = if target.base() == Target::N64 {
                target
            } else {
                Target::N64
            };
            crate::budget::use_rsp_split(&mut rows, crate::budget::budget_of(b, n64));
        }
        // The console's two tiers side by side (issue #54): the scene on
        // the base console and on its expanded tier, then the active tier's
        // RAM and the hardware it assumes.
        let tiers = budgets.and_then(|b| crate::budget::tier_table(b, target));
        let tier_lines: Vec<(String, Color32)> = budgets
            .zip(tiers.as_ref())
            .map(|(b, table)| {
                let share = table.ram[usize::from(self.tier.is_expanded())];
                let ram =
                    trino_scene::budget::ram_line(crate::budget::budget_of(b, target)).map(|t| {
                        let color = if share > 1.0 {
                            c::ERROR
                        } else if share > 0.75 {
                            c::WARN
                        } else {
                            c::TEXT_WEAK
                        };
                        (t, color)
                    });
                let hardware =
                    trino_scene::budget::hardware_line(target).map(|t| (t, c::TEXT_FAINT));
                ram.into_iter().chain(hardware).collect()
            })
            .unwrap_or_default();
        // What the scene's groups take of the console's RAM left for assets
        // (issue #56): the Memory tab's bar for this tier, in one line.
        let console = match self.profile {
            SimProfile::N64 => Some(trino_asset_pipeline::Platform::N64),
            SimProfile::N3ds => Some(trino_asset_pipeline::Platform::N3ds),
            _ => None,
        };
        let assets_ram = memory.zip(console).and_then(|((plans, scene), console)| {
            let plan = plans
                .iter()
                .find(|p| p.platform == console && p.tier == self.tier)?;
            let groups = trino_asset_pipeline::groups::scene_groups(scene);
            let refs: Vec<&str> = groups.iter().map(String::as_str).collect();
            let (line, level) = crate::budget::assets_ram_line(target.label(), plan, &refs);
            let color = match level {
                crate::budget::Level::Ok => c::TEXT_WEAK,
                crate::budget::Level::Near => c::WARN,
                crate::budget::Level::Over => c::ERROR,
            };
            Some((line, color))
        });
        let tier_lines: Vec<(String, Color32)> = tier_lines.into_iter().chain(assets_ram).collect();
        let tier_h = 15.0;
        let summary = self.stats.lod.summary();
        let row_h = 18.0;
        // Per platform: the N64 frame (CPU vs RSP, why draws miss the fast
        // path), the 3DS triangles, and draws sent for nothing.
        let tmem = |id: trino_core::SpriteId| tmem.and_then(|t| t.get(&id.0).copied().flatten());
        // Simulating a browser: first what this view just drew (calls,
        // pipeline changes, GPU memory), the scene estimate after.
        let measured = (self.profile == SimProfile::Web).then(|| {
            let m = self.renderer.sprite_memory();
            (
                crate::budget::web_measured_line(
                    &self.renderer.frame_times(),
                    m.bytes,
                    self.renderer.resident_mesh_bytes(),
                ),
                crate::budget::Level::Ok,
            )
        });
        // The profile's audio (issue #53): storage, RAM, streams.
        let platform = match self.profile {
            SimProfile::N64 => trino_asset_pipeline::Platform::N64,
            SimProfile::N3ds => trino_asset_pipeline::Platform::N3ds,
            SimProfile::Web => trino_asset_pipeline::Platform::Web,
            _ => trino_asset_pipeline::Platform::Pc,
        };
        let audio = audio.map(|t| {
            let pt = t.of(platform);
            (
                crate::audio::budget_line(pt, platform),
                if pt.errors > 0 {
                    crate::budget::Level::Over
                } else {
                    crate::budget::Level::Ok
                },
            )
        });
        // The PC's and the Web's quality (#55): preset, the 3D's live scale.
        let quality = self.quality_settings().map(|q| {
            (
                crate::budget::quality_line(&q, self.render_scale()),
                crate::budget::Level::Ok,
            )
        });
        let platform_lines = measured
            .into_iter()
            .chain(quality)
            .chain(audio)
            .chain(
                budgets
                    .map(|b| crate::budget::platform_lines_on(b, &tmem, self.target()))
                    .unwrap_or_default(),
            )
            .map(|(t, l)| {
                let color = match l {
                    crate::budget::Level::Ok if t.starts_with("N64") => c::OK,
                    crate::budget::Level::Ok => c::TEXT_WEAK,
                    crate::budget::Level::Near => c::WARN,
                    crate::budget::Level::Over => c::ERROR,
                };
                (t, color)
            });
        let platform_lines: Vec<(String, Color32)> =
            tier_lines.into_iter().chain(platform_lines).collect();
        // As wide as the longest platform line (the PC GPU and 3DS lines
        // name a reason), at least 210 px and never past the view.
        let font = egui::FontId::proportional(10.5);
        let mono = egui::FontId::monospace(10.5);
        let text_w = |t: &str, font: &egui::FontId| {
            painter
                .layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE)
                .size()
                .x
        };
        // The tier table's columns: its labels, then each tier's values
        // under its name.
        let columns = tiers.as_ref().map(|table| {
            let label = table
                .rows
                .iter()
                .map(|r| text_w(r.0, &font))
                .fold(0.0f32, f32::max);
            let col = |i: usize| {
                table
                    .rows
                    .iter()
                    .map(|r| text_w(if i == 0 { &r.1 } else { &r.2 }, &mono))
                    .chain([text_w(table.targets[i].label(), &font)])
                    .fold(0.0f32, f32::max)
            };
            (label, col(0), col(1))
        });
        let widest = platform_lines
            .iter()
            .map(|(t, _)| t.as_str())
            .chain(summary.as_deref())
            .map(|t| text_w(t, &font))
            .chain(columns.map(|(l, a, b)| l + a + b + 24.0))
            .fold(0.0f32, f32::max);
        let w = crate::budget::hud_width(widest, avail.width());
        let table_h = tiers
            .as_ref()
            .map_or(0.0, |t| (t.rows.len() + 1) as f32 * tier_h + 4.0);
        let extra = table_h
            + if summary.is_some() { row_h } else { 0.0 }
            + platform_lines.len() as f32 * row_h;
        let rect = Rect::from_min_size(
            Pos2::new(avail.max.x - w - 10.0, avail.min.y + 50.0),
            Vec2::new(w, 24.0 + rows.len() as f32 * row_h + extra),
        );
        painter.rect_filled(rect, CornerRadius::same(6), c::BG1.gamma_multiply(0.88));
        painter.text(
            rect.min + Vec2::new(8.0, 6.0),
            egui::Align2::LEFT_TOP,
            format!("{} budget", self.target_label()),
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
        let mut y = rect.max.y - extra + 2.0;
        if let Some((table, (_, _, b))) = tiers.as_ref().zip(columns) {
            // Right-aligned value columns, the active tier's brighter.
            let right = [rect.max.x - 8.0 - b - 12.0, rect.max.x - 8.0];
            for (i, t) in table.targets.iter().enumerate() {
                painter.text(
                    Pos2::new(right[i], y),
                    egui::Align2::RIGHT_TOP,
                    t.label(),
                    font.clone(),
                    if *t == target {
                        c::ACCENT_TEXT
                    } else {
                        c::TEXT_FAINT
                    },
                );
            }
            y += tier_h;
            for (label, base, expanded) in &table.rows {
                painter.text(
                    Pos2::new(rect.min.x + 8.0, y),
                    egui::Align2::LEFT_TOP,
                    *label,
                    font.clone(),
                    c::TEXT_WEAK,
                );
                for (i, v) in [base, expanded].into_iter().enumerate() {
                    painter.text(
                        Pos2::new(right[i], y),
                        egui::Align2::RIGHT_TOP,
                        v,
                        mono.clone(),
                        if table.targets[i] == target {
                            c::TEXT
                        } else {
                            c::TEXT_WEAK
                        },
                    );
                }
                y += tier_h;
            }
            y += 4.0;
        }
        // The platform lines' area: clickable (they come from the scene's
        // budget, whose Inspector section names each reason's entities).
        let lines = (!platform_lines.is_empty()).then(|| {
            Rect::from_min_size(
                Pos2::new(rect.min.x, y),
                Vec2::new(w, platform_lines.len() as f32 * row_h),
            )
        });
        for (text, color) in platform_lines {
            painter.text(
                Pos2::new(rect.min.x + 8.0, y),
                egui::Align2::LEFT_TOP,
                text,
                egui::FontId::proportional(10.5),
                color,
            );
            y += row_h;
        }
        if let Some(text) = summary {
            painter.text(
                Pos2::new(rect.min.x + 8.0, y),
                egui::Align2::LEFT_TOP,
                text,
                egui::FontId::proportional(10.5),
                c::TEXT_WEAK,
            );
        }
        lines
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
                            // The AO menu, when the toolbar has room for it and
                            // the two buttons after it (narrow windows: View ›
                            // Ambient occlusion).
                            let room = ui.available_width() >= 3.0 * 33.0 + 12.0;
                            let color = if self.ao_view == AoView::Off {
                                c::TEXT_WEAK
                            } else {
                                c::ACCENT_TEXT
                            };
                            let menu = room.then(|| {
                                ui.menu_button(
                                    RichText::new(icon::CIRCLE_HALF_TILT)
                                        .size(15.0)
                                        .color(color),
                                    |ui| {
                                        if ao_menu(ui, &mut self.ao_view) {
                                            self.rendered = None;
                                        }
                                    },
                                )
                            });
                            if let Some(menu) = menu {
                                menu.response.on_hover_text(format!(
                                    "Ambient occlusion: {} ({})",
                                    ao_view_label(self.ao_view),
                                    if self.stats.ao {
                                        "drawn in this view"
                                    } else {
                                        "not drawn: the scene has none, or it is off"
                                    }
                                ));
                            }
                            // The shadows' menu, with room for it too
                            // (else View › Shadow view).
                            let room = ui.available_width() >= 3.0 * 33.0 + 12.0;
                            let color = if self.shadow_view == ShadowView::Off {
                                c::TEXT_WEAK
                            } else {
                                c::ACCENT_TEXT
                            };
                            let menu = room.then(|| {
                                ui.menu_button(
                                    RichText::new(icon::SUN_HORIZON).size(15.0).color(color),
                                    |ui| {
                                        if shadow_menu(ui, &mut self.shadow_view) {
                                            self.rendered = None;
                                        }
                                    },
                                )
                            });
                            if let Some(menu) = menu {
                                menu.response
                                    .on_hover_text(shadow_hover(self.shadow_view, &self.stats));
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
                            let (mut profile, mut tier) = (self.profile, self.tier);
                            // Both consoles' expanded tiers when the view is
                            // wide enough, else the simulated console's.
                            let every_tier = avail.width() >= TOOLBAR_EVERY_TIER;
                            for (p, name) in [
                                (SimProfile::N64, "N64"),
                                (SimProfile::N3ds, "3DS"),
                                (SimProfile::Native, "PC"),
                                (SimProfile::Web, "Web"),
                            ] {
                                let on = self.profile == p && !self.tier.is_expanded();
                                if theme::toggle_icon(
                                    ui,
                                    on,
                                    name,
                                    &format!("Simulate {name} output"),
                                )
                                .clicked()
                                {
                                    (profile, tier) = (p, HwTier::Base);
                                }
                                // The console's expanded tier (issue #54), after it.
                                if p.has_tiers() && (every_tier || self.profile == p) {
                                    let on = self.profile == p && self.tier.is_expanded();
                                    if tier_toggle(ui, p, on).clicked() {
                                        (profile, tier) = (p, HwTier::Expanded);
                                    }
                                }
                            }
                            if (profile, tier) != (self.profile, self.tier) {
                                self.set_profile_tier(profile, tier, input.project);
                            }
                            if self.profile == SimProfile::Web {
                                let mut canvas = self.web_canvas;
                                egui::ComboBox::from_id_salt("web-canvas")
                                    .selected_text(canvas.label())
                                    .width(96.0)
                                    .show_ui(ui, |ui| {
                                        for c in WebCanvas::ALL {
                                            ui.selectable_value(&mut canvas, c, c.label());
                                        }
                                    })
                                    .response
                                    .on_hover_text(
                                        "The browser canvas: a page of this size, or the panel \
                                         itself (a canvas that fills its page)",
                                    );
                                if canvas != self.web_canvas {
                                    self.web_canvas = canvas;
                                    self.rendered = None;
                                }
                            }
                            if self.profile.is_native() {
                                let mut preset = self.quality;
                                let r = egui::ComboBox::from_id_salt("game-quality")
                                    .selected_text(preset.label())
                                    .width(72.0)
                                    .show_ui(ui, |ui| {
                                        for p in QualityPreset::NAMED {
                                            ui.selectable_value(&mut preset, p, p.label());
                                        }
                                    })
                                    .response
                                    .on_hover_text(
                                        "The quality preset the PC and the Web draw with (the \
                                         Quality tab says what each means)",
                                    );
                                r.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::WidgetType::ComboBox,
                                        true,
                                        "Game view quality",
                                    )
                                });
                                if preset != self.quality {
                                    self.quality = preset;
                                    // The preset's own choice until toggled.
                                    self.dynres = input.quality.is_some_and(|t| {
                                        t.get(preset).dynamic_resolution.is_some()
                                    });
                                }
                                let r = theme::toggle_icon(
                                    ui,
                                    self.dynres,
                                    icon::ARROWS_OUT_SIMPLE,
                                    "Dynamic resolution: the 3D's scale follows the frame time",
                                );
                                let on = self.dynres;
                                r.widget_info(|| {
                                    egui::WidgetInfo::selected(
                                        egui::WidgetType::Checkbox,
                                        true,
                                        on,
                                        "Dynamic resolution",
                                    )
                                });
                                if r.clicked() {
                                    self.dynres = !self.dynres;
                                }
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

/// Game views at least this wide show both consoles' expanded tiers in the
/// profile picker (narrower: the simulated console's only).
const TOOLBAR_EVERY_TIER: f32 = 560.0;

/// The profile picker's toggle for `profile`'s expanded tier: `+Pak` (the
/// N64 + Expansion Pak) or `New` (the New 3DS), named in full for screen
/// readers and the UI tests.
fn tier_toggle(ui: &mut egui::Ui, profile: SimProfile, on: bool) -> egui::Response {
    let target = target_of(profile).with_tier(HwTier::Expanded);
    let (glyph, why) = match profile {
        SimProfile::N64 => ("+Pak", "8 MB of RAM, the same CPU and RSP"),
        _ => ("New", "804 MHz, 2 cores: twice the crowds and particles"),
    };
    let name = target.long_label();
    let r = theme::toggle_icon(ui, on, glyph, &format!("Simulate {name} output: {why}"));
    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, name));
    r
}

/// The Scene view's ambient occlusion modes, as radio buttons (toolbar and
/// View menu). Returns true when the mode changed.
pub fn ao_menu(ui: &mut egui::Ui, view: &mut AoView) -> bool {
    let before = *view;
    for v in [AoView::Game, AoView::On, AoView::Off, AoView::Mask] {
        if ui.radio_value(view, v, ao_view_label(v)).clicked() {
            ui.close();
        }
    }
    *view != before
}

/// The Scene view's shadow modes, as radio buttons (toolbar and View
/// menu). Returns true when the mode changed.
pub fn shadow_menu(ui: &mut egui::Ui, view: &mut ShadowView) -> bool {
    let before = *view;
    for v in ShadowView::ALL {
        if ui
            .radio_value(view, v, format!("Shadows: {}", v.label().to_lowercase()))
            .clicked()
        {
            ui.close();
        }
    }
    *view != before
}

/// The shadow button's hover: the mode, the tier drawn and what cast.
fn shadow_hover(view: ShadowView, stats: &Stats) -> String {
    let s = &stats.frame.shadows;
    if stats.shadow_tier == trino_core::ShadowTier::None {
        return format!(
            "Shadows: {} (none drawn: the scene has none, or they are off)",
            view.label().to_lowercase()
        );
    }
    format!(
        "Shadows: {} — {} tier: {} blobs, {} planar, {} in the map",
        view.label().to_lowercase(),
        stats.shadow_tier.label(),
        s.blobs,
        s.planar,
        s.mapped
    )
}

pub fn ao_view_label(view: AoView) -> &'static str {
    match view {
        AoView::Game => "As the scene sets it",
        AoView::On => "Always on (preview)",
        AoView::Off => "Off",
        AoView::Mask => "AO only (mask)",
    }
}

/// Nearest entity under `pos`: model boxes (ray vs OBB) and marker dots.
/// The Scene view's fog: as if the orbit eye were never farther than the
/// default orbit distance from its target, so framing a big object (a 30 m
/// river) doesn't back the eye into the fog and paint the view the fog's
/// color. The Game view keeps the scene's fog as is.
pub(crate) fn scene_view_fog(fog: Fog, dist: f32) -> Fog {
    if fog.far <= fog.near {
        return fog;
    }
    let extra = (dist - gizmo::DEFAULT_DIST).max(0.0);
    Fog {
        near: fog.near + extra,
        far: fog.far + extra,
        ..fog
    }
}

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

/// The Scene view's shadow debug: each receiver's rectangle (where blob
/// and planar shadows land) and each caster's bounds, in the debug color
/// its shadows draw in.
fn shadow_debug(painter: &egui::Painter, proj: &Proj, input: &ViewInput) {
    let d = ShadowView::DEBUG_COLOR;
    let color = Color32::from_rgb(d.r, d.g, d.b);
    let stroke = Stroke::new(1.4, color);
    for plane in input.resolved.receivers() {
        // An endless ground shows its middle 40 m.
        let h = |v: f32| v.min(20.0);
        let (hx, hy) = (h(plane.half.x), h(plane.half.y));
        let corners = [(-hx, -hy), (hx, -hy), (hx, hy), (-hx, hy)]
            .map(|(x, y)| proj.project(plane.at(TVec2::new(x, y))));
        for i in 0..4 {
            if let (Some(a), Some(b)) = (corners[i], corners[(i + 1) % 4]) {
                painter.line_segment([a, b], stroke);
            }
        }
    }
    let stroke = Stroke::new(1.0, color.gamma_multiply(0.6));
    for item in &input.resolved.items {
        if !item.params.shadow.casts() {
            continue;
        }
        if let Some((id, _)) = &item.model
            && let Some(m) = input.project.model_by_id(*id)
        {
            for [a, b] in gizmo::box_lines(proj, &item.matrix, m.min, m.max) {
                painter.line_segment([a, b], stroke);
            }
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
pub(crate) fn grid_tmdl() -> Vec<u8> {
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

#[cfg(test)]
mod web_canvas_tests {
    use super::WebCanvas;

    /// The Game view renders what a browser would for its canvas: the
    /// output over the auto pixel scale (`sim::native_internal`).
    #[test]
    fn presets_render_the_browsers_picture() {
        assert_eq!(WebCanvas::Hd1080.picture((10, 10)), (640, 360));
        assert_eq!(WebCanvas::Hd720.picture((10, 10)), (640, 360));
        // Responsive: the panel itself (a 1200×800 panel at 2 lines per px).
        assert_eq!(WebCanvas::Responsive.picture((1200, 800)), (600, 400));
        assert_eq!(WebCanvas::parse("720"), Some(WebCanvas::Hd720));
        assert_eq!(WebCanvas::parse("1080p"), Some(WebCanvas::Hd1080));
        assert_eq!(WebCanvas::parse("responsive"), Some(WebCanvas::Responsive));
        assert_eq!(WebCanvas::parse("4k"), None);
    }
}
