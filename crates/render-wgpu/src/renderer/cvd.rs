//! The color-vision simulation view (issue #60, ADR 0027): the finished
//! frame as a viewer with protanopia, deuteranopia, tritanopia or no color
//! vision sees it — Machado et al. 2009 at severity 1.0 in linear RGB, the
//! same matrices as `trino_core::access::color`. A review tool for the
//! editor, `cargo xtask access shot` and `TRINO_CVD` in a game window,
//! never part of a game's look: it is off unless asked for, and builds
//! nothing until then.
//!
//! The pass copies the frame into a scratch texture and draws it back
//! through the matrix, so every consumer (the window blit, the editor's
//! texture, a read-back) sees the simulated picture.
//!
//! The same pass draws the players' **correction filter** (daltonization,
//! issue #64: [`CvdKind::Correct`]) through `access::color::correction`'s
//! matrix: the error a viewer misses moved to the channels they see. That
//! one is a player setting (`AccessSettings::cvd_filter`), drawn before the
//! simulation view, so the simulation shows a corrected frame as its viewer
//! sees it.

use trino_core::CvdMode;
use trino_core::access::color::{correction, matrix};

/// Which matrix the pass draws through.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum CvdKind {
    /// How a viewer with the mode sees the frame (a review view).
    Simulate,
    /// The frame daltonized for that viewer (a player's filter).
    Correct,
}

use super::OFFSCREEN_FORMAT;

/// The simulation shader for `mode`: its matrix written in as constants.
#[cfg(test)]
pub(super) fn shader(mode: CvdMode) -> String {
    shader_for(mode, CvdKind::Simulate)
}

/// The shader for `mode` and `kind`.
pub(super) fn shader_for(mode: CvdMode, kind: CvdKind) -> String {
    let m = match kind {
        CvdKind::Simulate => *matrix(mode),
        CvdKind::Correct => correction(mode),
    };
    // WGSL's mat3x3 takes columns.
    let col = |c: usize| format!("vec3<f32>({:?}, {:?}, {:?})", m[0][c], m[1][c], m[2][c]);
    format!(
        "{}\nconst SIM: mat3x3<f32> = mat3x3<f32>({}, {}, {});\n",
        SHADER,
        col(0),
        col(1),
        col(2)
    )
}

pub(super) const SHADER: &str = r#"
struct BlitOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_cvd(@builtin(vertex_index) vi: u32) -> BlitOut {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: BlitOut;
    out.clip = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

@group(0) @binding(0) var t_frame: texture_2d<f32>;

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

fn to_srgb(l: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(l, vec3<f32>(1.0 / 2.4)) - 0.055, 12.92 * l, l <= vec3<f32>(0.0031308));
}

@fragment
fn fs_cvd(in: BlitOut) -> @location(0) vec4<f32> {
    let c = textureLoad(t_frame, vec2<i32>(in.clip.xy), 0);
    let lin = to_linear(clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0)));
    let seen = clamp(SIM * lin, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(to_srgb(seen), c.a);
}
"#;

struct Scratch {
    size: (u32, u32),
    tex: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

/// The pass: a pipeline per mode and a scratch texture per screen, all
/// built on first use.
pub(super) struct CvdPass {
    bgl: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    /// Per [`CvdKind`] (simulate, correct), per mode.
    pipelines: [[Option<wgpu::RenderPipeline>; 5]; 2],
    /// The main screen's and the second screen's.
    scratch: [Option<Scratch>; 2],
}

impl CvdPass {
    pub fn new(device: &wgpu::Device) -> CvdPass {
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-cvd-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-cvd-layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        CvdPass {
            bgl,
            layout,
            pipelines: Default::default(),
            scratch: [None, None],
        }
    }

    fn pipeline(
        &mut self,
        device: &wgpu::Device,
        mode: CvdMode,
        kind: CvdKind,
    ) -> &wgpu::RenderPipeline {
        let layout = &self.layout;
        self.pipelines[kind as usize][mode as usize]
            .get_or_insert_with(|| build_pipeline_for(device, layout, mode, kind))
    }

    /// The pipeline layout every mode's pipeline is built on (precompile
    /// jobs build them on workers: [`build_pipeline`]).
    pub fn layout(&self) -> &wgpu::PipelineLayout {
        &self.layout
    }

    pub fn has(&self, mode: CvdMode, kind: CvdKind) -> bool {
        self.pipelines[kind as usize][mode as usize].is_some()
    }

    /// A pipeline a precompile job built (kept if one is already there).
    pub fn install(&mut self, mode: CvdMode, kind: CvdKind, pipeline: wgpu::RenderPipeline) {
        self.pipelines[kind as usize][mode as usize].get_or_insert(pipeline);
    }

    /// Simulate (or correct for) `mode` over `target` (`size`, a
    /// `COPY_SRC` render target): copy it aside, draw it back through the
    /// matrix. `slot` 0 is the main screen, 1 the second.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        target_view: &wgpu::TextureView,
        size: (u32, u32),
        mode: CvdMode,
        kind: CvdKind,
        slot: usize,
    ) {
        if mode == CvdMode::Off || size.0 == 0 || size.1 == 0 {
            return;
        }
        if self.scratch[slot].as_ref().is_none_or(|s| s.size != size) {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("trino-cvd-scratch"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: OFFSCREEN_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("trino-cvd-bg"),
                layout: &self.bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            });
            self.scratch[slot] = Some(Scratch {
                size,
                tex,
                bind_group,
            });
        }
        let extent = wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        };
        let scratch = self.scratch[slot].as_ref().expect("just made");
        encoder.copy_texture_to_texture(
            target.as_image_copy(),
            scratch.tex.as_image_copy(),
            extent,
        );
        let bind_group = scratch.bind_group.clone();
        let pipeline = self.pipeline(device, mode, kind).clone();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("trino-cvd-pass"),
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

/// The `kind` pipeline of `mode` (not `Off`) on `layout`.
pub(super) fn build_pipeline_for(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    mode: CvdMode,
    kind: CvdKind,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("trino-cvd"),
        source: wgpu::ShaderSource::Wgsl(shader_for(mode, kind).into()),
    });
    crate::renderer::pipes::render(
        device,
        wgpu::RenderPipelineDescriptor {
            label: Some("trino-cvd-pipeline"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_cvd"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_cvd"),
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
