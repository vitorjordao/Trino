//! The PC native profile's GPU vertex T&L (issue #51, ADR 0016): resident
//! mesh buffers, the per-draw context and joint uniforms, the per-frame
//! instance stream and the instanced draw calls of `shaders_tnl.wgsl`.
//!
//! A draw that `trino_core::render3d::pc_gpu_check` accepts becomes one
//! [`GpuInstance`] (its model and model-view matrices, tint, emissive,
//! palette and flags) pointing at a resident mesh, a [`MeshCtx`] (the light,
//! fog and projection) and a joint block. At each batch flush the pending
//! draws are sorted by pipeline, texture, mesh, context and joints and runs
//! that share all of them become one instanced draw call ([`GpuBatch`]):
//! repeated models (castle64's blocks, Solid Eleven's players' parts, a
//! crowd through `draw_model_instances`) cost one call each, whatever API
//! drew them. The draws are opaque and depth-tested, so their order inside
//! a batch doesn't change the picture (exact depth ties aside).

use std::collections::HashMap;
use std::ops::Range;

use trino_core::render3d::{Mesh, NEAR_PLANE, TnlContext};
use trino_core::{Color, Mat34, ModelParams, Shading};
use wgpu::util::DeviceExt;

use super::OFFSCREEN_FORMAT;
use super::batch::TexKey;

/// A mesh vertex as the vertex stage reads it (`MeshIn`): one per triangle
/// corner, so a corner carries its triangle's face normal (flat shading).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    /// The triangle's model-space `(p1 - p0) × (p2 - p0)`.
    pub face: [f32; 3],
    pub color: [u8; 4],
    pub uv: [f32; 2],
    /// Joint, second joint (`SKW2`), color slot, unused.
    pub skin: [u8; 4],
    /// The second joint's weight (0 = the first joint only).
    pub weight: f32,
}

/// One drawn copy (`InstIn`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuInstance {
    /// The model matrix's rows (normals, face normals, joints).
    pub model: [[f32; 4]; 3],
    /// `view · model`, multiplied on the CPU like the CPU path does.
    pub model_view: [[f32; 4]; 3],
    /// Tint, emissive (RGBA8), flags, the detail layer's UV scale (f32
    /// bits).
    pub misc: [u32; 4],
    /// The palette's colors (white without one).
    pub palette: [u32; 4],
}

/// `MeshCtx` in the shader: what a [`TnlContext`] derives from the camera,
/// the light and the screen.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct MeshCtx {
    proj: [f32; 4],
    sun: [f32; 4],
    sun_color: [f32; 4],
    ambient: [f32; 4],
    fog_color: [f32; 4],
    fog_range: [f32; 4],
}

/// Joint matrices of one draw: 32 joints × 3 rows (`Joints`).
const JOINT_ROWS: usize = 3 * trino_core::render3d::MAX_JOINTS;
type JointBlock = [[f32; 4]; JOINT_ROWS];

const SHADING_FLAT: u32 = 1;
const SHADING_UNLIT: u32 = 2;
const FLAG_AFFINE: u32 = 4;
const FLAG_SKINNED: u32 = 8;

const MESH_VERTEX_STRIDE: u64 = core::mem::size_of::<MeshVertex>() as u64;
const INSTANCE_STRIDE: u64 = core::mem::size_of::<GpuInstance>() as u64;

fn pack(c: Color) -> u32 {
    u32::from_le_bytes([c.r, c.g, c.b, c.a])
}

/// The vertex stream of `mesh` for the GPU path: three vertices per
/// triangle, in index order.
pub fn mesh_vertices(mesh: &Mesh) -> Vec<MeshVertex> {
    let tris = mesh.index_count / 3;
    let mut out = Vec::with_capacity(tris * 3);
    let textured = mesh.has_uvs();
    let skinned = mesh.joint_count() != 0;
    for t in 0..tris {
        let idx = [0, 1, 2].map(|k| mesh.index(t * 3 + k) as usize);
        let p = idx.map(|i| mesh.position(i));
        let face = (p[1] - p[0]).cross(p[2] - p[0]);
        for (k, &i) in idx.iter().enumerate() {
            let n = mesh.normal(i);
            let c = mesh.color(i);
            let uv = if textured {
                mesh.uv(i)
            } else {
                trino_core::Vec2::ZERO
            };
            let joint = if skinned { mesh.joint_of_vertex(i) } else { 0 };
            let (joint2, weight) = mesh.blend_of(i).unwrap_or((joint, 0.0));
            out.push(MeshVertex {
                pos: [p[k].x, p[k].y, p[k].z],
                normal: [n.x, n.y, n.z],
                face: [face.x, face.y, face.z],
                color: [c.r, c.g, c.b, c.a],
                uv: [uv.x, uv.y],
                skin: [joint as u8, joint2 as u8, mesh.slot(i), 0],
                weight,
            });
        }
    }
    out
}

