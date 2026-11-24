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
//!
//! Draws shaded per pixel (a local light reaches them, gloss, wetness)
//! take the lit pipelines (`vs_mesh_lit` / `fs_mesh_lit`): the same vertex
//! work plus the surface the CPU path's lit triangles carry, and the
//! renderer's light sets bound as group 3. Their instance packs the gloss,
//! the light set and the mask of the lights that reach the draw
//! ([`LitDraw`]). Draws whose texture's alpha is a mask
//! ([`super::SpriteAlpha::Cutout`]) take the cutout pipelines: cut at half
//! alpha, opaque and depth-written.
//!
//! Two more passes since ADR 0016 §13 ([`MeshPass`]). **Translucent**
//! draws (a tint or vertex alpha below 255) blend without writing depth,
//! after the batch's opaque draws: each copy is its own item, sorted back
//! to front by [`trino_core::render3d::pc_sort_depth`] and interleaved by
//! the renderer with the CPU path's blended triangles; runs of the same
//! mesh still become one instanced call, and a double-sided run draws its
//! back faces first. **Water** surfaces are displaced in the vertex stage
//! (`shaders_tnl_water.wgsl`) and shaded by the CPU path's own per-pixel
//! water (`water.wgsl`), last in the batch.

use std::collections::HashMap;
use std::ops::Range;

use trino_core::render3d::{Mesh, NEAR_PLANE, TnlContext};
use trino_core::{Color, Mat34, ModelParams, Shading};

use super::OFFSCREEN_FORMAT;
use super::batch::TexKey;

/// The opaque, cutout, lit and translucent pipelines' module.
pub(crate) const MESH_WGSL: &str = concat!(
    include_str!("shaders_tnl.wgsl"),
    include_str!("shaders_tnl_lit.wgsl")
);

/// The water pipelines' module: the vertex stage, the per-pixel water the
/// screen-space pass shares (`water.wgsl`) and its entry points.
pub(crate) const MESH_WATER_WGSL: &str = concat!(
    include_str!("shaders_tnl.wgsl"),
    include_str!("water.wgsl"),
    include_str!("shaders_tnl_water.wgsl")
);

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
    view_up: [f32; 4],
    /// The two-color ambient's ground (the sky's color without one); w: 1
    /// when the light has a ground or a rim (`Light::has_terms`).
    ground: [f32; 4],
    /// The rim's color × strength; w: 1 when it is on.
    rim: [f32; 4],
    /// The camera's world position (the rim's view direction).
    eye: [f32; 4],
}

/// Joint matrices of one draw: 32 joints × 3 rows (`Joints`).
const JOINT_ROWS: usize = 3 * trino_core::render3d::MAX_JOINTS;
type JointBlock = [[f32; 4]; JOINT_ROWS];

const SHADING_FLAT: u32 = 1;
const SHADING_UNLIT: u32 = 2;
const FLAG_AFFINE: u32 = 4;
const FLAG_SKINNED: u32 = 8;
/// Shaded per pixel (the lit pipelines); bits 8..16 the gloss, 16..19 the
/// light set, 19..27 the light mask.
const FLAG_LIT: u32 = 16;

/// What a per-pixel lit draw adds to its instance: the light set it reads
/// (`lights::LightSets::index`, 0..8), the lights that reach it (one bit
/// per local light) and its gloss (0..=255, `ModelParams::gloss`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LitDraw {
    pub set: u32,
    pub mask: u32,
    pub gloss: u8,
}

impl LitDraw {
    fn flags(self) -> u32 {
        FLAG_LIT | (self.gloss as u32) << 8 | (self.set & 7) << 16 | (self.mask & 255) << 19
    }
}

const MESH_VERTEX_STRIDE: u64 = core::mem::size_of::<MeshVertex>() as u64;
const INSTANCE_STRIDE: u64 = core::mem::size_of::<GpuInstance>() as u64;

fn pack(c: Color) -> u32 {
    u32::from_le_bytes([c.r, c.g, c.b, c.a])
}

/// The vertex stream of `mesh` for the GPU path: three vertices per
/// triangle, in index order.
pub fn mesh_vertices(mesh: &Mesh) -> Vec<MeshVertex> {
    let mut out = Vec::new();
    mesh_vertices_into(mesh, &mut out);
    out
}

