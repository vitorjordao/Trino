//! The game viewport: renders the scene through the real PC backend
//! (`PcRenderer`) into its offscreen framebuffer — on **eframe's own wgpu
//! device** — and shows that texture in an egui panel.
//!
//! Because the offscreen framebuffer has a fixed size per simulation
//! profile, the native texture is registered once per profile switch, not
//! per frame or per panel resize (re-registering on resize is the classic
//! egui-wgpu GPU leak).

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use trino_core::{Color, Renderer, SpriteId, SpriteParams, Vec2};
use trino_platform_pc::{PcRenderer, SimProfile};
use trino_scene::Scene;

pub struct Viewport {
    render_state: RenderState,
    renderer: PcRenderer,
    texture: egui::TextureId,
    pub profile: SimProfile,
}

impl Viewport {
    pub fn new(render_state: RenderState, profile: SimProfile) -> Self {
        let device = wgpu::Device::clone(&render_state.device);
        let queue = wgpu::Queue::clone(&render_state.queue);
        let renderer = PcRenderer::with_device(device, queue, profile);
        let texture = render_state.renderer.write().register_native_texture(
            &render_state.device,
            renderer.offscreen_view(),
            wgpu::FilterMode::Nearest,
        );
        Viewport {
            render_state,
            renderer,
            texture,
            profile,
        }
    }

    /// Switch console-simulation profile: rebuild the renderer (new internal
    /// resolution) and swap the registered texture, freeing the old one.
    pub fn set_profile(&mut self, profile: SimProfile) -> &mut PcRenderer {
        if profile != self.profile {
            let device = wgpu::Device::clone(&self.render_state.device);
            let queue = wgpu::Queue::clone(&self.render_state.queue);
            self.renderer = PcRenderer::with_device(device, queue, profile);
            let mut egui_renderer = self.render_state.renderer.write();
            egui_renderer.free_texture(&self.texture);
            self.texture = egui_renderer.register_native_texture(
                &self.render_state.device,
                self.renderer.offscreen_view(),
                wgpu::FilterMode::Nearest,
            );
            self.profile = profile;
        }
        &mut self.renderer
    }

    pub fn renderer_mut(&mut self) -> &mut PcRenderer {
        &mut self.renderer
    }

    /// Value for the app's `TRINO_SIM` env var.
    pub fn profile_key(&self) -> &'static str {
        match self.profile {
            SimProfile::Native => "native",
            SimProfile::N64 => "n64",
            SimProfile::N3ds => "3ds",
        }
    }

    /// Render `scene` into the offscreen framebuffer.
    pub fn render_scene(&mut self, scene: &Scene, selected: Option<usize>) {
        self.renderer.begin_frame(Color::rgb(24, 26, 40));
        for (i, entity) in scene.entities.iter().enumerate() {
            let Some(sprite) = &entity.sprite else {
                continue;
            };
            let t = &entity.transform;
            let mut tint = sprite.tint;
            // Cheap selection highlight until a gizmo lands (see ADR 0001).
            if selected == Some(i) {
                tint = [255, 255, 160, tint[3]];
            }
            self.renderer.draw_sprite(
                SpriteId(trino_core::asset_id(&sprite.path)),
                Vec2::new(t.pos[0], t.pos[1]),
                &SpriteParams {
                    scale: Vec2::new(t.scale[0], t.scale[1]),
                    rotation: t.rotation,
                    tint: Color::rgba(tint[0], tint[1], tint[2], tint[3]),
                    flip_x: sprite.flip_x,
                    flip_y: sprite.flip_y,
                },
            );
        }
        self.renderer.end_frame();
    }

    /// Draw the framebuffer into the current panel, integer-scaled and
    /// centered (letterboxed), and report the entity clicked, if any.
    pub fn show(&mut self, ui: &mut egui::Ui, scene: &Scene) -> Option<usize> {
        let (iw, ih) = self.renderer.internal_size();
        let avail = ui.available_size();
        let fit = (avail.x / iw as f32).min(avail.y / ih as f32);
        let scale = if fit >= 1.0 {
            fit.floor()
        } else {
            fit.max(0.05)
        };
        let size = egui::vec2(iw as f32 * scale, ih as f32 * scale);

        let response = ui
            .centered_and_justified(|ui| {
                ui.add(
                    egui::Image::new((self.texture, size))
                        .fit_to_exact_size(size)
                        .sense(egui::Sense::click()),
                )
            })
            .inner;

        // Click-to-select: map panel coords back to internal pixels and hit
        // the topmost sprite whose rect contains the point.
        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let origin = response.rect.center() - size * 0.5;
            let local = (pointer - origin) / scale;
            let point = Vec2::new(local.x, local.y);
            for (i, entity) in scene.entities.iter().enumerate().rev() {
                let Some(_sprite) = &entity.sprite else {
                    continue;
                };
                let t = &entity.transform;
                // Hit test against the unrotated sprite rect (32px default
                // until sprite sizes flow through the scene — good enough
                // for selection).
                let rect =
                    trino_core::Rect::new(t.pos[0], t.pos[1], 32.0 * t.scale[0], 32.0 * t.scale[1]);
                if rect.contains(point) {
                    return Some(i);
                }
            }
        }
        None
    }
}