/// The copy of a draw ([`GpuInstance`]): matrices, surface and flags.
/// `affine`: the CPU path interpolates this draw's colors affinely (no
/// depth splits: untextured and unfogged, `split_policy` false).
pub fn instance(
    ctx: &TnlContext,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
    affine: bool,
    detail_scale: f32,
) -> GpuInstance {
    let (view, _, _) = ctx.projection();
    let mv = view.mul(model);
    let shading = match params.shading {
        Shading::Gouraud => 0,
        Shading::Flat => SHADING_FLAT,
        Shading::Unlit => SHADING_UNLIT,
    };
    let flags = shading
        | if affine { FLAG_AFFINE } else { 0 }
        | if mesh.joint_count() != 0 {
            FLAG_SKINNED
        } else {
            0
        };
    let palette = match params.palette {
        Some(p) if mesh.has_slots() => p.map(pack),
        _ => [pack(Color::WHITE); 4],
    };
    GpuInstance {
        model: model.m,
        model_view: mv.m,
        misc: [
            pack(params.tint),
            pack(params.emissive),
            flags,
            detail_scale.to_bits(),
        ],
        palette,
    }
}

/// The light, fog and projection of `ctx` as the shader reads them.
fn mesh_ctx(ctx: &TnlContext) -> MeshCtx {
    let (_, focal, screen) = ctx.projection();
    let light = ctx.light();
    let ldir = light.dir.normalized();
    let f = |c: Color| [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0];
    let (lc, ac) = (f(light.color), f(light.ambient_color));
    let white = light.color == Color::WHITE && light.ambient_color == Color::WHITE;
    let fog = light.fog;
    MeshCtx {
        proj: [
            2.0 * focal / screen.x,
            2.0 * focal / screen.y,
            NEAR_PLANE,
            0.0,
        ],
        sun: [ldir.x, ldir.y, ldir.z, light.ambient],
        sun_color: [lc[0], lc[1], lc[2], white as u32 as f32],
        ambient: [ac[0], ac[1], ac[2], 0.0],
        fog_color: [
            fog.color.r as f32,
            fog.color.g as f32,
            fog.color.b as f32,
            fog.enabled() as u32 as f32,
        ],
        fog_range: [fog.near, fog.far, 0.0, 0.0],
    }
}

/// A resident mesh: its vertex stream on the GPU.
struct GpuMesh {
    buf: wgpu::Buffer,
    verts: u32,
}

/// A recorded draw of the current batch: `count` instances from `first`
/// in the batch's instance list.
#[derive(Clone, Copy, Debug)]
struct GpuDraw {
    model: u32,
    ctx: u32,
    joints: u32,
    texture: Option<TexKey>,
    double_sided: bool,
    first: u32,
    count: u32,
}

/// One instanced draw call of the frame.
#[derive(Clone, Debug, PartialEq)]
pub struct GpuBatch {
    model: u32,
    ctx: u32,
    joints: u32,
    pub texture: Option<TexKey>,
    double_sided: bool,
    pub instances: Range<u32>,
}

/// What a draw needs besides its instance: the mesh, the joints of its pose
/// (none for a static mesh), its texture and whether back faces show.
pub struct DrawKey<'a> {
    pub model: u32,
    pub mesh: &'a Mesh<'a>,
    pub params: &'a ModelParams,
    pub texture: Option<TexKey>,
}

