//! Vertex T&L on a GPU that transforms, lights and clips by itself: the
//! 3DS's PICA200 (its vertex shader units run `tnl3d.v.pica`). The rules
//! here decide, per draw, whether a backend may hand the mesh to that GPU
//! path instead of [`tessellate`](super::tessellate), and give it the
//! constants it needs. They are separate from the N64's RSP rules
//! ([`rsp_check`](super::rsp_check)) on purpose: the PICA clips in
//! hardware and lights in floating point, so it takes colored lights,
//! emissive, double-sided draws and meshes of any size the RSP must refuse,
//! while the RSP rules stay what the N64's hot path tests.
//!
//! The CPU path stays the reference: the GPU path draws what
//! `tessellate` would (same transform, the same lighting formula in
//! floats instead of bytes, the same per-vertex depth `1 - NEAR / z`),
//! and draws it refuses keep the CPU path (fog, flat shading, translucency,
//! textures, skeletons, palettes, local lights and anything crossing the
//! near plane).

use super::{Mesh, NEAR, TnlContext, model_space_light};
use crate::math::Vec3;
use crate::math3d::Mat34;
use crate::render::{ModelParams, Shading};

/// Why a draw stays off the GPU T&L path ([`gpu_check`]): the first rule it
/// fails, in the order the rules are tested.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum GpuReject {
    // The mesh (decided once per mesh, [`gpu_mesh_check`]).
    /// A skeleton (skinned or rigid-animated) mesh.
    Skinned,
    /// UVs (a textured mesh: the CPU path sorts its triangles).
    TexturedMesh,
    /// Vertex alpha below 255 (blending needs the CPU path's order).
    TranslucentMesh,
    /// No vertices or no triangles.
    Empty,
    /// More vertices than 16-bit indices address.
    TooManyVertices,
    // The draw.
    Fog,
    /// A tint with alpha below 255.
    Translucent,
    Textured,
    /// A palette on a mesh with color slots.
    Palette,
    /// `Shading::Flat`.
    Flat,
    /// No bounding sphere.
    NoBounds,
    /// Wholly outside the guard band: culled (invisible work).
    Offscreen,
    /// The sphere reaches past the near plane: the CPU path clips there.
    NearPlane,
    /// Lit with a matrix that isn't rotation + uniform scale (a shear, or
    /// an uneven scale on a mesh with sloped normals).
    NonUniformScale,
    /// A local light reaches it: the per-vertex lights are the CPU's.
    LocalLight,
}

impl GpuReject {
    /// A few words for tools (budget reports).
    pub const fn label(self) -> &'static str {
        match self {
            GpuReject::Skinned => "skinned mesh",
            GpuReject::TexturedMesh => "textured mesh",
            GpuReject::TranslucentMesh => "translucent vertices",
            GpuReject::Empty => "empty mesh",
            GpuReject::TooManyVertices => "too many vertices",
            GpuReject::Fog => "fog",
            GpuReject::Translucent => "translucent tint",
            GpuReject::Textured => "texture",
            GpuReject::Palette => "palette",
            GpuReject::Flat => "flat shading",
            GpuReject::NoBounds => "no bounds",
            GpuReject::Offscreen => "off screen",
            GpuReject::NearPlane => "crosses the near plane",
            GpuReject::NonUniformScale => "sheared or uneven scale",
            GpuReject::LocalLight => "local light",
        }
    }
}

/// The constants of one GPU T&L draw ([`gpu_check`]). A vertex's color is
/// `color / 255 · tint · (ambient + diffuse · max(0, n · light)) +
/// emissive`, clamped to 1: [`tessellate`](super::tessellate)'s formula in
/// floats.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GpuDraw {
    /// Unit vector toward the light in model space (zero for `Unlit`).
    pub light: Vec3,
    /// Ambient factor per channel (1 for `Unlit`).
    pub ambient: [f32; 3],
    /// Diffuse factor per channel (0 for `Unlit`).
    pub diffuse: [f32; 3],
    /// The tint, 0..1.
    pub tint: [f32; 4],
    /// The emissive color, 0..1.
    pub emissive: [f32; 3],
    /// Cull back faces (`false` for double-sided draws).
    pub cull_back: bool,
}

/// Whether a mesh can ever take the GPU T&L path (checked once, at
/// register).
pub fn gpu_mesh_check(mesh: &Mesh) -> Result<(), GpuReject> {
    if mesh.joint_count() != 0 || mesh.has_blend() {
        return Err(GpuReject::Skinned);
    }
    if mesh.has_uvs() {
        return Err(GpuReject::TexturedMesh);
    }
    if !mesh.opaque {
        return Err(GpuReject::TranslucentMesh);
    }
    if mesh.vertex_count == 0 || mesh.index_count < 3 {
        return Err(GpuReject::Empty);
    }
    if mesh.vertex_count > u16::MAX as usize {
        return Err(GpuReject::TooManyVertices);
    }
    Ok(())
}

