//! A depth-to-depth copy by drawing, for WebGL2 (issue #47): wgpu's GL
//! backend copies textures with `glCopyTexSubImage2D`, which can't read a
//! depth attachment, so the water's copy of the scene depth (its foam and
//! shallows read it) stayed empty in a browser without WebGPU. Here a
//! full-screen triangle reads each depth texel (as a float texture,
//! [`super::depth_as_float`]) and writes it as `frag_depth`.

const SHADER: &str = r#"
@group(0) @binding(0) var t_src: texture_2d<f32>;
@group(0) @binding(1) var s_src: sampler;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

struct Out {
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_copy(@builtin(position) p: vec4<f32>) -> Out {
    var out: Out;
    // Through a nearest sampler: a depth texture's own GL filter leaves it
    // incomplete (`super::depth_as_float`).
    let size = vec2<f32>(textureDimensions(t_src));
    out.depth = textureSampleLevel(t_src, s_src, p.xy / size, 0.0).r;
    return out;
}
"#;

pub(super) struct DepthBlit {
    bgl: wgpu::BindGroupLayout,
    nearest: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
}

impl DepthBlit {
    pub(super) fn new(device: &wgpu::Device) -> DepthBlit {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("trino-depth-blit"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-depth-blit-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                super::nearest_sampler_entry(1),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-depth-blit-layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("trino-depth-blit"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_copy"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        DepthBlit {
            bgl,
            nearest: super::nearest_sampler(device),
            pipeline,
        }
    }

    /// Copy `src` (a single-sampled depth view) into `dst` (a Depth32Float
    /// render attachment of the same size).
    pub(super) fn copy(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
    ) {
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trino-depth-blit-bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.nearest),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("trino-depth-blit-pass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: dst,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }
}
