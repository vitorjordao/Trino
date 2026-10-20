//! Shadow maps: the PC native (and Web) tier of [`trino_core::shadow`]
//! (ADR 0025). The core fits the light's box over the view
//! ([`trino_core::shadow::map_fit`]); this module owns the depth map, the
//! light pass that draws the casters' triangles into it and the
//! fullscreen composite that shadows every surface of the frame by it
//! (`shadow_map.wgsl`).
//!
//! Built on the first frame with a mapped caster (most renderers never
//! draw one: the console profiles, games without shadows), once more for a
//! multisampled scene. The composite's bind group holds the scene's depth
//! and is rebuilt after a resize ([`MapPass::invalidate`]) or a new map
//! size.

use trino_core::math3d::Mat34;
use trino_core::{Color, ShadowSetting};

const MAP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The binding of the map's comparison sampler (after the WebGL2 depth
/// sampler at [`super::NEAREST_DEPTH_BINDING`]'s 3).
const SAMPLER_BINDING: u32 = 4;

/// WebGL2 only: the map again as an unfilterable float texture, and the
/// nearest sampler that reads it, for the blocker search (#86). A
/// sampler-less read of a depth texture reads 0 there (wgpu's GL backend
/// leaves its filter mipmapped), and naga can't translate a `textureLoad`
/// of a `texture_depth_2d` to GLSL at all.
const MAP_FLOAT_BINDING: u32 = 5;
const MAP_NEAREST_BINDING: u32 = 6;

/// The blocker search's map read, as written in shadow_map.wgsl.
const MAP_READ: &str = "return textureLoad(t_shadow, q, 0);";

/// shadow_map.wgsl for WebGL2's map: the blocker search reads the float
/// binding through the nearest sampler (the `depth_as_float` pattern).
pub(super) fn map_read_as_float(source: &str) -> String {
    assert!(
        source.contains(MAP_READ),
        "shadow_map.wgsl's map read moved"
    );
    source
        .replace(
            MAP_READ,
            "return textureSampleLevel(t_shadow_f, s_shadow_f, (vec2<f32>(q) + vec2<f32>(0.5)) \
             / vec2<f32>(textureDimensions(t_shadow_f)), 0.0).r;",
        )
        .replace("textureDimensions(t_shadow)", "textureDimensions(t_shadow_f)")
        .replace(
            "var t_shadow: SHADOWMAP;",
            &format!(
                "var t_shadow: SHADOWMAP;\n@group(0) @binding({MAP_FLOAT_BINDING}) var t_shadow_f: \
                 texture_2d<f32>;\n@group(0) @binding({MAP_NEAREST_BINDING}) var s_shadow_f: sampler;"
            ),
        )
}

/// The map's texels per side: the game's, or the quality preset's (1024 on
/// Low, 2048 above), within what the device takes.
pub fn map_size(game: u16, quality: Option<ShadowSetting>, max: u32) -> u32 {
    let size = match (game, quality) {
        (0, Some(ShadowSetting::Low)) => 1024,
        (0, _) => 2048,
        (s, _) => s as u32,
    };
    size.min(max)
}

/// PCF taps per side for the quality preset: 2×2 on Low, 3×3 on Medium
/// (the default), 4×4 on High; every tap a bilinear compare of 2×2 texels.
pub fn taps(quality: Option<ShadowSetting>) -> u32 {
    match quality {
        Some(ShadowSetting::Low) => 2,
        Some(ShadowSetting::High) => 4,
        _ => 3,
    }
}

/// Everything the composite reads besides the textures; layout mirrored by
/// `MapParams` in shadow_map.wgsl.
#[derive(Clone, Copy, Debug)]
pub struct MapUniform {
    pub size: (u32, u32),
    /// Focal length in pixels and the near plane of the depth `1 - near/z`
    /// (the T&L's, as the AO's).
    pub focal: f32,
    pub near: f32,
    /// The light's fog (view depth): the shadows fade with it.
    pub fog: (f32, f32),
    /// The camera's view matrix (world to view).
    pub view: Mat34,
    pub fit: trino_core::shadow::ShadowMapFit,
    /// The direction the light travels, world.
    pub light: trino_core::Vec3,
    pub color: Color,
    pub softness: u8,
    pub distance: f32,
    pub map_size: u32,
    pub taps: u32,
    /// [`trino_core::ShadowSettings::soft_growth`] and
    /// [`fade_distance`](trino_core::ShadowSettings::fade_distance): 0 and
    /// 0 skip the blocker search (ADR 0025's composite, bit for bit).
    pub soft_growth: f32,
    pub fade_distance: f32,
}