/// The GPU T&L path's pipelines, resident meshes and per-frame streams.
pub struct GpuTnl {
    bgl: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    shader: wgpu::ShaderModule,
    /// Back faces culled, double-sided; single- and (built on demand)
    /// multisampled.
    pipelines: [wgpu::RenderPipeline; 2],
    ms_pipelines: Option<[wgpu::RenderPipeline; 2]>,
    meshes: HashMap<u32, GpuMesh>,
    /// Bytes between two context / joint blocks (the device's uniform
    /// offset alignment).
    ctx_stride: u64,
    joint_stride: u64,
    ctx_buf: wgpu::Buffer,
    joint_buf: wgpu::Buffer,
    inst_buf: wgpu::Buffer,
    bg: wgpu::BindGroup,
    /// The frame's contexts (deduplicated against the last one), joint
    /// blocks (0 = identity, for static meshes), instances in draw-call
    /// order and draw calls.
    ctxs: Vec<MeshCtx>,
    last_ctx: Option<MeshCtx>,
    joints: Vec<JointBlock>,
    instances: Vec<GpuInstance>,
    batches: Vec<GpuBatch>,
    /// The current batch (between two flushes of the renderer's batch).
    pending: Vec<GpuDraw>,
    pending_inst: Vec<GpuInstance>,
    /// Posing scratch.
    skin: Box<[Mat34; trino_core::render3d::MAX_JOINTS]>,
}

fn align(n: u64, to: u64) -> u64 {
    n.div_ceil(to) * to
}

fn identity_block() -> JointBlock {
    let mut b = [[0.0; 4]; JOINT_ROWS];
    b[..3].copy_from_slice(&Mat34::IDENTITY.m);
    b
}

