//! wgpu 2D sprite renderer with an internal offscreen framebuffer.
//!
//! Two construction modes:
//! - [`PcRenderer::new_windowed`]: renders offscreen, then blits to the
//!   window surface with nearest-neighbor integer upscaling (letterboxed).
//! - [`PcRenderer::new_headless`]: offscreen only; tests read pixels back
//!   with [`PcRenderer::read_offscreen`]. This is also the path the editor
//!   viewport reuses in Fase 3 (render-to-texture).

mod ao;
mod batch;
mod cpu_tnl;
mod depth_blit;
mod gpu_tnl;
mod lights;
mod water;

use std::collections::HashMap;
use std::sync::Arc;
use web_time::Instant;

use trino_core::render3d::{
    Camera3, DEFAULT_LIGHT, Light, Mesh, MeshLayout, PcGpuReject, ScreenTri, SurfaceScratch,
    SurfaceVert, TnlContext, TnlScratch,
};
use trino_core::{
    AmbientOcclusion, AoTier, Caps, Color, FontId, FrameStats, Instance, LightList, LightTier,
    LocalLight, Mat34, Material, ModelId, ModelParams, Palette, Renderer, Ripple, Screen, SpriteId,
    SpriteParams, TexFilter, TextParams, Transform3, Vec2, WaterState, WaterTier,
};

use crate::sim::SimProfile;
pub use ao::AoView;
use batch::{
    Cmd, INSTANCE_STRIDE, Segment, TRI_VERTEX_STRIDE, TexKey, TriKind, TriVertex, build_segments,
    make_command,
};

const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[derive(Debug)]
pub enum RendererError {
    NoAdapter(String),
    Device(String),
    Surface(String),
}

impl std::fmt::Display for RendererError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RendererError::NoAdapter(e) => write!(f, "no compatible GPU adapter: {e}"),
            RendererError::Device(e) => write!(f, "failed to open GPU device: {e}"),
            RendererError::Surface(e) => write!(f, "surface error: {e}"),
        }
    }
}

impl std::error::Error for RendererError {}

struct SpriteEntry {
    bind_group: wgpu::BindGroup,
    /// Kept for the 3D (wrapping) bind group, created on first 3D use.
    view: wgpu::TextureView,
    size: (u32, u32),
    /// GPU bytes of the texture, and whether it is block-compressed
    /// ([`PcRenderer::sprite_memory`]).
    bytes: u64,
    compressed: bool,
    /// Some texel is below full alpha (it blends): a model drawn with it
    /// stays on the CPU path (`PcGpuReject::TextureAlpha`).
    alpha: bool,
}

/// A block-compressed sprite format the renderer uploads as is
/// ([`PcRenderer::upload_sprite_compressed`]); 4×4 blocks in row-major
/// order. The PC bakes BC; the Web transcodes its KTX2 textures to
/// whichever of these the browser's GPU samples (issue #47).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompressedFormat {
    /// 8 bytes a block: color with 1-bit alpha.
    Bc1,
    /// 16 bytes: BC1 color plus interpolated alpha.
    Bc3,
    /// 16 bytes: RGBA.
    Bc7,
    /// 8 bytes: ETC2 color (phones; ETC1 blocks are ETC2 blocks).
    Etc2Rgb8,
    /// 16 bytes: ETC2 color plus EAC alpha.
    Etc2Rgba8,
    /// 16 bytes: ASTC 4×4 (RGBA).
    Astc4x4,
}

impl CompressedFormat {
    fn wgpu(self) -> wgpu::TextureFormat {
        // Unorm, like the RGBA8 sprites (`OFFSCREEN_FORMAT`): colors pass
        // through unconverted.
        match self {
            CompressedFormat::Bc1 => wgpu::TextureFormat::Bc1RgbaUnorm,
            CompressedFormat::Bc3 => wgpu::TextureFormat::Bc3RgbaUnorm,
            CompressedFormat::Bc7 => wgpu::TextureFormat::Bc7RgbaUnorm,
            CompressedFormat::Etc2Rgb8 => wgpu::TextureFormat::Etc2Rgb8Unorm,
            CompressedFormat::Etc2Rgba8 => wgpu::TextureFormat::Etc2Rgba8Unorm,
            CompressedFormat::Astc4x4 => wgpu::TextureFormat::Astc {
                block: wgpu::AstcBlock::B4x4,
                channel: wgpu::AstcChannel::Unorm,
            },
        }
    }

    pub fn block_bytes(self) -> u32 {
        match self {
            CompressedFormat::Bc1 | CompressedFormat::Etc2Rgb8 => 8,
            CompressedFormat::Bc3
            | CompressedFormat::Bc7
            | CompressedFormat::Etc2Rgba8
            | CompressedFormat::Astc4x4 => 16,
        }
    }

    /// The device feature that samples it.
    pub fn feature(self) -> wgpu::Features {
        match self {
            CompressedFormat::Bc1 | CompressedFormat::Bc3 | CompressedFormat::Bc7 => {
                wgpu::Features::TEXTURE_COMPRESSION_BC
            }
            CompressedFormat::Etc2Rgb8 | CompressedFormat::Etc2Rgba8 => {
                wgpu::Features::TEXTURE_COMPRESSION_ETC2
            }
            CompressedFormat::Astc4x4 => wgpu::Features::TEXTURE_COMPRESSION_ASTC,
        }
    }

    /// A BC format (the PC's; [`PcRenderer::set_bc`] gates them).
    pub fn is_bc(self) -> bool {
        matches!(
            self,
            CompressedFormat::Bc1 | CompressedFormat::Bc3 | CompressedFormat::Bc7
        )
    }
}

/// What the device behind a renderer can't do, by backend
/// ([`PcRenderer::new_canvas`] on WebGL2). Desktop backends and WebGPU
/// have everything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BackendLimits {
    /// wgpu's GL backend in a browser: WebGL2's downlevel limits and no
    /// multisampled depth sampling (no MSAA).
    webgl2: bool,
}

/// The graphics API a browser renderer asks for ([`PcRenderer::new_canvas`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WebBackend {
    /// WebGPU when the browser gives an adapter, else WebGL2.
    #[default]
    Auto,
    WebGpu,
    WebGl2,
}

impl WebBackend {
    /// `auto`, `webgpu`, `webgl2` (`webgl` and `gl` too): the Web app's
    /// `?backend=` value.
    pub fn parse(s: &str) -> Option<WebBackend> {
        match s.to_ascii_lowercase().as_str() {
            "auto" | "" => Some(WebBackend::Auto),
            "webgpu" | "gpu" => Some(WebBackend::WebGpu),
            "webgl2" | "webgl" | "gl" => Some(WebBackend::WebGl2),
            _ => None,
        }
    }
}

/// The sprite textures a renderer holds ([`PcRenderer::sprite_memory`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpriteMemory {
    pub textures: u32,
    /// GPU bytes of every sprite texture.
    pub bytes: u64,
    /// Of those, the block-compressed ones.
    pub compressed: u32,
    pub compressed_bytes: u64,
}

struct SurfaceState {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

/// A registered font: its glyph table (validated at upload) and the direct
/// index of its codepoints below 256.
struct FontEntry {
    table: Box<[u8]>,
    index: trino_core::text::GlyphIndex,
}

impl FontEntry {
    fn font(&self) -> trino_core::text::BitmapFont<'_> {
        trino_core::text::BitmapFont::parse(&self.table)
            .expect("validated on upload")
            .with_index(&self.index)
    }
}

/// Where a PC frame's rendering time goes on the CPU (wall clock, µs), and
/// what it hands the GPU: [`PcRenderer::frame_times`] of the last frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PcFrameTimes {
    /// The software T&L of every model draw (`tessellate_with` /
    /// `tessellate_surface`).
    pub tnl_us: u32,
    /// Model batches: the painter sort and the expansion into GPU
    /// vertices.
    pub batch_us: u32,
    /// `end_frame`'s uploads, command encoding, submits and present.
    pub submit_us: u32,
    /// GPU draw calls (instanced sprite runs and triangle runs, both
    /// screens).
    pub draw_calls: u32,
    /// Model draws shaded per pixel (`tessellate_surface`: a local light,
    /// gloss or wetness reaches them); the rest take `tessellate_with`.
    pub lit_draws: u32,
    /// Bytes written to GPU buffers (sprite instances, triangle vertices,
    /// light sets, AO parameters).
    pub upload_bytes: u32,
    /// Screen-space AO passes run (0 or 1: the frame's 3D, once).
    pub ao_passes: u32,
    /// Pipeline changes between those draw calls (what a browser's
    /// WebGL2 or WebGPU pays for in JavaScript, issue #47).
    pub pipeline_switches: u32,
}

/// Microseconds since `t0`.
fn micros(t0: Instant) -> u32 {
    t0.elapsed().as_micros().min(u32::MAX as u128) as u32
}

pub struct PcRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: Option<SurfaceState>,
    offscreen_view: wgpu::TextureView,
    offscreen_tex: wgpu::Texture,
    depth_view: wgpu::TextureView,
    /// The depth target itself: copied for the water's depth foam.
    depth_tex: wgpu::Texture,
    sprite_pipeline: wgpu::RenderPipeline,
    tri_pipeline: wgpu::RenderPipeline,
    /// Same as `tri_pipeline` with clamped 3-point sampling (billboards).
    tri_clamp_pipeline: wgpu::RenderPipeline,
    /// Per-pixel lighting (`LightTier::PerPixel`): local lights, specular
    /// and wetness on top of the vertex color.
    lit_pipeline: wgpu::RenderPipeline,
    /// What the scene pipelines are built from, and the four again for a
    /// multisampled target, built by the first [`PcRenderer::set_msaa`]
    /// that turns it on: sprite, tri, tri clamp, lit. (Building them for
    /// every renderer made parallel headless tests crash RADV.)
    sources: SceneSources,
    ms_pipelines: Option<[wgpu::RenderPipeline; 4]>,
    /// Samples per pixel of the main screen's scene pass (1 = off), and
    /// its multisampled color and depth (resolved into the offscreen
    /// target at the end of the pass).
    msaa: u32,
    msaa_targets: Option<MsaaTargets>,
    /// WebGL2 (wgpu's GL backend in a browser): no multisampled depth to
    /// sample (GLSL ES 3.0 has no `sampler2DMS`), so [`PcRenderer::set_msaa`]
    /// keeps one sample, and shaders read depth as a float texture
    /// ([`depth_as_float`]).
    webgl2: bool,
    /// WebGL2's depth copy for the water (built on its first use).
    depth_blit: Option<depth_blit::DepthBlit>,
    lights_buf: wgpu::Buffer,
    lights_bg: wgpu::BindGroup,
    blit_pipeline: wgpu::RenderPipeline,
    blit_bind_group: wgpu::BindGroup,
    blit_sampler: wgpu::Sampler,
    sprite_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    globals_bg: wgpu::BindGroup,
    globals_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    instance_capacity: u64,
    tri_buf: wgpu::Buffer,
    tri_capacity: u64,
    sprites: HashMap<u32, SpriteEntry>,
    /// Sprite textures as 3D textures (base + detail layer, their samplers),
    /// by texture state.
    tex3d: HashMap<TexKey, wgpu::BindGroup>,
    tex3d_bgl: wgpu::BindGroupLayout,
    /// Wrapping (models) and clamp-to-edge (billboards: sprite regions must
    /// not bleed) samplers, nearest and linear.
    tex3d_nearest: wgpu::Sampler,
    tex3d_linear: wgpu::Sampler,
    clamp_nearest: wgpu::Sampler,
    clamp_linear: wgpu::Sampler,
    /// 1x1 white: what untextured triangles "sample" (×1 = unchanged), and
    /// a draw without a detail layer.
    white_view: wgpu::TextureView,
    white_bg: wgpu::BindGroup,
    /// What `TexFilter::Default` means (`set_texture_filter`; the profile's
    /// own: nearest native, the 3-point look on N64, bilinear on 3DS).
    tex_filter: TexFilter,
    /// TMDL blobs with their validated layout (parsed once, at register).
    meshes: HashMap<u32, (Vec<u8>, MeshLayout)>,
    /// Glyph tables (`TFNT`) by font handle; the atlas is the sprite with
    /// the same handle. `Arc` so `draw_text` can lay out while drawing.
    fonts: HashMap<u32, Arc<FontEntry>>,
    commands: Vec<Cmd>,
    /// Counters of the frame being recorded / the last finished frame.
    frame_stats: FrameStats,
    /// The N64 profile's frame as the console would split it (strict mode).
    n64: N64Split,
    last_stats: FrameStats,
    /// The native profile's draws off the GPU T&L path, per
    /// [`PcGpuReject`], of the frame being recorded / the last one.
    pc_gpu_off: [u32; PcGpuReject::COUNT],
    last_pc_gpu_off: [u32; PcGpuReject::COUNT],
    /// CPU time per rendering stage and the GPU work handed over, of the
    /// frame being recorded / the last finished frame ([`PcFrameTimes`]).
    times: PcFrameTimes,
    last_times: PcFrameTimes,
    tri_verts: Vec<TriVertex>,
    /// Triangles of the current model batch: consecutive `draw_model` calls
    /// accumulate here and depth-sort together (painter across meshes) when
    /// a sprite draw, a camera change or `end_frame` flushes the batch.
    pending_tris: Vec<ScreenTri>,
    /// Texture of each pending triangle (parallel to `pending_tris`; only
    /// allocated into when the batch has textured draws).
    pending_tex: Vec<Option<TexKey>>,
    /// Per-pixel lighting inputs of each pending triangle (parallel; only
    /// allocated into once a lit draw joins the batch).
    pending_surf: Vec<Option<SurfTri>>,
    /// This frame's local lights and wetness (`set_local_lights`,
    /// `set_wetness`; cleared at `begin_frame`), and the lighting states
    /// lit draws used (the lit pipeline's uniform).
    locals: LightList,
    wetness: f32,
    /// This frame's screen-space AO (`set_ambient_occlusion`, cleared at
    /// `begin_frame`), what shows of it (`set_ao_view`, `TRINO_AO`) and
    /// its passes (built on first use).
    ao: Option<AmbientOcclusion>,
    ao_view: AoView,
    ao_pass: Option<ao::AoPass>,
    /// The same over the multisampled depth (MSAA on).
    ao_pass_ms: Option<ao::AoPass>,
    light_sets: lights::LightSets,
    /// This frame's water clock and ripples (`set_water`, cleared at
    /// `begin_frame`), the water surfaces drawn (the water pipelines'
    /// uniform) and their GPU side.
    water_state: WaterState,
    water_sets: water::WaterSets,
    water: WaterGpu,
    /// The T&L's working memory for per-pixel lit draws.
    surf_scratch: Box<SurfaceScratch>,
    camera: Camera3,
    light: Light,
    /// T&L constants of `camera` + `light` at the current internal size
    /// (rebuilt when any of them changes).
    tnl_ctx: Option<(TnlContext, LightList)>,
    /// The T&L's working memory, reused by every draw (boxed: ~9 KB).
    tnl_scratch: Box<TnlScratch>,
    clear: Color,
    caps: Caps,
    profile: SimProfile,
    internal_size: (u32, u32),
    n64_look: bool,
    /// N64 Video Interface dedither ("restore") filter, run on the 16-bit
    /// look at `end_frame` — see [`PcRenderer::set_vi_dedither`].
    vi_dedither: bool,
    vi_pipeline: wgpu::RenderPipeline,
    vi_tex: wgpu::Texture,
    strict: bool,
    /// The 3DS profile's touch screen (320×240, 2D and 3D).
    second: Option<SecondScreen>,
    /// "name (backend)" of the GPU adapter.
    adapter_name: String,
    /// The device samples BC textures (`TEXTURE_COMPRESSION_BC`) and
    /// [`PcRenderer::set_bc`] hasn't turned them off.
    bc: bool,
    /// Where draws go now (0 main, 1 second). `set_screen(1)` renders the
    /// main screen right away and keeps its counts (commands, triangles):
    /// strict mode's budgets and the stats cover the whole frame.
    screen: u8,
    main_cmds: u32,
    main_tris: u32,
    /// The GPU T&L path (issue #51): on for the native profile unless
    /// `TRINO_PC_TNL=cpu` or [`PcRenderer::set_hw_tnl`] turned it off; its
    /// pipelines and resident meshes, built on the first GPU draw.
    hw_tnl: bool,
    gpu: Option<gpu_tnl::GpuTnl>,
    /// Group 0's layout (the GPU T&L pipelines share it).
    globals_bgl: wgpu::BindGroupLayout,
    /// The native profile's CPU draws of the current batch (tessellated at
    /// the flush, over `tnl_threads` workers), the T&L contexts they use
    /// and the workers' scratch memory.
    cpu_jobs: Vec<cpu_tnl::CpuJob>,
    job_ctxs: Vec<TnlContext>,
    workers: Vec<cpu_tnl::Scratch>,
    tnl_threads: usize,
}

/// What a lit triangle adds to its vertices: the surface, the gloss and the
/// light set (or, for water, its water set).
#[derive(Clone, Copy, Debug)]
struct SurfTri {
    v: [SurfaceVert; 3],
    gloss: f32,
    set: u32,
    /// The local lights that reach the draw (`lights::reach_mask`).
    mask: u32,
    /// A water triangle (`set` is a water set): the water pipelines, after
    /// the batch's other triangles.
    water: bool,
}

const NO_SURFACE: SurfaceVert = SurfaceVert {
    pos: trino_core::Vec3::ZERO,
    normal: trino_core::Vec3::ZERO,
    base: Color::WHITE,
};