impl MapUniform {
    pub(super) fn bytes(&self) -> [u8; 192] {
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        // The view's rotation is orthonormal: its inverse is the
        // transpose, the translation turned back by it.
        let v = &self.view.m;
        let t = [v[0][3], v[1][3], v[2][3]];
        let inv = |i: usize| {
            let (a, b, c) = (v[0][i], v[1][i], v[2][i]);
            [a, b, c, -(a * t[0] + b * t[1] + c * t[2])]
        };
        let to_light = (-self.light).normalized();
        let m = &self.fit.matrix.m;
        let texel = self.fit.texel;
        let k = 1.0 / 255.0;
        let spacing = (1.0 + self.softness as f32 / 255.0 * 1.5) / self.map_size as f32;
        let mut f: Vec<f32> = vec![w, h, w * 0.5, h * 0.5];
        f.extend([self.focal, self.near, self.fog.0, self.fog.1]);
        for i in 0..3 {
            f.extend(inv(i));
        }
        for row in m {
            f.extend(row);
        }
        let c = self.color;
        f.extend([
            c.r as f32 * k,
            c.g as f32 * k,
            c.b as f32 * k,
            c.a as f32 * k,
        ]);
        f.extend([to_light.x, to_light.y, to_light.z, 0.0]);
        // A texel along the normal, and the depth of one more in the map.
        f.extend([texel, texel / self.fit.depth, spacing, self.distance]);
        let mut words: Vec<u32> = f.iter().map(|x| x.to_bits()).collect();
        // The soft edge (twice the filter radius) widens by `soft_growth`
        // per world unit of blocker distance; the map's uv spans twice its
        // half width.
        let growth = self.soft_growth * 0.25 / self.fit.radius;
        words.extend([
            self.taps,
            growth.to_bits(),
            self.fade_distance.to_bits(),
            self.fit.depth.to_bits(),
        ]);
        let mut b = [0u8; 192];
        for (i, w) in words.iter().enumerate() {
            b[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        b
    }
}

/// The depth map, its light pass and the composite.
pub(super) struct MapPass {
    light_params: wgpu::Buffer,
    light_group: wgpu::BindGroup,
    light: wgpu::RenderPipeline,
    params: wgpu::Buffer,
    bgl: wgpu::BindGroupLayout,
    composite: wgpu::RenderPipeline,
    compare: wgpu::Sampler,
    nearest: Option<wgpu::Sampler>,
    /// The map (its size) and the composite's bind group over it and the
    /// scene's depth.
    map: Option<(u32, wgpu::TextureView)>,
    group: Option<wgpu::BindGroup>,
    verts: wgpu::Buffer,
    vert_capacity: u64,
}

const LIGHT_WGSL: &str = "
struct Light { row0: vec4<f32>, row1: vec4<f32>, row2: vec4<f32> };
@group(0) @binding(0) var<uniform> L: Light;
@vertex
fn vs_light(@location(0) p: vec3<f32>) -> @builtin(position) vec4<f32> {
    let h = vec4<f32>(p, 1.0);
    return vec4<f32>(dot(L.row0, h), dot(L.row1, h), dot(L.row2, h), 1.0);
}
";

impl MapPass {
    /// `multisampled`: the scene depth it reads is the MSAA target's
    /// (each pixel's first sample).
    pub(super) fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        multisampled: bool,
        webgl2: bool,
    ) -> MapPass {
        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        // The light pass: world triangles, depth only.
        let light_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-shadow-light-bgl"),
            entries: &[uniform(0)],
        });
        let light_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-shadow-light"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trino-shadow-light-bg"),
            layout: &light_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: light_params.as_entire_binding(),
            }],
        });
        let light_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("trino-shadow-light"),
            source: wgpu::ShaderSource::Wgsl(LIGHT_WGSL.into()),
        });
        let light_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-shadow-light-layout"),
            bind_group_layouts: &[Some(&light_bgl)],
            immediate_size: 0,
        });
        let light = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("trino-shadow-light"),
            layout: Some(&light_layout),
            vertex: wgpu::VertexState {
                module: &light_shader,
                entry_point: Some("vs_light"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                }],
            },
            fragment: None,
            // Both sides: a one-sided sheet (a leaf, a flag) casts too.
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: MAP_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                // Steep faces' depth grows fastest across a texel.
                bias: wgpu::DepthBiasState {
                    constant: 0,
                    slope_scale: 1.5,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // The composite: the scene depth's type per variant (ao.rs), then
        // the map's.
        let source = include_str!("shadow_map.wgsl");
        let source = if multisampled {
            source.replace("texture_depth_2d", "texture_depth_multisampled_2d")
        } else if webgl2 {
            super::depth_as_float(source)
        } else {
            source.to_string()
        };
        let source = if webgl2 {
            map_read_as_float(&source)
        } else {
            source
        };
        let source = source.replace("SHADOWMAP", "texture_depth_2d");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("trino-shadow-map"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let mut entries = vec![
            uniform(0),
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
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: SAMPLER_BINDING,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
        ];
        if webgl2 {
            entries.push(super::nearest_sampler_entry(super::NEAREST_DEPTH_BINDING.1));
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: MAP_FLOAT_BINDING,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
            entries.push(super::nearest_sampler_entry(MAP_NEAREST_BINDING));
        }
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-shadow-map-bgl"),
            entries: &entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-shadow-map-layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        // The shadow color over the frame by its alpha; the frame's alpha
        // stays.
        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let composite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("trino-shadow-map"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_shadow_map"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        MapPass {
            light_params,
            light_group,
            light,
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("trino-shadow-map-params"),
                size: 192,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            bgl,
            composite,
            compare: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("trino-shadow-compare"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                compare: Some(wgpu::CompareFunction::LessEqual),
                ..Default::default()
            }),
            nearest: webgl2.then(|| super::nearest_sampler(device)),
            map: None,
            group: None,
            verts: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("trino-shadow-casters"),
                size: 4096,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            vert_capacity: 4096,
        }
    }

    /// The scene's depth was recreated (a resize, MSAA): rebuild the bind
    /// group on the next frame.
    pub(super) fn invalidate(&mut self) {
        self.group = None;
    }

    /// Record the light pass over `verts` (world positions, three per
    /// triangle) and the composite into `color` over `depth`. Returns the
    /// bytes uploaded.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        verts: &[[f32; 3]],
        depth: &wgpu::TextureView,
        color: &wgpu::TextureView,
        params: &MapUniform,
    ) -> u32 {
        let size = params.map_size;
        if self.map.as_ref().is_none_or(|(s, _)| *s != size) {
            let view = device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("trino-shadow-map"),
                    size: wgpu::Extent3d {
                        width: size,
                        height: size,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: MAP_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default());
            self.map = Some((size, view));
            self.group = None;
        }
        let (_, map) = self.map.as_ref().expect("made above");
        if self.group.is_none() {
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
                    resource: wgpu::BindingResource::TextureView(map),
                },
                wgpu::BindGroupEntry {
                    binding: SAMPLER_BINDING,
                    resource: wgpu::BindingResource::Sampler(&self.compare),
                },
            ];
            if let Some(s) = &self.nearest {
                entries.push(wgpu::BindGroupEntry {
                    binding: super::NEAREST_DEPTH_BINDING.1,
                    resource: wgpu::BindingResource::Sampler(s),
                });
                entries.push(wgpu::BindGroupEntry {
                    binding: MAP_FLOAT_BINDING,
                    resource: wgpu::BindingResource::TextureView(map),
                });
                entries.push(wgpu::BindGroupEntry {
                    binding: MAP_NEAREST_BINDING,
                    resource: wgpu::BindingResource::Sampler(s),
                });
            }
            self.group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("trino-shadow-map-bg"),
                layout: &self.bgl,
                entries: &entries,
            }));
        }
        let mut uploaded = 0u32;
        let bytes: &[u8] = bytemuck::cast_slice(verts);
        if bytes.len() as u64 > self.vert_capacity {
            self.vert_capacity = (bytes.len() as u64).next_power_of_two();
            self.verts = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("trino-shadow-casters"),
                size: self.vert_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.verts, 0, bytes);
            uploaded += bytes.len() as u32;
        }
        let mut rows = [0u8; 48];
        for (i, v) in params.fit.matrix.m.iter().flatten().enumerate() {
            rows[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        queue.write_buffer(&self.light_params, 0, &rows);
        let p = params.bytes();
        queue.write_buffer(&self.params, 0, &p);
        uploaded += (rows.len() + p.len()) as u32;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("trino-shadow-light-pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: map,
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
            if !verts.is_empty() {
                pass.set_pipeline(&self.light);
                pass.set_bind_group(0, &self.light_group, &[]);
                pass.set_vertex_buffer(0, self.verts.slice(..bytes.len() as u64));
                pass.draw(0..verts.len() as u32, 0..1);
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("trino-shadow-map-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color,
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
            let (w, h) = params.size;
            pass.set_viewport(0.0, 0.0, w as f32, h as f32, 0.0, 1.0);
            pass.set_pipeline(&self.composite);
            pass.set_bind_group(0, self.group.as_ref().expect("made above"), &[]);
            pass.draw(0..3, 0..1);
        }
        uploaded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::Vec3;

    #[test]
    fn sizes_and_taps_follow_the_quality_preset() {
        assert_eq!(map_size(0, Some(ShadowSetting::Low), 8192), 1024);
        assert_eq!(map_size(0, Some(ShadowSetting::High), 8192), 2048);
        assert_eq!(map_size(0, None, 8192), 2048);
        assert_eq!(map_size(4096, Some(ShadowSetting::Low), 8192), 4096);
        assert_eq!(map_size(4096, None, 2048), 2048);
        assert_eq!(taps(Some(ShadowSetting::Low)), 2);
        assert_eq!(taps(None), 3);
        assert_eq!(taps(Some(ShadowSetting::High)), 4);
    }

    #[test]
    fn the_uniform_matches_the_shader_layout() {
        let view = Mat34::look_at(
            Vec3::new(1.0, 2.0, -3.0),
            Vec3::ZERO,
            Vec3::new(0.0, 1.0, 0.0),
        );
        let camera = trino_core::Camera3 {
            eye: Vec3::new(1.0, 2.0, -3.0),
            target: Vec3::ZERO,
            fov_y: 1.0,
        };
        let light = Vec3::new(0.0, -1.0, 0.0);
        let fit = trino_core::shadow::map_fit(&camera, 1.5, light, 40.0, 2048, &[]).unwrap();
        let u = MapUniform {
            size: (640, 360),
            focal: 300.0,
            near: 0.05,
            fog: (10.0, 30.0),
            view,
            fit,
            light,
            color: Color::rgba(0, 0, 0, 255),
            softness: 0,
            distance: 40.0,
            map_size: 2048,
            taps: 3,
            soft_growth: 0.0,
            fade_distance: 0.0,
        };
        let b = u.bytes();
        let f = |i: usize| f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!((f(0), f(1), f(2), f(3)), (640.0, 360.0, 320.0, 180.0));
        assert_eq!((f(4), f(5), f(6), f(7)), (300.0, 0.05, 10.0, 30.0));
        // The inverse view takes the view origin back to the eye.
        assert!((f(11) - 1.0).abs() < 1e-5 && (f(15) - 2.0).abs() < 1e-5);
        assert!((f(19) + 3.0).abs() < 1e-5);
        assert_eq!(f(35), 1.0, "the color's alpha");
        let toward = Vec3::new(f(36), f(37), f(38)) - Vec3::new(0.0, 1.0, 0.0);
        assert!(toward.dot(toward) < 1e-8, "toward the light");
        assert_eq!(f(42), 1.0 / 2048.0, "a texel apart at softness 0");
        assert_eq!(f(43), 40.0);
        assert_eq!(u32::from_le_bytes(b[176..180].try_into().unwrap()), 3);
        // Contact hardening off: growth and fade 0 (the shader skips the
        // blocker search); the depth range always there.
        assert_eq!((f(45), f(46)), (0.0, 0.0));
        assert_eq!(f(47), fit.depth);
        let on = MapUniform {
            soft_growth: 0.2,
            fade_distance: 3.0,
            ..u
        }
        .bytes();
        let g = |i: usize| f32::from_le_bytes(on[i * 4..i * 4 + 4].try_into().unwrap());
        assert!(
            (g(45) - 0.2 * 0.25 / fit.radius).abs() < 1e-9,
            "uv per world unit"
        );
        assert_eq!(g(46), 3.0);
        assert_eq!(on[..180], b[..180], "the rest unchanged");
    }

    #[test]
    fn webgl2_reads_the_map_through_a_float_binding() {
        let src = map_read_as_float(include_str!("shadow_map.wgsl"));
        assert!(!src.contains(MAP_READ));
        assert!(!src.contains("textureDimensions(t_shadow)"));
        assert!(src.contains("@group(0) @binding(5) var t_shadow_f: texture_2d<f32>;"));
        assert!(src.contains("@group(0) @binding(6) var s_shadow_f: sampler;"));
        assert!(src.contains("textureSampleLevel(t_shadow_f, s_shadow_f"));
    }
}
