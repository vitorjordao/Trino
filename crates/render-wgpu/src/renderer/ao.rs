//! Screen-space ambient occlusion: the PC native tier of
//! [`trino_core::ao`] (ADR 0014). `ao.wgsl` has the passes; this module
//! owns their pipelines, the two R8 occlusion targets and the encoding.
//!
//! Built on the first frame that needs it (most renderers never do: the
//! console profiles, thumbnails, games without AO), and its targets again
//! after a resize.

use trino_core::AmbientOcclusion;

const AO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

/// What the AO passes show (`PcRenderer::set_ao_view`, `TRINO_AO`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AoView {
    /// As the game sets it (`Renderer::set_ambient_occlusion`).
    #[default]
    Game,
    /// Never, whatever the game sets.
    Off,
    /// Always: the game's settings, or [`AmbientOcclusion::DEFAULT`] in a
    /// frame where it set none.
    On,
    /// [`AoView::On`], showing only the occlusion (white = open, dark =
    /// occluded) in place of the 3D.
    Mask,
}

impl AoView {
    /// `TRINO_AO` values: `off`, `on`, `mask` (anything else: `Game`).
    pub fn parse(s: &str) -> Option<AoView> {
        match s.to_ascii_lowercase().as_str() {
            "game" | "auto" => Some(AoView::Game),
            "off" | "0" | "false" => Some(AoView::Off),
            "on" | "1" | "true" => Some(AoView::On),
            "mask" => Some(AoView::Mask),
            _ => None,
        }
    }

    /// The settings a frame runs with: `game` is what the game set.
    pub fn resolve(self, game: Option<AmbientOcclusion>) -> Option<(AmbientOcclusion, bool)> {
        match self {
            AoView::Game => game.map(|a| (a, false)),
            AoView::Off => None,
            AoView::On => Some((game.unwrap_or_default(), false)),
            AoView::Mask => Some((game.unwrap_or_default(), true)),
        }
    }
}

/// Everything the passes read besides the textures; layout mirrored by
/// `AoParams` in ao.wgsl.
#[derive(Clone, Copy, Debug)]
pub struct AoUniform {
    pub size: (u32, u32),
    /// Focal length in pixels (the T&L's, from the camera's vertical
    /// field of view) and the near plane of the depth `1 - near/z`.
    pub focal: f32,
    pub near: f32,
    pub ao: AmbientOcclusion,
    /// The light's fog (view depth): the occlusion fades with it.
    pub fog: (f32, f32),
    /// Show the occlusion alone instead of multiplying the frame (not a
    /// shader input: it picks the composite pipeline).
    pub mask: bool,
}

/// Pictures taller than this compute the occlusion at half resolution:
/// its cost grows faster than the pixel count (the samples spread over
/// more texels), and soft occlusion loses nothing at half the size. The
/// native profile's usual pictures (the auto pixel scale keeps them near
/// 360 lines) stay at full resolution.
pub const HALF_RES_ABOVE: u32 = 540;

impl AoUniform {
    /// Frame pixels per occlusion texel.
    pub fn scale(&self) -> u32 {
        if self.size.1 > HALF_RES_ABOVE { 2 } else { 1 }
    }

    /// The occlusion targets' size.
    pub fn ao_size(&self) -> (u32, u32) {
        let k = self.scale();
        (self.size.0.div_ceil(k), self.size.1.div_ceil(k))
    }
}