/// [`gpu_check_mv`] computing `view · model` itself.
pub fn gpu_check(
    ctx: &TnlContext,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
) -> Result<GpuDraw, GpuReject> {
    gpu_check_mv(ctx, mesh, model, &ctx.view.mul(model), params)
}

/// Whether a backend with a GPU T&L path may send this draw there, and
/// with which constants; `mv` is the draw's `view · model`. Only for meshes
/// [`gpu_mesh_check`] accepted. In the core so a host test can predict the
/// console's work split exactly.
pub fn gpu_check_mv(
    ctx: &TnlContext,
    mesh: &Mesh,
    model: &Mat34,
    mv: &Mat34,
    params: &ModelParams,
) -> Result<GpuDraw, GpuReject> {
    if ctx.fog_on {
        return Err(GpuReject::Fog);
    }
    if params.tint.a < 255 {
        return Err(GpuReject::Translucent);
    }
    if params.texture.is_some() {
        return Err(GpuReject::Textured);
    }
    if params.palette.is_some() && mesh.has_slots() {
        return Err(GpuReject::Palette);
    }
    if params.shading == Shading::Flat {
        return Err(GpuReject::Flat);
    }
    let (c, r) = mesh.bounds().ok_or(GpuReject::NoBounds)?;
    let c = mv.transform_point(c);
    let s = model.axis_scales_sq();
    let r2 = r * r * s.x.max(s.y).max(s.z);
    // The CPU path culls a draw wholly outside its guard band: do the same,
    // so both paths draw the same set of meshes.
    let outside = |d: f32, k: f32| d < 0.0 && d * d > r2 * k;
    let abs = |v: f32| if v < 0.0 { -v } else { v };
    let (gx, gy) = (ctx.lim_x, ctx.lim_y);
    if outside(gx * c.z - abs(c.x), gx * gx + 1.0) || outside(gy * c.z - abs(c.y), gy * gy + 1.0) {
        return Err(GpuReject::Offscreen);
    }
    // Wholly past NEAR: no triangle the CPU path would have clipped there.
    let past = c.z - NEAR;
    if !(past > 0.0 && past * past > r2) {
        return Err(GpuReject::NearPlane);
    }
    if ctx.n_locals > 0 && params.shading != Shading::Unlit && ctx.lights_touch(c, r2) {
        return Err(GpuReject::LocalLight);
    }
    let k = |c: u8| c as f32 / 255.0;
    let t = params.tint;
    let e = params.emissive;
    let (light, ambient, diffuse) = match params.shading {
        Shading::Unlit => (Vec3::ZERO, [1.0; 3], [0.0; 3]),
        _ => {
            let l = model_space_light(mesh, model, ctx.ldir).ok_or(GpuReject::NonUniformScale)?;
            let a = ctx.light.ambient;
            let d = 1.0 - a;
            if ctx.white {
                (l * -1.0, [a; 3], [d; 3])
            } else {
                (
                    l * -1.0,
                    core::array::from_fn(|i| a * ctx.ac[i]),
                    core::array::from_fn(|i| d * ctx.lc[i]),
                )
            }
        }
    };
    Ok(GpuDraw {
        light,
        ambient,
        diffuse,
        tint: [k(t.r), k(t.g), k(t.b), k(t.a)],
        emissive: [k(e.r), k(e.g), k(e.b)],
        cull_back: !params.double_sided,
    })
}

/// Rows of the matrix that takes a model-space position to `(sx·z, sy·z,
/// depth·z, z)`: the screen pixel (`sx`, `sy`), the z-buffer depth
/// `1 - NEAR / z` the CPU path writes, and the view depth `z`, all
/// multiplied by `z` so a GPU's perspective divide gives the CPU path's
/// projection back and interpolates perspective-correctly. `mv` is the
/// draw's `view · model`.
pub fn gpu_screen_matrix(ctx: &TnlContext, mv: &Mat34) -> [[f32; 4]; 4] {
    let (f, c) = (ctx.focal, ctx.center);
    let m = &mv.m;
    let row = |i: usize| m[i];
    let comb = |a: f32, ra: [f32; 4], b: f32, rb: [f32; 4]| -> [f32; 4] {
        core::array::from_fn(|j| a * ra[j] + b * rb[j])
    };
    let (rx, ry, rz) = (row(0), row(1), row(2));
    let mut depth = rz;
    depth[3] -= NEAR;
    [comb(f, rx, c.x, rz), comb(-f, ry, c.y, rz), depth, rz]
}
