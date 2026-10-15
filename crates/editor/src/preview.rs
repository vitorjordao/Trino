//! A small offscreen preview with its own `PcRenderer` (and so the real PC,
//! N64 and 3DS looks): the Animation tab and the particle inspector draw
//! into one, orbited with the mouse, with a ground grid.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Vec2};
use eframe::egui_wgpu::RenderState;
use trino_core::render3d::Camera3;
use trino_core::{Color, Material, ModelId, ModelParams, Renderer, Shading, Transform3};
use trino_platform_pc::{PcRenderer, SimProfile};

use crate::gizmo::OrbitCam;
use crate::theme::c;

pub const GRID: ModelId = ModelId(trino_core::asset_id("__editor/preview-grid"));

pub struct Preview {
    render_state: RenderState,
    pub renderer: PcRenderer,
    texture: egui::TextureId,
    pub profile: SimProfile,
    size: (u32, u32),
    pub cam: OrbitCam,
    /// What was uploaded (a key the owner picks) at which project generation.
    pub uploaded: Option<(String, u64)>,
}

impl Preview {
    pub fn new(render_state: RenderState, profile: SimProfile) -> Self {
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

    /// The preview kept in `slot`, rebuilt when the look changes.
    pub fn ensure<'a>(
        slot: &'a mut Option<Preview>,
        rs: &RenderState,
        profile: SimProfile,
    ) -> &'a mut Preview {
        if slot.as_ref().is_some_and(|p| p.profile != profile) {
            *slot = None;
        }
        slot.get_or_insert_with(|| Preview::new(rs.clone(), profile))
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

    /// Lay the preview out in `rect`: mouse orbit and zoom, then one frame
    /// (`draw` between `begin_frame` and `end_frame`, the camera set), then
    /// the image (letterboxed for console looks).
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        grid: bool,
        draw: impl FnOnce(&mut PcRenderer, &Camera3),
    ) {
        let ppp = ui.ctx().pixels_per_point();
        let size = if self.profile == SimProfile::Native {
            let s = (
                ((rect.width() * ppp) as u32).max(16),
                ((rect.height() * ppp) as u32).max(16),
            );
            self.renderer.set_internal_size(s.0, s.1);
            s
        } else {
            self.renderer.internal_size()
        };
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        if resp.dragged() {
            self.cam.orbit(resp.drag_delta());
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.cam.zoom(scroll);
            }
        }
        if self.cam.step(ui.input(|i| i.stable_dt).min(0.1)) {
            ui.ctx().request_repaint();
        }
        let camera = self.cam.camera();
        let r = &mut self.renderer;
        r.begin_frame(Color::rgb(20, 22, 30));
        r.set_camera(&camera);
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
        draw(r, &camera);
        r.end_frame();
        if self.size != size {
            self.reregister();
            self.size = size;
        }
        let image = if self.profile == SimProfile::Native {
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
            self.texture,
            image,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }
}
