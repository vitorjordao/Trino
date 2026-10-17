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
//! floats instead of bytes, the same per-vertex depth `1 - NEAR / z`, the
//! same distance fog by view depth), and draws it refuses keep the CPU path
//! (flat shading, translucency, textured meshes, skeletons, palettes, local
//! lights and anything crossing the near plane).

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
    /// A tint with alpha below 255.
    Translucent,
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
    /// A water surface: the water vertex program draws it, or the CPU.
    Water,
    /// The backend found no memory for the GPU copy of the mesh (the 3DS's
    /// linear heap): reported by the renderer, never by [`gpu_check`].
    NoGpuMemory,
}

// `ALL` ends with the last variant: a new reason must join it.
const _: () = assert!(GpuReject::NoGpuMemory as usize + 1 == GpuReject::COUNT);

impl GpuReject {
    /// Every reason, in [`GpuReject::index`] order.
    pub const ALL: [GpuReject; GpuReject::COUNT] = [
        GpuReject::Skinned,
        GpuReject::TexturedMesh,
        GpuReject::TranslucentMesh,
        GpuReject::Empty,
        GpuReject::TooManyVertices,
        GpuReject::Translucent,
        GpuReject::Palette,
        GpuReject::Flat,
        GpuReject::NoBounds,
        GpuReject::Offscreen,
        GpuReject::NearPlane,
        GpuReject::NonUniformScale,
        GpuReject::LocalLight,
        GpuReject::Water,
        GpuReject::NoGpuMemory,
    ];

    /// Reasons in all ([`GpuReject::ALL`]'s length).
    pub const COUNT: usize = 15;

    /// The reason's slot in a per-reason histogram (`[u32; COUNT]`).
    pub const fn index(self) -> usize {
        self as usize
    }

    /// A few words for tools (budget reports).
    pub const fn label(self) -> &'static str {
        match self {
            GpuReject::Skinned => "skinned mesh",
            GpuReject::TexturedMesh => "textured mesh",
            GpuReject::TranslucentMesh => "translucent vertices",
            GpuReject::Empty => "empty mesh",
            GpuReject::TooManyVertices => "too many vertices",
            GpuReject::Translucent => "translucent tint",
            GpuReject::Palette => "palette",
            GpuReject::Flat => "flat shading",
            GpuReject::NoBounds => "no bounds",
            GpuReject::Offscreen => "off screen",
            GpuReject::NearPlane => "crosses the near plane",
            GpuReject::NonUniformScale => "sheared or uneven scale",
            GpuReject::LocalLight => "local light",
            GpuReject::Water => "water surface",
            GpuReject::NoGpuMemory => "no GPU memory",
        }
    }
}

/// The constants of one GPU T&L draw ([`gpu_check`]). A vertex's color is
/// `c = min(1, color / 255 · tint · (ambient + diffuse · max(0, n · light))
/// + emissive)`, then `c + (fog_color - c) · clamp(fog · p, 0, 1)` (alpha
/// kept): [`tessellate`](super::tessellate)'s formula in floats.
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
    /// The fog factor as a row on the model-space point `(x, y, z, 1)`:
    /// `(depth - near) / (far - near)` with `depth` the view depth, before
    /// the clamp to 0..1. All zero without fog.
    pub fog: [f32; 4],
    /// The fog color, 0..1.
    pub fog_color: [f32; 3],
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
    // No rule for `params.texture`: the mesh has no UVs (`gpu_mesh_check`),
    // so the CPU path draws it untextured too (`Mesh::texture_for`).
    if params.water.is_some() {
        return Err(GpuReject::Water);
    }
    if params.tint.a < 255 {
        return Err(GpuReject::Translucent);
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
    let (fog, fog_color) = match ctx.fog_on {
        true => {
            let f = ctx.light.fog;
            let s = 1.0 / (f.far - f.near);
            let mut row: [f32; 4] = core::array::from_fn(|j| mv.m[2][j] * s);
            row[3] -= f.near * s;
            (row, [k(f.color.r), k(f.color.g), k(f.color.b)])
        }
        false => ([0.0; 4], [0.0; 3]),
    };
    Ok(GpuDraw {
        light,
        ambient,
        diffuse,
        tint: [k(t.r), k(t.g), k(t.b), k(t.a)],
        emissive: [k(e.r), k(e.g), k(e.b)],
        fog,
        fog_color,
        cull_back: !params.double_sided,
    })
}

