//! The T&L for draws local lights reach, and for the PC's per-pixel tier
//! ([`tessellate_surface`]): the same transform, clip, split and project
//! as `tessellate_joints`, plus each vertex's local lights (per-vertex
//! tier) and, for per-pixel lighting, each vertex's [`SurfaceVert`].
//!
//! A separate copy on purpose. The static path's body is what most N64
//! frames spend their CPU in, and it must fit the VR4300's 16 KB
//! instruction cache: folding lights and varyings into it (even as
//! never-taken branches and zero-sized generics) cost every unlit draw
//! 10–35 % in ares. Draws no local light reaches never get here, so the
//! consoles' common path is the code it was before local lights existed.

use super::{
    DEPTH_SPLIT, MAX_POLY, NEAR, SPLIT_STACK, ScreenTri, Setup, TnlContext, TnlScratch, TnlStats,
    VCACHE, ViewLight, lerp_color, model_space_light, split_policy,
};
use crate::light::MAX_VERTEX_LIGHTS;
use crate::math::{Color, Vec2, Vec3};
use crate::math3d::Mat34;
use crate::render::{ModelParams, Shading};
use crate::render3d::{MAX_JOINTS, Mesh};

/// A vertex's surface for per-pixel lighting ([`tessellate_surface`]):
/// world position, world normal and its unlit color.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SurfaceVert {
    /// World position.
    pub pos: Vec3,
    /// World normal (unit at vertices, interpolated between them); zero
    /// for unlit draws, which per-pixel lighting leaves alone.
    pub normal: Vec3,
    /// The color lighting multiplies: vertex color × palette × tint (no
    /// light, emissive or fog).
    pub base: Color,
}

const NO_SURFACE: SurfaceVert = SurfaceVert {
    pos: Vec3::ZERO,
    normal: Vec3::ZERO,
    base: Color::WHITE,
};

/// A clip vertex of this path: the original `(position, color, uv)` for
/// the consoles' lit draws (the same scratch as the static path), or that
/// plus a [`SurfaceVert`] for per-pixel lighting.
trait Vert: Copy {
    type X: Copy;
    fn new(p: Vec3, c: Color, uv: Vec2, x: Self::X) -> Self;
    fn parts(self) -> (Vec3, Color, Vec2, Self::X);
    fn mix(a: Self::X, b: Self::X, t: f32) -> Self::X;
    /// The varying of a vertex; `f` runs only for the surface path.
    fn make(f: impl FnOnce() -> SurfaceVert) -> Self::X;
    fn faceted(x: Self::X, n: Vec3) -> Self::X;
}

impl Vert for (Vec3, Color, Vec2) {
    type X = ();
    #[inline(always)]
    fn new(p: Vec3, c: Color, uv: Vec2, _: ()) -> Self {
        (p, c, uv)
    }
    #[inline(always)]
    fn parts(self) -> (Vec3, Color, Vec2, ()) {
        (self.0, self.1, self.2, ())
    }
    #[inline(always)]
    fn mix(_: (), _: (), _: f32) {}
    #[inline(always)]
    fn make(_: impl FnOnce() -> SurfaceVert) {}
    #[inline(always)]
    fn faceted(_: (), _: Vec3) {}
}

type SurfVert = (Vec3, Color, Vec2, SurfaceVert);

impl Vert for SurfVert {
    type X = SurfaceVert;
    #[inline(always)]
    fn new(p: Vec3, c: Color, uv: Vec2, x: SurfaceVert) -> Self {
        (p, c, uv, x)
    }
    #[inline(always)]
    fn parts(self) -> (Vec3, Color, Vec2, SurfaceVert) {
        self
    }
    #[inline]
    fn mix(a: SurfaceVert, b: SurfaceVert, t: f32) -> SurfaceVert {
        SurfaceVert {
            pos: a.pos + (b.pos - a.pos) * t,
            normal: a.normal + (b.normal - a.normal) * t,
            base: lerp_color(a.base, b.base, t),
        }
    }
    #[inline(always)]
    fn make(f: impl FnOnce() -> SurfaceVert) -> SurfaceVert {
        f()
    }
    #[inline(always)]
    fn faceted(x: SurfaceVert, n: Vec3) -> SurfaceVert {
        SurfaceVert { normal: n, ..x }
    }
}

