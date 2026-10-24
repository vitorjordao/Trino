//! The post stack on the PC's native profile and the Web (issue #64, ADR
//! 0033): the [`trino_core::post::PostTier::Full`] tier.
//!
//! When a frame has post settings (`Renderer::set_post`), its 3D draws into
//! the scene targets ([`super::upscale::SceneTargets`]) in [`HDR_FORMAT`]
//! instead of the picture, with an HDR twin of every pipeline that writes
//! the scene's color ([`HdrSet`]: built on the first such frame, or by a
//! precompile job). After the AO and the shadow map, [`PostPass`] runs the
//! bloom chain and the composite into the picture, which also stretches a
//! scaled scene onto it (the render scale's upscale, sharpening included);
//! the 2D after the frame's last 3D draw — the HUD — draws on top.
//! Without post settings none of this exists and the frame is drawn
//! exactly as before.
//!
//! Where the adapter can't render or blend `Rgba16Float` (WebGL2 without
//! `EXT_color_buffer_float`) the scene stays 8-bit and the same pass runs
//! over it: grading, vignette, grain and an LDR bloom still apply, only the
//! range past 1 is lost.

use trino_core::post::{Lut, PostSettings};

use super::upscale::SceneTargets;
use super::{
    MSAA_SAMPLES, SceneSources, ao, scene_pipelines, shadow, shadow_map, water_bgl, water_pipelines,
};

/// The scene's color while post runs.
pub(crate) const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// What the post stack shows (`PcRenderer::set_post_view`, `TRINO_POST`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PostView {
    /// As the game sets it (`Renderer::set_post`).
    #[default]
    Game,
    /// Never, whatever the game sets.
    Off,
    /// Always: the game's settings, or [`PostSettings::FILMIC`] in a frame
    /// where it set none.
    On,
    /// [`PostView::On`], showing only the bloom in place of the picture.
    Bloom,
}

impl PostView {
    /// `TRINO_POST` values: `off`, `on`, `bloom` (`game` / `auto`: Game).
    pub fn parse(s: &str) -> Option<PostView> {
        match s.to_ascii_lowercase().as_str() {
            "game" | "auto" => Some(PostView::Game),
            "off" | "0" | "false" => Some(PostView::Off),
            "on" | "1" | "true" => Some(PostView::On),
            "bloom" | "mask" => Some(PostView::Bloom),
            _ => None,
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            PostView::Game => "game",
            PostView::Off => "off",
            PostView::On => "on",
            PostView::Bloom => "bloom",
        }
    }

    /// The settings a frame runs with (`game`: what the game set) and
    /// whether it shows the bloom alone.
    pub fn resolve(self, game: Option<PostSettings>) -> Option<(PostSettings, bool)> {
        let on = || game.unwrap_or(PostSettings::FILMIC);
        match self {
            PostView::Game => game.map(|p| (p, false)),
            PostView::Off => None,
            PostView::On => Some((on(), false)),
            PostView::Bloom => {
                let mut p = on();
                p.bloom.get_or_insert(trino_core::Bloom::DEFAULT);
                Some((p, true))
            }
        }
    }
}

/// Every pipeline that writes the scene's color, again for [`HDR_FORMAT`]
/// at one sample count: the four scene pipelines, the two water ones, the
/// blob/planar shadows, the SSAO and the shadow map's composite. (The GPU
/// T&L's twin lives in `GpuTnl`, next to its own.)
pub(super) struct HdrSet {
    pub scene: [wgpu::RenderPipeline; 4],
    pub water: [wgpu::RenderPipeline; 2],
    pub shadow: shadow::ShadowPipes,
    pub ao: ao::AoPass,
    pub map: shadow_map::MapPass,
}