impl GpuTnl {
    /// `groups`: the globals and the 3D texture layouts (groups 0 and 1,
    /// shared with the screen-space triangle pipelines).
    pub fn new(device: &wgpu::Device, groups: [&wgpu::BindGroupLayout; 2]) -> GpuTnl {
        let dynamic = |binding: u32, size: u64| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: wgpu::BufferSize::new(size),
            },
            count: None,
        };
        let ctx_size = core::mem::size_of::<MeshCtx>() as u64;
        let joint_size = core::mem::size_of::<JointBlock>() as u64;
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-tnl-bgl"),
            entries: &[dynamic(0, ctx_size), dynamic(1, joint_size)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-tnl-layout"),
            bind_group_layouts: &[Some(groups[0]), Some(groups[1]), Some(&bgl)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::include_wgsl!("shaders_tnl.wgsl"));
        let pipelines = pipelines(device, &layout, &shader, 1);
        let a = device.limits().min_uniform_buffer_offset_alignment as u64;
        let (ctx_stride, joint_stride) = (align(ctx_size, a), align(joint_size, a));
        let ctx_buf = uniform(device, "trino-tnl-ctx", ctx_stride * 16);
        let joint_buf = uniform(device, "trino-tnl-joints", joint_stride * 4);
        let inst_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-tnl-instances"),
            size: INSTANCE_STRIDE * 256,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = bind_group(device, &bgl, &ctx_buf, &joint_buf);
        GpuTnl {
            bgl,
            layout,
            shader,
            pipelines,
            ms_pipelines: None,
            meshes: HashMap::new(),
            ctx_stride,
            joint_stride,
            ctx_buf,
            joint_buf,
            inst_buf,
            bg,
            ctxs: Vec::new(),
            last_ctx: None,
            joints: vec![identity_block()],
            instances: Vec::new(),
            batches: Vec::new(),
            pending: Vec::new(),
            pending_inst: Vec::new(),
            skin: Box::new([Mat34::IDENTITY; trino_core::render3d::MAX_JOINTS]),
        }
    }

    /// The multisampled pipelines (`PcRenderer::set_msaa`).
    pub fn ensure_ms(&mut self, device: &wgpu::Device, samples: u32) {
        if self.ms_pipelines.is_none() {
            self.ms_pipelines = Some(pipelines(device, &self.layout, &self.shader, samples));
        }
    }

    /// Upload (or replace) the resident vertex stream of mesh `model`.
    pub fn upload_mesh(&mut self, device: &wgpu::Device, model: u32, mesh: &Mesh) {
        let verts = mesh_vertices(mesh);
        if verts.is_empty() {
            self.meshes.remove(&model);
            return;
        }
        let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("trino-tnl-mesh"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.meshes.insert(
            model,
            GpuMesh {
                buf,
                verts: verts.len() as u32,
            },
        );
    }

    /// Drop mesh `model`'s stream (its blob changed: re-uploaded on the
    /// next draw).
    pub fn forget_mesh(&mut self, model: u32) {
        self.meshes.remove(&model);
    }

    pub fn has_mesh(&self, model: u32) -> bool {
        self.meshes.contains_key(&model)
    }

    /// Resident vertex bytes (every uploaded mesh).
    pub fn resident_bytes(&self) -> u64 {
        self.meshes
            .values()
            .map(|m| m.verts as u64 * MESH_VERTEX_STRIDE)
            .sum()
    }

    /// Forget the last frame's streams.
    pub fn begin_frame(&mut self) {
        self.ctxs.clear();
        self.last_ctx = None;
        self.joints.truncate(1);
        self.instances.clear();
        self.batches.clear();
        self.pending.clear();
        self.pending_inst.clear();
    }

    /// Record a draw of `instances` copies of `key.mesh` with `ctx`.
    pub fn draw(&mut self, ctx: &TnlContext, key: DrawKey<'_>, instances: &[GpuInstance]) {
        if instances.is_empty() {
            return;
        }
        let c = mesh_ctx(ctx);
        if self.last_ctx != Some(c) {
            self.ctxs.push(c);
            self.last_ctx = Some(c);
        }
        let joints = if key.mesh.joint_count() == 0 {
            0
        } else {
            let n = key.mesh.pose(key.params.anim, &mut self.skin);
            let mut block = identity_block();
            for (j, m) in self.skin[..n].iter().enumerate() {
                block[j * 3..j * 3 + 3].copy_from_slice(&m.m);
            }
            self.joints.push(block);
            self.joints.len() as u32 - 1
        };
        self.pending.push(GpuDraw {
            model: key.model,
            ctx: self.ctxs.len() as u32 - 1,
            joints,
            texture: key.texture,
            double_sided: key.params.double_sided,
            first: self.pending_inst.len() as u32,
            count: instances.len() as u32,
        });
        self.pending_inst.extend_from_slice(instances);
    }

    /// Any draw since the last [`GpuTnl::flush_batch`]?
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Close the current batch: sort its draws by pipeline, texture, mesh,
    /// context and joints, merge the runs into instanced draw calls and
    /// return the frame's draw calls it added.
    pub fn flush_batch(&mut self) -> Range<u32> {
        let start = self.batches.len() as u32;
        let mut order: Vec<usize> = (0..self.pending.len()).collect();
        let pending = &self.pending;
        order.sort_by_key(|&i| {
            let d = &pending[i];
            (
                d.double_sided,
                d.texture.map_or(0, |t| t.group() + 1),
                d.model,
                d.ctx,
                d.joints,
                i,
            )
        });
        for i in order {
            let d = self.pending[i];
            let first = self.instances.len() as u32;
            let src = &self.pending_inst[d.first as usize..(d.first + d.count) as usize];
            self.instances.extend_from_slice(src);
            let end = self.instances.len() as u32;
            let in_batch = self.batches.len() as u32 > start;
            match self.batches.last_mut() {
                Some(b)
                    if in_batch
                        && b.model == d.model
                        && b.ctx == d.ctx
                        && b.joints == d.joints
                        && b.texture == d.texture
                        && b.double_sided == d.double_sided
                        && b.instances.end == first =>
                {
                    b.instances.end = end;
                }
                _ => self.batches.push(GpuBatch {
                    model: d.model,
                    ctx: d.ctx,
                    joints: d.joints,
                    texture: d.texture,
                    double_sided: d.double_sided,
                    instances: first..end,
                }),
            }
        }
        self.pending.clear();
        self.pending_inst.clear();
        start..self.batches.len() as u32
    }

    /// The frame's draw calls (textures to bind before a pass).
    pub fn batches(&self, range: Range<u32>) -> &[GpuBatch] {
        &self.batches[range.start as usize..range.end as usize]
    }

    /// Write the frame's contexts, joint blocks and instances, growing the
    /// buffers (and the bind group) as needed. Returns the bytes written.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> u32 {
        if self.batches.is_empty() {
            return 0;
        }
        let mut grown = false;
        let ctx_bytes = strided(&self.ctxs, self.ctx_stride);
        if ctx_bytes.len() as u64 > self.ctx_buf.size() {
            self.ctx_buf = uniform(
                device,
                "trino-tnl-ctx",
                (ctx_bytes.len() as u64).next_power_of_two(),
            );
            grown = true;
        }
        let joint_bytes = strided(&self.joints, self.joint_stride);
        if joint_bytes.len() as u64 > self.joint_buf.size() {
            self.joint_buf = uniform(
                device,
                "trino-tnl-joints",
                (joint_bytes.len() as u64).next_power_of_two(),
            );
            grown = true;
        }
        if grown {
            self.bg = bind_group(device, &self.bgl, &self.ctx_buf, &self.joint_buf);
        }
        let inst_bytes: &[u8] = bytemuck::cast_slice(&self.instances);
        if inst_bytes.len() as u64 > self.inst_buf.size() {
            self.inst_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("trino-tnl-instances"),
                size: (inst_bytes.len() as u64).next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.ctx_buf, 0, &ctx_bytes);
        queue.write_buffer(&self.joint_buf, 0, &joint_bytes);
        queue.write_buffer(&self.inst_buf, 0, inst_bytes);
        (ctx_bytes.len() + joint_bytes.len() + inst_bytes.len()) as u32
    }

    /// Record the draw calls `range` into `pass` (`ms`: the multisampled
    /// pipelines). `texture` gives a draw's group 1. Returns the calls.
    pub fn record<'a>(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        range: Range<u32>,
        ms: bool,
        texture: &dyn Fn(Option<TexKey>) -> &'a wgpu::BindGroup,
    ) -> u32 {
        let pipes = match (&self.ms_pipelines, ms) {
            (Some(p), true) => p,
            _ => &self.pipelines,
        };
        let mut calls = 0;
        pass.set_vertex_buffer(1, self.inst_buf.slice(..));
        for b in self.batches(range) {
            let Some(mesh) = self.meshes.get(&b.model) else {
                continue;
            };
            pass.set_pipeline(&pipes[b.double_sided as usize]);
            pass.set_bind_group(1, texture(b.texture), &[]);
            pass.set_bind_group(
                2,
                &self.bg,
                &[
                    (b.ctx as u64 * self.ctx_stride) as u32,
                    (b.joints as u64 * self.joint_stride) as u32,
                ],
            );
            pass.set_vertex_buffer(0, mesh.buf.slice(..));
            pass.draw(0..mesh.verts, b.instances.clone());
            calls += 1;
        }
        calls
    }
}