/// Where this path hands each triangle (and its vertices' varying).
trait Sink<X> {
    fn tri(&mut self, tri: ScreenTri, x: &[X; 3]);
}

impl Sink<()> for dyn FnMut(ScreenTri) + '_ {
    #[inline(always)]
    fn tri(&mut self, tri: ScreenTri, _: &[(); 3]) {
        self(tri)
    }
}

impl Sink<SurfaceVert> for dyn FnMut(ScreenTri, &[SurfaceVert; 3]) + '_ {
    #[inline(always)]
    fn tri(&mut self, tri: ScreenTri, x: &[SurfaceVert; 3]) {
        self(tri, x)
    }
}

/// The working memory of [`tessellate_surface`] (like [`TnlScratch`]; keep
/// one per backend, boxed).
pub struct SurfaceScratch {
    cache_pos: [Vec3; VCACHE],
    cache_col: [Color; VCACHE],
    cache_x: [SurfaceVert; VCACHE],
    poly_a: [SurfVert; MAX_POLY],
    poly_b: [SurfVert; MAX_POLY],
    stack: [[SurfVert; 3]; SPLIT_STACK],
    skin: [Mat34; MAX_JOINTS],
    world: [Mat34; MAX_JOINTS],
    viewm: [Mat34; MAX_JOINTS],
}

impl SurfaceScratch {
    pub const fn new() -> SurfaceScratch {
        let v: SurfVert = (Vec3::ZERO, Color::WHITE, Vec2::ZERO, NO_SURFACE);
        SurfaceScratch {
            cache_pos: [Vec3::ZERO; VCACHE],
            cache_col: [Color::WHITE; VCACHE],
            cache_x: [NO_SURFACE; VCACHE],
            poly_a: [v; MAX_POLY],
            poly_b: [v; MAX_POLY],
            stack: [[v; 3]; SPLIT_STACK],
            skin: [Mat34::IDENTITY; MAX_JOINTS],
            world: [Mat34::IDENTITY; MAX_JOINTS],
            viewm: [Mat34::IDENTITY; MAX_JOINTS],
        }
    }
}

impl Default for SurfaceScratch {
    fn default() -> Self {
        Self::new()
    }
}

/// The per-triangle buffers of one draw.
struct Bufs<'a, V: Vert> {
    cache_pos: &'a mut [Vec3; VCACHE],
    cache_col: &'a mut [Color; VCACHE],
    cache_x: &'a mut [V::X; VCACHE],
    poly_a: &'a mut [V; MAX_POLY],
    poly_b: &'a mut [V; MAX_POLY],
    stack: &'a mut [[V; 3]; SPLIT_STACK],
}

/// [`super::Setup`] plus the local lights that reach the draw.
struct LitSetup<'a> {
    s: Setup<'a>,
    view: &'a Mat34,
    locals: &'a [ViewLight; MAX_VERTEX_LIGHTS],
    pick: [u8; MAX_VERTEX_LIGHTS],
    n_pick: usize,
}