/// [`mesh_vertices`] into `out` (cleared first; its capacity reused).
pub fn mesh_vertices_into(mesh: &Mesh, out: &mut Vec<MeshVertex>) {
    let tris = mesh.index_count / 3;
    out.clear();
    out.reserve(tris * 3);
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
    lit: Option<LitDraw>,
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
        }
        | lit.map_or(0, LitDraw::flags);
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

/// A water draw's copy: [`instance`] naming its water set (`water.rs`,
/// 0..8) in the bits a lit draw uses for its light set.
pub fn water_instance(mut inst: GpuInstance, set: u32) -> GpuInstance {
    inst.misc[2] = (inst.misc[2] & !(7 << 16)) | (set & 7) << 16;
    inst
}

/// The light, fog and projection of `ctx` as the shader reads them.
fn mesh_ctx(ctx: &TnlContext) -> MeshCtx {
    let (view, focal, screen) = ctx.projection();
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
        view_up: [view.m[0][1], view.m[1][1], view.m[2][1], 0.0],
        ground: {
            let g = light.ground.map_or(ac, f);
            [g[0], g[1], g[2], light.has_terms() as u32 as f32]
        },
        rim: {
            let (r, k) = (f(light.rim.color), light.rim.strength);
            let on = light.has_terms() && k > 0.0;
            [r[0] * k, r[1] * k, r[2] * k, on as u32 as f32]
        },
        eye: {
            let e = ctx.camera().eye;
            [e.x, e.y, e.z, 0.0]
        },
    }
}

/// A resident mesh: its vertex stream on the GPU.
struct GpuMesh {
    buf: wgpu::Buffer,
    verts: u32,
}

/// Which of a batch's passes a draw belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MeshPass {
    /// Depth-tested and written, in any order (ahead of the CPU path's
    /// triangles).
    Opaque,
    /// Blended, depth-tested, never written: after the batch's opaque
    /// triangles and shadows, back to front with the CPU path's blended
    /// triangles ([`trino_core::render3d::pc_translucent`]).
    Blend,
    /// A water surface: displaced here, shaded per pixel, blended last.
    Water,
}

/// A recorded draw of the current batch: `count` instances from `first`
/// in the batch's instance list. Blended and water draws hold one copy
/// each, with the view depth they sort by.
#[derive(Clone, Copy, Debug)]
struct GpuDraw {
    model: u32,
    ctx: u32,
    joints: u32,
    texture: Option<TexKey>,
    double_sided: bool,
    lit: bool,
    cutout: bool,
    pass: MeshPass,
    depth: f32,
    first: u32,
    count: u32,
}

impl GpuDraw {
    /// Whether two draws can share one instanced call (same pipeline set,
    /// texture, mesh, context and pose).
    fn joins(&self, o: &GpuDraw) -> bool {
        self.model == o.model
            && self.ctx == o.ctx
            && self.joints == o.joints
            && self.texture == o.texture
            && self.double_sided == o.double_sided
            && self.lit == o.lit
            && self.cutout == o.cutout
            && self.pass == o.pass
    }
}

/// One instanced draw call of the frame.
#[derive(Clone, Debug, PartialEq)]
pub struct GpuBatch {
    model: u32,
    ctx: u32,
    joints: u32,
    pub texture: Option<TexKey>,
    /// Its pipeline in a [`pipelines`] set ([`pipe_index`]).
    pipe: u8,
    /// The lit pipelines (the light sets bound).
    pub lit: bool,
    /// The cutout pipelines.
    pub cutout: bool,
    pub pass: MeshPass,
    pub instances: Range<u32>,
}

/// What a draw needs besides its instance: the mesh, the joints of its pose
/// (none for a static mesh), its texture and whether back faces show.
pub struct DrawKey<'a> {
    pub model: u32,
    pub mesh: &'a Mesh<'a>,
    pub params: &'a ModelParams,
    pub texture: Option<TexKey>,
    /// Shaded per pixel: every instance carries a [`LitDraw`].
    pub lit: bool,
    /// The texture's alpha is a mask: cut at half alpha.
    pub cutout: bool,
    /// Opaque, translucent (every copy sorted on its own) or water (the
    /// instance names its set, [`water_instance`]).
    pub pass: MeshPass,
}