/// Does a block-compressed texture have a texel below full alpha? Exact for
/// BC1 (a transparent index in a three-color block) and BC3 (the alpha
/// block's values); BC7 counts as opaque only in modes without alpha
/// (0–3) and in mode 6 (the asset pipeline's) with both alpha endpoints
/// at 255, so other alpha modes keep their draws on the CPU path.
pub fn bc_alpha(format: super::CompressedFormat, blocks: &[u8]) -> bool {
    use super::CompressedFormat as F;
    match format {
        F::Bc1 => blocks.as_chunks::<8>().0.iter().any(|b| {
            let (c0, c1) = (
                u16::from_le_bytes([b[0], b[1]]),
                u16::from_le_bytes([b[2], b[3]]),
            );
            let idx = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
            c0 <= c1 && (0..16).any(|t| (idx >> (2 * t)) & 3 == 3)
        }),
        F::Bc3 => blocks.as_chunks::<16>().0.iter().any(|b| {
            let (a0, a1) = (b[0] as u32, b[1] as u32);
            let bits = u64::from_le_bytes([b[2], b[3], b[4], b[5], b[6], b[7], 0, 0]);
            let value = |i: u32| match i {
                0 => a0,
                1 => a1,
                i if a0 > a1 => ((8 - i) * a0 + (i - 1) * a1) / 7,
                6 => 0,
                7 => 255,
                i => ((6 - i) * a0 + (i - 1) * a1) / 5,
            };
            (0..16).any(|t| value(((bits >> (3 * t)) & 7) as u32) < 255)
        }),
        F::Bc7 => blocks.as_chunks::<16>().0.iter().any(|b| {
            let bits = u128::from_le_bytes(*b);
            match b[0].trailing_zeros() {
                0..=3 => false,
                6 => {
                    let field = |at: u32, n: u32| ((bits >> at) & ((1 << n) - 1)) as u32;
                    let a0 = field(49, 7) << 1 | field(63, 1);
                    let a1 = field(56, 7) << 1 | field(64, 1);
                    a0 != 255 || a1 != 255
                }
                _ => true,
            }
        }),
    }
}

/// `items` laid out `stride` bytes apart (dynamic uniform offsets).
fn strided<T: bytemuck::Pod>(items: &[T], stride: u64) -> Vec<u8> {
    let mut out = vec![0u8; items.len() * stride as usize];
    for (i, item) in items.iter().enumerate() {
        let b = bytemuck::bytes_of(item);
        let at = i * stride as usize;
        out[at..at + b.len()].copy_from_slice(b);
    }
    out
}