impl LitSetup<'_> {
    /// Does a picked light reach view-space point `p`? (No square root:
    /// vertices out of every radius take the usual shading.)
    #[inline(always)]
    fn lit_at(&self, p: Vec3) -> bool {
        self.pick[..self.n_pick].iter().any(|&i| {
            let l = &self.locals[i as usize];
            let v = l.pos - p;
            v.dot(v) * l.inv_r2 < 1.0
        })
    }

    /// [`Setup::shade`] plus the local lights, from a world-space normal
    /// `n`, the view-space position `p` and the view depth.
    fn shade_local(&self, c: Color, n: Vec3, p: Vec3, z: f32) -> Color {
        let s = &self.s;
        let light = s.light;
        let diffuse = -n.dot(s.ldir);
        let d = (1.0 - light.ambient) * if diffuse > 0.0 { diffuse } else { 0.0 };
        let mut k = if s.white {
            let i = light.ambient + d;
            [i, i, i]
        } else {
            [
                light.ambient * s.ac[0] + d * s.lc[0],
                light.ambient * s.ac[1] + d * s.lc[1],
                light.ambient * s.ac[2] + d * s.lc[2],
            ]
        };
        let nv = self.view.transform_dir(n);
        for &i in &self.pick[..self.n_pick] {
            let l = &self.locals[i as usize];
            let v = l.pos - p;
            let d2 = v.dot(v);
            let f = 1.0 - d2 * l.inv_r2;
            if f <= 0.0 || d2 <= 0.0 {
                continue;
            }
            let inv = 1.0 / crate::math3d::sqrt(d2);
            let ndl = nv.dot(v) * inv;
            if ndl <= 0.0 {
                continue;
            }
            let mut a = f * f * ndl;
            if let Some((dir, co, ir)) = l.spot {
                a *= crate::light::cone_factor(-v.dot(dir) * inv, co, ir);
            }
            k[0] += l.color[0] * a;
            k[1] += l.color[1] * a;
            k[2] += l.color[2] * a;
        }
        s.finish(c, k, z)
    }

    /// Vertex `i`'s unlit color for per-pixel lighting: the base color ×
    /// tint (alpha included).
    #[inline(always)]
    fn albedo(&self, mesh: &Mesh, i: usize) -> Color {
        let c = self.s.base_color(mesh, i);
        let t = self.s.tint_k;
        let mul = |v: u8, t: f32| (v as f32 * t) as u8;
        Color::rgba(
            mul(c.r, t[0]),
            mul(c.g, t[1]),
            mul(c.b, t[2]),
            mul(c.a, t[3]),
        )
    }
}

/// The local lights of `ctx` that reach the draw (`bound`: its view-space
/// bounding sphere center and squared radius; `None` = every light).
fn pick(ctx: &TnlContext, bound: Option<(Vec3, f32)>) -> ([u8; MAX_VERTEX_LIGHTS], usize) {
    let mut pick = [0u8; MAX_VERTEX_LIGHTS];
    let mut n = 0;
    for (i, l) in ctx.view_locals[..ctx.n_locals as usize].iter().enumerate() {
        if bound.is_none_or(|(c, r2)| super::reaches(l, c, r2)) {
            pick[n] = i as u8;
            n += 1;
        }
    }
    (pick, n)
}

/// The cull of `tessellate_with`: `Err` with the stats of a culled draw,
/// else the view-space bound (`None` for meshes without one).
#[inline(always)]
fn cull(
    ctx: &TnlContext,
    mesh: &Mesh,
    model: &Mat34,
    mv: &Mat34,
) -> Result<Option<(Vec3, f32)>, TnlStats> {
    let Some((c, r)) = mesh.bounds() else {
        return Ok(None);
    };
    let (lim_x, lim_y) = (ctx.lim_x, ctx.lim_y);
    let c = mv.transform_point(c);
    let s = model.axis_scales_sq();
    let r2 = r * r * s.x.max(s.y).max(s.z);
    let outside = |d: f32, lim: f32| d < 0.0 && d * d > r2 * (lim * lim + 1.0);
    let behind = c.z <= NEAR && (NEAR - c.z) * (NEAR - c.z) >= r2;
    if behind
        || outside(lim_x * c.z - c.x, lim_x)
        || outside(lim_x * c.z + c.x, lim_x)
        || outside(lim_y * c.z - c.y, lim_y)
        || outside(lim_y * c.z + c.y, lim_y)
    {
        return Err(TnlStats {
            culled: 1,
            ..TnlStats::default()
        });
    }
    Ok(Some((c, r2)))
}

