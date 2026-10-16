//! wgpu 2D sprite renderer with an internal offscreen framebuffer.
//!
//! Two construction modes:
//! - [`PcRenderer::new_windowed`]: renders offscreen, then blits to the
//!   window surface with nearest-neighbor integer upscaling (letterboxed).
//! - [`PcRenderer::new_headless`]: offscreen only; tests read pixels back
//!   with [`PcRenderer::read_offscreen`]. This is also the path the editor
//!   viewport reuses in Fase 3 (render-to-texture).

mod batch;
mod lights;

use std::collections::HashMap;
use std::sync::Arc;

use trino_core::render3d::{
    Camera3, DEFAULT_LIGHT, Light, Mesh, MeshLayout, ScreenTri, SurfaceScratch, SurfaceVert,
    TnlContext, TnlScratch,
};
use trino_core::{
    Caps, Color, FontId, FrameStats, LightList, LightTier, LocalLight, Mat34, Material, ModelId,
    ModelParams, Renderer, SpriteId, SpriteParams, TextParams, Transform3, Vec2,
};

use crate::sim::SimProfile;
use batch::{
    Cmd, INSTANCE_STRIDE, Segment, TRI_VERTEX_STRIDE, TexKey, TriVertex, build_segments,
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
}

struct SurfaceState {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

pub struct PcRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: Option<SurfaceState>,
    offscreen_view: wgpu::TextureView,
    offscreen_tex: wgpu::Texture,
    depth_view: wgpu::TextureView,
    sprite_pipeline: wgpu::RenderPipeline,
    tri_pipeline: wgpu::RenderPipeline,
    /// Same as `tri_pipeline` with clamped 3-point sampling (billboards).
    tri_clamp_pipeline: wgpu::RenderPipeline,
    /// Per-pixel lighting (`LightTier::PerPixel`): local lights, specular
    /// and wetness on top of the vertex color.
    lit_pipeline: wgpu::RenderPipeline,
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
    /// Sprite textures as 3D textures (wrapping sampler), by handle.
    tex3d: HashMap<TexKey, wgpu::BindGroup>,
    tex3d_sampler: wgpu::Sampler,
    /// Clamp-to-edge twin for billboards (sprite regions must not bleed).
    clamp_sampler: wgpu::Sampler,
    /// 1x1 white: what untextured triangles "sample" (×1 = unchanged).
    white_bg: wgpu::BindGroup,
    /// TMDL blobs with their validated layout (parsed once, at register).
    meshes: HashMap<u32, (Vec<u8>, MeshLayout)>,
    /// Glyph tables (`TFNT`) by font handle; the atlas is the sprite with
    /// the same handle. `Arc` so `draw_text` can lay out while drawing.
    fonts: HashMap<u32, Arc<[u8]>>,
    commands: Vec<Cmd>,
    /// Counters of the frame being recorded / the last finished frame.
    frame_stats: FrameStats,
    last_stats: FrameStats,
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
    light_sets: lights::LightSets,
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
    /// The 3DS profile's touch screen (320×240, 2D only).
    second: Option<SecondScreen>,
    /// "name (backend)" of the GPU adapter.
    adapter_name: String,
    /// Where draws go now (0 main, 1 second) and whether the main screen
    /// was already rendered this frame (`set_screen(1)` renders it).
    screen: u8,
    main_done: bool,
}

/// What a lit triangle adds to its vertices: the surface, the gloss and the
/// light set.
#[derive(Clone, Copy, Debug)]
struct SurfTri {
    v: [SurfaceVert; 3],
    gloss: f32,
    set: u32,
    /// The local lights that reach the draw (`lights::reach_mask`).
    mask: u32,
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
) -> (wgpu::Texture, wgpu::TextureView, wgpu::TextureView) {
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());
    (offscreen_tex, offscreen_view, depth_view)
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

impl PcRenderer {
    /// Renderer that presents to a window.
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
        Self::init(
            instance,
            Some((surface, size.width.max(1), size.height.max(1))),
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
        Self::build(device, queue, None, profile, "editor device".into())
    }