/// The constants of a water surface drawn by a GPU's vertex unit (the 3DS's
/// `water.v.pica`, ADR 0015): the same swell, ripples, light, Fresnel, glint
/// and fog as the CPU's water T&L ([`crate::water`]), as shader uniforms.
/// Ripples are packed four to a vector (x, z, radius, strength): one lane
/// each, so the shader evaluates them together.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GpuWater {
    /// World to the screen (see [`gpu_screen_matrix`]: pixel, depth, view
    /// depth, all times the view depth).
    pub screen: [[f32; 4]; 4],
    /// Model to world, three rows.
    pub model: [[f32; 4]; 3],
    /// The swell: directions (x z, x z), frequencies and minus the phases
    /// (`k0, k1, -p0, -p1`), heights and slopes (`a0, a1, a0·2πk0, a1·2πk1`).
    pub wdir: [f32; 4],
    pub wk: [f32; 4],
    pub wamp: [f32; 4],
    /// Ripples: centers x and z, radii, strengths (strength × response ×
    /// decay; 0 = none).
    pub rx: [f32; 4],
    pub rz: [f32; 4],
    pub rrad: [f32; 4],
    pub rstr: [f32; 4],
    /// Toward the light (world, unit); ambient and diffuse factors (0..1;
    /// unlit: ambient 1, diffuse 0); tint, emissive (0..1).
    pub light: [f32; 4],
    pub ambient: [f32; 4],
    pub diffuse: [f32; 4],
    pub tint: [f32; 4],
    pub emissive: [f32; 4],
    /// The eye (world), the reflected sky (0..1), the glint (the light's
    /// color × gloss, 0..1; w = its exponent).
    pub eye: [f32; 4],
    pub sky: [f32; 4],
    pub glint: [f32; 4],
    /// Fog: the factor's row on the world point (zero without fog), color.
    pub fog: [f32; 4],
    pub fog_color: [f32; 4],
    /// The texture's scroll (u, v) and the detail layer's offset (u, v).
    pub scroll: [f32; 4],
    pub detail_offset: [f32; 4],
    pub cull_back: bool,
}