/// The draw's constant setup (`tessellate_with`'s, same values).
fn setup<'a>(
    ctx: &'a TnlContext,
    mesh: &Mesh<'a>,
    model: &Mat34,
    params: &'a ModelParams,
) -> Setup<'a> {
    Setup {
        focal: ctx.focal,
        center: ctx.center,
        lim_x: ctx.lim_x,
        lim_y: ctx.lim_y,
        ldir: ctx.ldir,
        light: &ctx.light,
        params,
        lc: ctx.lc,
        ac: ctx.ac,
        white: ctx.white,
        has_emissive: params.emissive.r | params.emissive.g | params.emissive.b != 0,
        fog_on: ctx.fog_on,
        tint_k: [
            params.tint.r as f32 / 255.0,
            params.tint.g as f32 / 255.0,
            params.tint.b as f32 / 255.0,
            params.tint.a as f32 / 255.0,
        ],
        split: split_policy(ctx, mesh, params),
        local_ldir: if mesh.joint_count() == 0 && params.shading == Shading::Gouraud {
            model_space_light(mesh, model, ctx.ldir)
        } else {
            None
        },
        palette: match (params.palette, mesh.slots) {
            (Some(p), Some(slots)) => Some((slots, [Color::WHITE, p[0], p[1], p[2], p[3]])),
            _ => None,
        },
    }
}

/// `tessellate_with` for a draw the context's local lights may reach:
/// `None` when none does (the caller takes its usual path).
#[inline(never)]
pub(super) fn tessellate_lit(
    ctx: &TnlContext,
    scratch: &mut TnlScratch,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
    emit: &mut dyn FnMut(ScreenTri),
) -> Option<TnlStats> {
    let mv = ctx.view.mul(model);
    let bound = match cull(ctx, mesh, model, &mv) {
        Ok(b) => b,
        Err(culled) => return Some(culled),
    };
    let (pick, n_pick) = pick(ctx, bound);
    if n_pick == 0 {
        return None;
    }
    let s = LitSetup {
        s: setup(ctx, mesh, model, params),
        view: &ctx.view,
        locals: &ctx.view_locals,
        pick,
        n_pick,
    };
    let super::TriScratch {
        cache_pos,
        cache_col,
        poly_a,
        poly_b,
        stack,
    } = &mut scratch.tri;
    let mut cache_x = [(); VCACHE];
    let bufs = Bufs::<(Vec3, Color, Vec2)> {
        cache_pos,
        cache_col,
        cache_x: &mut cache_x,
        poly_a,
        poly_b,
        stack,
    };
    Some(run(
        mesh,
        model,
        mv,
        params,
        &s,
        bufs,
        (&mut scratch.skin, &mut scratch.world, &mut scratch.viewm),
        emit,
    ))
}

/// [`super::tessellate_with`] that also hands over each vertex's
/// [`SurfaceVert`] (world position, world normal, unlit color): what a
/// per-pixel lighting backend ([`crate::LightTier::PerPixel`]) shades local
/// lights, specular and wetness with. The [`ScreenTri`]s are the ones
/// `tessellate_with` makes (with the context's per-vertex local lights too,
/// when it has any).
pub fn tessellate_surface(
    ctx: &TnlContext,
    scratch: &mut SurfaceScratch,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
    emit: &mut dyn FnMut(ScreenTri, &[SurfaceVert; 3]),
) -> TnlStats {
    let mv = ctx.view.mul(model);
    let bound = match cull(ctx, mesh, model, &mv) {
        Ok(b) => b,
        Err(culled) => return culled,
    };
    let (pick, n_pick) = if ctx.n_locals > 0 && params.shading != Shading::Unlit {
        pick(ctx, bound)
    } else {
        ([0; MAX_VERTEX_LIGHTS], 0)
    };
    let s = LitSetup {
        s: setup(ctx, mesh, model, params),
        view: &ctx.view,
        locals: &ctx.view_locals,
        pick,
        n_pick,
    };
    let SurfaceScratch {
        cache_pos,
        cache_col,
        cache_x,
        poly_a,
        poly_b,
        stack,
        skin,
        world,
        viewm,
    } = scratch;
    let bufs = Bufs::<SurfVert> {
        cache_pos,
        cache_col,
        cache_x,
        poly_a,
        poly_b,
        stack,
    };
    run(
        mesh,
        model,
        mv,
        params,
        &s,
        bufs,
        (skin, world, viewm),
        emit,
    )
}

type Joints<'a> = (
    &'a mut [Mat34; MAX_JOINTS],
    &'a mut [Mat34; MAX_JOINTS],
    &'a mut [Mat34; MAX_JOINTS],
);