    async fn init(
        instance: wgpu::Instance,
        surface: Option<(wgpu::Surface<'static>, u32, u32)>,
        profile: SimProfile,
    ) -> Result<Self, RendererError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: surface.as_ref().map(|(s, _, _)| s),
                ..Default::default()
            })
            .await
            .map_err(|e| RendererError::NoAdapter(e.to_string()))?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("trino-pc"),
                required_features: wgpu::Features::empty(),
                // Downlevel limits, but the adapter's real texture size:
                // a 4K or ultrawide window (surface) and full-resolution
                // pictures need more than downlevel's 2048.
                required_limits: wgpu::Limits {
                    max_texture_dimension_2d: adapter.limits().max_texture_dimension_2d.min(8192),
                    ..wgpu::Limits::downlevel_defaults()
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

        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        Ok(Self::build(device, queue, surface, profile, name))
    }

    fn build(
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface: Option<SurfaceState>,
        profile: SimProfile,
        adapter_name: String,
    ) -> Self {
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

        let (offscreen_tex, offscreen_view, depth_view) = create_targets(&device, internal_size);

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
        // 3D textures tile (wrap); same filter policy as sprites (the N64
        // 3-point filter is emulated in the shader).
        let tex3d_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("trino-tex3d-sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: sprite_filter,
            min_filter: sprite_filter,
            ..Default::default()
        });
        let clamp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("trino-clamp-sampler"),
            mag_filter: sprite_filter,
            min_filter: sprite_filter,
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

        let shader = device.create_shader_module(wgpu::include_wgsl!("shaders.wgsl"));

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

        let sprite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("trino-sprite-pipeline"),
            layout: Some(&sprite_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sprite"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: INSTANCE_STRIDE,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &instance_attrs,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
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
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // 3D triangle pipeline: same offscreen target; vertex colors
        // modulated by one texture per segment (white when untextured).
        let tri_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-tri-layout"),
            bind_group_layouts: &[Some(&globals_bgl), Some(&texture_bgl)],
            immediate_size: 0,
        });
        let tri_attrs = [
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
        ];
        let make_tri_pipeline = |fragment: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("trino-tri-pipeline"),
                layout: Some(&tri_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_tri"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: TRI_VERTEX_STRIDE,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &tri_attrs,
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
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
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let tri_pipeline = make_tri_pipeline("fs_tri");
        let tri_clamp_pipeline = make_tri_pipeline("fs_tri_clamp");

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
            bind_group_layouts: &[Some(&globals_bgl), Some(&texture_bgl), Some(&lights_bgl)],
            immediate_size: 0,
        });
        let lit_attrs = {
            let extra = [
                (wgpu::VertexFormat::Float32x3, 40, 5),
                (wgpu::VertexFormat::Float32x3, 52, 6),
                (wgpu::VertexFormat::Float32x4, 64, 7),
                (wgpu::VertexFormat::Float32, 80, 8),
                (wgpu::VertexFormat::Float32, 84, 9),
            ];
            let mut a = tri_attrs.to_vec();
            a.extend(extra.iter().map(|&(format, offset, shader_location)| {
                wgpu::VertexAttribute {
                    format,
                    offset,
                    shader_location,
                }
            }));
            a
        };
        let lit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("trino-lit-pipeline"),
            layout: Some(&lit_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_tri_lit"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: TRI_VERTEX_STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &lit_attrs,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
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
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

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
                module: &shader,
                entry_point: Some("vs_blit"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
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
            let (tex, view, depth_view) = create_targets(&device, size);
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
                module: &shader,
                entry_point: Some("vs_blit"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
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
        let white_bg = create_blit_bind_group(&device, &texture_bgl, &white_view, &tex3d_sampler);

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
            sprite_pipeline,
            tri_pipeline,
            tri_clamp_pipeline,
            lit_pipeline,
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
            tex3d_sampler,
            clamp_sampler,
            white_bg,
            meshes: HashMap::new(),
            fonts: HashMap::new(),
            commands: Vec::new(),
            frame_stats: FrameStats::default(),
            last_stats: FrameStats::default(),
            tri_verts: Vec::new(),
            pending_tris: Vec::new(),
            pending_tex: Vec::new(),
            pending_surf: Vec::new(),
            locals: LightList::EMPTY,
            wetness: 0.0,
            light_sets: lights::LightSets::default(),
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
            main_done: false,
            adapter_name,
        }
    }

    /// Register a baked TMDL mesh under a stable handle. Re-uploading the
    /// same id replaces the content (live reload).
    pub fn upload_mesh(&mut self, id: ModelId, tmdl: Vec<u8>) {
        let layout = match Mesh::from_tmdl(&tmdl) {
            Ok(m) => m.layout(&tmdl),
            Err(e) => panic!("mesh {id:?}: invalid TMDL blob: {e:?}"),
        };
        self.meshes.insert(id.0, (tmdl, layout));
    }

    /// Register a baked font's glyph table (`TFNT`) under its handle; the
    /// atlas must be uploaded as the sprite with the same handle.
    /// Re-uploading replaces it (live reload).
    pub fn upload_font(&mut self, id: FontId, table: Vec<u8>) {
        if let Err(e) = trino_core::text::BitmapFont::parse(&table) {
            panic!("font {id:?}: invalid TFNT table: {e:?}");
        }
        self.fonts.insert(id.0, table.into());
    }

    /// Toggle the N64 output emulation (3-point filtering, RGBA5551 +
    /// ordered dither). Defaults to on for [`SimProfile::N64`].
    pub fn set_n64_look(&mut self, on: bool) {
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
        let (tex, view, depth) = create_targets(&self.device, size);
        self.blit_bind_group =
            create_blit_bind_group(&self.device, &self.sprite_bgl, &view, &self.blit_sampler);
        self.offscreen_tex = tex;
        self.offscreen_view = view;
        self.depth_view = depth;
        self.vi_tex = create_vi_target(&self.device, size);
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
            format: OFFSCREEN_FORMAT,
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
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: None,
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
        self.tex3d.retain(|k, _| k.id != id.0);
        self.sprites.insert(
            id.0,
            SpriteEntry {
                bind_group,
                view,
                size: (width, height),
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

    /// Read the internal framebuffer back as tightly-packed RGBA8.
    /// Test/tooling path — synchronous, not for per-frame use.
    pub fn read_offscreen(&self) -> Vec<u8> {
        self.read_texture(&self.offscreen_tex, self.internal_size)
    }

    fn read_texture(&self, tex: &wgpu::Texture, size: (u32, u32)) -> Vec<u8> {
        let (w, h) = size;
        let unpadded = w * 4;
        let padded = unpadded.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
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

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv()
            .expect("map_async callback dropped")
            .expect("failed to map readback buffer");

        let data = slice.get_mapped_range();
        let mut out = Vec::with_capacity((unpadded * h) as usize);
        for row in 0..h {
            let start = (row * padded) as usize;
            out.extend_from_slice(&data[start..start + unpadded as usize]);
        }
        out
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
    fn flush_model_batch(&mut self) {
        if self.pending_tris.is_empty() {
            return;
        }
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
                (group, trino_core::render3d::painter_key(tri.depth, i))
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
        let mut run_start = self.tri_verts.len() as u32;
        let mut run = order.first().map(|(t, _, s)| (*t, s.is_some()));
        for (tex, tri, surf) in &order {
            let lit = surf.is_some();
            if Some((*tex, lit)) != run {
                let first = run_start;
                let count = self.tri_verts.len() as u32 - first;
                if let Some((texture, lit)) = run {
                    self.commands.push(Cmd::Tris {
                        first,
                        count,
                        texture,
                        lit,
                    });
                }
                run_start = self.tri_verts.len() as u32;
                run = Some((*tex, lit));
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
                };
                if let Some(s) = surf {
                    let sv = &s.v[i];
                    v.world = [sv.pos.x, sv.pos.y, sv.pos.z];
                    v.normal = [sv.normal.x, sv.normal.y, sv.normal.z];
                    v.base = rgba(sv.base);
                    v.gloss = s.gloss;
                    // The set (0..8) and the light mask, packed: exact in f32.
                    v.set = (s.set + 8 * s.mask) as f32;
                }
                self.tri_verts.push(v);
            }
        }
        if let Some((texture, lit)) = run {
            let count = self.tri_verts.len() as u32 - run_start;
            self.commands.push(Cmd::Tris {
                first: run_start,
                count,
                texture,
                lit,
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
            let sampler = if key.clamp {
                &self.clamp_sampler
            } else {
                &self.tex3d_sampler
            };
            let bg = create_blit_bind_group(&self.device, &self.sprite_bgl, &entry.view, sampler);
            self.tex3d.insert(key, bg);
        }
        self.tex3d.get(&key)
    }

    /// Render the recorded commands into the main target, or (`second`)
    /// into the second screen's, and submit.
    fn draw_commands(&mut self, second: bool) {
        if self.strict {
            if self.commands.len() as u32 > self.caps.max_sprites_per_frame {
                panic!(
                    "strict mode: {} sprites this frame busts the {:?} profile budget \
                     of {} — reduce overdraw or batch static content.",
                    self.commands.len(),
                    self.profile,
                    self.caps.max_sprites_per_frame
                );
            }
            let tris = (self.tri_verts.len() / 3) as u32;
            if tris > self.caps.max_tris_per_frame {
                panic!(
                    "strict mode: {tris} 3D triangles this frame busts the {:?} \
                     profile budget of {} — simplify the meshes.",
                    self.profile, self.caps.max_tris_per_frame
                );
            }
            if self.caps.max_3d_us_per_frame != u32::MAX {
                let us = trino_core::perf::N64_COST
                    .estimate_us(&self.frame_stats.tnl, self.frame_stats.models);
                if us > self.caps.max_3d_us_per_frame {
                    panic!(
                        "strict mode: this frame's 3D is ~{:.1} ms on the {:?} profile \
                         (trino_core::perf::N64_COST, from {} draws, {} vertices, {} \
                         triangles) — over its {:.0} ms ceiling. Fewer draws, fewer \
                         visible triangles, or a `lod` level (n64-performance skill).",
                        us as f32 / 1000.0,
                        self.profile,
                        self.frame_stats.models,
                        self.frame_stats.tnl.verts,
                        self.frame_stats.tnl.tris_in,
                        self.caps.max_3d_us_per_frame as f32 / 1000.0
                    );
                }
            }
        }
        let (instances, segments) = build_segments(&self.commands);
        self.commands.clear();
        // 3D texture bind groups must exist before the pass borrows self.
        for seg in &segments {
            if let Segment::Tris {
                texture: Some(t), ..
            } = seg
            {
                let _ = self.tex3d_group(*t);
            }
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
        self.tri_verts.clear();
        if segments
            .iter()
            .any(|s| matches!(s, Segment::Tris { lit: true, .. }))
        {
            self.queue
                .write_buffer(&self.lights_buf, 0, &self.light_sets.bytes());
        }

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
        let (color_view, depth_view, globals_bg) = match (&self.second, second) {
            (Some(sc), true) => (&sc.view, &sc.depth_view, &sc.globals_bg),
            _ => (&self.offscreen_view, &self.depth_view, &self.globals_bg),
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("trino-sprite-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
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
            pass.set_bind_group(0, globals_bg, &[]);
            for segment in &segments {
                match segment {
                    Segment::Sprites { sprite, instances } => {
                        if let Some(entry) = self.sprites.get(sprite) {
                            pass.set_pipeline(&self.sprite_pipeline);
                            pass.set_vertex_buffer(0, self.instance_buf.slice(..));
                            pass.set_bind_group(1, &entry.bind_group, &[]);
                            pass.draw(0..4, instances.clone());
                        }
                    }
                    Segment::Tris {
                        first,
                        count,
                        texture,
                        lit,
                    } => {
                        let pipeline = if *lit {
                            pass.set_bind_group(2, &self.lights_bg, &[]);
                            &self.lit_pipeline
                        } else if texture.is_some_and(|t| t.clamp) {
                            &self.tri_clamp_pipeline
                        } else {
                            &self.tri_pipeline
                        };
                        pass.set_pipeline(pipeline);
                        pass.set_vertex_buffer(0, self.tri_buf.slice(..));
                        let bg = texture
                            .and_then(|t| self.tex3d.get(&t))
                            .unwrap_or(&self.white_bg);
                        pass.set_bind_group(1, bg, &[]);
                        pass.draw(*first..*first + *count, 0..1);
                    }
                }
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
        self.main_done = false;
        self.commands.clear();
        self.tri_verts.clear();
        self.pending_tris.clear();
        self.pending_tex.clear();
        self.pending_surf.clear();
        self.locals = LightList::EMPTY;
        self.wetness = 0.0;
        self.light_sets.clear();
        self.frame_stats = FrameStats::default();
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

    fn set_wetness(&mut self, wetness: f32) {
        self.wetness = wetness.clamp(0.0, 1.0);
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
        let screen = Vec2::new(self.internal_size.0 as f32, self.internal_size.1 as f32);
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
        self.frame_stats.tnl.add(&tnl);
        let added = self.pending_tris.len() - before;
        self.tag_pending(
            before,
            added,
            texture.map(|t| TexKey {
                id: t.0,
                clamp: false,
            }),
        );
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
        let screen = Vec2::new(self.internal_size.0 as f32, self.internal_size.1 as f32);
        let locals = match self.caps.light_tier {
            LightTier::PerPixel => &[],
            LightTier::PerVertex => self.locals.as_slice(),
        };
        let ctx = tnl_context(&mut self.tnl_ctx, &self.camera, &self.light, screen, locals);
        let Some(tris) = trino_core::render3d::billboard_with(&ctx, pos, params, uv) else {
            return;
        };
        self.frame_stats.sprites += 1;
        let before = self.pending_tris.len();
        self.pending_tris.extend_from_slice(&tris);
        self.tag_pending(
            before,
            2,
            Some(TexKey {
                id: sprite.0,
                clamp: true,
            }),
        );
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
            self.frame_stats.tris = (self.tri_verts.len() / 3) as u32;
            self.draw_commands(false);
            self.main_done = true;
            self.screen = 1;
        }
    }

    fn end_frame(&mut self) {
        self.flush_model_batch();
        if self.screen == 0 {
            self.frame_stats.tris = (self.tri_verts.len() / 3) as u32;
        }
        self.last_stats = self.frame_stats;
        self.flush();
    }

    fn draw_text(&mut self, font: FontId, pos: Vec2, text: &str, params: &TextParams) {
        let Some(table) = self.fonts.get(&font.0).cloned() else {
            return;
        };
        let parsed = trino_core::text::BitmapFont::parse(&table).expect("validated on upload");
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
            .and_then(|t| trino_core::text::BitmapFont::parse(t).ok())
            .map_or(Vec2::ZERO, |f| f.measure(text, scale))
    }

    fn stats(&self) -> FrameStats {
        self.last_stats
    }

    fn read_pixel(&mut self, x: u32, y: u32) -> Option<Color> {
        let (w, h) = self.internal_size;
        if x >= w || y >= h {
            return None;
        }
        let rgba = self.read_offscreen();
        let at = ((y * w + x) * 4) as usize;
        Some(Color::rgba(
            rgba[at],
            rgba[at + 1],
            rgba[at + 2],
            rgba[at + 3],
        ))
    }
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