/// Build an [`HdrSet`] (`ms`: at [`MSAA_SAMPLES`]). `water` is groups 0..2
/// of the water layout and its single-sampled group 3.
pub(super) fn build_hdr_set(
    device: &wgpu::Device,
    sources: &SceneSources,
    water: (&[wgpu::BindGroupLayout; 3], &wgpu::BindGroupLayout),
    ms: bool,
    webgl2: bool,
) -> HdrSet {
    let samples = if ms { MSAA_SAMPLES } else { 1 };
    let [g0, g1, g2] = water.0;
    let water = if ms {
        let source = include_str!("shaders.wgsl")
            .replace("texture_depth_2d", "texture_depth_multisampled_2d");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("trino-shaders-ms-water-hdr"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        // An equal layout to the multisampled water's (wgpu deduplicates
        // them, so its bind group fits).
        let bgl = water_bgl(device, true, false);
        water_pipelines(device, &shader, [g0, g1, g2, &bgl], samples, HDR_FORMAT)
    } else {
        water_pipelines(
            device,
            &sources.shader,
            [g0, g1, g2, water.1],
            samples,
            HDR_FORMAT,
        )
    };
    HdrSet {
        scene: scene_pipelines(device, sources, samples, HDR_FORMAT),
        water,
        shadow: shadow::ShadowPipes::new(device, sources, samples, HDR_FORMAT),
        ao: ao::AoPass::new(device, HDR_FORMAT, ms, webgl2 && !ms),
        map: shadow_map::MapPass::new(device, HDR_FORMAT, ms, webgl2 && !ms),
    }
}

/// A grading LUT on the GPU: a `size`³ RGBA8 3D texture.
pub(super) struct LutTex {
    pub size: u8,
    pub view: wgpu::TextureView,
}

impl LutTex {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, lut: &Lut<'_>) -> LutTex {
        let n = lut.size() as u32;
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("trino-post-lut"),
            size: wgpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: n,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let rgba: Vec<u8> = lut
            .texels()
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|t| [t[0], t[1], t[2], 255])
            .collect();
        queue.write_texture(
            tex.as_image_copy(),
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(n * 4),
                rows_per_image: Some(n),
            },
            wgpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: n,
            },
        );
        LutTex {
            size: lut.size(),
            view: tex.create_view(&wgpu::TextureViewDescriptor::default()),
        }
    }

    /// GPU bytes of a `size` LUT.
    pub fn bytes(size: u8) -> u64 {
        (size as u64).pow(3) * 4
    }
}

pub(super) const SHADER: &str = include_str!("post.wgsl");

/// Uniform slots (each 256 bytes: every adapter's offset alignment).
const SLOT: u64 = 256;
/// The composite, the prefilter, then the down and up passes.
const SLOTS: u64 = 2 + 2 * MAX_LEVELS as u64;
/// The bloom chain's deepest level count.
pub(super) const MAX_LEVELS: u32 = 6;

/// One frame's post pass inputs.
pub(super) struct PostFrame<'a> {
    pub scene: &'a SceneTargets,
    /// The picture: its view and size.
    pub dst: (&'a wgpu::TextureView, (u32, u32)),
    pub settings: &'a PostSettings,
    pub bloom_only: bool,
    /// The quality preset's bloom: the most levels, and whether the chain
    /// starts at half the picture's size (else a quarter); `None`: no bloom.
    pub bloom_cap: Option<(u32, bool)>,
    pub lut: Option<&'a LutTex>,
    /// The upscale's sharpening (a scaled scene smaller than the picture).
    pub sharpen: f32,
    pub seed: u32,
}

/// What the pass did ([`super::PcFrameTimes`]).
#[derive(Clone, Copy, Default, Debug)]
pub(super) struct PostWork {
    pub bloom_levels: u32,
    pub upload: u32,
}

struct Level {
    size: (u32, u32),
    view: wgpu::TextureView,
}

/// The bloom chain and the composite (built on the first post frame, or
/// by a precompile job).
pub(super) struct PostPass {
    chain_format: wgpu::TextureFormat,
    blur_bgl: wgpu::BindGroupLayout,
    comp_bgl: wgpu::BindGroupLayout,
    prefilter: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    /// A black 1×1 bloom and a neutral 2³ LUT for frames without them.
    no_bloom: wgpu::TextureView,
    no_lut: wgpu::TextureView,
    /// The chain, for the base size it was made for.
    chain: Vec<Level>,
    chain_base: (u32, u32),
}

fn tex_entry(binding: u32, dim: wgpu::TextureViewDimension) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dim,
            multisampled: false,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size.0.max(1),
            height: size.1.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

fn words_to_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

