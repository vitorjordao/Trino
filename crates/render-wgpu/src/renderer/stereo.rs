//! Stereoscopic 3D on the PC's 3DS profile (issue #63, ADR 0032): the
//! console's two eyes drawn exactly as the 3DS draws them (the frame's
//! top-screen calls recorded, then replayed once per eye through the
//! core's off-axis projection), and shown as a preview a flat monitor can
//! display: red/cyan anaglyph, side by side, one eye, or a "wiggle" that
//! flips between the eyes. The native profile has no stereo screen
//! ([`trino_core::Caps::stereo`]) and never records.
//!
//! The eyes land in two textures (read back by
//! [`trino_core::Renderer::read_eye_pixel`]); the preview is composed into
//! the main target, so every consumer (the window, the editor's Game view,
//! a read-back of the picture) sees it.

use trino_core::stereo::{Eye, LayerDepth, Span, StereoCmd, StereoData, StereoFrame, StereoMono};
use trino_core::{LocalLight, Ripple, ShadowPlane};

use super::OFFSCREEN_FORMAT;

/// How the PC shows a stereo frame ([`super::PcRenderer::set_stereo_view`],
/// `TRINO_STEREO`, the editor's Game view).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum StereoView {
    /// No preview: the simulated slider is down, frames are mono. A game
    /// that fixes the slider ([`trino_core::StereoSettings::slider`]) still
    /// draws both eyes; the window shows the left one.
    #[default]
    Off,
    /// Red from the left eye, green and blue from the right one (red/cyan
    /// glasses).
    Anaglyph,
    /// Both eyes squeezed into the picture, the left on the left
    /// (parallel viewing).
    SideBySide,
    /// The left eye only.
    Left,
    /// The right eye only.
    Right,
    /// The eyes in turn, every [`FLIP_FRAMES`] frames ("wiggle" stereo):
    /// depth by motion, no glasses.
    Flip,
}

/// Frames each eye stays up in [`StereoView::Flip`] (about 8 per second).
pub const FLIP_FRAMES: u32 = 8;

impl StereoView {
    pub const ALL: [StereoView; 6] = [
        StereoView::Off,
        StereoView::Anaglyph,
        StereoView::SideBySide,
        StereoView::Left,
        StereoView::Right,
        StereoView::Flip,
    ];

    /// The text form (`TRINO_STEREO`, `ui shot --stereo`).
    pub const fn key(self) -> &'static str {
        match self {
            StereoView::Off => "off",
            StereoView::Anaglyph => "anaglyph",
            StereoView::SideBySide => "side-by-side",
            StereoView::Left => "left",
            StereoView::Right => "right",
            StereoView::Flip => "flip",
        }
    }

    /// A label for menus.
    pub const fn label(self) -> &'static str {
        match self {
            StereoView::Off => "Off (mono)",
            StereoView::Anaglyph => "Anaglyph (red/cyan)",
            StereoView::SideBySide => "Side by side",
            StereoView::Left => "Left eye",
            StereoView::Right => "Right eye",
            StereoView::Flip => "Left/right flip",
        }
    }

    pub fn parse(s: &str) -> Option<StereoView> {
        let s = s.trim().to_ascii_lowercase();
        Some(match s.as_str() {
            "off" | "mono" | "0" => StereoView::Off,
            "anaglyph" | "redcyan" | "red-cyan" => StereoView::Anaglyph,
            "side-by-side" | "sbs" | "sidebyside" => StereoView::SideBySide,
            "left" => StereoView::Left,
            "right" => StereoView::Right,
            "flip" | "wiggle" => StereoView::Flip,
            _ => return None,
        })
    }

    /// `TRINO_STEREO`'s `<view>[:<slider>]` (slider 0..=1, default 1).
    pub fn parse_with_slider(s: &str) -> Option<(StereoView, f32)> {
        let (view, slider) = match s.split_once(':') {
            Some((v, k)) => (v, k.trim().parse::<f32>().ok()?),
            None => (s, 1.0),
        };
        Some((Self::parse(view)?, slider.clamp(0.0, 1.0)))
    }

    /// The eye the window shows on frame `n`, or `None` for a composite.
    pub fn shown_eye(self, n: u32) -> Option<Eye> {
        match self {
            StereoView::Off | StereoView::Left => Some(Eye::Left),
            StereoView::Right => Some(Eye::Right),
            StereoView::Flip if (n / FLIP_FRAMES) % 2 == 0 => Some(Eye::Left),
            StereoView::Flip => Some(Eye::Right),
            StereoView::Anaglyph | StereoView::SideBySide => None,
        }
    }

    fn pipeline_index(self) -> Option<usize> {
        match self {
            StereoView::Anaglyph => Some(0),
            StereoView::SideBySide => Some(1),
            _ => None,
        }
    }

    /// Whether this view composes on the GPU (has a pipeline).
    pub fn composes(self) -> bool {
        self.pipeline_index().is_some()
    }
}