/// The GPU T&L path's pipelines, resident meshes and per-frame streams.
pub struct GpuTnl {
    bgl: wgpu::BindGroupLayout,
    /// The opaque, translucent and water pipelines ([`pipe_index`]);
    /// single- and (built on demand) multisampled ([`pipelines`]).
    pipelines: [wgpu::RenderPipeline; MESH_PIPES],
    ms_pipelines: Option<[wgpu::RenderPipeline; MESH_PIPES]>,
    /// The same into the post stack's HDR target (issue #64): single
    /// sampled, then multisampled; built when post first runs.
    hdr_pipelines: [Option<[wgpu::RenderPipeline; MESH_PIPES]>; 2],
    meshes: HashMap<u32, GpuMesh>,
    /// Vertex buffers created for meshes so far (uploads, and a dynamic
    /// mesh growing past its buffer): a mesh rewritten every frame in
    /// place adds none.
    buffers_created: u64,
    /// [`write_mesh`](Self::write_mesh)'s vertex stream, kept between
    /// calls.
    stream: Vec<MeshVertex>,
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
    /// The batch's opaque draws are flushed ([`GpuTnl::flush_opaque`]):
    /// its translucent and water ones wait for [`GpuTnl::emit`].
    opaque_done: bool,
    /// The groups 0..2 and WebGL2 flag the pipelines were built with.
    source: MeshSource,
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

/// The pipelines of one target ([`pipe_index`]): opaque (plain, lit,
/// whole and cut; back faces culled or double-sided), translucent (plain,
/// lit; front faces, or back faces for a double-sided draw's first pass)
/// and water (culled, double-sided).
pub const MESH_PIPES: usize = 14;

/// Which faces a pipeline rasterizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Faces {
    Front,
    /// A double-sided translucent draw's first pass: the far side of a
    /// convex shape, under its near side.
    Back,
    Both,
}

/// The pipeline of a draw call in a [`pipelines`] set.
fn pipe_index(pass: MeshPass, faces: Faces, lit: bool, cutout: bool) -> usize {
    match pass {
        MeshPass::Opaque => {
            cutout as usize * 4 + lit as usize * 2 + (faces == Faces::Both) as usize
        }
        MeshPass::Blend => 8 + lit as usize * 2 + (faces == Faces::Back) as usize,
        MeshPass::Water => 12 + (faces == Faces::Both) as usize,
    }
}

/// A [`pipe_index`]'s pipeline: pass, faces, lit, cutout.
fn pipe_kind(i: usize) -> (MeshPass, Faces, bool, bool) {
    match i {
        0..8 => (
            MeshPass::Opaque,
            if i & 1 != 0 {
                Faces::Both
            } else {
                Faces::Front
            },
            i & 2 != 0,
            i & 4 != 0,
        ),
        8..12 => (
            MeshPass::Blend,
            if i & 1 != 0 {
                Faces::Back
            } else {
                Faces::Front
            },
            i & 2 != 0,
            false,
        ),
        _ => (
            MeshPass::Water,
            if i & 1 != 0 {
                Faces::Both
            } else {
                Faces::Front
            },
            false,
            false,
        ),
    }
}

/// What building the pipelines takes ([`pipelines`]): the plain and the lit
/// layouts and the shader, and for the water pipelines (whose group 3, the
/// water sets and the scene's depth, depends on the sample count) the
/// first three groups and whether the device is WebGL2.
#[derive(Clone)]
pub struct MeshSource {
    pub layout: wgpu::PipelineLayout,
    pub lit_layout: wgpu::PipelineLayout,
    pub shader: wgpu::ShaderModule,
    pub groups: [wgpu::BindGroupLayout; 3],
    pub webgl2: bool,
}