impl AoUniform {
    fn bytes(&self) -> [u8; 64] {
        let a = self.ao.clamped();
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let words: [u32; 16] = [
            w.to_bits(),
            h.to_bits(),
            (w * 0.5).to_bits(),
            (h * 0.5).to_bits(),
            self.focal.to_bits(),
            self.near.to_bits(),
            a.radius.to_bits(),
            a.intensity.to_bits(),
            a.bias.to_bits(),
            a.quality.samples(),
            self.fog.0.to_bits(),
            self.fog.1.to_bits(),
            self.scale(),
            0,
            0,
            0,
        ];
        let mut b = [0u8; 64];
        for (i, w) in words.iter().enumerate() {
            b[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        b
    }
}

/// The two occlusion targets and the bind groups reading each (with the
/// depth buffer they were made for). Allocated at `size`, drawn at most
/// that: dynamic resolution's frames use their top-left corner.
struct Targets {
    size: (u32, u32),
    raw: wgpu::TextureView,
    tmp: wgpu::TextureView,
    /// Reads `raw` (the horizontal blur, the composite) / `tmp` (the AO
    /// pass, which reads no occlusion, and the vertical blur).
    from_raw: wgpu::BindGroup,
    from_tmp: wgpu::BindGroup,
}

pub(super) struct AoPass {
    bgl: wgpu::BindGroupLayout,
    /// WebGL2's nearest sampler for the depth (`super::depth_as_float`).
    nearest: Option<wgpu::Sampler>,
    params: wgpu::Buffer,
    ao: wgpu::RenderPipeline,
    blur_h: wgpu::RenderPipeline,
    blur_v: wgpu::RenderPipeline,
    apply: wgpu::RenderPipeline,
    mask: wgpu::RenderPipeline,
    targets: Option<Targets>,
}

impl AoPass {
    /// `multisampled`: the depth it reads is the MSAA depth target (each
    /// pixel's first sample).
    pub(super) fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        multisampled: bool,
        webgl2: bool,
    ) -> AoPass {
        let source = include_str!("ao.wgsl");
        let source = if multisampled {
            source.replace("texture_depth_2d", "texture_depth_multisampled_2d")
        } else if webgl2 {
            super::depth_as_float(source)
        } else {
            source.to_string()
        };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("trino-ao"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: super::depth_sample_type(webgl2),
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ];
        // WebGL2 reads the depth through a nearest sampler (`depth_as_float`).
        if webgl2 {
            entries.push(super::nearest_sampler_entry(super::NEAREST_DEPTH_BINDING.1));
        }
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-ao-bgl"),
            entries: &entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-ao-layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = |label: &str, entry: &str, format, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_full"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // frame × occlusion; the frame's alpha stays.
        let multiply = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::Zero,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        AoPass {
            nearest: webgl2.then(|| super::nearest_sampler(device)),
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("trino-ao-params"),
                size: 64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            ao: pipeline("trino-ao", "fs_ao", AO_FORMAT, None),
            blur_h: pipeline("trino-ao-blur-h", "fs_blur_h", AO_FORMAT, None),
            blur_v: pipeline("trino-ao-blur-v", "fs_blur_v", AO_FORMAT, None),
            apply: pipeline("trino-ao-apply", "fs_apply", color_format, Some(multiply)),
            mask: pipeline("trino-ao-mask", "fs_apply", color_format, None),
            bgl,
            targets: None,
        }
    }

    /// The depth buffer was recreated (a resize): rebuild the targets on
    /// the next frame.
    pub(super) fn invalidate(&mut self) {
        self.targets = None;
    }

    fn targets(&mut self, device: &wgpu::Device, size: (u32, u32), depth: &wgpu::TextureView) {
        if let Some(t) = &self.targets
            && t.size.0 >= size.0
            && t.size.1 >= size.1
        {
            return;
        }
        // Grown, never shrunk: the larger of the two in each direction.
        let size = self
            .targets
            .as_ref()
            .map_or(size, |t| (t.size.0.max(size.0), t.size.1.max(size.1)));
        let target = |label| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: AO_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let raw = target("trino-ao-raw");
        let tmp = target("trino-ao-tmp");
        let group = |input: &wgpu::TextureView| {
            let mut entries = vec![
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(input),
                },
            ];
            if let Some(s) = &self.nearest {
                entries.push(wgpu::BindGroupEntry {
                    binding: super::NEAREST_DEPTH_BINDING.1,
                    resource: wgpu::BindingResource::Sampler(s),
                });
            }
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("trino-ao-bg"),
                layout: &self.bgl,
                entries: &entries,
            })
        };
        let from_raw = group(&raw);
        let from_tmp = group(&tmp);
        self.targets = Some(Targets {
            size,
            raw,
            tmp,
            from_raw,
            from_tmp,
        });
    }

    /// Record the passes: occlusion from `depth`, the two blurs, then the
    /// composite into `color` (multiplied, or replaced by the mask).
    /// Returns the bytes uploaded.
    pub(super) fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::TextureView,
        color: &wgpu::TextureView,
        params: &AoUniform,
    ) -> u32 {
        let ao_size = params.ao_size();
        self.targets(device, ao_size, depth);
        let bytes = params.bytes();
        queue.write_buffer(&self.params, 0, &bytes);
        let t = self.targets.as_ref().expect("built above");
        let run = |encoder: &mut wgpu::CommandEncoder,
                   pipeline: &wgpu::RenderPipeline,
                   bg: &wgpu::BindGroup,
                   target: &wgpu::TextureView,
                   (load, area): (wgpu::LoadOp<wgpu::Color>, (u32, u32))| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("trino-ao-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_viewport(0.0, 0.0, area.0 as f32, area.1 as f32, 0.0, 1.0);
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bg, &[]);
            pass.draw(0..3, 0..1);
        };
        let clear = (wgpu::LoadOp::Clear(wgpu::Color::WHITE), ao_size);
        run(encoder, &self.ao, &t.from_tmp, &t.raw, clear);
        run(encoder, &self.blur_h, &t.from_raw, &t.tmp, clear);
        run(encoder, &self.blur_v, &t.from_tmp, &t.raw, clear);
        let composite = if params.mask { &self.mask } else { &self.apply };
        let load = (wgpu::LoadOp::Load, params.size);
        run(encoder, composite, &t.from_raw, color, load);
        bytes.len() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_resolve_the_game_settings() {
        let tuned = AmbientOcclusion {
            radius: 2.0,
            ..AmbientOcclusion::DEFAULT
        };
        assert_eq!(AoView::Game.resolve(None), None);
        assert_eq!(AoView::Game.resolve(Some(tuned)), Some((tuned, false)));
        assert_eq!(AoView::Off.resolve(Some(tuned)), None);
        assert_eq!(
            AoView::On.resolve(None),
            Some((AmbientOcclusion::DEFAULT, false))
        );
        assert_eq!(AoView::On.resolve(Some(tuned)), Some((tuned, false)));
        assert_eq!(AoView::Mask.resolve(Some(tuned)), Some((tuned, true)));
        assert_eq!(AoView::parse("MASK"), Some(AoView::Mask));
        assert_eq!(AoView::parse("off"), Some(AoView::Off));
        assert_eq!(AoView::parse("what"), None);
    }

    #[test]
    fn the_uniform_matches_the_shader_layout() {
        let u = AoUniform {
            size: (640, 480),
            focal: 415.0,
            near: 0.05,
            ao: AmbientOcclusion {
                quality: trino_core::AoQuality::High,
                ..AmbientOcclusion::DEFAULT
            },
            fog: (10.0, 30.0),
            mask: false,
        };
        let b = u.bytes();
        let f = |i: usize| f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!((f(0), f(1), f(2), f(3)), (640.0, 480.0, 320.0, 240.0));
        assert_eq!((f(4), f(5)), (415.0, 0.05));
        assert_eq!(u32::from_le_bytes(b[36..40].try_into().unwrap()), 32);
        assert_eq!((f(10), f(11)), (10.0, 30.0));
        assert_eq!(u32::from_le_bytes(b[48..52].try_into().unwrap()), 1);
        assert_eq!(u.ao_size(), (640, 480));
        let tall = AoUniform {
            size: (1921, 1081),
            ..u
        };
        assert_eq!(tall.scale(), 2);
        assert_eq!(tall.ao_size(), (961, 541));
    }
}