/// One frame's recorded top-screen calls and their arenas (the 3DS
/// backend's twin: the `Vec`s keep their capacity).
#[derive(Default)]
pub(super) struct Recording {
    pub cmds: Vec<StereoCmd>,
    text: Vec<u8>,
    lights: Vec<LocalLight>,
    ripples: Vec<Ripple>,
    planes: Vec<ShadowPlane>,
}

fn span<T: Copy>(arena: &mut Vec<T>, items: &[T]) -> Span {
    let start = arena.len() as u32;
    arena.extend_from_slice(items);
    Span {
        start,
        len: items.len() as u32,
    }
}

impl Recording {
    pub fn clear(&mut self) {
        self.cmds.clear();
        self.text.clear();
        self.lights.clear();
        self.ripples.clear();
        self.planes.clear();
    }
    pub fn text(&mut self, s: &str) -> Span {
        span(&mut self.text, s.as_bytes())
    }
    pub fn lights(&mut self, l: &[LocalLight]) -> Span {
        span(&mut self.lights, l)
    }
    pub fn ripples(&mut self, r: &[Ripple]) -> Span {
        span(&mut self.ripples, r)
    }
    pub fn planes(&mut self, p: &[ShadowPlane]) -> Span {
        span(&mut self.planes, p)
    }
    pub fn data(&self) -> StereoData<'_> {
        StereoData {
            text: &self.text,
            lights: &self.lights,
            ripples: &self.ripples,
            planes: &self.planes,
        }
    }
}

/// The renderer's per-frame stereo state.
pub(super) struct Stereo {
    pub frame: StereoFrame,
    pub recording: bool,
    pub eye: Eye,
    pub depth: LayerDepth,
    pub top_drawn: bool,
    pub rec: Recording,
}

impl Stereo {
    pub fn new() -> Stereo {
        Stereo {
            frame: StereoFrame::mono(StereoMono::Off),
            recording: false,
            eye: Eye::Mono,
            depth: LayerDepth::SCREEN,
            top_drawn: false,
            rec: Recording::default(),
        }
    }

    pub fn begin(&mut self) {
        self.frame = StereoFrame::mono(StereoMono::Off);
        self.recording = false;
        self.eye = Eye::Mono;
        self.depth = LayerDepth::SCREEN;
        self.top_drawn = false;
        self.rec.clear();
    }

    /// This eye's 2D offset, pixels.
    pub fn shift_2d(&self) -> f32 {
        self.depth.shift_px(self.eye, self.frame.strength)
    }
}

pub(super) const SHADER: &str = r#"
struct BlitOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_stereo(@builtin(vertex_index) vi: u32) -> BlitOut {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: BlitOut;
    out.clip = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

@group(0) @binding(0) var t_left: texture_2d<f32>;
@group(0) @binding(1) var t_right: texture_2d<f32>;

@fragment
fn fs_anaglyph(in: BlitOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.clip.xy);
    let l = textureLoad(t_left, p, 0);
    let r = textureLoad(t_right, p, 0);
    return vec4<f32>(l.r, r.g, r.b, 1.0);
}

@fragment
fn fs_side_by_side(in: BlitOut) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(t_left));
    let p = vec2<i32>(in.clip.xy);
    let half = size.x / 2;
    // Each eye squeezed to half the width: the average of its two texels.
    var x = p.x * 2;
    if (p.x >= half) {
        x = (p.x - half) * 2;
    }
    let a = vec2<i32>(min(x, size.x - 1), p.y);
    let b = vec2<i32>(min(x + 1, size.x - 1), p.y);
    if (p.x < half) {
        return (textureLoad(t_left, a, 0) + textureLoad(t_left, b, 0)) * 0.5;
    }
    return (textureLoad(t_right, a, 0) + textureLoad(t_right, b, 0)) * 0.5;
}
"#;