impl GpuTnl {
    /// `groups`: the globals and the 3D texture layouts (groups 0 and 1,
    /// shared with the screen-space triangle pipelines) and the light sets'
    /// (the lit pipelines' group 3, the screen-space lit pipeline's 2).
    /// `webgl2`: the water pipelines read the depth copy as a float texture
    /// (`depth_as_float`).
    pub fn new(device: &wgpu::Device, groups: [&wgpu::BindGroupLayout; 3], webgl2: bool) -> GpuTnl {
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
        let lit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-tnl-lit-layout"),
            bind_group_layouts: &[
                Some(groups[0]),
                Some(groups[1]),
                Some(&bgl),
                Some(groups[2]),
            ],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shaders_tnl.wgsl"),
            source: wgpu::ShaderSource::Wgsl(MESH_WGSL.into()),
        });
        let src = MeshSource {
            layout,
            lit_layout,
            shader,
            groups: [groups[0].clone(), groups[1].clone(), bgl.clone()],
            webgl2,
        };
        let pipelines = pipelines(device, &src, 1, OFFSCREEN_FORMAT);
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
            pipelines,
            ms_pipelines: None,
            hdr_pipelines: [None, None],
            meshes: HashMap::new(),
            buffers_created: 0,
            stream: Vec::new(),
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
            opaque_done: false,
            source: src,
            skin: Box::new([Mat34::IDENTITY; trino_core::render3d::MAX_JOINTS]),
        }
    }

    /// The multisampled pipelines (`PcRenderer::set_msaa`).
    pub fn ensure_ms(&mut self, device: &wgpu::Device, samples: u32) {
        if self.ms_pipelines.is_none() {
            self.ms_pipelines = Some(pipelines(
                device,
                &self.ms_source(),
                samples,
                OFFSCREEN_FORMAT,
            ));
        }
    }

    pub fn has_ms(&self) -> bool {
        self.ms_pipelines.is_some()
    }

    /// What a precompile job needs to build the multisampled pipelines on
    /// a worker ([`pipelines`]).
    pub fn ms_source(&self) -> MeshSource {
        self.source.clone()
    }

    /// Multisampled pipelines a precompile job built (kept if already set).
    pub fn install_ms(&mut self, pipes: [wgpu::RenderPipeline; MESH_PIPES]) {
        self.ms_pipelines.get_or_insert(pipes);
    }

    /// Whether the HDR pipelines (`ms`: multisampled) exist.
    pub fn has_hdr(&self, ms: bool) -> bool {
        self.hdr_pipelines[ms as usize].is_some()
    }

    /// HDR pipelines (kept if already set).
    pub fn install_hdr(&mut self, ms: bool, pipes: [wgpu::RenderPipeline; MESH_PIPES]) {
        self.hdr_pipelines[ms as usize].get_or_insert(pipes);
    }

    /// Upload (or replace) the resident vertex stream of mesh `model`.
    ///
    /// Written through the queue, not mapped at creation: Chromium's WebGPU
    /// on SwiftShader (the CI runner) refused a mapped-at-creation buffer
    /// partway through Solid Eleven's 197 meshes ("createBuffer failed ...
    /// too large for the implementation when mappedAtCreation == true").
    pub fn upload_mesh(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: u32,
        mesh: &Mesh,
    ) {
        let verts = mesh_vertices(mesh);
        if verts.is_empty() {
            self.meshes.remove(&model);
            return;
        }
        let bytes: &[u8] = bytemuck::cast_slice(&verts);
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-tnl-mesh"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buf, 0, bytes);
        self.buffers_created += 1;
        self.meshes.insert(
            model,
            GpuMesh {
                buf,
                verts: verts.len() as u32,
            },
        );
    }

    /// Rewrite mesh `model`'s resident vertex stream in place (a dynamic
    /// mesh, `Renderer::draw_mesh`): its buffer is reused while the stream
    /// fits and replaced by one of the next power of two when it doesn't,
    /// so a mesh redrawn every frame creates no buffer in steady state.
    /// The queue writes before the next submit: one content per frame.
    pub fn write_mesh(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: u32,
        mesh: &Mesh,
    ) {
        let mut stream = core::mem::take(&mut self.stream);
        mesh_vertices_into(mesh, &mut stream);
        if stream.is_empty() {
            self.meshes.remove(&model);
        } else {
            let bytes: &[u8] = bytemuck::cast_slice(&stream);
            let need = bytes.len() as u64;
            if self.meshes.get(&model).is_none_or(|m| m.buf.size() < need) {
                let buf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("trino-tnl-dynamic-mesh"),
                    size: need.next_power_of_two().max(4096),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.buffers_created += 1;
                self.meshes.insert(model, GpuMesh { buf, verts: 0 });
            }
            if let Some(m) = self.meshes.get_mut(&model) {
                queue.write_buffer(&m.buf, 0, bytes);
                m.verts = stream.len() as u32;
            }
        }
        self.stream = stream;
    }

    /// Vertex buffers created for meshes so far.
    pub fn buffers_created(&self) -> u64 {
        self.buffers_created
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
        self.opaque_done = false;
    }

    /// Record a draw of `instances` copies of `key.mesh` with `ctx`. A
    /// translucent or water draw becomes one item per copy, each with the
    /// view depth it sorts by.
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
        let draw = GpuDraw {
            model: key.model,
            ctx: self.ctxs.len() as u32 - 1,
            joints,
            texture: key.texture,
            double_sided: key.params.double_sided,
            lit: key.lit,
            cutout: key.cutout && key.pass == MeshPass::Opaque,
            pass: key.pass,
            depth: 0.0,
            first: self.pending_inst.len() as u32,
            count: instances.len() as u32,
        };
        if key.pass == MeshPass::Opaque {
            self.pending.push(draw);
            self.pending_inst.extend_from_slice(instances);
            return;
        }
        for inst in instances {
            let mv = Mat34 { m: inst.model_view };
            self.pending.push(GpuDraw {
                depth: trino_core::render3d::pc_sort_depth(key.mesh, &mv),
                first: self.pending_inst.len() as u32,
                count: 1,
                ..draw
            });
            self.pending_inst.push(*inst);
        }
    }

    /// Any draw since the last [`GpuTnl::end_batch`]?
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Any opaque draw not yet flushed?
    pub fn has_opaque(&self) -> bool {
        !self.opaque_done && self.pending.iter().any(|d| d.pass == MeshPass::Opaque)
    }

    /// Any translucent or water draw waiting for [`GpuTnl::emit`]?
    pub fn has_sorted(&self) -> bool {
        self.pending.iter().any(|d| d.pass != MeshPass::Opaque)
    }

    /// Close the current batch's opaque draws: sort them by pipeline,
    /// texture, mesh, context and joints, merge the runs into instanced
    /// draw calls and return the frame's draw calls it added. The batch's
    /// translucent and water draws wait for [`GpuTnl::emit`].
    pub fn flush_opaque(&mut self) -> Range<u32> {
        let start = self.batches.len() as u32;
        if self.opaque_done {
            return start..start;
        }
        self.opaque_done = true;
        let pending = &self.pending;
        let mut order: Vec<usize> = (0..pending.len())
            .filter(|&i| pending[i].pass == MeshPass::Opaque)
            .collect();
        order.sort_by_key(|&i| {
            let d = &pending[i];
            (
                d.cutout,
                d.lit,
                d.double_sided,
                d.texture.map_or(0, |t| t.group() + 1),
                d.model,
                d.ctx,
                d.joints,
                i,
            )
        });
        let order: Vec<u32> = order.into_iter().map(|i| i as u32).collect();
        self.push_runs(&order);
        start..self.batches.len() as u32
    }

    /// The batch's `pass` draws (translucent or water), back to front:
    /// `(painter key, draw)`, the key ordered like the CPU path's
    /// [`trino_core::render3d::painter_key`]s (farther first, then in
    /// submission order) so the renderer can merge both streams.
    pub fn sorted(&self, pass: MeshPass) -> Vec<(u64, u32)> {
        let mut keys: Vec<(u64, u32)> = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, d)| d.pass == pass)
            .map(|(i, d)| (trino_core::render3d::painter_key(d.depth, i), i as u32))
            .collect();
        keys.sort_unstable();
        keys
    }

    /// Draw calls for the batch's draws `ids` (from [`GpuTnl::sorted`]) in
    /// that order: consecutive copies of one mesh, context, pose, texture
    /// and pipeline share an instanced call (instances rasterize in order,
    /// so the blending order holds). A double-sided translucent run draws
    /// its back faces, then its front faces. Returns the frame's draw calls
    /// it added.
    pub fn emit(&mut self, ids: &[u32]) -> Range<u32> {
        let start = self.batches.len() as u32;
        self.push_runs(ids);
        start..self.batches.len() as u32
    }

    /// Forget the batch's draws (every pass emitted).
    pub fn end_batch(&mut self) {
        self.pending.clear();
        self.pending_inst.clear();
        self.opaque_done = false;
    }

    /// Copy the draws `ids`' instances to the frame's stream in that order
    /// and add their draw calls, merging consecutive draws that
    /// [`GpuDraw::joins`].
    fn push_runs(&mut self, ids: &[u32]) {
        let mut at = 0;
        while at < ids.len() {
            let head = self.pending[ids[at] as usize];
            let mut end = at + 1;
            // A double-sided translucent copy draws its back faces, then its
            // front faces: copies in one call would lay a far copy's front
            // over a near copy's back, so each is its own pair of calls.
            let alone = head.pass == MeshPass::Blend && head.double_sided;
            while !alone && end < ids.len() && self.pending[ids[end] as usize].joins(&head) {
                end += 1;
            }
            let first = self.instances.len() as u32;
            for &i in &ids[at..end] {
                let d = self.pending[i as usize];
                let src = &self.pending_inst[d.first as usize..(d.first + d.count) as usize];
                self.instances.extend_from_slice(src);
            }
            let instances = first..self.instances.len() as u32;
            let faces: &[Faces] = match (head.pass, head.double_sided) {
                (MeshPass::Blend, true) => &[Faces::Back, Faces::Front],
                (_, true) => &[Faces::Both],
                (_, false) => &[Faces::Front],
            };
            for &f in faces {
                self.batches.push(GpuBatch {
                    model: head.model,
                    ctx: head.ctx,
                    joints: head.joints,
                    texture: head.texture,
                    pipe: pipe_index(head.pass, f, head.lit, head.cutout) as u8,
                    lit: head.lit,
                    cutout: head.cutout,
                    pass: head.pass,
                    instances: instances.clone(),
                });
            }
            at = end;
        }
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

    /// Whether any draw call of `range` is lit (its pass needs the light
    /// sets written).
    pub fn any_lit(&self, range: Range<u32>) -> bool {
        self.batches(range).iter().any(|b| b.lit)
    }

    /// Record the draw calls `range` into `pass` (`ms`: the multisampled
    /// pipelines). `texture` gives a draw's group 1, `lights` the lit
    /// draws' group 3, `water` the water draws'. Returns the calls.
    #[allow(clippy::too_many_arguments)]
    pub fn record<'a>(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        range: Range<u32>,
        ms: bool,
        hdr: bool,
        texture: &dyn Fn(Option<TexKey>) -> &'a wgpu::BindGroup,
        lights: &wgpu::BindGroup,
        water: &wgpu::BindGroup,
    ) -> u32 {
        let pipes = match (
            &self.ms_pipelines,
            ms,
            &self.hdr_pipelines[ms as usize],
            hdr,
        ) {
            (_, _, Some(p), true) => p,
            (Some(p), true, _, false) => p,
            _ => &self.pipelines,
        };
        let mut calls = 0;
        pass.set_vertex_buffer(1, self.inst_buf.slice(..));
        for b in self.batches(range) {
            let Some(mesh) = self.meshes.get(&b.model) else {
                continue;
            };
            pass.set_pipeline(&pipes[b.pipe as usize]);
            if b.pass == MeshPass::Water {
                pass.set_bind_group(3, water, &[]);
            } else if b.lit {
                pass.set_bind_group(3, lights, &[]);
            }
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

/// What a compressed texture's alpha does ([`super::SpriteAlpha`]), read
/// from its blocks: BC1's alpha is one bit (a mask whenever it has any);
/// BC3's texel values are classified as they decode; BC7 and the Web's
/// formats can't be read cheaply, so any alpha blends (a host with the
/// master passes the class instead).
pub fn bc_alpha_class(format: super::CompressedFormat, blocks: &[u8]) -> super::SpriteAlpha {
    use super::CompressedFormat as F;
    use super::SpriteAlpha;
    match format {
        F::Bc1 if bc_alpha(format, blocks) => SpriteAlpha::Cutout,
        F::Bc3 => SpriteAlpha::of_values(blocks.as_chunks::<16>().0.iter().flat_map(|b| {
            let values = bc3_alpha(b);
            let bits = u64::from_le_bytes([b[2], b[3], b[4], b[5], b[6], b[7], 0, 0]);
            (0..16).map(move |t| values[((bits >> (3 * t)) & 7) as usize])
        })),
        F::Bc1 | F::Bc7 => bc_alpha(format, blocks).into(),
        F::Etc2Rgb8 => SpriteAlpha::Opaque,
        F::Etc2Rgba8 | F::Astc4x4 => SpriteAlpha::Blend,
    }
}

/// A BC3 alpha block's eight values (the 3-bit indices' table).
fn bc3_alpha(b: &[u8; 16]) -> [u8; 8] {
    let (a0, a1) = (b[0] as u32, b[1] as u32);
    core::array::from_fn(|i| {
        let i = i as u32;
        (match i {
            0 => a0,
            1 => a1,
            i if a0 > a1 => ((8 - i) * a0 + (i - 1) * a1) / 7,
            6 => 0,
            7 => 255,
            i => ((6 - i) * a0 + (i - 1) * a1) / 5,
        }) as u8
    })
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
            let values = bc3_alpha(b);
            let bits = u64::from_le_bytes([b[2], b[3], b[4], b[5], b[6], b[7], 0, 0]);
            (0..16).any(|t| values[((bits >> (3 * t)) & 7) as usize] < 255)
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
        // Not read from the blocks: the caller knows (the Web's KTX2).
        F::Etc2Rgb8 => false,
        F::Etc2Rgba8 | F::Astc4x4 => true,
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

/// The [`MESH_PIPES`] pipelines for `samples` ([`pipe_index`]): opaque
/// (back faces culled and double-sided, plain and shaded per pixel, whole
/// and cut at half alpha), translucent (blended, depth tested but never
/// written: front or back faces, plain or lit) and water (`vs_mesh_water`
/// over the water sets and the scene's depth copy, blended, never written).
pub(super) fn pipelines(
    device: &wgpu::Device,
    src: &MeshSource,
    samples: u32,
    format: wgpu::TextureFormat,
) -> [wgpu::RenderPipeline; MESH_PIPES] {
    let shader = &src.shader;
    // The water's module and layout: its group 3 binds the depth copy as
    // the scene's own (multisampled with MSAA, a float texture on WebGL2).
    let ms = samples > 1;
    let webgl2 = src.webgl2 && !ms;
    let water_source = if ms {
        MESH_WATER_WGSL.replace("texture_depth_2d", "texture_depth_multisampled_2d")
    } else if webgl2 {
        super::depth_as_float(MESH_WATER_WGSL)
    } else {
        MESH_WATER_WGSL.to_string()
    };
    let water_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("trino-tnl-water"),
        source: wgpu::ShaderSource::Wgsl(water_source.into()),
    });
    let water_bgl = super::water_bgl(device, ms, webgl2);
    let [g0, g1, g2] = &src.groups;
    let water_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("trino-tnl-water-layout"),
        bind_group_layouts: &[Some(g0), Some(g1), Some(g2), Some(&water_bgl)],
        immediate_size: 0,
    });
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
    let make = |i: usize| {
        let (pass, faces, lit, cut) = pipe_kind(i);
        let (module, layout, vs, fs) = match (pass, lit, cut) {
            (MeshPass::Water, _, _) => (
                &water_shader,
                &water_layout,
                "vs_mesh_water",
                "fs_mesh_water",
            ),
            (_, false, false) => (shader, &src.layout, "vs_mesh", "fs_mesh"),
            (_, true, false) => (shader, &src.lit_layout, "vs_mesh_lit", "fs_mesh_lit"),
            (_, false, true) => (shader, &src.layout, "vs_mesh", "fs_mesh_cut"),
            (_, true, true) => (shader, &src.lit_layout, "vs_mesh_lit", "fs_mesh_lit_cut"),
        };
        crate::renderer::pipes::render(
            device,
            wgpu::RenderPipelineDescriptor {
                label: Some(match pass {
                    MeshPass::Opaque if lit => "trino-tnl-lit-pipeline",
                    MeshPass::Opaque => "trino-tnl-pipeline",
                    MeshPass::Blend => "trino-tnl-blend-pipeline",
                    MeshPass::Water => "trino-tnl-water-pipeline",
                }),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(vs),
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
                    module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                // Front faces are counter-clockwise in the engine's Y-down
                // screen space (`tessellate_with`'s positive area), which is
                // clockwise in Y-up clip space.
                primitive: wgpu::PrimitiveState {
                    front_face: wgpu::FrontFace::Cw,
                    cull_mode: match faces {
                        Faces::Front => Some(wgpu::Face::Back),
                        Faces::Back => Some(wgpu::Face::Front),
                        Faces::Both => None,
                    },
                    ..Default::default()
                },
                // What blends (translucent copies, water) is tested
                // against the depth but never writes it: what is drawn
                // after it and lies behind it still shows.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(pass == MeshPass::Opaque),
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
            },
        )
    };
    core::array::from_fn(make)
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

    /// A mask (every alpha near 0 or 255) is cut on the GPU; a half
    /// transparent texel makes the texture blend. BC1's one-bit alpha is a
    /// mask; BC3's texels are read as they decode.
    #[test]
    fn alpha_masks_are_told_from_blends() {
        use super::super::CompressedFormat as F;
        use super::super::SpriteAlpha as A;
        assert_eq!(A::of_values([255, 255]), A::Opaque);
        assert_eq!(A::of_values([0, 255, 255]), A::Cutout);
        assert_eq!(A::of_values([3, 250, 16, 239]), A::Cutout);
        assert_eq!(A::of_values([0, 255, 17]), A::Blend);
        assert_eq!(A::of_values([128]), A::Blend);
        assert_eq!(A::of_rgba(&[9, 9, 9, 255, 9, 9, 9, 0]), A::Cutout);
        let opaque_bc1 = [0xFF, 0xFF, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF];
        assert_eq!(bc_alpha_class(F::Bc1, &opaque_bc1), A::Opaque);
        let holed_bc1 = [0x00, 0x00, 0xFF, 0xFF, 0x03, 0x00, 0x00, 0x00];
        assert_eq!(bc_alpha_class(F::Bc1, &holed_bc1), A::Cutout);
        // BC3: a0 = 255, a1 = 0 (eight-value table): texel 0 at a1 is a
        // mask; at index 4 (between them) it blends.
        let mut bc3 = [0u8; 16];
        bc3[0] = 255;
        assert_eq!(bc_alpha_class(F::Bc3, &bc3), A::Opaque);
        bc3[2] = 1;
        assert_eq!(bc_alpha_class(F::Bc3, &bc3), A::Cutout);
        bc3[2] = 4;
        assert_eq!(bc_alpha_class(F::Bc3, &bc3), A::Blend);
        // BC7 can't be read cheaply: any alpha blends.
        assert_eq!(
            bc_alpha_class(F::Bc7, &(1u128 << 5).to_le_bytes()),
            A::Blend
        );
        assert_eq!(bc_alpha_class(F::Etc2Rgb8, &[]), A::Opaque);
    }

    #[test]
    fn pipeline_indices_cover_the_set() {
        // Every index names a kind that maps back to it: no gap, no alias.
        for i in 0..MESH_PIPES {
            let (pass, faces, lit, cut) = pipe_kind(i);
            assert_eq!(pipe_index(pass, faces, lit, cut), i);
        }
        // The opaque ones kept #153's indices.
        assert_eq!(pipe_index(MeshPass::Opaque, Faces::Both, true, true), 7);
        assert_eq!(pipe_index(MeshPass::Blend, Faces::Back, true, false), 11);
        assert_eq!(pipe_index(MeshPass::Water, Faces::Both, false, false), 13);
    }

    #[test]
    fn layouts_match_the_shader() {
        // MeshIn: 3 + 3 + 3 floats, 4 bytes, 2 floats, 4 bytes, 1 float.
        assert_eq!(MESH_VERTEX_STRIDE, 56);
        // InstIn: two 3x4 matrices and two uvec4.
        assert_eq!(INSTANCE_STRIDE, 128);
        // Ten vec4f: #181 added the ground, the rim and the eye.
        assert_eq!(core::mem::size_of::<MeshCtx>(), 160);
        assert_eq!(core::mem::size_of::<JointBlock>(), 1536);
        assert_eq!(identity_block()[..3], Mat34::IDENTITY.m);
    }
}
