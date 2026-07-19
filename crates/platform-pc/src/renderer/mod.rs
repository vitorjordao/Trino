//! wgpu 2D sprite renderer with an internal offscreen framebuffer.
//!
//! Two construction modes:
//! - [`PcRenderer::new_windowed`]: renders offscreen, then blits to the
//!   window surface with nearest-neighbor integer upscaling (letterboxed).
//! - [`PcRenderer::new_headless`]: offscreen only; tests read pixels back
//!   with [`PcRenderer::read_offscreen`]. This is also the path the editor
//!   viewport reuses in Fase 3 (render-to-texture).

mod batch;

use std::collections::HashMap;
use std::sync::Arc;

use trino_core::render3d::{Camera3, DEFAULT_LIGHT, Mesh, ScreenTri};
use trino_core::{
    Caps, Color, Material, ModelId, ModelParams, Renderer, SpriteId, SpriteParams, Transform3, Vec2,
};

use crate::sim::SimProfile;
use batch::{
    Cmd, INSTANCE_STRIDE, Segment, TRI_VERTEX_STRIDE, TriVertex, build_segments, make_command,
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
    blit_pipeline: wgpu::RenderPipeline,
    blit_bind_group: wgpu::BindGroup,
    sprite_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    globals_bg: wgpu::BindGroup,
    globals_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    instance_capacity: u64,
    tri_buf: wgpu::Buffer,
    tri_capacity: u64,
    sprites: HashMap<u32, SpriteEntry>,
    meshes: HashMap<u32, Vec<u8>>,
    commands: Vec<Cmd>,
    tri_verts: Vec<TriVertex>,
    /// Triangles of the current model batch: consecutive `draw_model` calls
    /// accumulate here and depth-sort together (painter across meshes) when
    /// a sprite draw, a camera change or `end_frame` flushes the batch.
    pending_tris: Vec<ScreenTri>,
    camera: Camera3,
    clear: Color,
    caps: Caps,
    profile: SimProfile,
    internal_size: (u32, u32),
    n64_look: bool,
    strict: bool,
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
        Self::build(device, queue, None, profile)
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
                required_limits: wgpu::Limits::downlevel_defaults(),
                ..Default::default()
            })
            .await
            .map_err(|e| RendererError::Device(e.to_string()))?;

        let surface = surface.map(|(surface, width, height)| {
            let surface_caps = surface.get_capabilities(&adapter);
            // Prefer a non-sRGB format so 8-bit colors pass through the blit
            // unchanged (retro palettes must not get gamma-shifted twice).
            let format = surface_caps
                .formats
                .iter()
                .copied()
                .find(|f| !f.is_srgb())
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

        Ok(Self::build(device, queue, surface, profile))
    }

    fn build(
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface: Option<SurfaceState>,
        profile: SimProfile,
    ) -> Self {
        let internal_size = profile.internal_resolution();
        let caps = profile.caps();
        // The N64 profile emulates the console's output (3-point filtering,
        // RGBA5551 + dither) by default; TRINO_LOOK=off|n64 overrides.
        let n64_look = match std::env::var("TRINO_LOOK").as_deref() {
            Ok("off" | "0" | "native") => false,
            Ok("n64" | "1" | "on") => true,
            _ => profile == SimProfile::N64,
        };
        // Strict mode: enforce the profile's Caps at development time.
        let strict = matches!(std::env::var("TRINO_STRICT").as_deref(), Ok("1" | "true"));

        let offscreen_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("trino-offscreen"),
            size: wgpu::Extent3d {
                width: internal_size.0,
                height: internal_size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let offscreen_view = offscreen_tex.create_view(&wgpu::TextureViewDescriptor::default());

        // Depth buffer for the 3D triangle pipeline (sprites test Always).
        let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("trino-depth"),
            size: wgpu::Extent3d {
                width: internal_size.0,
                height: internal_size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());

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

        // 3D triangle pipeline: same offscreen target, vertex colors only.
        let tri_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-tri-layout"),
            bind_group_layouts: &[Some(&globals_bgl)],
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
        ];
        let tri_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                entry_point: Some("fs_tri"),
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

        let blit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trino-blit-bg"),
            layout: &texture_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&offscreen_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&blit_sampler),
                },
            ],
        });

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
            blit_pipeline,
            blit_bind_group,
            sprite_bgl: texture_bgl,
            sampler,
            globals_bg,
            globals_buf,
            instance_buf,
            instance_capacity,
            tri_buf,
            tri_capacity,
            sprites: HashMap::new(),
            meshes: HashMap::new(),
            commands: Vec::new(),
            tri_verts: Vec::new(),
            pending_tris: Vec::new(),
            camera: Camera3::default(),
            clear: Color::BLACK,
            caps,
            profile,
            internal_size,
            n64_look,
            strict,
        }
    }

    /// Register a baked TMDL mesh under a stable handle. Re-uploading the
    /// same id replaces the content (live reload).
    pub fn upload_mesh(&mut self, id: ModelId, tmdl: Vec<u8>) {
        if let Err(e) = Mesh::from_tmdl(&tmdl) {
            panic!("mesh {id:?}: invalid TMDL blob: {e:?}");
        }
        self.meshes.insert(id.0, tmdl);
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
        if self.strict {
            // Console formats are 16-bit (RGBA5551) or smaller; indexed
            // formats (CI4/CI8) are validated at bake time where the real
            // format is known — this is the worst-case direct-color check.
            let bpp = if self.caps.color_depth_bits <= 16 {
                2
            } else {
                4
            };
            if let Err(e) = self.caps.validate_texture(
                width.min(u16::MAX as u32) as u16,
                height.min(u16::MAX as u32) as u16,
                bpp,
            ) {
                panic!(
                    "strict mode: sprite {id:?} ({width}x{height}) busts the {:?} \
                     profile budget: {e:?}. Shrink the sprite or pick an indexed \
                     format (CI4/CI8) in assets/manifest.toml.",
                    self.profile
                );
            }
        }
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
        self.sprites.insert(
            id.0,
            SpriteEntry {
                bind_group,
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

    /// Read the internal framebuffer back as tightly-packed RGBA8.
    /// Test/tooling path — synchronous, not for per-frame use.
    pub fn read_offscreen(&self) -> Vec<u8> {
        let (w, h) = self.internal_size;
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
                texture: &self.offscreen_tex,
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

    /// Depth-sort the pending model batch and enqueue it as one triangle
    /// command (painter's order across every mesh of the batch).
    fn flush_model_batch(&mut self) {
        if self.pending_tris.is_empty() {
            return;
        }
        self.pending_tris.sort_unstable_by(|a, b| {
            b.depth
                .partial_cmp(&a.depth)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let first = self.tri_verts.len() as u32;
        for tri in &self.pending_tris {
            for i in 0..3 {
                let (p, c) = (tri.pts[i], tri.colors[i]);
                self.tri_verts.push(TriVertex {
                    pos: [p.x, p.y],
                    color: [
                        c.r as f32 / 255.0,
                        c.g as f32 / 255.0,
                        c.b as f32 / 255.0,
                        c.a as f32 / 255.0,
                    ],
                    depth: tri.z[i],
                    _pad: 0.0,
                });
            }
        }
        self.commands.push(Cmd::Tris {
            first,
            count: (self.pending_tris.len() * 3) as u32,
        });
        self.pending_tris.clear();
    }

    fn flush(&mut self) {
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
        }
        let (instances, segments) = build_segments(&self.commands);
        self.commands.clear();

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

        // In N64 look the framebuffer is 16-bit: quantize the clear color to
        // 5 bits per channel (sprites are dithered+quantized in the shader).
        let ch = |v: u8| -> f64 {
            if self.n64_look {
                ((v as f64 / 255.0 * 31.0).round() / 31.0).clamp(0.0, 1.0)
            } else {
                v as f64 / 255.0
            }
        };
        let clear = wgpu::Color {
            r: ch(self.clear.r),
            g: ch(self.clear.g),
            b: ch(self.clear.b),
            a: self.clear.a as f64 / 255.0,
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("trino-sprite-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.offscreen_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
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
            pass.set_bind_group(0, &self.globals_bg, &[]);
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
                    Segment::Tris { first, count } => {
                        pass.set_pipeline(&self.tri_pipeline);
                        pass.set_vertex_buffer(0, self.tri_buf.slice(..));
                        pass.draw(*first..*first + *count, 0..1);
                    }
                }
            }
        }

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
            // Nearest integer upscale, centered. Fractional only when the
            // window is smaller than the internal framebuffer.
            let (iw, ih) = (self.internal_size.0 as f32, self.internal_size.1 as f32);
            let (sw, sh) = (ss.config.width as f32, ss.config.height as f32);
            let fit = (sw / iw).min(sh / ih);
            let scale = if fit >= 1.0 { fit.floor() } else { fit };
            let (vw, vh) = (iw * scale, ih * scale);
            let (vx, vy) = ((sw - vw) * 0.5, (sh - vh) * 0.5);
            pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, &self.blit_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        self.queue.submit([encoder.finish()]);
        if let Some(frame) = frame {
            frame.present();
        }
    }
}

impl Renderer for PcRenderer {
    fn caps(&self) -> &Caps {
        &self.caps
    }

    fn begin_frame(&mut self, clear: Color) {
        self.clear = clear;
        self.commands.clear();
        self.tri_verts.clear();
        self.pending_tris.clear();
    }

    fn draw_sprite(&mut self, sprite: SpriteId, pos: Vec2, params: &SpriteParams) {
        self.flush_model_batch();
        let Some(entry) = self.sprites.get(&sprite.0) else {
            // Unknown handle: skip. The asset pipeline (Fase 2) turns this
            // into a hard error at bake time instead.
            return;
        };
        self.commands
            .push(Cmd::Sprite(make_command(sprite.0, pos, entry.size, params)));
    }

    fn set_camera(&mut self, camera: &Camera3) {
        self.flush_model_batch();
        self.camera = *camera;
    }

    fn draw_model(
        &mut self,
        model: ModelId,
        transform: &Transform3,
        _material: Material,
        params: &ModelParams,
    ) {
        let Some(tmdl) = self.meshes.get(&model.0) else {
            return; // unknown handle: skip, like sprites
        };
        let mesh = Mesh::from_tmdl(tmdl).expect("validated on upload");
        let screen = Vec2::new(self.internal_size.0 as f32, self.internal_size.1 as f32);
        let pending = &mut self.pending_tris;
        trino_core::render3d::tessellate(
            &mesh,
            &transform.matrix(),
            &self.camera,
            &DEFAULT_LIGHT,
            params.tint,
            screen,
            &mut |tri| pending.push(tri),
        );
    }

    fn end_frame(&mut self) {
        self.flush_model_batch();
        self.flush();
    }
}