/// The water pipelines and their group 3: the water sets and a copy of
/// the scene's depth (the per-pixel tier's foam reads it).
struct WaterGpu {
    bgl: wgpu::BindGroupLayout,
    buf: wgpu::Buffer,
    depth_copy: wgpu::Texture,
    bg: wgpu::BindGroup,
    /// WebGL2's nearest sampler for the depth copy (`depth_as_float`).
    nearest: Option<wgpu::Sampler>,
    /// `fs_water`: the per-pixel tier (native).
    per_pixel: wgpu::RenderPipeline,
    /// `fs_water_vertex`: the console tiers' combiner (N64 look, 3DS).
    vertex: wgpu::RenderPipeline,
    /// Groups 0..2 of the water layout (globals, texture, light sets), kept
    /// to build the multisampled set.
    groups: [wgpu::BindGroupLayout; 3],
    /// The same pipelines for the multisampled scene pass, and a
    /// multisampled depth copy: built by the first [`PcRenderer::set_msaa`].
    ms: Option<WaterMs>,
}

/// The water pipelines over a multisampled target (MSAA on). The foam
/// reads the multisampled depth's first sample, as the SSAO does.
struct WaterMs {
    bgl: wgpu::BindGroupLayout,
    depth_copy: wgpu::Texture,
    bg: wgpu::BindGroup,
    per_pixel: wgpu::RenderPipeline,
    vertex: wgpu::RenderPipeline,
}

/// WebGL2's copy of a shader that reads a depth texture. naga's GLSL
/// backend can't `textureLoad` a `texture_depth_2d`, so the binding becomes
/// an unfilterable float texture (WebGPU and wgpu accept a depth view
/// there, [`depth_sample_type`]); and wgpu's GL backend leaves a depth
/// texture's own filter at GL's mipmapped-linear default, which makes it
/// incomplete for a depth format (a `texelFetch` reads 0), so each
/// shader's one depth read samples it through a nearest sampler
/// ([`NEAREST_DEPTH_BINDING`]) at the texel's center.
pub(crate) fn depth_as_float(source: &str) -> String {
    let mut out = source.replace("texture_depth_2d", "texture_2d<f32>");
    for (texture, group, binding) in [("t_scene_depth", 3, 2), ("t_depth", 0, 3)] {
        let read = format!("return textureLoad({texture}, p, 0);");
        if !out.contains(&read) {
            continue;
        }
        let sampler = format!("s_{texture}");
        out = out.replace(
            &read,
            &format!(
                "return textureSampleLevel({texture}, {sampler}, (vec2<f32>(p) + vec2<f32>(0.5)) \
                 / vec2<f32>(textureDimensions({texture})), 0.0).r;"
            ),
        );
        let decl = format!("var {texture}: texture_2d<f32>;");
        out = out.replace(
            &decl,
            &format!("{decl}\n@group({group}) @binding({binding}) var {sampler}: sampler;"),
        );
    }
    out
}

/// The binding of the nearest sampler [`depth_as_float`] adds on WebGL2:
/// the water's group 3 and the AO's group 0.
pub(crate) const NEAREST_DEPTH_BINDING: (u32, u32) = (2, 3);

/// A non-filtering sampler for the WebGL2 depth reads.
pub(crate) fn nearest_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("trino-depth-nearest"),
        ..Default::default()
    })
}

/// The layout entry of that sampler at `binding`.
pub(crate) fn nearest_sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
        count: None,
    }
}

/// How a shader binds the scene's depth: as depth, or on WebGL2 as an
/// unfilterable float texture ([`depth_as_float`]).
pub(crate) fn depth_sample_type(webgl2: bool) -> wgpu::TextureSampleType {
    if webgl2 {
        wgpu::TextureSampleType::Float { filterable: false }
    } else {
        wgpu::TextureSampleType::Depth
    }
}

/// The group 3 layout of the water pipelines: the sets and the scene's
/// depth copy (multisampled with MSAA on).
fn water_bgl(device: &wgpu::Device, multisampled: bool, webgl2: bool) -> wgpu::BindGroupLayout {
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
                sample_type: depth_sample_type(webgl2),
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled,
            },
            count: None,
        },
    ];
    // WebGL2 reads the depth through a nearest sampler (`depth_as_float`).
    if webgl2 {
        entries.push(nearest_sampler_entry(NEAREST_DEPTH_BINDING.0));
    }
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("trino-water-bgl"),
        entries: &entries,
    })
}

/// The two water pipelines (`fs_water`, `fs_water_vertex`) for a target
/// with `samples` samples per pixel. Translucent: depth-tested, never
/// written.
fn water_pipelines(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    groups: [&wgpu::BindGroupLayout; 4],
    samples: u32,
) -> [wgpu::RenderPipeline; 2] {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("trino-water-layout"),
        bind_group_layouts: &groups.map(Some),
        immediate_size: 0,
    });
    let attrs = lit_attributes();
    let make = |fragment: &str| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("trino-water-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_tri_lit"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: TRI_VERTEX_STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attrs,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fragment),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OFFSCREEN_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        })
    };
    [make("fs_water"), make("fs_water_vertex")]
}

impl WaterGpu {
    fn depth_copy(device: &wgpu::Device, size: (u32, u32), samples: u32) -> wgpu::Texture {
        // A render attachment too: multisampled textures must be one
        // (WebGPU), and WebGL2 copies the depth by drawing it
        // (`depth_blit`).
        let usage = wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT;
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("trino-water-depth"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage,
            view_formats: &[],
        })
    }

    fn bind_group(
        device: &wgpu::Device,
        bgl: &wgpu::BindGroupLayout,
        buf: &wgpu::Buffer,
        depth: &wgpu::Texture,
        nearest: Option<&wgpu::Sampler>,
    ) -> wgpu::BindGroup {
        let view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&view),
            },
        ];
        if let Some(s) = nearest {
            entries.push(wgpu::BindGroupEntry {
                binding: NEAREST_DEPTH_BINDING.0,
                resource: wgpu::BindingResource::Sampler(s),
            });
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trino-water-bg"),
            layout: bgl,
            entries: &entries,
        })
    }

    /// Builds the multisampled set once (MSAA turned on).
    fn ensure_ms(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.ms.is_some() {
            return;
        }
        let source = include_str!("shaders.wgsl")
            .replace("texture_depth_2d", "texture_depth_multisampled_2d");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("trino-shaders-ms-water"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let bgl = water_bgl(device, true, false);
        let [g0, g1, g2] = &self.groups;
        let [per_pixel, vertex] =
            water_pipelines(device, &shader, [g0, g1, g2, &bgl], MSAA_SAMPLES);
        let depth_copy = Self::depth_copy(device, size, MSAA_SAMPLES);
        let bg = Self::bind_group(device, &bgl, &self.buf, &depth_copy, None);
        self.ms = Some(WaterMs {
            bgl,
            depth_copy,
            bg,
            per_pixel,
            vertex,
        });
    }

    /// A new size: new depth copies (and their bind groups).
    fn resize(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        self.depth_copy = Self::depth_copy(device, size, 1);
        self.bg = Self::bind_group(
            device,
            &self.bgl,
            &self.buf,
            &self.depth_copy,
            self.nearest.as_ref(),
        );
        if let Some(ms) = &mut self.ms {
            ms.depth_copy = Self::depth_copy(device, size, MSAA_SAMPLES);
            ms.bg = Self::bind_group(device, &ms.bgl, &self.buf, &ms.depth_copy, None);
        }
    }
}

/// What `TexFilter::Default` means on a profile: nearest texels on the PC
/// (pixel art stays crisp, as always), the RDP's 3-point bilinear in the
/// N64 look, bilinear on the 3DS (its GPU default).
fn default_filter(profile: SimProfile) -> TexFilter {
    match profile {
        SimProfile::Native | SimProfile::Web => TexFilter::Point,
        SimProfile::N64 | SimProfile::N3ds => TexFilter::Bilinear,
    }
}

/// A 3D texture bind group: the base texture and the detail layer, each
/// with its sampler.
fn create_tex3d_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    base: (&wgpu::TextureView, &wgpu::Sampler),
    detail: (&wgpu::TextureView, &wgpu::Sampler),
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("trino-tex3d-bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(base.0),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(base.1),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(detail.0),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(detail.1),
            },
        ],
    })
}

/// The second screen's own target, globals and blit source.
struct SecondScreen {
    size: (u32, u32),
    tex: wgpu::Texture,
    view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    globals_bg: wgpu::BindGroup,
    blit_bg: wgpu::BindGroup,
}

/// Offscreen color target + its view + the matching depth buffer.
fn create_targets(
    device: &wgpu::Device,
    size: (u32, u32),
) -> (
    wgpu::Texture,
    wgpu::TextureView,
    wgpu::TextureView,
    wgpu::Texture,
) {
    let extent = wgpu::Extent3d {
        width: size.0,
        height: size.1,
        depth_or_array_layers: 1,
    };
    let offscreen_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("trino-offscreen"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let offscreen_view = offscreen_tex.create_view(&wgpu::TextureViewDescriptor::default());
    // Depth buffer for the 3D triangle pipeline (sprites test Always).
    let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("trino-depth"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        // Read back by the screen-space AO passes; copied for the water's
        // depth foam.
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());
    (offscreen_tex, offscreen_view, depth_view, depth_tex)
}

/// Samples per pixel when multisampling is on.
pub const MSAA_SAMPLES: u32 = 4;
/// The shader and layouts the scene pipelines are built from, kept so the
/// multisampled set ([`PcRenderer::set_msaa`]) is only built when asked for.
struct SceneSources {
    shader: wgpu::ShaderModule,
    sprite_layout: wgpu::PipelineLayout,
    tri_layout: wgpu::PipelineLayout,
    lit_layout: wgpu::PipelineLayout,
}

/// The screen-space triangle vertex (`TriVertex`): position, color, depth,
/// params, UV and the detail layer's UV.
fn tri_attributes() -> [wgpu::VertexAttribute; 6] {
    [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 8,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 24,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 28,
            shader_location: 3,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 32,
            shader_location: 4,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 88,
            shader_location: 10,
        },
    ]
}

/// The lit vertex: the triangle's attributes plus the per-pixel inputs
/// (world position, normal, base color, light set, gloss).
fn lit_attributes() -> Vec<wgpu::VertexAttribute> {
    let extra = [
        (wgpu::VertexFormat::Float32x3, 40, 5),
        (wgpu::VertexFormat::Float32x3, 52, 6),
        (wgpu::VertexFormat::Float32x4, 64, 7),
        (wgpu::VertexFormat::Float32, 80, 8),
        (wgpu::VertexFormat::Float32, 84, 9),
    ];
    let mut a = tri_attributes().to_vec();
    a.extend(
        extra
            .iter()
            .map(|&(format, offset, shader_location)| wgpu::VertexAttribute {
                format,
                offset,
                shader_location,
            }),
    );
    a
}

/// The four scene pipelines for a target with `samples` samples per pixel:
/// sprite, tri, tri clamp (billboards), lit.
fn scene_pipelines(
    device: &wgpu::Device,
    src: &SceneSources,
    samples: u32,
) -> [wgpu::RenderPipeline; 4] {
    let SceneSources {
        shader,
        sprite_layout,
        tri_layout,
        lit_layout,
    } = src;
    let instance_attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 8,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 16,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 24,
            shader_location: 3,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 32,
            shader_location: 4,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 40,
            shader_location: 5,
        },
    ];

    let make_sprite_pipeline = |samples: u32| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("trino-sprite-pipeline"),
            layout: Some(sprite_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_sprite"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: INSTANCE_STRIDE,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &instance_attrs,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_sprite"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OFFSCREEN_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            // Sprites are 2D overlay: draw in submission order, never test
            // or write depth (matches the N64's rdpq sprite path).
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        })
    };
    let tri_attrs = tri_attributes();
    let make_tri_pipeline = |fragment: &str, samples: u32| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("trino-tri-pipeline"),
            layout: Some(tri_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_tri"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: TRI_VERTEX_STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &tri_attrs,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fragment),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OFFSCREEN_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            // Real per-pixel occlusion: interpenetrating meshes (a cap
            // through a head) resolve correctly, which no painter sort can.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        })
    };
    let lit_attrs = lit_attributes();
    let make_lit_pipeline = |samples: u32| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("trino-lit-pipeline"),
            layout: Some(lit_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_tri_lit"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: TRI_VERTEX_STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &lit_attrs,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_tri_lit"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OFFSCREEN_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        })
    };
    [
        make_sprite_pipeline(samples),
        make_tri_pipeline("fs_tri", samples),
        make_tri_pipeline("fs_tri_clamp", samples),
        make_lit_pipeline(samples),
    ]
}

/// How a scene pass treats the multisampled targets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MsPass {
    /// Single-sampled: MSAA off, the second screen, the 2D after the AO.
    Off,
    /// Multisampled, the scene's last pass: the color resolves into the
    /// offscreen target, the depth is kept when the AO pass reads it.
    Resolve { keep_depth: bool },
    /// Multisampled, more of the scene to come in another pass (the
    /// per-pixel water reads a copy of the depth in between): color and
    /// depth are stored, nothing resolves yet.
    Hold,
}

/// The multisampled color and depth of the scene pass (resolved into the
/// offscreen target).
struct MsaaTargets {
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    /// The depth texture itself: the per-pixel water copies it.
    depth_tex: wgpu::Texture,
}

fn create_msaa_targets(device: &wgpu::Device, size: (u32, u32)) -> MsaaTargets {
    let extent = wgpu::Extent3d {
        width: size.0,
        height: size.1,
        depth_or_array_layers: 1,
    };
    let make = |label, format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: extent,
            mip_level_count: 1,
            sample_count: MSAA_SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let target = wgpu::TextureUsages::RENDER_ATTACHMENT;
    let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
    let color = make("trino-msaa-color", OFFSCREEN_FORMAT, target);
    // The SSAO pass reads it; the per-pixel water copies it.
    let depth_tex = make(
        "trino-msaa-depth",
        wgpu::TextureFormat::Depth32Float,
        target | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
    );
    MsaaTargets {
        color: view(&color),
        depth: view(&depth_tex),
        depth_tex,
    }
}

/// Scratch target of the VI pass (copied back into the offscreen target).
fn create_vi_target(device: &wgpu::Device, size: (u32, u32)) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("trino-vi"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn create_blit_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("trino-blit-bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

/// Uniform buffer contents; layout mirrored by `Globals` in shaders.wgsl.
fn globals_bytes(width: u32, height: u32, n64_look: bool) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[0..4].copy_from_slice(&(width as f32).to_le_bytes());
    b[4..8].copy_from_slice(&(height as f32).to_le_bytes());
    b[8..12].copy_from_slice(&(n64_look as u32).to_le_bytes());
    b
}

/// `TRINO_POWER=low|high`: the adapter to ask for where a machine has
/// two (an iGPU and a discrete GPU), for measurements. Default: wgpu's.
fn power_from_env() -> wgpu::PowerPreference {
    match std::env::var("TRINO_POWER").as_deref() {
        Ok("low") => wgpu::PowerPreference::LowPower,
        Ok("high") => wgpu::PowerPreference::HighPerformance,
        _ => wgpu::PowerPreference::default(),
    }
}

/// `TRINO_PC_TNL` ([`pc_tnl_setting`]). The console profiles always keep
/// the CPU path.
fn hw_tnl_from_env() -> bool {
    pc_tnl_setting(std::env::var("TRINO_PC_TNL").ok().as_deref())
}

/// `TRINO_PC_TNL`'s value: `cpu` (or `0`, `off`) keeps every draw on the
/// CPU T&L; `gpu`, `auto` and anything else (unset too) send the draws
/// `pc_gpu_check` accepts to the GPU.
fn pc_tnl_setting(value: Option<&str>) -> bool {
    !matches!(value, Some("cpu" | "0" | "off"))
}

/// `TRINO_PC_TNL_THREADS`: workers of the native profile's CPU T&L (1 =
/// the calling thread only). Default: the machine's parallelism, at most 8.
fn tnl_threads_from_env() -> usize {
    std::env::var("TRINO_PC_TNL_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
                .min(8)
        })
        .max(1)
}

