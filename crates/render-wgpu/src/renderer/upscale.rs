//! The render scale's targets and upscale pass (issue #55, ADR 0022 §3).
//!
//! With a render scale other than 1 the native profile draws the frame's 3D
//! (everything up to its last 3D draw) into [`SceneTargets`] at the scaled
//! size, runs the AO there, then [`Upscale`] stretches it onto the
//! picture (bilinear, plus contrast-adaptive sharpening below a scale of
//! 1). The 2D after the last 3D draw — the HUD — draws on the picture at
//! its own resolution. At a scale of 1 none of this exists and the frame
//! is drawn exactly as before.
//!
//! Under dynamic resolution the targets are allocated once, at the largest
//! size its range reaches, and the 3D draws into their top-left corner at
//! the current size: a new allocation per step stalled the frame it landed
//! on (+25 ms with 4× MSAA on llvmpipe).

use super::OFFSCREEN_FORMAT;

/// The 3D's own color and depth: allocated at `extent`, drawn at `size`
/// (its top-left corner; the same unless dynamic resolution moves it).
pub(super) struct SceneTargets {
    pub size: (u32, u32),
    pub extent: (u32, u32),
    pub color_view: wgpu::TextureView,
    pub depth_tex: wgpu::Texture,
    pub depth_view: wgpu::TextureView,
    /// The upscale's bind group over `color_view` (built on first use).
    pub bind_group: Option<wgpu::BindGroup>,
}

impl SceneTargets {
    pub fn new(device: &wgpu::Device, extent: (u32, u32), size: (u32, u32)) -> SceneTargets {
        let alloc = extent;
        let extent = wgpu::Extent3d {
            width: alloc.0,
            height: alloc.1,
            depth_or_array_layers: 1,
        };
        let color_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("trino-scene-color"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("trino-scene-depth"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            // The AO reads it; the per-pixel water copies it.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        SceneTargets {
            size,
            extent: alloc,
            color_view: view(&color_tex),
            depth_view: view(&depth_tex),
            depth_tex,
            bind_group: None,
        }
    }
}

/// The upscale pipeline (built on the first scaled frame).
pub(super) struct Upscale {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
}

pub(super) const SHADER: &str = include_str!("upscale.wgsl");

impl Upscale {
    pub fn new(device: &wgpu::Device) -> Upscale {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("upscale.wgsl"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-upscale-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-upscale-layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("trino-upscale-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_upscale"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_upscale"),
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
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("trino-upscale-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-upscale-params"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Upscale {
            pipeline,
            bgl,
            sampler,
            params,
        }
    }

    /// Stretch `scene`'s color onto `dst` (the picture), sharpened by
    /// `sharpen` (0..=1) when the scene is smaller than the picture.
    /// Returns the bytes uploaded.
    pub fn encode(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &mut SceneTargets,
        dst: (&wgpu::TextureView, (u32, u32)),
        sharpen: f32,
    ) -> u32 {
        let (view, size) = dst;
        let sharpen = if scene.size.0 < size.0 { sharpen } else { 0.0 };
        // UVs over the drawn corner, kept half a texel inside it (beyond
        // it is an older frame's).
        let (w, h) = (scene.extent.0 as f32, scene.extent.1 as f32);
        let (sw, sh) = (scene.size.0 as f32, scene.size.1 as f32);
        let words = [
            1.0 / w,
            1.0 / h,
            sharpen,
            0.0,
            sw / w,
            sh / h,
            (sw - 0.5) / w,
            (sh - 0.5) / h,
        ];
        let mut bytes = [0u8; 32];
        for (i, v) in words.iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        queue.write_buffer(&self.params, 0, &bytes);
        let bg = scene.bind_group.get_or_insert_with(|| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("trino-upscale-bg"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&scene.color_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.params.as_entire_binding(),
                    },
                ],
            })
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("trino-upscale-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &*bg, &[]);
        pass.draw(0..3, 0..1);
        bytes.len() as u32
    }
}

/// The 3D's size for a picture of `picture` at render `scale`: rounded to
/// even pixels (centered pictures stay on whole pixels), at least 16, at
/// most `max` per side.
pub fn scene_size(picture: (u32, u32), scale: f32, max: u32) -> (u32, u32) {
    if (scale - 1.0).abs() < 1e-4 {
        return picture;
    }
    let side = |v: u32| (((v as f32 * scale).round() as u32) & !1).clamp(16, max);
    (side(picture.0), side(picture.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_sizes_round_to_even_and_clamp() {
        assert_eq!(scene_size((1920, 1080), 1.0, 8192), (1920, 1080));
        assert_eq!(scene_size((1920, 1080), 0.5, 8192), (960, 540));
        assert_eq!(scene_size((1920, 1080), 0.75, 8192), (1440, 810));
        assert_eq!(scene_size((641, 361), 0.5, 8192), (320, 180));
        assert_eq!(scene_size((20, 20), 0.25, 8192), (16, 16));
        assert_eq!(scene_size((4000, 3000), 2.0, 4096), (4096, 4096));
    }
}