#[allow(clippy::too_many_arguments)]
fn run<V: Vert, K: Sink<V::X> + ?Sized>(
    mesh: &Mesh,
    model: &Mat34,
    mv: Mat34,
    params: &ModelParams,
    s: &LitSetup,
    bufs: Bufs<V>,
    (skin, world, viewm): Joints,
    emit: &mut K,
) -> TnlStats {
    if mesh.joint_count() == 0 {
        return joints::<false, false, V, K>(mesh, &[*model], &[mv], s, bufs, emit);
    }
    let n = mesh.pose(params.anim, skin);
    for j in 0..n {
        world[j] = model.mul(&skin[j]);
        viewm[j] = s.view.mul(&world[j]);
    }
    let (world, viewm) = (&world[..n], &viewm[..n]);
    if mesh.has_blend() {
        joints::<true, true, V, K>(mesh, world, viewm, s, bufs, emit)
    } else {
        joints::<true, false, V, K>(mesh, world, viewm, s, bufs, emit)
    }
}

/// Clip a view-space polygon against `dist(v) >= 0` (Sutherland-Hodgman).
fn clip_plane<V: Vert>(
    verts: &[V; MAX_POLY],
    n_in: usize,
    out: &mut [V; MAX_POLY],
    dist: impl Fn(Vec3) -> f32,
) -> usize {
    let mut n_out = 0;
    for i in 0..n_in {
        let (a, ca, ua, xa) = verts[i].parts();
        let (b, cb, ub, xb) = verts[(i + 1) % n_in].parts();
        let (da, db) = (dist(a), dist(b));
        if da >= 0.0 {
            if n_out == MAX_POLY {
                return n_out;
            }
            out[n_out] = verts[i];
            n_out += 1;
        }
        if (da >= 0.0) != (db >= 0.0) {
            if n_out == MAX_POLY {
                return n_out;
            }
            let t = da / (da - db);
            out[n_out] = V::new(
                a + (b - a) * t,
                lerp_color(ca, cb, t),
                ua + (ub - ua) * t,
                V::mix(xa, xb, t),
            );
            n_out += 1;
        }
    }
    n_out
}

#[cold]
#[inline(never)]
fn clip_frustum<V: Vert>(
    poly_a: &mut [V; MAX_POLY],
    poly_b: &mut [V; MAX_POLY],
    lim_x: f32,
    lim_y: f32,
) -> usize {
    let mut n = clip_plane(poly_a, 3, poly_b, |v| v.z - NEAR);
    n = clip_plane(poly_b, n, poly_a, |v| lim_x * v.z - v.x);
    n = clip_plane(poly_a, n, poly_b, |v| lim_x * v.z + v.x);
    n = clip_plane(poly_b, n, poly_a, |v| lim_y * v.z - v.y);
    n = clip_plane(poly_a, n, poly_b, |v| lim_y * v.z + v.y);
    *poly_a = *poly_b;
    n
}