/// Whether a backend with a GPU water program may draw this water draw
/// there ([`GpuWater`]), with the frame's water state from the context.
/// Static meshes only, wholly past the near plane, no local light reaching
/// them (the same rules as [`gpu_check`]); `detail_scale` is the detail
/// layer's UV scale (1 without one).
pub fn gpu_water(
    ctx: &TnlContext,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
    detail_scale: f32,
) -> Result<GpuWater, GpuReject> {
    let w = params.water.as_ref().ok_or(GpuReject::Water)?;
    if mesh.joint_count() != 0 || mesh.has_blend() {
        return Err(GpuReject::Skinned);
    }
    if mesh.vertex_count == 0 || mesh.index_count < 3 {
        return Err(GpuReject::Empty);
    }
    if mesh.vertex_count > u16::MAX as usize {
        return Err(GpuReject::TooManyVertices);
    }
    let mv = ctx.view.mul(model);
    let (c, r) = mesh.bounds().ok_or(GpuReject::NoBounds)?;
    let c = mv.transform_point(c);
    let s = model.axis_scales_sq();
    // The swell and ripples lift it a little past its bounds.
    let lift = w.wave_amplitude * 1.45 + 0.3 * w.ripple_responsiveness;
    let r = crate::math3d::sqrt(r * r * s.x.max(s.y).max(s.z)) + lift;
    let r2 = r * r;
    let outside = |d: f32, k: f32| d < 0.0 && d * d > r2 * k;
    let abs = |v: f32| if v < 0.0 { -v } else { v };
    let (gx, gy) = (ctx.lim_x, ctx.lim_y);
    if outside(gx * c.z - abs(c.x), gx * gx + 1.0) || outside(gy * c.z - abs(c.y), gy * gy + 1.0) {
        return Err(GpuReject::Offscreen);
    }
    // Wholly past NEAR (the bounding sphere, or else every vertex, lifted
    // by the most the surface can move: a long river's sphere reaches the
    // camera while all of it lies ahead): no triangle the CPU path would
    // have clipped there.
    let past = c.z - NEAR;
    if !(past > 0.0 && past * past > r2) {
        let ahead = (0..mesh.vertex_count).all(|i| {
            let p = mv.transform_point(mesh.position(i));
            p.z - lift > NEAR
        });
        if !ahead {
            return Err(GpuReject::NearPlane);
        }
    }
    if ctx.n_locals > 0 && params.shading != Shading::Unlit && ctx.lights_touch(c, r2) {
        return Err(GpuReject::LocalLight);
    }
    let state = ctx.water();
    let waves = crate::water::WaveSet::new(w, state.time);
    let tau = core::f32::consts::TAU;
    let mut rx = [0.0; 4];
    let mut rz = [0.0; 4];
    let mut rrad = [0.0; 4];
    let mut rstr = [0.0; 4];
    if w.ripple_responsiveness != 0.0 {
        for (i, rp) in state.ripples().iter().take(4).enumerate() {
            rx[i] = rp.pos.x;
            rz[i] = rp.pos.y;
            rrad[i] = rp.radius();
            rstr[i] = rp.strength * w.ripple_responsiveness * rp.decay();
        }
    }
    let k = |c: u8| c as f32 / 255.0;
    let light = ctx.light;
    let (ambient, diffuse) = match params.shading {
        Shading::Unlit => ([1.0, 1.0, 1.0, 0.0], [0.0; 4]),
        _ => {
            let a = light.ambient;
            (
                [a * ctx.ac[0], a * ctx.ac[1], a * ctx.ac[2], 0.0],
                [
                    (1.0 - a) * ctx.lc[0],
                    (1.0 - a) * ctx.lc[1],
                    (1.0 - a) * ctx.lc[2],
                    0.0,
                ],
            )
        }
    };
    let sky = if light.fog.enabled() {
        light.fog.color
    } else {
        light.ambient_color
    };
    let gloss = params.gloss as f32 / 255.0;
    let (fog, fog_color) = if ctx.fog_on {
        let f = light.fog;
        let s = 1.0 / (f.far - f.near);
        let mut row: [f32; 4] = core::array::from_fn(|j| ctx.view.m[2][j] * s);
        row[3] -= f.near * s;
        (row, [k(f.color.r), k(f.color.g), k(f.color.b), 1.0])
    } else {
        ([0.0; 4], [0.0; 4])
    };
    let (base, _) = w.scroll(state.time);
    let off = w.detail_offset(state.time, detail_scale);
    let (t, e) = (params.tint, params.emissive);
    let sun = -ctx.ldir;
    Ok(GpuWater {
        screen: gpu_screen_matrix(ctx, &ctx.view),
        model: [model.m[0], model.m[1], model.m[2]],
        wdir: [
            waves.dir[0].x,
            waves.dir[0].y,
            waves.dir[1].x,
            waves.dir[1].y,
        ],
        wk: [waves.k[0], waves.k[1], -waves.phase[0], -waves.phase[1]],
        wamp: [
            waves.amp[0],
            waves.amp[1],
            waves.amp[0] * tau * waves.k[0],
            waves.amp[1] * tau * waves.k[1],
        ],
        rx,
        rz,
        rrad,
        rstr,
        light: [sun.x, sun.y, sun.z, 0.0],
        ambient,
        diffuse,
        tint: [k(t.r), k(t.g), k(t.b), k(t.a)],
        emissive: [k(e.r), k(e.g), k(e.b), 0.0],
        eye: [ctx.camera.eye.x, ctx.camera.eye.y, ctx.camera.eye.z, 1.0],
        sky: [k(sky.r), k(sky.g), k(sky.b), 1.0],
        glint: [
            k(light.color.r) * gloss,
            k(light.color.g) * gloss,
            k(light.color.b) * gloss,
            if params.gloss > 128 { 16.0 } else { 8.0 },
        ],
        fog,
        fog_color,
        scroll: [base.x, base.y, 0.0, 0.0],
        detail_offset: [off.x, off.y, 0.0, 0.0],
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

#[cfg(test)]
mod tests {
    use super::GpuReject;

    #[test]
    fn every_reason_has_its_own_histogram_slot() {
        for (i, r) in GpuReject::ALL.iter().enumerate() {
            assert_eq!(r.index(), i, "{r:?}");
        }
    }
}