impl PostPass {
    /// `hdr`: the chain in [`HDR_FORMAT`] (else 8-bit, for adapters
    /// without float targets).
    pub fn new(device: &wgpu::Device, hdr: bool) -> PostPass {
        let chain_format = if hdr {
            HDR_FORMAT
        } else {
            super::OFFSCREEN_FORMAT
        };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post.wgsl"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let blur_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-post-blur-bgl"),
            entries: &[
                tex_entry(0, wgpu::TextureViewDimension::D2),
                sampler_entry(1),
                uniform_entry(2),
            ],
        });
        let comp_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-post-comp-bgl"),
            entries: &[
                tex_entry(0, wgpu::TextureViewDimension::D2),
                tex_entry(1, wgpu::TextureViewDimension::D2),
                tex_entry(2, wgpu::TextureViewDimension::D3),
                sampler_entry(3),
                uniform_entry(4),
            ],
        });
        let layout = |bgl: &wgpu::BindGroupLayout, label: &str| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(bgl)],
                immediate_size: 0,
            })
        };
        let blur_layout = layout(&blur_bgl, "trino-post-blur-layout");
        let comp_layout = layout(&comp_bgl, "trino-post-comp-layout");
        let make = |layout: &wgpu::PipelineLayout,
                    entry: &str,
                    format: wgpu::TextureFormat,
                    blend: Option<wgpu::BlendState>| {
            super::pipes::render(
                device,
                wgpu::RenderPipelineDescriptor {
                    label: Some(entry),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_post"),
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
                },
            )
        };
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let additive = wgpu::BlendState {
            color: add,
            alpha: add,
        };
        let prefilter = make(&blur_layout, "fs_prefilter", chain_format, None);
        let down = make(&blur_layout, "fs_down", chain_format, None);
        let up = make(&blur_layout, "fs_up", chain_format, Some(additive));
        let composite = make(&comp_layout, "fs_post", super::OFFSCREEN_FORMAT, None);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("trino-post-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-post-params"),
            size: SLOT * SLOTS,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        let no_bloom = texture(device, "trino-post-no-bloom", (1, 1), chain_format, usage)
            .create_view(&wgpu::TextureViewDescriptor::default());
        let lut = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("trino-post-no-lut"),
            size: wgpu::Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 2,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage,
            view_formats: &[],
        });
        let no_lut = lut.create_view(&wgpu::TextureViewDescriptor::default());
        PostPass {
            chain_format,
            blur_bgl,
            comp_bgl,
            prefilter,
            down,
            up,
            composite,
            sampler,
            params,
            no_bloom,
            no_lut,
            chain: Vec::new(),
            chain_base: (0, 0),
        }
    }

    /// The chain for a `base`-sized first level with `levels` levels.
    fn ensure_chain(&mut self, device: &wgpu::Device, base: (u32, u32), levels: u32) {
        if self.chain_base == base && self.chain.len() as u32 >= levels {
            return;
        }
        self.chain_base = base;
        self.chain = (0..levels.max(self.chain.len() as u32).min(MAX_LEVELS))
            .map(|k| {
                let size = ((base.0 >> k).max(1), (base.1 >> k).max(1));
                let tex = texture(
                    device,
                    "trino-post-bloom",
                    size,
                    self.chain_format,
                    wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                );
                Level {
                    size,
                    view: tex.create_view(&wgpu::TextureViewDescriptor::default()),
                }
            })
            .collect();
    }

    fn blur_group(
        &self,
        device: &wgpu::Device,
        src: &wgpu::TextureView,
        slot: u64,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trino-post-blur-bg"),
            layout: &self.blur_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.params,
                        offset: slot * SLOT,
                        size: wgpu::BufferSize::new(64),
                    }),
                },
            ],
        })
    }

    fn draw(
        encoder: &mut wgpu::CommandEncoder,
        label: &str,
        target: &wgpu::TextureView,
        load: wgpu::LoadOp<wgpu::Color>,
        pipeline: &wgpu::RenderPipeline,
        group: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
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
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// The bloom chain (when the settings and the preset have one), then
    /// the composite into `frame.dst`.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: &PostFrame<'_>,
    ) -> PostWork {
        let s = frame.settings;
        let scene = frame.scene;
        let (dst, out) = frame.dst;
        let (ew, eh) = (scene.extent.0 as f32, scene.extent.1 as f32);
        let (sw, sh) = (scene.size.0 as f32, scene.size.1 as f32);
        let bloom = match (s.bloom, frame.bloom_cap) {
            (Some(b), Some((cap, half))) => Some((b, b.levels(cap), half)),
            _ => None,
        };
        let mut slots = vec![0u8; (SLOT * SLOTS) as usize];
        let mut put = |slot: u64, words: &[u32]| {
            let at = (slot * SLOT) as usize;
            let b = words_to_bytes(words);
            slots[at..at + b.len()].copy_from_slice(&b);
        };
        let f = f32::to_bits;
        let mut work = PostWork::default();
        if let Some((b, levels, half)) = bloom {
            let div = if half { 2 } else { 4 };
            let base = ((out.0 / div).max(1), (out.1 / div).max(1));
            self.ensure_chain(device, base, levels);
            let levels = levels.min(self.chain.len() as u32);
            // The prefilter: the scene's drawn corner into level 0.
            put(
                1,
                &[
                    f(1.0 / ew),
                    f(1.0 / eh),
                    f(sw / ew),
                    f(sh / eh),
                    f((sw - 0.5) / ew),
                    f((sh - 0.5) / eh),
                    f(b.threshold),
                    f(b.threshold * 0.5),
                    f(s.exposure),
                    f(1.0),
                ],
            );
            for k in 1..levels as usize {
                let src = self.chain[k - 1].size;
                put(
                    1 + k as u64,
                    &[f(1.0 / src.0 as f32), f(1.0 / src.1 as f32), f(1.0), f(1.0)],
                );
            }
            for k in 0..levels.saturating_sub(1) as usize {
                let src = self.chain[k + 1].size;
                put(
                    1 + MAX_LEVELS as u64 + k as u64,
                    &[
                        f(1.0 / src.0 as f32),
                        f(1.0 / src.1 as f32),
                        f(1.0),
                        f(1.0),
                        0,
                        0,
                        0,
                        0,
                        0,
                        f(0.5 + b.radius),
                    ],
                );
            }
            work.bloom_levels = levels;
            let comp = self.comp_words(frame, b.intensity / levels as f32);
            put(0, &comp);
            queue.write_buffer(&self.params, 0, &slots);
            work.upload = slots.len() as u32;
            let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
            let g = self.blur_group(device, &scene.color_view, 1);
            Self::draw(
                encoder,
                "trino-post-prefilter",
                &self.chain[0].view,
                clear,
                &self.prefilter,
                &g,
            );
            for k in 1..levels as usize {
                let g = self.blur_group(device, &self.chain[k - 1].view, 1 + k as u64);
                Self::draw(
                    encoder,
                    "trino-post-down",
                    &self.chain[k].view,
                    clear,
                    &self.down,
                    &g,
                );
            }
            for k in (0..levels.saturating_sub(1) as usize).rev() {
                let g = self.blur_group(
                    device,
                    &self.chain[k + 1].view,
                    1 + MAX_LEVELS as u64 + k as u64,
                );
                Self::draw(
                    encoder,
                    "trino-post-up",
                    &self.chain[k].view,
                    wgpu::LoadOp::Load,
                    &self.up,
                    &g,
                );
            }
        } else {
            let comp = self.comp_words(frame, 0.0);
            put(0, &comp);
            let b = &slots[..SLOT as usize];
            queue.write_buffer(&self.params, 0, b);
            work.upload = b.len() as u32;
        }
        let bloom_view = if bloom.is_some() {
            &self.chain[0].view
        } else {
            &self.no_bloom
        };
        let lut_view = frame.lut.map_or(&self.no_lut, |l| &l.view);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trino-post-comp-bg"),
            layout: &self.comp_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&scene.color_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(bloom_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(lut_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.params,
                        offset: 0,
                        size: wgpu::BufferSize::new(64),
                    }),
                },
            ],
        });
        Self::draw(
            encoder,
            "trino-post-composite",
            dst,
            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
            &self.composite,
            &group,
        );
        work
    }

    fn comp_words(&self, frame: &PostFrame<'_>, bloom: f32) -> [u32; 16] {
        let s = frame.settings;
        let scene = frame.scene;
        let (ew, eh) = (scene.extent.0 as f32, scene.extent.1 as f32);
        let (sw, sh) = (scene.size.0 as f32, scene.size.1 as f32);
        let f = f32::to_bits;
        let sharpen = if scene.size.0 < frame.dst.1.0 {
            frame.sharpen
        } else {
            0.0
        };
        [
            f(1.0 / ew),
            f(1.0 / eh),
            f(sw / ew),
            f(sh / eh),
            f((sw - 0.5) / ew),
            f((sh - 0.5) / eh),
            f(s.exposure),
            s.tonemap.index(),
            f(bloom),
            f(s.vignette),
            f(s.grain),
            frame.seed,
            f(frame.lut.map_or(0.0, |l| l.size as f32)),
            f(sharpen),
            frame.bloom_only as u32,
            0,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_resolve_like_the_ao_views() {
        let game = PostSettings {
            exposure: 2.0,
            ..PostSettings::DEFAULT
        };
        assert_eq!(PostView::Game.resolve(None), None);
        assert_eq!(PostView::Game.resolve(Some(game)), Some((game, false)));
        assert_eq!(PostView::Off.resolve(Some(game)), None);
        assert_eq!(
            PostView::On.resolve(None),
            Some((PostSettings::FILMIC, false))
        );
        let (b, mask) = PostView::Bloom.resolve(Some(game)).unwrap();
        assert!(mask && b.bloom.is_some() && b.exposure == 2.0);
        for v in [PostView::Game, PostView::Off, PostView::On, PostView::Bloom] {
            assert_eq!(PostView::parse(v.key()), Some(v));
        }
        assert_eq!(PostView::parse("sideways"), None);
    }
}