fn uniform(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn bind_group(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    ctx: &wgpu::Buffer,
    joints: &wgpu::Buffer,
) -> wgpu::BindGroup {
    fn entry(binding: u32, buffer: &wgpu::Buffer, size: usize) -> wgpu::BindGroupEntry<'_> {
        wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: wgpu::BufferSize::new(size as u64),
            }),
        }
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("trino-tnl-bg"),
        layout: bgl,
        entries: &[
            entry(0, ctx, core::mem::size_of::<MeshCtx>()),
            entry(1, joints, core::mem::size_of::<JointBlock>()),
        ],
    })
}

/// The two pipelines (back faces culled, double-sided) for `samples`.
fn pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    samples: u32,
) -> [wgpu::RenderPipeline; 2] {
    let vertex_attrs = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Float32x3,
        3 => Uint8x4,
        4 => Float32x2,
        5 => Uint8x4,
        6 => Float32,
    ];
    let instance_attrs = wgpu::vertex_attr_array![
        7 => Float32x4,
        8 => Float32x4,
        9 => Float32x4,
        10 => Float32x4,
        11 => Float32x4,
        12 => Float32x4,
        13 => Uint32x4,
        14 => Uint32x4,
    ];
    let make = |cull: Option<wgpu::Face>| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("trino-tnl-pipeline"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_mesh"),
                compilation_options: Default::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: MESH_VERTEX_STRIDE,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &vertex_attrs,
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: INSTANCE_STRIDE,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &instance_attrs,
                    },
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_mesh"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OFFSCREEN_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            // Front faces are counter-clockwise in the engine's Y-down
            // screen space (`tessellate_with`'s positive area), which is
            // clockwise in Y-up clip space.
            primitive: wgpu::PrimitiveState {
                front_face: wgpu::FrontFace::Cw,
                cull_mode: cull,
                ..Default::default()
            },
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
    [make(Some(wgpu::Face::Back)), make(None)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_alpha_is_found() {
        use super::super::CompressedFormat as F;
        // BC1: four-color blocks are opaque; a three-color one using
        // index 3 is not.
        let four = [0xFF, 0xFF, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF];
        assert!(!bc_alpha(F::Bc1, &four));
        let three = [0x00, 0x00, 0xFF, 0xFF, 0x03, 0x00, 0x00, 0x00];
        assert!(bc_alpha(F::Bc1, &three));
        let three_unused = [0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00];
        assert!(!bc_alpha(F::Bc1, &three_unused));
        // BC3: endpoints 255 everywhere vs one texel at a1 = 0.
        let mut solid = [0u8; 16];
        solid[0] = 255;
        solid[1] = 255;
        assert!(!bc_alpha(F::Bc3, &solid));
        let mut holed = solid;
        holed[1] = 0;
        holed[2] = 1; // texel 0 takes a1
        assert!(bc_alpha(F::Bc3, &holed));
        // BC7: mode 1 (no alpha) and mode 6 with alpha endpoints 255 are
        // opaque; mode 6 with a lower endpoint and mode 5 are not.
        assert!(!bc_alpha(F::Bc7, &[0b10; 16]));
        let mut m6 = 1u128 << 6;
        m6 |= 0x7F << 49 | 0x7F << 56 | 1 << 63 | 1 << 64;
        assert!(!bc_alpha(F::Bc7, &m6.to_le_bytes()));
        let low = m6 & !(1u128 << 63);
        assert!(bc_alpha(F::Bc7, &low.to_le_bytes()));
        assert!(bc_alpha(F::Bc7, &(1u128 << 5).to_le_bytes()));
    }

    #[test]
    fn layouts_match_the_shader() {
        // MeshIn: 3 + 3 + 3 floats, 4 bytes, 2 floats, 4 bytes, 1 float.
        assert_eq!(MESH_VERTEX_STRIDE, 56);
        // InstIn: two 3x4 matrices and two uvec4.
        assert_eq!(INSTANCE_STRIDE, 128);
        assert_eq!(core::mem::size_of::<MeshCtx>(), 96);
        assert_eq!(core::mem::size_of::<JointBlock>(), 1536);
        assert_eq!(identity_block()[..3], Mat34::IDENTITY.m);
    }
}