impl PcRenderer {
    /// Renderer that presents to a window.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn new_windowed(
        window: Arc<winit::window::Window>,
        profile: SimProfile,
    ) -> Result<Self, RendererError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(window.clone()),
        ));
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| RendererError::Surface(e.to_string()))?;
        let size = window.inner_size();
        let mut r = Self::init(
            instance,
            Some((surface, size.width.max(1), size.height.max(1))),
            profile,
        )
        .await?;
        // A game window honors TRINO_AO=off|on|mask (headless renderers,
        // the tests', never read it).
        if let Some(view) = std::env::var("TRINO_AO")
            .ok()
            .and_then(|v| AoView::parse(&v))
        {
            r.set_ao_view(view);
        }
        Ok(r)
    }

    /// Renderer that presents to a browser canvas (the Web backend, issue
    /// #47), `width`×`height` physical pixels: WebGPU where the browser has
    /// it ([`WebBackend::Auto`] falls back to WebGL2 when `navigator.gpu`
    /// gives no adapter), or the API `backend` names.
    #[cfg(target_arch = "wasm32")]
    pub async fn new_canvas(
        canvas: web_sys::HtmlCanvasElement,
        width: u32,
        height: u32,
        profile: SimProfile,
        backend: WebBackend,
    ) -> Result<Self, RendererError> {
        /// A page's display: wgpu creates canvas surfaces against one.
        #[derive(Debug)]
        struct WebDisplay;
        impl wgpu::rwh::HasDisplayHandle for WebDisplay {
            fn display_handle(
                &self,
            ) -> Result<wgpu::rwh::DisplayHandle<'_>, wgpu::rwh::HandleError> {
                let raw = wgpu::rwh::RawDisplayHandle::Web(wgpu::rwh::WebDisplayHandle::new());
                // SAFETY: a web display handle holds no pointer; it stays
                // valid for the page's life.
                Ok(unsafe { wgpu::rwh::DisplayHandle::borrow_raw(raw) })
            }
        }
        let mut desc = wgpu::InstanceDescriptor::new_with_display_handle(Box::new(WebDisplay));
        desc.backends = match backend {
            WebBackend::Auto => wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL,
            WebBackend::WebGpu => wgpu::Backends::BROWSER_WEBGPU,
            WebBackend::WebGl2 => wgpu::Backends::GL,
        };
        if backend == WebBackend::WebGpu && !wgpu::util::is_browser_webgpu_supported().await {
            return Err(RendererError::NoAdapter(
                "this browser gives no WebGPU adapter (try ?backend=webgl2)".into(),
            ));
        }
        let instance = wgpu::util::new_instance_with_webgpu_detection(desc).await;
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .map_err(|e| RendererError::Surface(e.to_string()))?;
        Self::init(
            instance,
            Some((surface, width.max(1), height.max(1))),
            profile,
        )
        .await
    }

    /// Offscreen-only renderer for tests and tooling.
    pub async fn new_headless(profile: SimProfile) -> Result<Self, RendererError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        Self::init(instance, None, profile).await
    }

    /// Offscreen renderer on an existing device — the editor viewport path:
    /// sharing eframe's device lets the offscreen texture be registered
    /// directly as an egui texture (render-to-texture, zero copies).
    pub fn with_device(device: wgpu::Device, queue: wgpu::Queue, profile: SimProfile) -> Self {
        Self::build(
            device,
            queue,
            None,
            profile,
            "editor device".into(),
            BackendLimits::default(),
        )
    }

    async fn init(
        instance: wgpu::Instance,
        surface: Option<(wgpu::Surface<'static>, u32, u32)>,
        profile: SimProfile,
    ) -> Result<Self, RendererError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: surface.as_ref().map(|(s, _, _)| s),
                power_preference: power_from_env(),
                ..Default::default()
            })
            .await
            .map_err(|e| RendererError::NoAdapter(e.to_string()))?;
        let info = adapter.get_info();
        // WebGL2 (wgpu's GL backend in a browser) has the downlevel WebGL2
        // limits: no storage buffers, 16 KB uniform blocks, no sampling of
        // multisampled depth.
        let limits = BackendLimits {
            webgl2: cfg!(target_arch = "wasm32") && info.backend == wgpu::Backend::Gl,
        };

        // Compressed textures where the adapter samples them: BC on every
        // desktop GPU (`TRINO_BC=0` leaves it off, the runner then uploads
        // decoded pixels), ETC2 and ASTC on phones (the Web's KTX2
        // textures transcode to whichever the browser has).
        let bc_off = std::env::var("TRINO_BC").is_ok_and(|v| v == "0");
        let mut wanted =
            wgpu::Features::TEXTURE_COMPRESSION_ETC2 | wgpu::Features::TEXTURE_COMPRESSION_ASTC;
        if !bc_off {
            wanted |= wgpu::Features::TEXTURE_COMPRESSION_BC;
        }
        let required_features = adapter.features() & wanted;
        let base = if limits.webgl2 {
            wgpu::Limits::downlevel_webgl2_defaults()
        } else {
            wgpu::Limits::downlevel_defaults()
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("trino-pc"),
                required_features,
                // Downlevel limits, but the adapter's real texture size:
                // a 4K or ultrawide window (surface) and full-resolution
                // pictures need more than downlevel's 2048.
                required_limits: wgpu::Limits {
                    max_texture_dimension_2d: adapter.limits().max_texture_dimension_2d.min(8192),
                    ..base
                },
                ..Default::default()
            })
            .await
            .map_err(|e| RendererError::Device(e.to_string()))?;

        let surface = surface.map(|(surface, width, height)| {
            let surface_caps = surface.get_capabilities(&adapter);
            // Prefer a non-sRGB format so 8-bit colors pass through the blit
            // unchanged (retro palettes must not get gamma-shifted twice).
            // 8-bit formats first: some compositors (Wayland) list 16-bit
            // normalized formats ahead of them, and those need a device
            // feature (TEXTURE_FORMAT_16BIT_NORM) the renderer does not
            // request — picking one panicked at pipeline creation.
            use wgpu::TextureFormat as F;
            let format = [F::Bgra8Unorm, F::Rgba8Unorm]
                .into_iter()
                .find(|f| surface_caps.formats.contains(f))
                .or_else(|| {
                    surface_caps
                        .formats
                        .iter()
                        .copied()
                        .find(|f| !f.is_srgb() && f.required_features().is_empty())
                })
                .unwrap_or(surface_caps.formats[0]);
            let config = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width,
                height,
                present_mode: wgpu::PresentMode::AutoVsync,
                alpha_mode: surface_caps.alpha_modes[0],
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            };
            surface.configure(&device, &config);
            SurfaceState { surface, config }
        });

        // Browsers hide the GPU's name from WebGPU pages.
        let shown = if info.name.is_empty() {
            "browser adapter"
        } else {
            &info.name
        };
        let name = format!("{shown} ({:?})", info.backend);
        Ok(Self::build(device, queue, surface, profile, name, limits))
    }

    fn build(
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface: Option<SurfaceState>,
        profile: SimProfile,
        adapter_name: String,
        limits: BackendLimits,
    ) -> Self {
        let device_bc = device
            .features()
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
        let internal_size = profile.internal_resolution();
        let mut caps = profile.caps();
        // The picture size games see is the internal resolution (the
        // native profile's table says 1080p, its picture starts at 640×480).
        caps.screen_width = internal_size.0 as u16;
        caps.screen_height = internal_size.1 as u16;
        // The N64 profile emulates the console's output (3-point filtering,
        // RGBA5551 + dither) by default; TRINO_LOOK=off|n64 overrides.
        let n64_look = match std::env::var("TRINO_LOOK").as_deref() {
            Ok("off" | "0" | "native") => false,
            Ok("n64" | "1" | "on") => true,
            _ => profile == SimProfile::N64,
        };
        // Strict mode: enforce the profile's Caps at development time.
        let strict = matches!(std::env::var("TRINO_STRICT").as_deref(), Ok("1" | "true"));

        let (offscreen_tex, offscreen_view, depth_view, depth_tex) =
            create_targets(&device, internal_size);

        // The 3DS GPU samples textures bilinearly by default (citro2d), so
        // its sim profile does too; N64/native use nearest (the N64's
        // 3-point filter is emulated in the shader instead).
        let sprite_filter = if profile == SimProfile::N3ds {
            wgpu::FilterMode::Linear
        } else {
            wgpu::FilterMode::Nearest
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("trino-sprite-sampler"),
            mag_filter: sprite_filter,
            min_filter: sprite_filter,
            ..Default::default()
        });
        // 3D draws choose per draw (`TexFilter`): nearest and linear, each
        // wrapping and clamped (the N64 look filters in the shader, from
        // nearest texels).
        let tex3d_nearest = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("trino-tex3d-nearest"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });
        let clamp_nearest = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("trino-clamp-nearest"),
            ..Default::default()
        });
        let tex3d_linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("trino-tex3d-linear"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let clamp_linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("trino-clamp-linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // The window blit stays nearest: consoles output at native
        // resolution; the integer upscale is a PC artifact and must not blur.
        let blit_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("trino-blit-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let source = include_str!("shaders.wgsl");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shaders.wgsl"),
            source: wgpu::ShaderSource::Wgsl(if limits.webgl2 {
                depth_as_float(source).into()
            } else {
                source.into()
            }),
        });

        // Group 0: globals uniform (internal resolution).
        let globals_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-globals-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &globals_buf,
            0,
            &globals_bytes(internal_size.0, internal_size.1, n64_look),
        );
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trino-globals-bg"),
            layout: &globals_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });

        // Group layout shared by sprite textures and the blit source.
        let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-texture-bgl"),
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
            ],
        });

        let sprite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-sprite-layout"),
            bind_group_layouts: &[Some(&globals_bgl), Some(&texture_bgl)],
            immediate_size: 0,
        });

        // 3D triangle pipeline: same offscreen target; vertex colors
        // modulated by one texture per segment (white when untextured) and
        // its detail layer (white when none).
        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let tex3d_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-tex3d-bgl"),
            entries: &[
                tex_entry(0),
                sampler_entry(1),
                tex_entry(2),
                sampler_entry(3),
            ],
        });
        let tri_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-tri-layout"),
            bind_group_layouts: &[Some(&globals_bgl), Some(&tex3d_bgl)],
            immediate_size: 0,
        });

        // Lit pipeline: the same vertices with the per-pixel inputs, and
        // the frame's light sets in group 2.
        let lights_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-lights-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let lights_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-light-sets"),
            size: (lights::SET_BYTES * lights::MAX_SETS) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let lights_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trino-lights-bg"),
            layout: &lights_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: lights_buf.as_entire_binding(),
            }],
        });
        let lit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-lit-layout"),
            bind_group_layouts: &[Some(&globals_bgl), Some(&tex3d_bgl), Some(&lights_bgl)],
            immediate_size: 0,
        });
        let sources = SceneSources {
            shader,
            sprite_layout,
            tri_layout,
            lit_layout,
        };
        let [
            sprite_pipeline,
            tri_pipeline,
            tri_clamp_pipeline,
            lit_pipeline,
        ] = scene_pipelines(&device, &sources, 1);

        // Water: the lit vertices, groups 0..2 as the lit pipeline (the
        // light sets stay bound) and the water sets + the scene's depth in
        // group 3. Translucent: depth-tested, never written.
        let water_bgl = water_bgl(&device, false, limits.webgl2);
        let water_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-water-sets"),
            size: (water::SET_BYTES * water::MAX_SETS) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let [water_per_pixel, water_vertex] = water_pipelines(
            &device,
            &sources.shader,
            [&globals_bgl, &tex3d_bgl, &lights_bgl, &water_bgl],
            1,
        );
        let water_depth = WaterGpu::depth_copy(&device, internal_size, 1);
        let water_nearest = limits.webgl2.then(|| nearest_sampler(&device));
        let water = WaterGpu {
            bg: WaterGpu::bind_group(
                &device,
                &water_bgl,
                &water_buf,
                &water_depth,
                water_nearest.as_ref(),
            ),
            nearest: water_nearest,
            bgl: water_bgl,
            buf: water_buf,
            depth_copy: water_depth,
            per_pixel: water_per_pixel,
            vertex: water_vertex,
            groups: [globals_bgl.clone(), tex3d_bgl.clone(), lights_bgl.clone()],
            ms: None,
        };

        let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-blit-layout"),
            bind_group_layouts: &[Some(&texture_bgl)],
            immediate_size: 0,
        });
        let blit_format = surface
            .as_ref()
            .map(|s| s.config.format)
            .unwrap_or(OFFSCREEN_FORMAT);
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("trino-blit-pipeline"),
            layout: Some(&blit_layout),
            vertex: wgpu::VertexState {
                module: &sources.shader,
                entry_point: Some("vs_blit"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &sources.shader,
                entry_point: Some("fs_blit"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: blit_format,
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

        let blit_bind_group =
            create_blit_bind_group(&device, &texture_bgl, &offscreen_view, &blit_sampler);
        // The 3DS profile simulates the touch screen too.
        let second = (profile == SimProfile::N3ds).then(|| {
            let size = (320, 240);
            let (tex, view, depth_view, _) = create_targets(&device, size);
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("trino-globals-2"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&buf, 0, &globals_bytes(size.0, size.1, false));
            let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("trino-globals-bg-2"),
                layout: &globals_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                }],
            });
            let blit_bg = create_blit_bind_group(&device, &texture_bgl, &view, &blit_sampler);
            SecondScreen {
                size,
                tex,
                view,
                depth_view,
                globals_bg,
                blit_bg,
            }
        });
        // VI pass: reads the offscreen target (through the blit bind group)
        // into a scratch texture of the same format.
        let vi_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("trino-vi-pipeline"),
            layout: Some(&blit_layout),
            vertex: wgpu::VertexState {
                module: &sources.shader,
                entry_point: Some("vs_blit"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &sources.shader,
                entry_point: Some("fs_vi_dedither"),
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
        let vi_tex = create_vi_target(&device, internal_size);

        let white_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("trino-white"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &white_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255, 255, 255, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let white_view = white_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let white_bg = create_tex3d_bind_group(
            &device,
            &tex3d_bgl,
            (&white_view, &tex3d_nearest),
            (&white_view, &tex3d_nearest),
        );
        let tex_filter = default_filter(profile);

        let instance_capacity = 1024 * INSTANCE_STRIDE;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-instances"),
            size: instance_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let tri_capacity = 3 * 1024 * TRI_VERTEX_STRIDE;
        let tri_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-tri-verts"),
            size: tri_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        PcRenderer {
            device,
            queue,
            surface,
            offscreen_view,
            offscreen_tex,
            depth_view,
            depth_tex,
            sprite_pipeline,
            tri_pipeline,
            tri_clamp_pipeline,
            lit_pipeline,
            sources,
            ms_pipelines: None,
            msaa: 1,
            webgl2: limits.webgl2,
            depth_blit: None,
            msaa_targets: None,
            lights_buf,
            lights_bg,
            blit_pipeline,
            blit_bind_group,
            blit_sampler,
            sprite_bgl: texture_bgl,
            sampler,
            globals_bg,
            globals_buf,
            instance_buf,
            instance_capacity,
            tri_buf,
            tri_capacity,
            sprites: HashMap::new(),
            tex3d: HashMap::new(),
            tex3d_bgl,
            tex3d_nearest,
            tex3d_linear,
            clamp_nearest,
            clamp_linear,
            white_view,
            white_bg,
            tex_filter,
            meshes: HashMap::new(),
            fonts: HashMap::new(),
            commands: Vec::new(),
            frame_stats: FrameStats::default(),
            n64: N64Split::default(),
            pc_gpu_off: [0; PcGpuReject::COUNT],
            last_pc_gpu_off: [0; PcGpuReject::COUNT],
            times: PcFrameTimes::default(),
            last_times: PcFrameTimes::default(),
            last_stats: FrameStats::default(),
            tri_verts: Vec::new(),
            pending_tris: Vec::new(),
            pending_tex: Vec::new(),
            pending_surf: Vec::new(),
            locals: LightList::EMPTY,
            wetness: 0.0,
            ao: None,
            ao_view: AoView::Game,
            ao_pass: None,
            ao_pass_ms: None,
            light_sets: lights::LightSets::default(),
            water_state: WaterState::NONE,
            water_sets: water::WaterSets::default(),
            water,
            surf_scratch: Box::new(SurfaceScratch::new()),
            camera: Camera3::default(),
            light: DEFAULT_LIGHT,
            tnl_ctx: None,
            tnl_scratch: Box::new(TnlScratch::new()),
            clear: Color::BLACK,
            caps,
            profile,
            internal_size,
            n64_look,
            vi_dedither: true,
            vi_pipeline,
            vi_tex,
            strict,
            second,
            screen: 0,
            main_cmds: 0,
            main_tris: 0,
            bc: device_bc,
            adapter_name,
            hw_tnl: hw_tnl_from_env(),
            gpu: None,
            globals_bgl,
            cpu_jobs: Vec::new(),
            job_ctxs: Vec::new(),
            workers: Vec::new(),
            tnl_threads: tnl_threads_from_env(),
        }
    }

    /// Register a baked TMDL mesh under a stable handle. Re-uploading the
    /// same id replaces the content (live reload).
    pub fn upload_mesh(&mut self, id: ModelId, tmdl: Vec<u8>) {
        let layout = match Mesh::from_tmdl(&tmdl) {
            Ok(m) => m.layout(&tmdl),
            Err(e) => panic!("mesh {id:?}: invalid TMDL blob: {e:?}"),
        };
        // Resident on the GPU at register when the GPU T&L path is on;
        // otherwise (and after a re-upload) at its first GPU draw.
        if self.gpu_tnl_active() {
            let mesh = Mesh::from_layout(&tmdl, &layout);
            self.ensure_gpu_tnl();
            if let Some(gpu) = &mut self.gpu {
                gpu.upload_mesh(&self.device, &self.queue, id.0, &mesh);
            }
        } else if let Some(gpu) = &mut self.gpu {
            gpu.forget_mesh(id.0);
        }
        self.meshes.insert(id.0, (tmdl, layout));
    }

    /// Send the draws `pc_gpu_check` accepts to the GPU (the default on the
    /// native profile; `TRINO_PC_TNL=cpu` starts with it off), or keep
    /// every draw on the CPU T&L — the A/B switch for regressions and
    /// bisecting. The console profiles always keep the CPU path. Takes
    /// effect at the next draw.
    pub fn set_hw_tnl(&mut self, on: bool) {
        self.hw_tnl = on;
    }

    /// Does the GPU T&L path take draws now ([`PcRenderer::set_hw_tnl`],
    /// the native profile, not the N64 look)?
    pub fn hw_tnl(&self) -> bool {
        self.gpu_tnl_active()
    }

    /// The [`PcRenderer::set_hw_tnl`] switch itself (on a console profile
    /// too, where it has no effect).
    pub fn hw_tnl_setting(&self) -> bool {
        self.hw_tnl
    }

    /// Workers of the native profile's CPU T&L (1 = on the calling thread;
    /// default `TRINO_PC_TNL_THREADS` or the machine's parallelism, at most
    /// 8). The triangles and counters are the same for any count.
    pub fn set_tnl_threads(&mut self, threads: usize) {
        // A browser's wasm has no threads (no SharedArrayBuffer build).
        self.tnl_threads = if cfg!(target_arch = "wasm32") {
            1
        } else {
            threads.max(1)
        };
    }

    pub fn tnl_threads(&self) -> usize {
        self.tnl_threads
    }

    /// Bytes of mesh vertices resident on the GPU (the GPU T&L path's
    /// buffers, uploaded once per mesh).
    pub fn resident_mesh_bytes(&self) -> u64 {
        self.gpu.as_ref().map_or(0, |g| g.resident_bytes())
    }

    fn gpu_tnl_active(&self) -> bool {
        self.hw_tnl && self.profile.is_native() && !self.n64_look
    }

    /// Build the GPU T&L path's pipelines and buffers (on first use).
    fn ensure_gpu_tnl(&mut self) {
        let samples = self.msaa;
        let gpu = self.gpu.get_or_insert_with(|| {
            gpu_tnl::GpuTnl::new(&self.device, [&self.globals_bgl, &self.tex3d_bgl])
        });
        if samples > 1 {
            gpu.ensure_ms(&self.device, samples);
        }
    }

    /// Register a baked font's glyph table (`TFNT`) under its handle; the
    /// atlas must be uploaded as the sprite with the same handle.
    /// Re-uploading replaces it (live reload).
    pub fn upload_font(&mut self, id: FontId, table: Vec<u8>) {
        let index = match trino_core::text::BitmapFont::parse(&table) {
            Ok(font) => trino_core::text::GlyphIndex::build(&font),
            Err(e) => panic!("font {id:?}: invalid TFNT table: {e:?}"),
        };
        let table = table.into_boxed_slice();
        self.fonts
            .insert(id.0, Arc::new(FontEntry { table, index }));
    }

    /// Toggle the N64 output emulation (3-point filtering, RGBA5551 +
    /// ordered dither). Defaults to on for [`SimProfile::N64`].
    pub fn set_n64_look(&mut self, on: bool) {
        // 3D bind groups pick their sampler by the look.
        self.tex3d.clear();
        if self.n64_look != on {
            self.n64_look = on;
            self.queue.write_buffer(
                &self.globals_buf,
                0,
                &globals_bytes(self.internal_size.0, self.internal_size.1, on),
            );
        }
    }

    pub fn n64_look(&self) -> bool {
        self.n64_look
    }

    /// The current frame's 3D on the N64 (the N64 profile only; zeros
    /// otherwise): CPU and RSP time estimated by `trino_core::perf::N64_COST`,
    /// each draw sent where the console sends it ([`N64Estimate`]).
    pub fn n64_estimate(&self) -> N64Estimate {
        let (cpu_us, rsp_us) = trino_core::perf::N64_COST.split_us(
            &self.n64.cpu,
            self.n64.cpu_draws,
            self.n64.rsp_draws,
            self.n64.rsp_tris,
        );
        N64Estimate {
            cpu_us,
            rsp_us,
            cpu_draws: self.n64.cpu_draws,
            rsp_draws: self.n64.rsp_draws,
            rsp_tris: self.n64.rsp_tris,
        }
    }

    /// Toggle strict mode: content that busts the profile's [`Caps`] panics
    /// with an actionable message instead of silently working on PC only.
    /// Defaults to off; `TRINO_STRICT=1` enables it.
    pub fn set_strict(&mut self, on: bool) {
        self.strict = on;
    }

    /// Change the internal framebuffer resolution (the editor renders its
    /// viewport at the panel's pixel size). Recreates the offscreen color
    /// and depth targets; sprites, meshes and pipelines are kept. Callers
    /// holding [`offscreen_view`](Self::offscreen_view) must re-fetch it.
    /// Budgets ([`Caps`]) still come from the profile.
    pub fn set_internal_size(&mut self, width: u32, height: u32) {
        let max = self.device.limits().max_texture_dimension_2d.min(8192);
        let size = (width.clamp(16, max), height.clamp(16, max));
        if size == self.internal_size {
            return;
        }
        let (tex, view, depth, depth_tex) = create_targets(&self.device, size);
        self.blit_bind_group =
            create_blit_bind_group(&self.device, &self.sprite_bgl, &view, &self.blit_sampler);
        self.offscreen_tex = tex;
        self.offscreen_view = view;
        self.depth_view = depth;
        self.depth_tex = depth_tex;
        self.water.resize(&self.device, size);
        if let Some(ao) = &mut self.ao_pass {
            ao.invalidate();
        }
        self.vi_tex = create_vi_target(&self.device, size);
        if self.msaa > 1 {
            self.msaa_targets = Some(create_msaa_targets(&self.device, size));
        }
        if let Some(ao) = &mut self.ao_pass_ms {
            ao.invalidate();
        }
        self.internal_size = size;
        // Games read the picture size from the caps (culling, layout).
        self.caps.screen_width = size.0 as u16;
        self.caps.screen_height = size.1 as u16;
        self.queue.write_buffer(
            &self.globals_buf,
            0,
            &globals_bytes(size.0, size.1, self.n64_look),
        );
    }

    /// Multisample the main screen's scene (edges of triangles and sprites)
    /// with [`MSAA_SAMPLES`] samples per pixel, or not: the PC's own
    /// profile at its best (`trino-runner-pc` and the PC app turn it on for
    /// it). The console profiles ignore it — their look is the console's.
    /// Off by default (goldens are single-sampled).
    pub fn set_msaa(&mut self, on: bool) {
        let samples = if on && self.profile.is_native() && !self.webgl2 {
            MSAA_SAMPLES
        } else {
            1
        };
        if samples == self.msaa {
            return;
        }
        self.msaa = samples;
        if samples > 1 && self.ms_pipelines.is_none() {
            self.ms_pipelines = Some(scene_pipelines(&self.device, &self.sources, samples));
        }
        if samples > 1 {
            self.water.ensure_ms(&self.device, self.internal_size);
            if let Some(gpu) = &mut self.gpu {
                gpu.ensure_ms(&self.device, samples);
            }
        }
        self.msaa_targets =
            (samples > 1).then(|| create_msaa_targets(&self.device, self.internal_size));
        if let Some(ao) = &mut self.ao_pass_ms {
            ao.invalidate();
        }
    }

    /// Samples per pixel of the scene (1 = no multisampling).
    pub fn msaa(&self) -> u32 {
        self.msaa
    }

    /// Toggle the N64 VI dedither filter (on by default; it only runs with
    /// the N64 look). The console enables the same filter
    /// (`FILTERS_RESAMPLE_ANTIALIAS_DEDITHER` in the N64 shim): each
    /// pixel's 5-bit channels move one 8-bit step toward each of its 8
    /// neighbours that differ — the restore filter that turns the RDP's
    /// magic-square dither back into smooth gradients.
    pub fn set_vi_dedither(&mut self, on: bool) {
        self.vi_dedither = on;
    }

    pub fn vi_dedither(&self) -> bool {
        self.vi_dedither
    }

    /// What the screen-space AO shows ([`AoView`]): as the game sets it
    /// (the default), off, always on, or the occlusion alone. The editor's
    /// Scene view toggle and `TRINO_AO=off|on|mask`. Only the native
    /// profile runs the pass (`Caps::ao_tier`); the console profiles show
    /// what the consoles do, the AO baked into the models.
    pub fn set_ao_view(&mut self, view: AoView) {
        self.ao_view = view;
    }

    pub fn ao_view(&self) -> AoView {
        self.ao_view
    }

    /// The AO this frame runs with, and whether it shows as the mask.
    fn frame_ao(&self) -> Option<(AmbientOcclusion, bool)> {
        if self.caps.ao_tier != AoTier::ScreenSpace {
            return None;
        }
        self.ao_view.resolve(self.ao)
    }

    /// View of the internal framebuffer — the editor registers this as an
    /// egui texture to show the game inside a viewport panel.
    pub fn offscreen_view(&self) -> &wgpu::TextureView {
        &self.offscreen_view
    }

    /// Upload an RGBA8 texture for `id`. Re-uploading the same id replaces
    /// the content (this is what makes asset live-reload work: handles are
    /// stable, content is swapped in place).
    pub fn upload_sprite(&mut self, id: SpriteId, width: u32, height: u32, rgba: &[u8]) {
        assert_eq!(
            rgba.len(),
            (width * height * 4) as usize,
            "sprite {id:?}: pixel data does not match {width}x{height} RGBA8"
        );
        let alpha = rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 255);
        self.install_sprite(
            id,
            (width, height),
            OFFSCREEN_FORMAT,
            rgba,
            width * 4,
            alpha,
        );
    }

    /// Does this renderer sample block-compressed sprites
    /// ([`PcRenderer::upload_sprite_bc`])? The device has
    /// `TEXTURE_COMPRESSION_BC` (requested where the adapter has it, unless
    /// `TRINO_BC=0`) and [`PcRenderer::set_bc`] left it on.
    pub fn supports_bc(&self) -> bool {
        self.bc
    }

    /// Does [`PcRenderer::upload_sprite_compressed`] take `format`: the
    /// device samples it (BC also needs [`PcRenderer::set_bc`] on).
    pub fn supports_compressed(&self, format: CompressedFormat) -> bool {
        if format.is_bc() {
            self.bc
        } else {
            self.device.features().contains(format.feature())
        }
    }

    /// Turn BC uploads off (`false`: [`PcRenderer::upload_sprite_bc`]
    /// refuses, as on a GPU without BC) or back on where the device has it.
    pub fn set_bc(&mut self, on: bool) {
        self.bc = on
            && self
                .device
                .features()
                .contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
    }

    /// Upload a BC texture for `id` ([`PcRenderer::upload_sprite_compressed`]
    /// with its alpha read from the blocks).
    pub fn upload_sprite_bc(
        &mut self,
        id: SpriteId,
        width: u32,
        height: u32,
        format: CompressedFormat,
        blocks: &[u8],
    ) -> bool {
        self.upload_sprite_compressed(id, width, height, format, blocks, None)
    }

    /// Upload a block-compressed texture for `id` (`width` and `height`
    /// multiples of 4, `blocks` in row-major order), replacing any previous
    /// one. Returns `false` and uploads nothing when the renderer doesn't
    /// sample `format` ([`PcRenderer::supports_compressed`]): the caller
    /// uploads decoded RGBA8 instead. `alpha`: whether some texel blends
    /// (a model drawn with it stays on the CPU path); `None` reads it from
    /// BC blocks, and takes the ETC2 RGBA and ASTC formats as blending.
    pub fn upload_sprite_compressed(
        &mut self,
        id: SpriteId,
        width: u32,
        height: u32,
        format: CompressedFormat,
        blocks: &[u8],
        alpha: Option<bool>,
    ) -> bool {
        if !self.supports_compressed(format) {
            return false;
        }
        assert!(
            width.is_multiple_of(4) && height.is_multiple_of(4) && width > 0 && height > 0,
            "sprite {id:?}: {width}x{height} is not whole 4x4 blocks"
        );
        let row = width / 4 * format.block_bytes();
        assert_eq!(
            blocks.len(),
            (row * height / 4) as usize,
            "sprite {id:?}: block data does not match {width}x{height} {format:?}"
        );
        let alpha = alpha.unwrap_or_else(|| match format {
            CompressedFormat::Bc1 | CompressedFormat::Bc3 | CompressedFormat::Bc7 => {
                gpu_tnl::bc_alpha(format, blocks)
            }
            CompressedFormat::Etc2Rgb8 => false,
            CompressedFormat::Etc2Rgba8 | CompressedFormat::Astc4x4 => true,
        });
        self.install_sprite(id, (width, height), format.wgpu(), blocks, row, alpha);
        true
    }

    /// The sprite textures held now: how many, their GPU bytes, how many
    /// are block-compressed (the PC's VRAM figure in `texture report` and
    /// the `TRINO_TEXTURES` line).
    pub fn sprite_memory(&self) -> SpriteMemory {
        let mut m = SpriteMemory::default();
        for e in self.sprites.values() {
            m.textures += 1;
            m.bytes += e.bytes;
            if e.compressed {
                m.compressed += 1;
                m.compressed_bytes += e.bytes;
            }
        }
        m
    }

    /// Create the texture, write `data` (rows of `row_bytes`: pixels, or
    /// 4×4 blocks for a compressed format) and register its bind group
    /// under `id`. `alpha`: some texel is below full alpha.
    fn install_sprite(
        &mut self,
        id: SpriteId,
        (width, height): (u32, u32),
        format: wgpu::TextureFormat,
        data: &[u8],
        row_bytes: u32,
        alpha: bool,
    ) {
        let rows = if format == OFFSCREEN_FORMAT {
            height
        } else {
            height / 4
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("trino-sprite"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(rows),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trino-sprite-bg"),
            layout: &self.sprite_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.tex3d
            .retain(|k, _| k.id != id.0 && k.detail.is_none_or(|d| d.sprite != id));
        self.sprites.insert(
            id.0,
            SpriteEntry {
                bind_group,
                view,
                size: (width, height),
                bytes: (row_bytes * rows) as u64,
                compressed: format != OFFSCREEN_FORMAT,
                alpha,
            },
        );
    }

    /// Window resized: reconfigure the surface (offscreen stays fixed).
    pub fn resize(&mut self, width: u32, height: u32) {
        if let Some(ss) = &mut self.surface
            && width > 0
            && height > 0
        {
            ss.config.width = width;
            ss.config.height = height;
            ss.surface.configure(&self.device, &ss.config);
        }
    }

    /// What a window of `output` physical pixels shows right now: the
    /// internal frame upscaled and centered exactly like the present pass
    /// (nearest, whole-number scale, black bars), composed on the CPU.
    /// Test/tooling path (screenshots of fullscreen/ultrawide layouts).
    pub fn present_preview(&self, output: (u32, u32)) -> Vec<u8> {
        let main = self.internal_size;
        let second = self.second.as_ref().map(|sc| sc.size);
        let src = self.read_offscreen();
        let src2 = second.map(|_| self.read_second_screen());
        let mut out = vec![0u8; (output.0 * output.1 * 4) as usize];
        for oy in 0..output.1 {
            for ox in 0..output.0 {
                let at = ((oy * output.0 + ox) * 4) as usize;
                out[at + 3] = 255;
                let hit = window_to_screen(main, second, output, ox as f32 + 0.5, oy as f32 + 0.5);
                if let Some((p, screen)) = hit {
                    let (w, buf) = match (screen, &src2, second) {
                        (1, Some(b), Some(s)) => (s.0, b),
                        _ => (main.0, &src),
                    };
                    let from = ((p.y as u32 * w + p.x as u32) * 4) as usize;
                    out[at..at + 4].copy_from_slice(&buf[from..from + 4]);
                }
            }
        }
        out
    }

    /// Read the second screen back (RGBA8), like
    /// [`read_offscreen`](Self::read_offscreen). Empty without one.
    pub fn read_second_screen(&self) -> Vec<u8> {
        match &self.second {
            Some(sc) => self.read_texture(&sc.tex, sc.size),
            None => Vec::new(),
        }
    }

    /// Block until the GPU has finished everything submitted (frame-time
    /// measurements include the GPU's work).
    pub fn wait_gpu(&self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// The adapter's name and backend (for performance reports).
    pub fn adapter_info(&self) -> String {
        self.adapter_name.clone()
    }

    /// The last finished frame's CPU time per rendering stage and the GPU
    /// work it handed over ([`PcFrameTimes`]).
    pub fn frame_times(&self) -> PcFrameTimes {
        self.last_times
    }

    /// The last finished frame's model draws that stay off the native
    /// profile's GPU T&L path, per reason ([`PcGpuReject::index`]); the
    /// draws it takes are `stats().tnl.pc_gpu_draws`. All zero on the
    /// console profiles, which never take it (issue #51).
    pub fn pc_gpu_rejects(&self) -> [u32; PcGpuReject::COUNT] {
        self.last_pc_gpu_off
    }

    /// Read the internal framebuffer back as tightly-packed RGBA8.
    /// Test/tooling path — synchronous, not for per-frame use.
    pub fn read_offscreen(&self) -> Vec<u8> {
        self.read_texture(&self.offscreen_tex, self.internal_size)
    }

    /// [`PcRenderer::read_offscreen`] where the backend can wait for the
    /// GPU: `None` in a browser on WebGPU, which maps buffers only once
    /// the page's script yields (read it with
    /// [`PcRenderer::start_read_offscreen`] there).
    pub fn try_read_offscreen(&self) -> Option<Vec<u8>> {
        self.try_read_texture(&self.offscreen_tex, self.internal_size)
    }

    /// Start reading the internal framebuffer back without waiting: poll
    /// the returned [`PendingRead`] once per frame (a browser maps the
    /// buffer between frames). The Web's golden frame takes this path.
    pub fn start_read_offscreen(&self) -> PendingRead {
        let (buffer, padded) = self.copy_to_readback(&self.offscreen_tex, self.internal_size);
        let done = std::sync::Arc::new(std::sync::Mutex::new(None));
        let flag = done.clone();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            *flag.lock().unwrap() = Some(r.map_err(|e| e.to_string()));
        });
        PendingRead {
            buffer,
            padded,
            size: self.internal_size,
            done,
        }
    }

    /// Poll a [`PendingRead`]: the pixels once the GPU has them.
    pub fn poll_read(&self, read: &PendingRead) -> Option<Vec<u8>> {
        let _ = self.device.poll(wgpu::PollType::Poll);
        let state = read.done.lock().unwrap().clone();
        match state {
            Some(Ok(())) => Some(unpad_rows(
                &read.buffer.slice(..).get_mapped_range(),
                read.size,
                read.padded,
            )),
            Some(Err(e)) => panic!("failed to map readback buffer: {e}"),
            None => None,
        }
    }

    fn read_texture(&self, tex: &wgpu::Texture, size: (u32, u32)) -> Vec<u8> {
        self.try_read_texture(tex, size)
            .expect("map_async callback never ran (a browser's WebGPU can't wait)")
    }

    /// Copy `tex` into a mappable buffer: the buffer and its padded row
    /// pitch.
    fn copy_to_readback(&self, tex: &wgpu::Texture, size: (u32, u32)) -> (wgpu::Buffer, u32) {
        let (w, h) = size;
        let padded = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-readback"),
            size: (padded * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        (buffer, padded)
    }

    /// Read `tex` back, waiting for the GPU; `None` where the backend
    /// can't wait (a browser's WebGPU).
    fn try_read_texture(&self, tex: &wgpu::Texture, size: (u32, u32)) -> Option<Vec<u8>> {
        // A browser's WebGPU maps only after the script yields: give up
        // before allocating. A dropped buffer isn't freed there until the
        // page's GC runs, and a probe per pixel leaked enough mapped
        // read-backs that Chromium on SwiftShader destroyed the device.
        if cfg!(target_arch = "wasm32") && !self.webgl2 {
            return None;
        }
        let (buffer, padded) = self.copy_to_readback(tex, size);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        // On WebGL2 the poll runs the callback; on WebGPU it fires only
        // after the script yields, so don't block (a browser can't).
        let mapped = if cfg!(target_arch = "wasm32") {
            rx.try_recv().ok()?
        } else {
            rx.recv().expect("map_async callback dropped")
        };
        mapped.expect("failed to map readback buffer");
        let data = slice.get_mapped_range();
        Some(unpad_rows(&data, size, padded))
    }

    pub fn internal_size(&self) -> (u32, u32) {
        self.internal_size
    }

    /// Window pixel (physical, as winit reports the cursor) → game pixel,
    /// through the same letterboxed integer upscale the window shows.
    /// `None` outside the picture or without a window surface.
    pub fn window_to_game(&self, x: f64, y: f64) -> Option<trino_core::Vec2> {
        self.window_to_pointer(x, y)
            .and_then(|(p, screen)| (screen == 0).then_some(p))
    }

    /// Window pixel → (position, screen): with the 3DS profile the window
    /// shows both screens stacked, and the second screen is 1.
    pub fn window_to_pointer(&self, x: f64, y: f64) -> Option<(trino_core::Vec2, u8)> {
        let ss = self.surface.as_ref()?;
        window_to_screen(
            self.internal_size,
            self.second.as_ref().map(|sc| sc.size),
            (ss.config.width, ss.config.height),
            x as f32,
            y as f32,
        )
    }

    /// Depth-sort the pending model batch and enqueue it as triangle
    /// commands (painter's order across every mesh of the batch). Batches
    /// with textured draws are then grouped by texture (stable: depth order
    /// holds within each texture) — one texture bind per group, the same
    /// grouping the N64 needs to load TMEM once per texture.
    ///
    /// The native profile's batch also holds its CPU draws as jobs (run
    /// here, [`cpu_tnl`]) and its GPU T&L draws, which become instanced
    /// draw calls ahead of the CPU triangles: opaque and depth-tested, they
    /// need no painter order.
    fn flush_model_batch(&mut self) {
        let gpu = self.gpu.as_ref().is_some_and(|g| g.has_pending());
        if self.pending_tris.is_empty() && self.cpu_jobs.is_empty() && !gpu {
            return;
        }
        self.run_cpu_jobs();
        let t0 = Instant::now();
        if let Some(g) = self.gpu.as_mut().filter(|g| g.has_pending()) {
            let r = g.flush_batch();
            self.commands.push(Cmd::Meshes {
                first: r.start,
                count: r.end - r.start,
            });
        }
        if !self.pending_tris.is_empty() {
            self.flush_model_batch_now();
        }
        self.times.batch_us += micros(t0);
    }

    /// Tessellate the batch's CPU jobs (over the workers when there is
    /// enough work) and append their triangles in submission order, as the
    /// draws would have one by one.
    fn run_cpu_jobs(&mut self) {
        if self.cpu_jobs.is_empty() {
            return;
        }
        let t0 = Instant::now();
        let jobs = std::mem::take(&mut self.cpu_jobs);
        let chunks = cpu_tnl::run(
            &self.meshes,
            &self.job_ctxs,
            &jobs,
            &mut self.workers,
            self.tnl_threads,
        );
        let mut next = jobs.iter();
        for chunk in &chunks {
            let mut at = 0;
            for &(n, stats) in &chunk.per_job {
                let Some(job) = next.next() else { break };
                let n = n as usize;
                let before = self.pending_tris.len();
                self.pending_tris.extend_from_slice(&chunk.tris[at..at + n]);
                if matches!(
                    job.kind,
                    cpu_tnl::JobKind::Lit { .. } | cpu_tnl::JobKind::Water { .. }
                ) {
                    self.pending_surf.resize(before, None);
                    self.pending_surf.extend_from_slice(&chunk.surf[at..at + n]);
                }
                self.tag_pending(before, n, job.texture);
                self.frame_stats.tnl.add(&stats);
                at += n;
            }
        }
        self.cpu_jobs = jobs;
        self.cpu_jobs.clear();
        self.job_ctxs.clear();
        self.times.tnl_us += micros(t0);
    }

    /// [`Self::flush_model_batch`]'s work: sort the pending triangles and
    /// expand them into GPU vertices.
    fn flush_model_batch_now(&mut self) {
        // Painter order as keys (texture group, depth, submission index) —
        // the same order the console backends draw in.
        let mut keys: Vec<(u64, u64)> = self
            .pending_tris
            .iter()
            .enumerate()
            .map(|(i, tri)| {
                let group = match self.pending_tex.get(i).copied().flatten() {
                    Some(t) => (((t.id as u64) << 1) | t.clamp as u64) + 1,
                    None => 0,
                };
                // Per-pixel lit triangles group apart too (another pipeline):
                // interleaved with plain ones in depth order they cut the
                // batch into a draw call per switch. The z-buffer resolves
                // the order between groups. Water comes last: it blends over
                // what the batch drew under it, and writes no depth.
                let surf = self.pending_surf.get(i).copied().flatten();
                let lit = surf.is_some() as u64;
                let water = surf.is_some_and(|s| s.water) as u64;
                (
                    (water << 63) | (group << 1) | lit,
                    trino_core::render3d::painter_key(tri.depth, i),
                )
            })
            .collect();
        keys.sort_unstable();
        let order: Vec<(Option<TexKey>, ScreenTri, Option<SurfTri>)> = keys
            .iter()
            .map(|&(_, key)| {
                let i = trino_core::render3d::painter_index(key);
                (
                    self.pending_tex.get(i).copied().flatten(),
                    self.pending_tris[i],
                    self.pending_surf.get(i).copied().flatten(),
                )
            })
            .collect();
        let rgba = |c: Color| {
            [
                c.r as f32 / 255.0,
                c.g as f32 / 255.0,
                c.b as f32 / 255.0,
                c.a as f32 / 255.0,
            ]
        };
        let kind = |s: &Option<SurfTri>| match s {
            None => TriKind::Plain,
            Some(s) if s.water => TriKind::Water,
            Some(_) => TriKind::Lit,
        };
        let mut run_start = self.tri_verts.len() as u32;
        let mut run = order.first().map(|(t, _, s)| (*t, kind(s)));
        for (tex, tri, surf) in &order {
            let lit = surf.is_some();
            let k = kind(surf);
            if Some((*tex, k)) != run {
                let first = run_start;
                let count = self.tri_verts.len() as u32 - first;
                if let Some((texture, kind)) = run {
                    self.commands.push(Cmd::Tris {
                        first,
                        count,
                        texture,
                        kind,
                    });
                }
                run_start = self.tri_verts.len() as u32;
                run = Some((*tex, k));
            }
            for i in 0..3 {
                let p = tri.pts[i];
                let mut v = TriVertex {
                    pos: [p.x, p.y],
                    color: rgba(tri.colors[i]),
                    depth: tri.z[i],
                    w: if tex.is_some() || lit { tri.w[i] } else { 1.0 },
                    uv: [tri.uv[i].x, tri.uv[i].y],
                    world: [0.0; 3],
                    normal: [0.0; 3],
                    base: [0.0; 4],
                    gloss: 0.0,
                    set: 0.0,
                    tex: match tex {
                        Some(t) => [
                            (t.filter == TexFilter::Point) as u32 as f32,
                            t.detail
                                .filter(|d| self.sprites.contains_key(&d.sprite.0))
                                .map_or(0.0, |d| d.scale()),
                        ],
                        None => [0.0; 2],
                    },
                };
                if let Some(s) = surf {
                    let sv = &s.v[i];
                    v.world = [sv.pos.x, sv.pos.y, sv.pos.z];
                    v.normal = [sv.normal.x, sv.normal.y, sv.normal.z];
                    v.base = rgba(sv.base);
                    v.gloss = s.gloss;
                    // The set (0..8) and the light mask, packed: exact in f32
                    // (a water set for water triangles).
                    v.set = (s.set + 8 * s.mask) as f32;
                }
                self.tri_verts.push(v);
            }
        }
        if let Some((texture, kind)) = run {
            let count = self.tri_verts.len() as u32 - run_start;
            self.commands.push(Cmd::Tris {
                first: run_start,
                count,
                texture,
                kind,
            });
        }
        self.pending_tris.clear();
        self.pending_tex.clear();
        self.pending_surf.clear();
    }

    /// Record the texture of the `added` triangles appended at `before`
    /// (the tag list is only materialized once a textured draw joins).
    fn tag_pending(&mut self, before: usize, added: usize, key: Option<TexKey>) {
        if key.is_some() || !self.pending_tex.is_empty() {
            self.pending_tex.resize(before, None);
            self.pending_tex.extend(std::iter::repeat_n(key, added));
        }
    }

    /// Bind group of a sprite as a 3D texture — wrapping (models) or
    /// clamped (billboards). Created once, dropped when the sprite is
    /// re-uploaded.
    fn tex3d_group(&mut self, key: TexKey) -> Option<&wgpu::BindGroup> {
        if !self.tex3d.contains_key(&key) {
            let entry = self.sprites.get(&key.id)?;
            // The N64 look filters in the shader (3-point or point), from
            // nearest texels; elsewhere the sampler does.
            let linear = key.filter == TexFilter::Bilinear && !self.n64_look;
            let sampler = match (key.clamp, linear) {
                (true, false) => &self.clamp_nearest,
                (true, true) => &self.clamp_linear,
                (false, false) => &self.tex3d_nearest,
                (false, true) => &self.tex3d_linear,
            };
            let detail = key
                .detail
                .and_then(|d| self.sprites.get(&d.sprite.0))
                .map_or(&self.white_view, |e| &e.view);
            let dsampler = if linear {
                &self.tex3d_linear
            } else {
                &self.tex3d_nearest
            };
            let bg = create_tex3d_bind_group(
                &self.device,
                &self.tex3d_bgl,
                (&entry.view, sampler),
                (detail, dsampler),
            );
            self.tex3d.insert(key, bg);
        }
        self.tex3d.get(&key)
    }

    /// The size of the screen draws go to now: what the T&L projects to.
    fn current_screen_size(&self) -> Vec2 {
        let (w, h) = match (&self.second, self.screen) {
            (Some(sc), 1) => sc.size,
            _ => self.internal_size,
        };
        Vec2::new(w as f32, h as f32)
    }

    /// Render the recorded commands into the main target, or (`second`)
    /// into the second screen's, and submit.
    fn draw_commands(&mut self, second: bool) {
        if self.strict {
            // With two screens, the main screen's counts are in `main_*`:
            // the budgets are per frame, both screens together.
            let cmds = self.commands.len() as u32 + self.main_cmds;
            if cmds > self.caps.max_sprites_per_frame {
                panic!(
                    "strict mode: {} sprites this frame busts the {:?} profile budget \
                     of {} — reduce overdraw or batch static content.",
                    cmds, self.profile, self.caps.max_sprites_per_frame
                );
            }
            let tris =
                (self.tri_verts.len() / 3) as u32 + self.main_tris + self.pc_gpu_frame_tris();
            if tris > self.caps.max_tris_per_frame {
                panic!(
                    "strict mode: {tris} 3D triangles this frame busts the {:?} \
                     profile budget of {} — simplify the meshes.",
                    self.profile, self.caps.max_tris_per_frame
                );
            }
            if self.caps.max_3d_us_per_frame != u32::MAX {
                let e = self.n64_estimate();
                if e.cpu_us.max(e.rsp_us) > self.caps.max_3d_us_per_frame {
                    panic!(
                        "strict mode: this frame's 3D is ~{:.1} ms of CPU and ~{:.1} ms of \
                         RSP on the {:?} profile (trino_core::perf::N64_COST: {} draws on \
                         the CPU with {} vertices and {} triangles, {} on the RSP with {} \
                         triangles) — over its {:.0} ms ceiling. Fewer draws, fewer \
                         visible triangles, a `lod` level, or draws the RSP takes \
                         (rsp_check; n64-performance skill).",
                        e.cpu_us as f32 / 1000.0,
                        e.rsp_us as f32 / 1000.0,
                        self.profile,
                        self.n64.cpu_draws,
                        self.n64.cpu.verts,
                        self.n64.cpu.tris_in,
                        self.n64.rsp_draws,
                        self.n64.rsp_tris,
                        self.caps.max_3d_us_per_frame as f32 / 1000.0
                    );
                }
            }
        }
        let (instances, segments) = build_segments(&self.commands);
        self.commands.clear();
        // 3D texture bind groups must exist before the pass borrows self.
        let mut textures = Vec::new();
        for seg in &segments {
            match seg {
                Segment::Tris {
                    texture: Some(t), ..
                } => textures.push(*t),
                Segment::Meshes { first, count } => {
                    if let Some(g) = &self.gpu {
                        textures.extend(
                            g.batches(*first..first + count)
                                .iter()
                                .filter_map(|b| b.texture),
                        );
                    }
                }
                _ => {}
            }
        }
        for t in textures {
            let _ = self.tex3d_group(t);
        }
        if let Some(g) = &mut self.gpu {
            self.times.upload_bytes += g.upload(&self.device, &self.queue);
        }

        let bytes: &[u8] = bytemuck::cast_slice(&instances);
        if bytes.len() as u64 > self.instance_capacity {
            self.instance_capacity = (bytes.len() as u64).next_power_of_two();
            self.instance_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("trino-instances"),
                size: self.instance_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !bytes.is_empty() {
            self.queue.write_buffer(&self.instance_buf, 0, bytes);
        }
        self.times.upload_bytes += bytes.len() as u32;

        let tri_bytes: &[u8] = bytemuck::cast_slice(&self.tri_verts);
        if tri_bytes.len() as u64 > self.tri_capacity {
            self.tri_capacity = (tri_bytes.len() as u64).next_power_of_two();
            self.tri_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("trino-tri-verts"),
                size: self.tri_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !tri_bytes.is_empty() {
            self.queue.write_buffer(&self.tri_buf, 0, tri_bytes);
        }
        self.times.upload_bytes += tri_bytes.len() as u32;
        self.tri_verts.clear();
        if segments.iter().any(|s| {
            matches!(
                s,
                Segment::Tris {
                    kind: TriKind::Lit | TriKind::Water,
                    ..
                }
            )
        }) {
            let lights = self.light_sets.bytes();
            self.queue.write_buffer(&self.lights_buf, 0, &lights);
            self.times.upload_bytes += lights.len() as u32;
        }
        // The first water segment: the water sets go up, and the per-pixel
        // tier copies the scene's depth before it (its soft foam reads it).
        let first_water = segments.iter().position(|s| {
            matches!(
                s,
                Segment::Tris {
                    kind: TriKind::Water,
                    ..
                }
            )
        });
        if first_water.is_some() {
            let sets = self.water_sets.bytes();
            self.queue.write_buffer(&self.water.buf, 0, &sets);
            self.times.upload_bytes += sets.len() as u32;
        }
        let depth_copy_at =
            first_water.filter(|_| !second && self.caps.water_tier == WaterTier::PerPixel);

        // In N64 look the framebuffer is 16-bit: the clear (an RDP fill, no
        // dither) keeps the top 5 bits, stored expanded like a 5551 readback
        // (sprites/triangles are dithered+quantized in the shader).
        let ch = |v: u8| -> f64 {
            if self.n64_look {
                let c5 = v >> 3;
                ((c5 << 3) | (c5 >> 2)) as f64 / 255.0
            } else {
                v as f64 / 255.0
            }
        };
        let clear = if second {
            // The touch screen's own background.
            wgpu::Color {
                r: 16.0 / 255.0,
                g: 18.0 / 255.0,
                b: 28.0 / 255.0,
                a: 1.0,
            }
        } else {
            wgpu::Color {
                r: ch(self.clear.r),
                g: ch(self.clear.g),
                b: ch(self.clear.b),
                a: self.clear.a as f64 / 255.0,
            }
        };
        // Screen-space AO darkens the 3D, not the 2D drawn over it: the
        // pass ends after the frame's last 3D draw, the AO passes run, and
        // the rest (HUD, text) draws on top.
        let ao = if second { None } else { self.frame_ao() };
        let split = ao.and_then(|_| {
            segments
                .iter()
                .rposition(|s| matches!(s, Segment::Tris { .. } | Segment::Meshes { .. }))
                .map(|i| i + 1)
        });
        let (first_part, rest) = segments.split_at(split.unwrap_or(segments.len()));

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        // Multisampled (the main screen only): the 3D draws into the MSAA
        // targets and resolves into the offscreen one at the end of the pass.
        let ms = !second && self.msaa_targets.is_some() && self.ms_pipelines.is_some();
        let mut load = (wgpu::LoadOp::Clear(clear), wgpu::LoadOp::Clear(1.0));
        let mut first_part = first_part;
        if let Some(at) = depth_copy_at.filter(|&at| at < first_part.len()) {
            // The scene before the water, its depth copied, then the rest.
            // With MSAA the first pass holds its samples (no resolve) and
            // the copy is multisampled too.
            let (before, after) = first_part.split_at(at);
            let hold = if ms { MsPass::Hold } else { MsPass::Off };
            let (draws, switches) = self.record_segments(&mut encoder, second, before, load, hold);
            self.times.draw_calls += draws;
            self.times.pipeline_switches += switches;
            let (src, dst) = match (&self.msaa_targets, &self.water.ms, ms) {
                (Some(t), Some(w), true) => (&t.depth_tex, &w.depth_copy),
                _ => (&self.depth_tex, &self.water.depth_copy),
            };
            if self.webgl2 {
                // GL can't copy a depth attachment: draw it over.
                let blit = self
                    .depth_blit
                    .get_or_insert_with(|| depth_blit::DepthBlit::new(&self.device));
                let dst = dst.create_view(&wgpu::TextureViewDescriptor::default());
                blit.copy(&self.device, &mut encoder, &self.depth_view, &dst);
            } else {
                encoder.copy_texture_to_texture(
                    src.as_image_copy(),
                    dst.as_image_copy(),
                    wgpu::Extent3d {
                        width: self.internal_size.0,
                        height: self.internal_size.1,
                        depth_or_array_layers: 1,
                    },
                );
            }
            load = (wgpu::LoadOp::Load, wgpu::LoadOp::Load);
            first_part = after;
        }
        let end = if ms {
            MsPass::Resolve {
                keep_depth: ao.is_some() && split.is_some(),
            }
        } else {
            MsPass::Off
        };
        let (draws, switches) = self.record_segments(&mut encoder, second, first_part, load, end);
        self.times.draw_calls += draws;
        self.times.pipeline_switches += switches;
        if let (Some((settings, mask)), Some(_)) = (ao, split) {
            let (w, h) = self.internal_size;
            let half = self.camera.fov_y * 0.5;
            let focal =
                trino_core::math3d::cos(half) / trino_core::math3d::sin(half) * h as f32 * 0.5;
            let fog = self.light.fog;
            let params = ao::AoUniform {
                size: (w, h),
                focal,
                near: trino_core::render3d::NEAR_PLANE,
                ao: settings,
                fog: if fog.enabled() {
                    (fog.near, fog.far)
                } else {
                    (0.0, 0.0)
                },
                mask,
            };
            // With MSAA the occlusion reads the multisampled depth (its
            // first sample); its own pass, as its bind groups hold the view.
            let (pass, depth) = match (&self.msaa_targets, ms) {
                (Some(t), true) => (
                    self.ao_pass_ms.get_or_insert_with(|| {
                        ao::AoPass::new(&self.device, OFFSCREEN_FORMAT, true, false)
                    }),
                    &t.depth,
                ),
                _ => (
                    self.ao_pass.get_or_insert_with(|| {
                        ao::AoPass::new(&self.device, OFFSCREEN_FORMAT, false, self.webgl2)
                    }),
                    &self.depth_view,
                ),
            };
            self.times.upload_bytes += pass.encode(
                &self.device,
                &self.queue,
                &mut encoder,
                depth,
                &self.offscreen_view,
                &params,
            );
            self.times.ao_passes += 1;
            if !rest.is_empty() {
                // The rest is 2D (no depth test): with MSAA it draws
                // single-sampled over the resolved, occluded frame.
                let depth_load = if ms {
                    wgpu::LoadOp::Clear(1.0)
                } else {
                    wgpu::LoadOp::Load
                };
                let (draws, switches) = self.record_segments(
                    &mut encoder,
                    second,
                    rest,
                    (wgpu::LoadOp::Load, depth_load),
                    MsPass::Off,
                );
                self.times.draw_calls += draws;
                self.times.pipeline_switches += switches;
            }
        }

        // N64 Video Interface: dedither the finished 16-bit frame, then put
        // it back where every consumer (window blit, editor, readback)
        // reads it.
        if self.n64_look && self.vi_dedither && !second {
            let vi_view = self
                .vi_tex
                .create_view(&wgpu::TextureViewDescriptor::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("trino-vi-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &vi_view,
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
                pass.set_pipeline(&self.vi_pipeline);
                pass.set_bind_group(0, &self.blit_bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            encoder.copy_texture_to_texture(
                self.vi_tex.as_image_copy(),
                self.offscreen_tex.as_image_copy(),
                wgpu::Extent3d {
                    width: self.internal_size.0,
                    height: self.internal_size.1,
                    depth_or_array_layers: 1,
                },
            );
        }
        self.queue.submit([encoder.finish()]);
    }

    /// One render pass over `segments` into the main target (or the
    /// second screen's), loading or clearing color and depth. Returns the
    /// GPU draw calls and the pipeline changes between them.
    fn record_segments(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        second: bool,
        segments: &[Segment],
        (color_load, depth_load): (wgpu::LoadOp<wgpu::Color>, wgpu::LoadOp<f32>),
        ms: MsPass,
    ) -> (u32, u32) {
        let (color_view, depth_view, globals_bg) = match (&self.second, second) {
            (Some(sc), true) => (&sc.view, &sc.depth_view, &sc.globals_bg),
            _ => (&self.offscreen_view, &self.depth_view, &self.globals_bg),
        };
        // Multisampled: draw into the MSAA targets, resolve the color into
        // the offscreen one at the end of the scene. Nothing is written
        // back but the depth the AO pass reads, unless the scene goes on in
        // another pass (`Hold`: the per-pixel water's depth copy).
        let ms = match (&self.msaa_targets, &self.ms_pipelines, ms) {
            (_, _, MsPass::Off) => None,
            (Some(t), Some(p), mode) => Some((t, p, mode)),
            _ => None,
        };
        let keep = |on: bool| {
            if on {
                wgpu::StoreOp::Store
            } else {
                wgpu::StoreOp::Discard
            }
        };
        let (pass_color, pass_resolve, pass_depth, color_store, depth_store) = match ms {
            Some((t, _, MsPass::Hold)) => (&t.color, None, &t.depth, keep(true), keep(true)),
            Some((t, _, mode)) => (
                &t.color,
                Some(color_view),
                &t.depth,
                keep(false),
                keep(mode == MsPass::Resolve { keep_depth: true }),
            ),
            None => (
                color_view,
                None,
                depth_view,
                wgpu::StoreOp::Store,
                wgpu::StoreOp::Store,
            ),
        };
        let pipes = match ms {
            Some((_, p, _)) => [&p[0], &p[1], &p[2], &p[3]],
            None => [
                &self.sprite_pipeline,
                &self.tri_pipeline,
                &self.tri_clamp_pipeline,
                &self.lit_pipeline,
            ],
        };
        let mut draws = 0;
        // Pipeline changes: the kind each segment binds (sprites, plain,
        // clamped, lit, water, GPU T&L), counted when it differs.
        let (mut last, mut switches) = (u8::MAX, 0u32);
        let mut bind = |kind: u8| {
            if kind != last {
                switches += (last != u8::MAX) as u32;
                last = kind;
            }
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("trino-sprite-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: pass_color,
                resolve_target: pass_resolve,
                ops: wgpu::Operations {
                    load: color_load,
                    store: color_store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: pass_depth,
                depth_ops: Some(wgpu::Operations {
                    load: depth_load,
                    store: depth_store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, globals_bg, &[]);
        for segment in segments {
            match segment {
                Segment::Sprites { sprite, instances } => {
                    if let Some(entry) = self.sprites.get(sprite) {
                        pass.set_pipeline(pipes[0]);
                        bind(0);
                        pass.set_vertex_buffer(0, self.instance_buf.slice(..));
                        pass.set_bind_group(1, &entry.bind_group, &[]);
                        pass.draw(0..4, instances.clone());
                        draws += 1;
                    }
                }
                Segment::Tris {
                    first,
                    count,
                    texture,
                    kind,
                } => {
                    let (pipeline, k) = match kind {
                        TriKind::Water => {
                            pass.set_bind_group(2, &self.lights_bg, &[]);
                            let (bg, per_pixel, vertex) = match (&self.water.ms, ms.is_some()) {
                                (Some(w), true) => (&w.bg, &w.per_pixel, &w.vertex),
                                _ => (&self.water.bg, &self.water.per_pixel, &self.water.vertex),
                            };
                            pass.set_bind_group(3, bg, &[]);
                            // The second screen (3DS profile) is never the
                            // per-pixel tier.
                            if self.caps.water_tier == WaterTier::PerPixel && !second {
                                (per_pixel, 4)
                            } else {
                                (vertex, 4)
                            }
                        }
                        TriKind::Lit => {
                            pass.set_bind_group(2, &self.lights_bg, &[]);
                            (pipes[3], 3)
                        }
                        TriKind::Plain if texture.is_some_and(|t| t.clamp) => (pipes[2], 2),
                        TriKind::Plain => (pipes[1], 1),
                    };
                    bind(k);
                    pass.set_pipeline(pipeline);
                    pass.set_vertex_buffer(0, self.tri_buf.slice(..));
                    let bg = texture
                        .and_then(|t| self.tex3d.get(&t))
                        .unwrap_or(&self.white_bg);
                    pass.set_bind_group(1, bg, &[]);
                    pass.draw(*first..*first + *count, 0..1);
                    draws += 1;
                }
                Segment::Meshes { first, count } => {
                    if let Some(g) = &self.gpu {
                        let texture = |t: Option<TexKey>| {
                            t.and_then(|t| self.tex3d.get(&t)).unwrap_or(&self.white_bg)
                        };
                        bind(5);
                        draws += g.record(&mut pass, *first..first + count, ms.is_some(), &texture);
                    }
                }
            }
        }
        drop(pass);
        (draws, switches)
    }

    /// One instanced GPU T&L draw of `model` (`draw_model_instances` on
    /// the GPU path): the copies' instances, the call's pose and texture.
    fn draw_gpu_instances(
        &mut self,
        model: ModelId,
        params: &ModelParams,
        accepted: &[GpuInstanceOf],
    ) {
        let Some((tmdl, layout)) = self.meshes.get(&model.0) else {
            return;
        };
        let mesh = Mesh::from_layout(tmdl, layout);
        let n = accepted.len() as u32;
        self.frame_stats.models += n;
        self.frame_stats.tnl.instanced += n;
        self.frame_stats.tnl.pc_gpu_draws += n;
        self.frame_stats.tnl.pc_gpu_tris += n * (mesh.index_count / 3) as u32;
        if let Some(t) = mesh
            .texture_for(params)
            .filter(|t| self.sprites.contains_key(&t.0))
        {
            texture_budget(
                TextureBudget {
                    sprites: &self.sprites,
                    caps: &self.caps,
                    strict: self.strict,
                    profile: self.profile,
                    stats: &mut self.frame_stats,
                },
                t,
                true,
            );
        }
        let screen = self.current_screen_size();
        let ctx = tnl_context(&mut self.tnl_ctx, &self.camera, &self.light, screen, &[]);
        let gpu = self.gpu.get_or_insert_with(|| {
            gpu_tnl::GpuTnl::new(&self.device, [&self.globals_bgl, &self.tex3d_bgl])
        });
        if !gpu.has_mesh(model.0) {
            gpu.upload_mesh(&self.device, &self.queue, model.0, &mesh);
        }
        if self.msaa > 1 {
            gpu.ensure_ms(&self.device, self.msaa);
        }
        let insts: Vec<gpu_tnl::GpuInstance> = accepted.iter().map(|a| a.0).collect();
        gpu.draw(
            &ctx,
            gpu_tnl::DrawKey {
                model: model.0,
                mesh: &mesh,
                params,
                texture: accepted[0].1,
            },
            &insts,
        );
    }

    /// Triangles the GPU T&L path submitted this frame (every copy's).
    fn pc_gpu_frame_tris(&self) -> u32 {
        self.frame_stats.tnl.pc_gpu_tris
    }

    /// The end of a frame: render what is left (the second screen is
    /// cleared even when nothing was drawn on it), then show it.
    fn flush(&mut self) {
        if self.screen == 1 {
            self.draw_commands(true);
        } else {
            self.draw_commands(false);
            if self.second.is_some() {
                self.draw_commands(true);
            }
        }
        self.present();
    }

    /// Blit the finished frame(s) to the window, if there is one.
    fn present(&mut self) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        fn acquire(frame: wgpu::CurrentSurfaceTexture) -> Result<wgpu::SurfaceTexture, bool> {
            use wgpu::CurrentSurfaceTexture as C;
            match frame {
                C::Success(f) | C::Suboptimal(f) => Ok(f),
                // Reconfigure-and-retry cases vs. skip-this-frame cases.
                C::Outdated | C::Lost => Err(true),
                C::Timeout | C::Occluded | C::Validation => Err(false),
            }
        }
        let frame =
            self.surface
                .as_ref()
                .and_then(|ss| match acquire(ss.surface.get_current_texture()) {
                    Ok(frame) => Some(frame),
                    Err(true) => {
                        ss.surface.configure(&self.device, &ss.config);
                        acquire(ss.surface.get_current_texture()).ok()
                    }
                    Err(false) => None,
                });

        if let (Some(ss), Some(frame)) = (self.surface.as_ref(), &frame) {
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("trino-blit-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            let surface = (ss.config.width, ss.config.height);
            let second = self.second.as_ref().map(|sc| sc.size);
            let (main, below) = present_layout(self.internal_size, second, surface);
            pass.set_pipeline(&self.blit_pipeline);
            let (vx, vy, vw, vh) = main;
            pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
            pass.set_bind_group(0, &self.blit_bind_group, &[]);
            pass.draw(0..3, 0..1);
            if let (Some(sc), Some((vx, vy, vw, vh))) = (&self.second, below) {
                pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
                pass.set_bind_group(0, &sc.blit_bg, &[]);
                pass.draw(0..3, 0..1);
            }
        }

        self.queue.submit([encoder.finish()]);
        if let Some(frame) = frame {
            frame.present();
        }
    }
}

/// The renderer fields texture accounting reads/writes (disjoint from the
/// mesh being drawn, so it works while a mesh is borrowed).
struct TextureBudget<'a> {
    sprites: &'a HashMap<u32, SpriteEntry>,
    caps: &'a Caps,
    strict: bool,
    profile: SimProfile,
    stats: &'a mut FrameStats,
}

/// The N64 profile's frame, split as the console runs it.
#[derive(Clone, Copy, Default, Debug)]
struct N64Split {
    cpu: trino_core::render3d::TnlStats,
    cpu_draws: u32,
    rsp_draws: u32,
    rsp_tris: u32,
}

/// The native profile's GPU T&L decision for one draw (issue #51,
/// [`trino_core::render3d::pc_gpu_check`]), with the reasons only the
/// renderer knows: `lit` (shaded per pixel: its surface varyings come from
/// the CPU path), `alpha` (its texture blends) and `enabled` (the path is
/// on).
fn pc_gpu_decide(
    (ctx, mesh, matrix, params): (&TnlContext, &Mesh, &Mat34, &ModelParams),
    lit: bool,
    alpha: bool,
    enabled: bool,
) -> Result<(), PcGpuReject> {
    use trino_core::render3d::{pc_gpu_check, pc_gpu_mesh_check};
    pc_gpu_mesh_check(mesh)?;
    pc_gpu_check(ctx, mesh, matrix, params)?;
    if lit {
        return Err(PcGpuReject::PerPixelLit);
    }
    if alpha {
        return Err(PcGpuReject::TextureAlpha);
    }
    if !enabled {
        return Err(PcGpuReject::Disabled);
    }
    Ok(())
}

/// The detail layer's UV scale of a textured draw (0 = none, or its sprite
/// isn't loaded), as the screen-space triangles carry it.
fn detail_scale(
    sprites: &HashMap<u32, SpriteEntry>,
    texture: Option<SpriteId>,
    params: &ModelParams,
) -> f32 {
    match (texture, params.detail) {
        (Some(_), Some(d)) if sprites.contains_key(&d.sprite.0) => d.scale(),
        _ => 0.0,
    }
}

/// `ctx`'s index in a batch's T&L contexts (the last one when equal).
fn push_ctx(ctxs: &mut Vec<TnlContext>, ctx: TnlContext) -> u32 {
    if ctxs.last() != Some(&ctx) {
        ctxs.push(ctx);
    }
    ctxs.len() as u32 - 1
}

/// A copy the GPU T&L path accepted, with its texture state.
type GpuInstanceOf = (gpu_tnl::GpuInstance, Option<TexKey>);

/// [`PcRenderer::n64_estimate`]: the frame's 3D on the N64.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct N64Estimate {
    /// CPU time: the draws it transforms, plus each RSP draw's setup.
    pub cpu_us: u32,
    /// RSP time (its triangles); in parallel with the CPU.
    pub rsp_us: u32,
    pub cpu_draws: u32,
    pub rsp_draws: u32,
    pub rsp_tris: u32,
}

/// Texture-memory accounting (+ strict-mode checks) for a 3D/billboard
/// texture: one texture per draw lives in texture memory as a whole
/// (TMEM on N64) — worst-case 16-bit texels, like sprites.
fn texture_budget(r: TextureBudget<'_>, t: SpriteId, tiled: bool) {
    let Some(entry) = r.sprites.get(&t.0) else {
        return;
    };
    let (w, h) = entry.size;
    // One texture per draw lives in texture memory as a whole
    // (TMEM on N64) — worst-case 16-bit texels, like sprites.
    let bpp = if r.caps.color_depth_bits <= 16 { 2 } else { 4 };
    r.stats.max_texture_bytes = r.stats.max_texture_bytes.max(w * h * bpp);
    if r.strict {
        if tiled && (!w.is_power_of_two() || !h.is_power_of_two()) {
            panic!(
                "strict mode: texture {t:?} ({w}x{h}) is used on a 3D model but is \
                 not power-of-two sized — consoles can only tile POT textures."
            );
        }
        if let Err(e) = r.caps.validate_texture(w as u16, h as u16, bpp) {
            panic!(
                "strict mode: 3D texture {t:?} ({w}x{h}) busts the {:?} profile \
                 budget: {e:?}. One texture per draw must fit texture memory \
                 (N64 TMEM: 4 KB — e.g. 32x32 RGBA16 or 64x64 CI4).",
                r.profile
            );
        }
    }
}

/// The T&L constants for `camera`, `light`, `screen` and the per-vertex
/// local lights (empty on the per-pixel tier), cached in `slot` with the
/// lights they were built for (rebuilt only when one of them changed).
fn tnl_context(
    slot: &mut Option<(TnlContext, LightList)>,
    camera: &Camera3,
    light: &Light,
    screen: Vec2,
    locals: &[LocalLight],
) -> TnlContext {
    match slot {
        Some((c, l)) if c.matches(camera, light, screen) && l.as_slice() == locals => *c,
        _ => {
            let mut list = LightList::EMPTY;
            list.set(locals, trino_core::MAX_LOCAL_LIGHTS as u8);
            let c = TnlContext::new(camera, light, screen).with_local_lights(locals);
            *slot = Some((c, list));
            c
        }
    }
}

impl Renderer for PcRenderer {
    fn caps(&self) -> &Caps {
        &self.caps
    }

    fn begin_frame(&mut self, clear: Color) {
        self.clear = clear;
        self.screen = 0;
        self.main_cmds = 0;
        self.main_tris = 0;
        self.commands.clear();
        self.tri_verts.clear();
        self.pending_tris.clear();
        self.pending_tex.clear();
        self.pending_surf.clear();
        self.locals = LightList::EMPTY;
        self.wetness = 0.0;
        self.ao = None;
        self.light_sets.clear();
        self.water_state = WaterState::NONE;
        self.water_sets.clear();
        self.frame_stats = FrameStats::default();
        self.n64 = N64Split::default();
        self.pc_gpu_off = [0; PcGpuReject::COUNT];
        self.times = PcFrameTimes::default();
        self.cpu_jobs.clear();
        self.job_ctxs.clear();
        if let Some(gpu) = &mut self.gpu {
            gpu.begin_frame();
        }
    }

    fn draw_sprite(&mut self, sprite: SpriteId, pos: Vec2, params: &SpriteParams) {
        self.flush_model_batch();
        let Some(entry) = self.sprites.get(&sprite.0) else {
            // Unknown handle: skip. The asset pipeline (Fase 2) turns this
            // into a hard error at bake time instead.
            return;
        };
        let cmd = make_command(sprite.0, pos, entry.size, params);
        // Texture pressure at the profile's texel size: what a console
        // must stream into texture memory for this draw (the region only —
        // sub-rect blits load just that on N64/3DS).
        let bpp = if self.caps.color_depth_bits <= 16 {
            2
        } else {
            4
        };
        let (tw, th) = (entry.size.0 as f32, entry.size.1 as f32);
        let (rw, rh) = (
            ((cmd.uv1[0] - cmd.uv0[0]).abs() * tw).round() as u32,
            ((cmd.uv1[1] - cmd.uv0[1]).abs() * th).round() as u32,
        );
        let texels = rw * rh;
        if self.strict {
            // Console formats are 16-bit (RGBA5551) or smaller; indexed
            // formats (CI4/CI8) are validated at bake time where the real
            // format is known — this is the worst-case direct-color check
            // of what one draw loads into texture memory.
            if let Err(e) = self.caps.validate_texture(
                rw.min(u16::MAX as u32) as u16,
                rh.min(u16::MAX as u32) as u16,
                bpp,
            ) {
                panic!(
                    "strict mode: sprite {sprite:?} drawn at {rw}x{rh} busts the {:?} \
                     profile budget: {e:?}. Shrink the sprite, draw it in regions (atlas), \
                     or pick an indexed format (CI4/CI8) in assets/manifest.toml.",
                    self.profile
                );
            }
        }
        self.frame_stats.sprites += 1;
        self.frame_stats.max_texture_bytes = self.frame_stats.max_texture_bytes.max(texels * bpp);
        self.commands.push(Cmd::Sprite(cmd));
    }

    fn fill_rect(&mut self, rect: trino_core::Rect, color: Color) {
        // A built-in 1×1 white texel through the sprite path.
        if !self.sprites.contains_key(&FILL_SPRITE.0) {
            self.upload_sprite(FILL_SPRITE, 1, 1, &[255, 255, 255, 255]);
        }
        self.flush_model_batch();
        let params = SpriteParams {
            scale: Vec2::new(rect.size.x, rect.size.y),
            tint: color,
            ..SpriteParams::DEFAULT
        };
        let mut cmd = make_command(FILL_SPRITE.0, rect.pos, (1, 1), &params);
        // The shader's flat-color sentinel (fs_sprite).
        cmd.uv0 = [-2.0, -2.0];
        cmd.uv1 = [-2.0, -2.0];
        self.frame_stats.sprites += 1;
        self.commands.push(Cmd::Sprite(cmd));
    }

    fn set_camera(&mut self, camera: &Camera3) {
        self.flush_model_batch();
        self.camera = *camera;
    }

    fn set_light(&mut self, light: &Light) {
        // Lighting is resolved into vertex colors at tessellation time, so
        // a change needs no batch flush.
        self.light = *light;
    }

    fn set_local_lights(&mut self, lights: &[LocalLight]) {
        // Per vertex they are resolved at tessellation; per pixel each lit
        // vertex names its light set. No flush either way.
        self.locals.set(lights, self.caps.max_local_lights);
    }

    fn set_texture_filter(&mut self, filter: TexFilter) {
        self.tex_filter = match filter {
            TexFilter::Default => default_filter(self.profile),
            f => f,
        };
    }

    fn set_ambient_occlusion(&mut self, ao: Option<&AmbientOcclusion>) {
        self.ao = ao.copied();
    }

    fn set_wetness(&mut self, wetness: f32) {
        self.wetness = wetness.clamp(0.0, 1.0);
    }

    fn set_water(&mut self, time: f32, ripples: &[Ripple]) {
        let cap = self.caps.max_ripples;
        if self.strict {
            let live = ripples.iter().filter(|r| r.alive()).count();
            if live > cap as usize {
                panic!(
                    "strict mode: {live} live ripples this frame, over the {:?} profile's \
                     {cap} (the 3DS water vertex program holds {} in its uniforms; every \
                     target evaluates the first {cap}). Keep a RippleBuffer<{}>.",
                    self.profile,
                    trino_core::MAX_RIPPLES,
                    trino_core::MAX_RIPPLES
                );
            }
        }
        self.water_state = WaterState::new(time, ripples, cap);
    }

    fn draw_model(
        &mut self,
        model: ModelId,
        transform: &Transform3,
        material: Material,
        params: &ModelParams,
    ) {
        self.draw_model_matrix(model, &transform.matrix(), material, params);
    }

    fn draw_model_matrix(
        &mut self,
        model: ModelId,
        matrix: &Mat34,
        _material: Material,
        params: &ModelParams,
    ) {
        let Some((tmdl, layout)) = self.meshes.get(&model.0) else {
            return; // unknown handle: skip, like sprites
        };
        let mesh = Mesh::from_layout(tmdl, layout);
        self.frame_stats.models += 1;
        let texture = mesh
            .texture_for(params)
            .filter(|t| self.sprites.contains_key(&t.0));
        if let Some(t) = texture {
            texture_budget(
                TextureBudget {
                    sprites: &self.sprites,
                    caps: &self.caps,
                    strict: self.strict,
                    profile: self.profile,
                    stats: &mut self.frame_stats,
                },
                t,
                true,
            );
        }
        if let (Some(t), Some(d), true) = (texture, params.detail, self.strict) {
            // Base and detail share texture memory (the N64's TMEM), at
            // worst 16-bit texels; the console drops a detail that doesn't
            // fit, strict mode says so.
            let bytes = |id: SpriteId| self.sprites.get(&id.0).map_or(0, |e| e.size.0 * e.size.1);
            let bpp = if self.caps.color_depth_bits <= 16 {
                2
            } else {
                4
            };
            let total = (bytes(t) + bytes(d.sprite)) * bpp;
            if total > self.caps.texture_memory_bytes {
                panic!(
                    "strict mode: texture {t:?} with detail {:?} needs {total} bytes of \
                     texture memory together, over the {:?} profile's {} (N64 TMEM: \
                     base + detail share 4 KB; use a small I4 detail and a non-palette \
                     base).",
                    d.sprite, self.profile, self.caps.texture_memory_bytes
                );
            }
        }
        let screen = self.current_screen_size();
        // The native profile defers its CPU T&L to the batch flush (worker
        // threads, `cpu_tnl`) and sends what `pc_gpu_check` accepts to the
        // GPU; the console profiles tessellate here, as the consoles do.
        let native = self.profile.is_native();
        let texkey = texture.map(|t| TexKey::model(t.0, params, self.tex_filter));
        if let Some(w) = params.water {
            // Water (ADR 0015): displaced and shaded by the core's water
            // T&L; the per-pixel tier also takes each vertex's surface.
            let before = self.pending_tris.len();
            let ctx = tnl_context(&mut self.tnl_ctx, &self.camera, &self.light, screen, &[])
                .with_water(&self.water_state);
            let set = self.water_sets.index(water::SetKey {
                water: w,
                state: self.water_state,
                gloss: params.gloss,
                detail_scale: match (texture, params.detail) {
                    (Some(_), Some(d)) if self.sprites.contains_key(&d.sprite.0) => d.scale(),
                    _ => 0.0,
                },
                camera: self.camera,
                light: self.light,
            });
            let gloss = params.gloss as f32 / 255.0;
            let per_pixel = self.caps.water_tier == WaterTier::PerPixel;
            if native {
                self.pc_gpu_off[PcGpuReject::Water.index()] += 1;
                let job_ctx = push_ctx(&mut self.job_ctxs, ctx);
                self.cpu_jobs.push(cpu_tnl::CpuJob {
                    kind: cpu_tnl::JobKind::Water {
                        set,
                        gloss,
                        per_pixel,
                    },
                    model: model.0,
                    matrix: *matrix,
                    params: *params,
                    ctx: job_ctx,
                    texture: texkey,
                    tris: (mesh.index_count / 3) as u32,
                });
                return;
            }
            let t0 = Instant::now();
            self.pending_surf.resize(before, None);
            let (pending, surf) = (&mut self.pending_tris, &mut self.pending_surf);
            let tnl = if per_pixel {
                trino_core::render3d::tessellate_surface(
                    &ctx,
                    &mut self.surf_scratch,
                    &mesh,
                    matrix,
                    params,
                    &mut |tri, v| {
                        pending.push(tri);
                        surf.push(Some(SurfTri {
                            v: *v,
                            gloss,
                            set,
                            mask: 0,
                            water: true,
                        }));
                    },
                )
            } else {
                trino_core::render3d::tessellate_with(
                    &ctx,
                    &mut self.tnl_scratch,
                    &mesh,
                    matrix,
                    params,
                    &mut |tri| {
                        pending.push(tri);
                        surf.push(Some(SurfTri {
                            v: [NO_SURFACE; 3],
                            gloss,
                            set,
                            mask: 0,
                            water: true,
                        }));
                    },
                )
            };
            self.times.tnl_us += micros(t0);
            self.frame_stats.tnl.add(&tnl);
            let added = self.pending_tris.len() - before;
            self.tag_pending(before, added, texkey);
            return;
        }
        let before = self.pending_tris.len();
        let per_pixel = self.caps.light_tier == LightTier::PerPixel;
        let ctx = tnl_context(
            &mut self.tnl_ctx,
            &self.camera,
            &self.light,
            screen,
            if per_pixel {
                &[]
            } else {
                self.locals.as_slice()
            },
        );
        // Only the draws a local light reaches (or glossy or wet ones) go
        // through the lit pipeline, and each names the lights that reach it:
        // a frame with a few lamps shades a few draws per pixel, not all.
        let unlit = params.shading == trino_core::Shading::Unlit;
        let reach = if per_pixel && !unlit && !self.locals.is_empty() {
            lights::reach_mask(&mesh, matrix, self.locals.as_slice())
        } else {
            0
        };
        let lit = per_pixel && !unlit && (reach != 0 || self.wetness > 0.0 || params.gloss > 0);
        if native {
            let alpha = texture.is_some_and(|t| self.sprites.get(&t.0).is_some_and(|e| e.alpha));
            let decided = pc_gpu_decide(
                (&ctx, &mesh, matrix, params),
                lit,
                alpha,
                self.gpu_tnl_active(),
            );
            if decided.is_ok() {
                let detail = detail_scale(&self.sprites, texture, params);
                let inst = gpu_tnl::instance(
                    &ctx,
                    &mesh,
                    matrix,
                    params,
                    !trino_core::render3d::split_policy(&ctx, &mesh, params),
                    detail,
                );
                let gpu = self.gpu.get_or_insert_with(|| {
                    gpu_tnl::GpuTnl::new(&self.device, [&self.globals_bgl, &self.tex3d_bgl])
                });
                if !gpu.has_mesh(model.0) {
                    gpu.upload_mesh(&self.device, &self.queue, model.0, &mesh);
                }
                if self.msaa > 1 {
                    gpu.ensure_ms(&self.device, self.msaa);
                }
                gpu.draw(
                    &ctx,
                    gpu_tnl::DrawKey {
                        model: model.0,
                        mesh: &mesh,
                        params,
                        texture: texkey,
                    },
                    &[inst],
                );
                self.frame_stats.tnl.pc_gpu_draws += 1;
                self.frame_stats.tnl.pc_gpu_tris += (mesh.index_count / 3) as u32;
                return;
            }
            if let Err(r) = decided {
                self.pc_gpu_off[r.index()] += 1;
            }
            self.times.lit_draws += lit as u32;
            let kind = if lit {
                cpu_tnl::JobKind::Lit {
                    set: self.light_sets.index(lights::SetKey::new(
                        &self.camera,
                        &self.light,
                        &self.locals,
                        self.wetness,
                    )),
                    mask: reach,
                    gloss: params.gloss as f32 / 255.0,
                }
            } else {
                cpu_tnl::JobKind::Plain
            };
            let job_ctx = push_ctx(&mut self.job_ctxs, ctx);
            self.cpu_jobs.push(cpu_tnl::CpuJob {
                kind,
                model: model.0,
                matrix: *matrix,
                params: *params,
                ctx: job_ctx,
                texture: texkey,
                tris: (mesh.index_count / 3) as u32,
            });
            return;
        }
        let t0 = Instant::now();
        self.times.lit_draws += lit as u32;
        let tnl = if lit {
            let set = self.light_sets.index(lights::SetKey::new(
                &self.camera,
                &self.light,
                &self.locals,
                self.wetness,
            ));
            let gloss = params.gloss as f32 / 255.0;
            self.pending_surf.resize(before, None);
            let (pending, surf) = (&mut self.pending_tris, &mut self.pending_surf);
            trino_core::render3d::tessellate_surface(
                &ctx,
                &mut self.surf_scratch,
                &mesh,
                matrix,
                params,
                &mut |tri, v| {
                    pending.push(tri);
                    surf.push(Some(SurfTri {
                        v: *v,
                        gloss,
                        set,
                        mask: reach,
                        water: false,
                    }));
                },
            )
        } else {
            let pending = &mut self.pending_tris;
            trino_core::render3d::tessellate_with(
                &ctx,
                &mut self.tnl_scratch,
                &mesh,
                matrix,
                params,
                &mut |tri| pending.push(tri),
            )
        };
        self.times.tnl_us += micros(t0);
        self.frame_stats.tnl.add(&tnl);
        if self.profile == SimProfile::N64 {
            // What the console does with it: its RSP overlay takes the draws
            // `rsp_eligible` accepts (the same rules, from the core); the CPU
            // transforms the rest.
            let rsp = trino_core::render3d::rsp_mesh_supported(&mesh)
                && trino_core::render3d::rsp_eligible(&ctx, &mesh, matrix, params).is_some();
            if rsp {
                self.n64.rsp_draws += 1;
                self.n64.rsp_tris += (mesh.index_count / 3) as u32;
            } else {
                self.n64.cpu_draws += 1;
                self.n64.cpu.add(&tnl);
            }
        }
        let added = self.pending_tris.len() - before;
        self.tag_pending(before, added, texkey);
    }

    fn draw_model_instances_with(
        &mut self,
        model: ModelId,
        instances: &[Instance],
        palettes: &[Palette],
        params: &ModelParams,
    ) {
        if !self.meshes.contains_key(&model.0) {
            return;
        }
        if !self.gpu_tnl_active() {
            // The trait's loop, counted (the CPU path and the console
            // profiles).
            for i in instances {
                let p = i.params(params, palettes);
                self.draw_model_matrix(model, &i.matrix, Material::VertexLit, &p);
                self.frame_stats.tnl.instanced += 1;
            }
            return;
        }
        // The GPU path: every copy it accepts goes into one instanced draw;
        // a copy it doesn't (off screen, translucent) takes the CPU path
        // alone, as a separate draw would.
        let mut accepted: Vec<GpuInstanceOf> = Vec::with_capacity(instances.len());
        let mut rest: Vec<(Mat34, ModelParams)> = Vec::new();
        {
            let Some((tmdl, layout)) = self.meshes.get(&model.0) else {
                return;
            };
            let mesh = Mesh::from_layout(tmdl, layout);
            let screen = self.current_screen_size();
            let ctx = tnl_context(&mut self.tnl_ctx, &self.camera, &self.light, screen, &[]);
            let per_pixel = self.caps.light_tier == LightTier::PerPixel;
            for i in instances {
                let p = i.params(params, palettes);
                let unlit = p.shading == trino_core::Shading::Unlit;
                let reach = if per_pixel && !unlit && !self.locals.is_empty() {
                    lights::reach_mask(&mesh, &i.matrix, self.locals.as_slice())
                } else {
                    0
                };
                let lit = per_pixel && !unlit && (reach != 0 || self.wetness > 0.0 || p.gloss > 0);
                let texture = mesh
                    .texture_for(&p)
                    .filter(|t| self.sprites.contains_key(&t.0));
                let alpha =
                    texture.is_some_and(|t| self.sprites.get(&t.0).is_some_and(|e| e.alpha));
                // Copies share the call's surface but for tint and palette,
                // so one texture state and one pose for all of them.
                if p.water.is_none()
                    && pc_gpu_decide((&ctx, &mesh, &i.matrix, &p), lit, alpha, true).is_ok()
                {
                    let detail = detail_scale(&self.sprites, texture, &p);
                    let affine = !trino_core::render3d::split_policy(&ctx, &mesh, &p);
                    accepted.push((
                        gpu_tnl::instance(&ctx, &mesh, &i.matrix, &p, affine, detail),
                        texture.map(|t| TexKey::model(t.0, &p, self.tex_filter)),
                    ));
                } else {
                    rest.push((i.matrix, p));
                }
            }
        }
        if !accepted.is_empty() {
            self.draw_gpu_instances(model, params, &accepted);
        }
        for (m, p) in rest {
            self.draw_model_matrix(model, &m, Material::VertexLit, &p);
            self.frame_stats.tnl.instanced += 1;
        }
    }

    fn draw_billboard(
        &mut self,
        sprite: SpriteId,
        pos: trino_core::Vec3,
        params: &trino_core::BillboardParams,
    ) {
        let Some(entry) = self.sprites.get(&sprite.0) else {
            return;
        };
        let (w, h) = (entry.size.0 as f32, entry.size.1 as f32);
        let uv = trino_core::render3d::region_uv(params.region, w, h);
        texture_budget(
            TextureBudget {
                sprites: &self.sprites,
                caps: &self.caps,
                strict: self.strict,
                profile: self.profile,
                stats: &mut self.frame_stats,
            },
            sprite,
            false,
        );
        let screen = self.current_screen_size();
        let locals = match self.caps.light_tier {
            LightTier::PerPixel => &[],
            LightTier::PerVertex => self.locals.as_slice(),
        };
        let ctx = tnl_context(&mut self.tnl_ctx, &self.camera, &self.light, screen, locals);
        let Some(tris) = trino_core::render3d::billboard_with(&ctx, pos, params, uv) else {
            return;
        };
        self.frame_stats.sprites += 1;
        let texture = Some(TexKey::billboard(sprite.0, self.tex_filter));
        if self.profile.is_native() {
            // In order with the batch's deferred CPU draws.
            self.cpu_jobs.push(cpu_tnl::CpuJob {
                kind: cpu_tnl::JobKind::Ready(tris),
                model: 0,
                matrix: Mat34::IDENTITY,
                params: ModelParams::DEFAULT,
                ctx: 0,
                texture,
                tris: 2,
            });
            return;
        }
        let before = self.pending_tris.len();
        self.pending_tris.extend_from_slice(&tris);
        self.tag_pending(before, 2, texture);
    }

    fn second_screen(&self) -> Option<Vec2> {
        self.second
            .as_ref()
            .map(|sc| Vec2::new(sc.size.0 as f32, sc.size.1 as f32))
    }

    fn set_screen(&mut self, screen: u8) {
        // Main screen first, then the second (once per frame).
        if screen == 1 && self.screen == 0 && self.second.is_some() {
            self.flush_model_batch();
            let (cmds, tris) = (
                self.commands.len() as u32,
                (self.tri_verts.len() / 3) as u32,
            );
            self.draw_commands(false);
            (self.main_cmds, self.main_tris) = (cmds, tris);
            self.screen = 1;
        }
    }

    fn screen(&self) -> Screen {
        if self.screen == 1 {
            Screen::Bottom
        } else {
            Screen::Top
        }
    }

    fn screen_size(&self, screen: Screen) -> Option<Vec2> {
        match screen {
            Screen::Top => Some(Vec2::new(
                self.internal_size.0 as f32,
                self.internal_size.1 as f32,
            )),
            Screen::Bottom => self.second_screen(),
        }
    }

    fn end_frame(&mut self) {
        self.flush_model_batch();
        self.frame_stats.tris =
            self.main_tris + (self.tri_verts.len() / 3) as u32 + self.frame_stats.tnl.pc_gpu_tris;
        self.last_stats = self.frame_stats;
        self.last_pc_gpu_off = self.pc_gpu_off;
        let t0 = Instant::now();
        self.flush();
        self.times.submit_us += micros(t0);
        self.last_times = self.times;
    }

    fn draw_text(&mut self, font: FontId, pos: Vec2, text: &str, params: &TextParams) {
        let Some(entry) = self.fonts.get(&font.0).cloned() else {
            return;
        };
        let parsed = entry.font();
        trino_core::text::draw(
            self,
            trino_core::text::atlas_of(font),
            &parsed,
            pos,
            text,
            params,
        );
    }

    fn measure_text(&self, font: FontId, text: &str, scale: f32) -> Vec2 {
        self.fonts
            .get(&font.0)
            .map_or(Vec2::ZERO, |e| e.font().measure(text, scale))
    }

    fn stats(&self) -> FrameStats {
        self.last_stats
    }

    fn read_pixel(&mut self, x: u32, y: u32) -> Option<Color> {
        let (w, h) = self.internal_size;
        if x >= w || y >= h {
            return None;
        }
        // None where the backend can't wait for the GPU (a browser's
        // WebGPU): the self-test reports its probes as skipped there.
        let rgba = self.try_read_offscreen()?;
        let at = ((y * w + x) * 4) as usize;
        Some(Color::rgba(
            rgba[at],
            rgba[at + 1],
            rgba[at + 2],
            rgba[at + 3],
        ))
    }

    fn read_screen_pixel(&mut self, screen: Screen, x: u32, y: u32) -> Option<Color> {
        let Screen::Bottom = screen else {
            return self.read_pixel(x, y);
        };
        let (w, h) = self.second.as_ref()?.size;
        if x >= w || y >= h {
            return None;
        }
        let rgba = self.try_read_texture(&self.second.as_ref()?.tex, (w, h))?;
        let at = ((y * w + x) * 4) as usize;
        Some(Color::rgba(
            rgba[at],
            rgba[at + 1],
            rgba[at + 2],
            rgba[at + 3],
        ))
    }
}

/// A framebuffer read under way ([`PcRenderer::start_read_offscreen`]).
pub struct PendingRead {
    buffer: wgpu::Buffer,
    padded: u32,
    size: (u32, u32),
    /// Set by the map callback: the map's outcome.
    done: std::sync::Arc<std::sync::Mutex<Option<Result<(), String>>>>,
}

impl PendingRead {
    /// The framebuffer size being read.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }
}

/// Tightly packed rows from a readback buffer whose rows are `padded`
/// bytes apart.
fn unpad_rows(data: &[u8], size: (u32, u32), padded: u32) -> Vec<u8> {
    let (w, h) = size;
    let unpadded = (w * 4) as usize;
    let mut out = Vec::with_capacity(unpadded * h as usize);
    for row in 0..h as usize {
        let start = row * padded as usize;
        out.extend_from_slice(&data[start..start + unpadded]);
    }
    out
}

/// The texel `fill_rect` stretches (a handle no asset path hashes to in
/// practice: fnv1a of a reserved name).
const FILL_SPRITE: SpriteId = SpriteId::from_path("\0trino/fill");

/// Where the internal framebuffer lands in the window: nearest integer
/// upscale, centered (fractional only when the window is smaller than the
/// framebuffer). `(x, y, width, height)` in window pixels.
fn present_rect(internal: (u32, u32), surface: (u32, u32)) -> (f32, f32, f32, f32) {
    let (iw, ih) = (internal.0 as f32, internal.1 as f32);
    let (sw, sh) = (surface.0 as f32, surface.1 as f32);
    let fit = (sw / iw).min(sh / ih);
    let scale = if fit >= 1.0 { fit.floor() } else { fit };
    let (vw, vh) = (iw * scale, ih * scale);
    ((sw - vw) * 0.5, (sh - vh) * 0.5, vw, vh)
}

/// A picture's place in the window: `(x, y, width, height)`, pixels.
type Viewport = (f32, f32, f32, f32);

/// Where the main picture and (with one) the second screen land in the
/// window: one whole-number scale for both, the second screen centered
/// under the main one (the 3DS's two screens), the pair centered.
fn present_layout(
    main: (u32, u32),
    second: Option<(u32, u32)>,
    surface: (u32, u32),
) -> (Viewport, Option<Viewport>) {
    let Some(sec) = second else {
        return (present_rect(main, surface), None);
    };
    let (tw, th) = (main.0.max(sec.0), main.1 + sec.1);
    let (x, y, w, _) = present_rect((tw, th), surface);
    let k = w / tw as f32;
    let top = (
        x + (tw - main.0) as f32 * 0.5 * k,
        y,
        main.0 as f32 * k,
        main.1 as f32 * k,
    );
    let below = (
        x + (tw - sec.0) as f32 * 0.5 * k,
        y + main.1 as f32 * k,
        sec.0 as f32 * k,
        sec.1 as f32 * k,
    );
    (top, Some(below))
}

/// A window pixel on the presented picture(s): the screen it falls on and
/// the position in that screen's pixels.
fn window_to_screen(
    main: (u32, u32),
    second: Option<(u32, u32)>,
    surface: (u32, u32),
    x: f32,
    y: f32,
) -> Option<(trino_core::Vec2, u8)> {
    let (top, below) = present_layout(main, second, surface);
    let hit = |r: (f32, f32, f32, f32), size: (u32, u32)| {
        let (u, v) = ((x - r.0) / r.2, (y - r.1) / r.3);
        ((0.0..1.0).contains(&u) && (0.0..1.0).contains(&v))
            .then(|| trino_core::Vec2::new(u * size.0 as f32, v * size.1 as f32))
    };
    if let Some(p) = hit(top, main) {
        return Some((p, 0));
    }
    let (b, s) = (below?, second?);
    hit(b, s).map(|p| (p, 1))
}

/// Inverse of [`present_rect`] for one window pixel.
#[cfg(test)]
fn window_to_game(
    internal: (u32, u32),
    surface: (u32, u32),
    x: f32,
    y: f32,
) -> Option<trino_core::Vec2> {
    let (vx, vy, vw, vh) = present_rect(internal, surface);
    let (u, v) = ((x - vx) / vw, (y - vy) / vh);
    if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
        return None;
    }
    Some(trino_core::Vec2::new(
        u * internal.0 as f32,
        v * internal.1 as f32,
    ))
}

#[cfg(test)]
mod present_tests {
    use super::*;

    /// Native pictures on real monitors: a whole-number upscale, the same
    /// in x and y (never stretched), bars only where the aspect is out of
    /// 4:3..21:9, and the pointer mapping back to the picture's corners.
    #[test]
    fn native_pictures_are_never_stretched() {
        for out in [
            (1280, 720),
            (1366, 768),
            (1920, 1080),
            (2560, 1080),
            (3440, 1440),
            (3840, 1080),
            (5120, 1440),
            (1280, 1024),
            (3840, 2160),
            (1080, 1920),
        ] {
            for scale in 0..=4 {
                let (w, h, k) = crate::sim::native_internal(out, scale);
                let (x, y, vw, vh) = present_rect((w, h), out);
                assert_eq!(
                    (vw, vh),
                    ((w * k) as f32, (h * k) as f32),
                    "{out:?} {scale}"
                );
                assert!(
                    x >= 0.0 && y >= 0.0,
                    "{out:?}: picture larger than the output"
                );
                let aspect = out.0 as f32 / out.1 as f32;
                if (crate::sim::MIN_ASPECT..=crate::sim::MAX_ASPECT).contains(&aspect) {
                    // In range: at most a few pixels of rounding per side.
                    assert!(
                        x <= k as f32 * 2.0 && y <= k as f32 * 2.0,
                        "{out:?} {scale}: {x},{y}"
                    );
                }
                let p = window_to_game((w, h), out, x + vw - 0.5, y + vh - 0.5).unwrap();
                assert!(p.x < w as f32 && p.y < h as f32 && p.x > w as f32 - 1.0);
                assert_eq!(window_to_game((w, h), out, x - 1.0, y), None);
            }
        }
        // 32:9: a 21:9 picture with side bars.
        let (w, h, k) = crate::sim::native_internal((3840, 1080), 0);
        let (x, _, _, _) = present_rect((w, h), (3840, 1080));
        assert!(x > 400.0, "side bars on 32:9 ({w}x{h} x{k}, bar {x})");
    }

    /// The 3DS profile: top 400×240 over bottom 320×240, one scale, the
    /// bottom centered; the pointer knows which screen it is on.
    #[test]
    fn two_screens_stack_and_map_the_pointer() {
        let (top, below) = present_layout((400, 240), Some((320, 240)), (1280, 960));
        // 400×480 into 1280×960: scale 2.
        assert_eq!(top, (240.0, 0.0, 800.0, 480.0));
        assert_eq!(below, Some((320.0, 480.0, 640.0, 480.0)));
        let s = Some((320, 240));
        let (p, screen) = window_to_screen((400, 240), s, (1280, 960), 250.0, 10.0).unwrap();
        assert_eq!((p.x, p.y, screen), (5.0, 5.0, 0));
        let (p, screen) = window_to_screen((400, 240), s, (1280, 960), 330.0, 500.0).unwrap();
        assert_eq!((p.x, p.y, screen), (5.0, 10.0, 1));
        // Beside the narrower bottom screen: nothing.
        assert_eq!(
            window_to_screen((400, 240), s, (1280, 960), 250.0, 600.0),
            None
        );
    }

    #[test]
    fn letterboxed_integer_upscale_maps_back() {
        // 320x240 in a 1280x720 window: scale 3, 960x720 centered at x=160.
        assert_eq!(
            present_rect((320, 240), (1280, 720)),
            (160.0, 0.0, 960.0, 720.0)
        );
        let p = window_to_game((320, 240), (1280, 720), 160.0 + 30.0, 9.0).unwrap();
        assert_eq!((p.x, p.y), (10.0, 3.0));
        // The black bars are outside the picture.
        assert_eq!(window_to_game((320, 240), (1280, 720), 100.0, 10.0), None);
        assert_eq!(window_to_game((320, 240), (1280, 720), 1125.0, 10.0), None);
        // Smaller window: fractional downscale still maps corners.
        let p = window_to_game((640, 480), (320, 240), 319.0, 239.0).unwrap();
        assert!((p.x - 638.0).abs() < 0.01 && (p.y - 478.0).abs() < 0.01);
    }
}

#[cfg(test)]
mod tnl_setting_tests {
    #[test]
    fn trino_pc_tnl_cpu_turns_the_gpu_path_off() {
        for v in [None, Some("gpu"), Some("auto"), Some("1"), Some("")] {
            assert!(super::pc_tnl_setting(v), "{v:?}");
        }
        for v in ["cpu", "0", "off"] {
            assert!(!super::pc_tnl_setting(Some(v)), "{v}");
        }
    }
}

#[cfg(test)]
mod webgl2_tests {
    use super::*;

    /// Every entry point of `source` through naga's GLSL ES 3.00 backend,
    /// the one wgpu's GL backend runs in a browser without WebGPU. `Err`
    /// names the first entry point that doesn't translate.
    fn glsl_es_300(source: &str) -> Result<usize, String> {
        use naga::back::glsl;
        let module = naga::front::wgsl::parse_str(source).map_err(|e| e.emit_to_string(source))?;
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .map_err(|e| format!("{e:?}"))?;
        let options = glsl::Options {
            version: glsl::Version::Embedded {
                version: 300,
                is_webgl: true,
            },
            ..Default::default()
        };
        for ep in &module.entry_points {
            let pipeline = glsl::PipelineOptions {
                shader_stage: ep.stage,
                entry_point: ep.name.clone(),
                multiview: None,
            };
            let mut out = String::new();
            glsl::Writer::new(
                &mut out,
                &module,
                &info,
                &options,
                &pipeline,
                naga::proc::BoundsCheckPolicies::default(),
            )
            .and_then(|mut w| w.write().map(|_| ()))
            .map_err(|e| format!("{}: {e}", ep.name))?;
        }
        Ok(module.entry_points.len())
    }

    /// The scene, AO and GPU T&L shaders as the WebGL2 renderer builds them
    /// (`depth_as_float`) translate to GLSL ES 3.00, entry point by entry
    /// point — and the scene shader as written would not: its depth read is
    /// why the transform exists (the browser's first run failed there).
    #[test]
    fn webgl2_shaders_translate_to_glsl_es_300() {
        for (name, source) in [
            ("shaders.wgsl", include_str!("shaders.wgsl")),
            ("ao.wgsl", include_str!("ao.wgsl")),
            ("shaders_tnl.wgsl", include_str!("shaders_tnl.wgsl")),
            ("depth_blit", depth_blit::SHADER),
        ] {
            let n = glsl_es_300(&depth_as_float(source)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(n > 0, "{name}");
        }
        let raw = glsl_es_300(include_str!("shaders.wgsl")).unwrap_err();
        assert!(raw.contains("fs_water"), "{raw}");
    }

    /// The transform adds one nearest sampler per depth texture, at the
    /// binding the bind groups give it, and reads the red channel.
    #[test]
    fn depth_reads_go_through_the_nearest_sampler() {
        let water = depth_as_float(include_str!("shaders.wgsl"));
        assert!(!water.contains("texture_depth_2d"));
        let (water_slot, ao_slot) = NEAREST_DEPTH_BINDING;
        assert!(
            water.contains(&format!(
                "@group(3) @binding({water_slot}) var s_t_scene_depth: sampler;"
            )),
            "the water's sampler"
        );
        assert!(water.contains("textureSampleLevel(t_scene_depth, s_t_scene_depth"));
        let ao = depth_as_float(include_str!("ao.wgsl"));
        assert!(ao.contains(&format!(
            "@group(0) @binding({ao_slot}) var s_t_depth: sampler;"
        )));
        // A shader without a depth read is left as it was.
        let tnl = include_str!("shaders_tnl.wgsl");
        assert_eq!(
            depth_as_float(tnl),
            tnl.replace("texture_depth_2d", "texture_2d<f32>")
        );
        assert_eq!(
            depth_sample_type(true),
            wgpu::TextureSampleType::Float { filterable: false }
        );
        assert_eq!(depth_sample_type(false), wgpu::TextureSampleType::Depth);
    }

    #[test]
    fn web_backends_parse_from_the_page_query() {
        assert_eq!(WebBackend::parse("auto"), Some(WebBackend::Auto));
        assert_eq!(WebBackend::parse("WebGPU"), Some(WebBackend::WebGpu));
        assert_eq!(WebBackend::parse("webgl"), Some(WebBackend::WebGl2));
        assert_eq!(WebBackend::parse("vulkan"), None);
        // ETC2 and ASTC uploads for phones, by their device features.
        assert_eq!(CompressedFormat::Etc2Rgb8.block_bytes(), 8);
        assert_eq!(CompressedFormat::Astc4x4.block_bytes(), 16);
        assert_eq!(
            CompressedFormat::Astc4x4.feature(),
            wgpu::Features::TEXTURE_COMPRESSION_ASTC
        );
        assert!(CompressedFormat::Bc7.is_bc() && !CompressedFormat::Etc2Rgba8.is_bc());
    }
}