/// The eye textures (the size of the main target).
struct Eyes {
    size: (u32, u32),
    left: wgpu::Texture,
    right: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

/// The compose pass: one pipeline per composing view and the eyes'
/// textures, built on first use.
pub(super) struct StereoPass {
    bgl: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    pipelines: [Option<wgpu::RenderPipeline>; 2],
    eyes: Option<Eyes>,
}

impl StereoPass {
    pub fn new(device: &wgpu::Device) -> StereoPass {
        let entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-stereo-bgl"),
            entries: &[entry(0), entry(1)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-stereo-layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        StereoPass {
            bgl,
            layout,
            pipelines: [None, None],
            eyes: None,
        }
    }

    /// The layout every view's pipeline is built on (precompile jobs).
    pub fn layout(&self) -> &wgpu::PipelineLayout {
        &self.layout
    }

    pub fn has(&self, view: StereoView) -> bool {
        view.pipeline_index()
            .is_none_or(|i| self.pipelines[i].is_some())
    }

    /// A pipeline a precompile job built (kept if one is already there).
    pub fn install(&mut self, view: StereoView, pipeline: wgpu::RenderPipeline) {
        if let Some(i) = view.pipeline_index() {
            self.pipelines[i].get_or_insert(pipeline);
        }
    }

    /// The eye textures for a `size` target (made or remade).
    fn eyes(&mut self, device: &wgpu::Device, size: (u32, u32)) -> &Eyes {
        if self.eyes.as_ref().is_none_or(|e| e.size != size) {
            let make = |label| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: OFFSCREEN_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
            };
            let (left, right) = (make("trino-stereo-left"), make("trino-stereo-right"));
            let lv = left.create_view(&wgpu::TextureViewDescriptor::default());
            let rv = right.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("trino-stereo-bg"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&lv),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&rv),
                    },
                ],
            });
            self.eyes = Some(Eyes {
                size,
                left,
                right,
                bind_group,
            });
        }
        self.eyes.as_ref().expect("just made")
    }

    /// The texture holding `eye` (`Left` / `Right`) of the last stereo
    /// frame.
    pub fn eye_texture(&self, eye: Eye) -> Option<&wgpu::Texture> {
        let e = self.eyes.as_ref()?;
        match eye {
            Eye::Right => Some(&e.right),
            _ => Some(&e.left),
        }
    }

    /// Copy the main target (`size`) into `eye`'s texture.
    pub fn store_eye(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        size: (u32, u32),
        eye: Eye,
    ) {
        let eyes = self.eyes(device, size);
        let dst = if eye == Eye::Right {
            &eyes.right
        } else {
            &eyes.left
        };
        encoder.copy_texture_to_texture(target.as_image_copy(), dst.as_image_copy(), extent(size));
    }

    /// Write the preview of the stored eyes into the main target: one eye
    /// copied, or a composite drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn compose(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        target_view: &wgpu::TextureView,
        size: (u32, u32),
        view: StereoView,
        frame_no: u32,
    ) {
        if let Some(eye) = view.shown_eye(frame_no) {
            let eyes = self.eyes(device, size);
            let src = if eye == Eye::Right {
                &eyes.right
            } else {
                &eyes.left
            };
            encoder.copy_texture_to_texture(
                src.as_image_copy(),
                target.as_image_copy(),
                extent(size),
            );
            return;
        }
        let Some(i) = view.pipeline_index() else {
            return;
        };
        let bind_group = self.eyes(device, size).bind_group.clone();
        let layout = &self.layout;
        let pipeline = self.pipelines[i]
            .get_or_insert_with(|| build_pipeline(device, layout, view))
            .clone();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("trino-stereo-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn extent(size: (u32, u32)) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: size.0,
        height: size.1,
        depth_or_array_layers: 1,
    }
}

/// The compose pipeline of a composing `view` on `layout`.
pub(super) fn build_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    view: StereoView,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("trino-stereo"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let fs = match view {
        StereoView::SideBySide => "fs_side_by_side",
        _ => "fs_anaglyph",
    };
    crate::renderer::pipes::render(
        device,
        wgpu::RenderPipelineDescriptor {
            label: Some("trino-stereo-pipeline"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_stereo"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(fs),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OFFSCREEN_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_parse_their_keys_and_a_slider() {
        for v in StereoView::ALL {
            assert_eq!(StereoView::parse(v.key()), Some(v));
        }
        assert_eq!(
            StereoView::parse_with_slider("anaglyph:0.5"),
            Some((StereoView::Anaglyph, 0.5))
        );
        assert_eq!(
            StereoView::parse_with_slider("sbs"),
            Some((StereoView::SideBySide, 1.0))
        );
        assert_eq!(StereoView::parse_with_slider("x"), None);
        assert_eq!(StereoView::parse_with_slider("left:abc"), None);
    }

    #[test]
    fn flip_alternates_every_few_frames() {
        let v = StereoView::Flip;
        assert_eq!(v.shown_eye(0), Some(Eye::Left));
        assert_eq!(v.shown_eye(FLIP_FRAMES - 1), Some(Eye::Left));
        assert_eq!(v.shown_eye(FLIP_FRAMES), Some(Eye::Right));
        assert_eq!(v.shown_eye(2 * FLIP_FRAMES), Some(Eye::Left));
        assert_eq!(StereoView::Anaglyph.shown_eye(3), None);
        assert!(StereoView::SideBySide.composes() && !StereoView::Off.composes());
    }
}