/// `tessellate_joints` with local lights and the varying (see the module
/// docs for why it is a copy).
#[inline(never)]
fn joints<const SKINNED: bool, const SMOOTH: bool, V: Vert, K: Sink<V::X> + ?Sized>(
    mesh: &Mesh,
    world: &[Mat34],
    viewm: &[Mat34],
    ls: &LitSetup,
    b: Bufs<V>,
    emit: &mut K,
) -> TnlStats {
    let s = &ls.s;
    let Bufs {
        cache_pos,
        cache_col,
        cache_x,
        poly_a,
        poly_b,
        stack,
    } = b;
    let mut stats = TnlStats::default();
    let mut verts = 0u32;
    let joint = |i: usize| if SKINNED { mesh.joint_of(i) } else { 0 };
    let flat = s.params.shading == Shading::Flat;
    let (lim_x, lim_y, focal, center) = (s.lim_x, s.lim_y, s.focal, s.center);
    let mut tag = [u32::MAX; VCACHE];
    let textured = mesh.has_uvs();
    let mut vertex = |i: usize| -> (Vec3, Color, V::X) {
        let slot = i % VCACHE;
        if tag[slot] == i as u32 {
            return (cache_pos[slot], cache_col[slot], cache_x[slot]);
        }
        verts += 1;
        let j = joint(i);
        let local = mesh.position(i);
        let mut p = viewm[j].transform_point(local);
        let blend = if SMOOTH { mesh.blend_of(i) } else { None };
        if let Some((j2, w)) = blend {
            let p2 = viewm[j2].transform_point(local);
            p = p + (p2 - p) * w;
        }
        let world_normal = || {
            let nl = mesh.normal(i);
            let mut n = world[j].transform_dir(nl);
            if let Some((j2, w)) = blend {
                n = n + (world[j2].transform_dir(nl) - n) * w;
            }
            n.normalized()
        };
        let col = match s.params.shading {
            Shading::Flat => Color::WHITE,
            Shading::Unlit => s.shade(s.base_color(mesh, i), Vec3::ZERO, p.z),
            Shading::Gouraud if ls.n_pick > 0 && ls.lit_at(p) => {
                ls.shade_local(s.base_color(mesh, i), world_normal(), p, p.z)
            }
            Shading::Gouraud if !SKINNED && s.local_ldir.is_some() => {
                let ll = s.local_ldir.unwrap_or(Vec3::ZERO);
                let nl = mesh.normal(i);
                s.shade_with(s.base_color(mesh, i), || -nl.dot(ll), p.z)
            }
            Shading::Gouraud => s.shade(s.base_color(mesh, i), world_normal(), p.z),
        };
        let x = V::make(|| {
            let mut wp = world[j].transform_point(local);
            if let Some((j2, w)) = blend {
                wp = wp + (world[j2].transform_point(local) - wp) * w;
            }
            SurfaceVert {
                pos: wp,
                normal: match s.params.shading {
                    Shading::Unlit => Vec3::ZERO,
                    _ => world_normal(),
                },
                base: ls.albedo(mesh, i),
            }
        });
        tag[slot] = i as u32;
        cache_pos[slot] = p;
        cache_col[slot] = col;
        cache_x[slot] = x;
        (p, col, x)
    };

    let tris = mesh.indices.as_flattened().as_chunks::<6>().0;
    stats.tris_in = tris.len() as u32;
    for t in tris {
        let idx = [
            u16::from_le_bytes([t[0], t[1]]) as usize,
            u16::from_le_bytes([t[2], t[3]]) as usize,
            u16::from_le_bytes([t[4], t[5]]) as usize,
        ];
        let v = [vertex(idx[0]), vertex(idx[1]), vertex(idx[2])];
        let vs = [v[0].0, v[1].0, v[2].0];
        if vs[0].z <= NEAR && vs[1].z <= NEAR && vs[2].z <= NEAR {
            continue;
        }
        if !s.params.double_sided && (vs[1] - vs[0]).cross(vs[2] - vs[0]).dot(vs[0]) >= 0.0 {
            stats.backfaced += 1;
            continue;
        }
        let mut xs = [v[0].2, v[1].2, v[2].2];
        let colors = if flat {
            let p: [Vec3; 3] = core::array::from_fn(|k| {
                let i = idx[k];
                let local = mesh.position(i);
                let a = world[joint(i)].transform_point(local);
                match if SMOOTH { mesh.blend_of(i) } else { None } {
                    Some((j2, w)) => a + (world[j2].transform_point(local) - a) * w,
                    None => a,
                }
            });
            let n = (p[1] - p[0]).cross(p[2] - p[0]).normalized();
            xs = [
                V::faceted(xs[0], n),
                V::faceted(xs[1], n),
                V::faceted(xs[2], n),
            ];
            core::array::from_fn(|k| {
                let c = s.base_color(mesh, idx[k]);
                if ls.n_pick > 0 && ls.lit_at(vs[k]) {
                    ls.shade_local(c, n, vs[k], vs[k].z)
                } else {
                    s.shade(c, n, vs[k].z)
                }
            })
        } else {
            [v[0].1, v[1].1, v[2].1]
        };

        let inside = |v: Vec3| {
            v.z > NEAR
                && v.x >= -lim_x * v.z
                && v.x <= lim_x * v.z
                && v.y >= -lim_y * v.z
                && v.y <= lim_y * v.z
        };
        for k in 0..3 {
            let uv = if textured {
                mesh.uv(idx[k])
            } else {
                Vec2::ZERO
            };
            poly_a[k] = V::new(vs[k], colors[k], uv, xs[k]);
        }
        let mut n_poly = 3;
        if !(inside(vs[0]) && inside(vs[1]) && inside(vs[2])) {
            stats.clipped += 1;
            n_poly = clip_frustum(poly_a, poly_b, lim_x, lim_y);
            if n_poly < 3 {
                continue;
            }
        }

        let project = |v: Vec3, inv: f32| {
            let k = focal * inv;
            Vec2::new(center.x + v.x * k, center.y - v.y * k)
        };
        for k in 1..n_poly - 1 {
            stack[0] = [poly_a[0], poly_a[k], poly_a[k + 1]];
            let mut top = 1usize;
            while top > 0 {
                top -= 1;
                let tri = stack[top];
                let z = |a: usize| tri[a].parts().0.z;
                let span = |a: usize, b: usize| {
                    let d = z(a) - z(b);
                    if d < 0.0 { -d } else { d }
                };
                let worst = if !s.split {
                    (0.0, 0)
                } else {
                    let (s01, s12, s20) = (span(0, 1), span(1, 2), span(2, 0));
                    if s01 >= s12 && s01 >= s20 {
                        (s01, 0)
                    } else if s12 >= s20 {
                        (s12, 1)
                    } else {
                        (s20, 2)
                    }
                };
                if worst.0 > DEPTH_SPLIT && top + 2 <= SPLIT_STACK {
                    let (a, b, c) = match worst.1 {
                        0 => (tri[0], tri[1], tri[2]),
                        1 => (tri[1], tri[2], tri[0]),
                        _ => (tri[2], tri[0], tri[1]),
                    };
                    let (pa, ca, ua, xa) = a.parts();
                    let (pb, cb, ub, xb) = b.parts();
                    let mid = V::new(
                        (pa + pb) * 0.5,
                        lerp_color(ca, cb, 0.5),
                        (ua + ub) * 0.5,
                        V::mix(xa, xb, 0.5),
                    );
                    stack[top] = [a, mid, c];
                    stack[top + 1] = [mid, b, c];
                    top += 2;
                    stats.splits += 1;
                    continue;
                }
                let (mut v0, mut c0, mut u0, mut x0) = tri[0].parts();
                let (mut v1, mut c1, mut u1, mut x1) = tri[1].parts();
                let (v2, c2, u2, x2) = tri[2].parts();
                let mut inv = [1.0 / v0.z, 1.0 / v1.z, 1.0 / v2.z];
                let mut pts = [
                    project(v0, inv[0]),
                    project(v1, inv[1]),
                    project(v2, inv[2]),
                ];
                let area = (pts[1].x - pts[0].x) * (pts[2].y - pts[0].y)
                    - (pts[2].x - pts[0].x) * (pts[1].y - pts[0].y);
                if area <= 0.0 {
                    if !s.params.double_sided || area == 0.0 {
                        stats.backfaced += 1;
                        continue;
                    }
                    core::mem::swap(&mut v0, &mut v1);
                    core::mem::swap(&mut c0, &mut c1);
                    core::mem::swap(&mut u0, &mut u1);
                    core::mem::swap(&mut x0, &mut x1);
                    pts.swap(0, 1);
                    inv.swap(0, 1);
                }
                let depth = v0.z.max(v1.z).max(v2.z);
                let zn = |inv: f32| 1.0 - NEAR * inv;
                stats.emitted += 1;
                emit.tri(
                    ScreenTri {
                        pts,
                        colors: [c0, c1, c2],
                        depth,
                        z: [zn(inv[0]), zn(inv[1]), zn(inv[2])],
                        uv: [u0, u1, u2],
                        w: [v0.z, v1.z, v2.z],
                    },
                    &[x0, x1, x2],
                );
            }
        }
    }
    stats.verts = verts;
    stats
}
