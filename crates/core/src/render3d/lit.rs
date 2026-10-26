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
use crate::water::{MAX_RIPPLES, RIPPLE_WIDTH, Ripple, Water, WaveSet};

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
    /// A water draw's constants (`None` for every other draw).
    water: Option<WaterSetup>,
}

/// The per-draw constants of a water surface ([`crate::water`]).
struct WaterSetup {
    waves: WaveSet,
    /// The ripples that reach the draw (at most [`MAX_RIPPLES`]).
    ripples: [Ripple; MAX_RIPPLES],
    n_ripples: usize,
    response: f32,
    /// The texture's scroll (added to every UV).
    uv: Vec2,
    /// World eye, unit direction toward the light, the reflected sky and
    /// the light's color.
    eye: Vec3,
    sun: Vec3,
    sky: [f32; 4],
    sun_color: [f32; 4],
    /// The fog color (0..255).
    sky_fog: [f32; 3],
    gloss: u8,
}

impl WaterSetup {
    fn new(ctx: &TnlContext, mesh: &Mesh, model: &Mat34, w: &Water, gloss: u8) -> WaterSetup {
        // Ripples whose band can't reach the mesh's bounding circle (x, z)
        // are left out of every vertex.
        let reach = mesh.bounds().map(|(c, r)| {
            let s = model.axis_scales_sq();
            let c = model.transform_point(c);
            (c, crate::math3d::sqrt(r * r * s.x.max(s.y).max(s.z)))
        });
        Self::within(ctx, reach, w, gloss)
    }

    /// [`WaterSetup::new`] keeping the ripples that reach the circle
    /// `reach` (world center, radius), or every ripple (`None`).
    fn within(ctx: &TnlContext, reach: Option<(Vec3, f32)>, w: &Water, gloss: u8) -> WaterSetup {
        let state = ctx.water();
        let light = ctx.light();
        let f = |c: Color| [c.r as f32, c.g as f32, c.b as f32, c.a as f32];
        let mut ripples = [Ripple::NONE; MAX_RIPPLES];
        let mut n = 0;
        if w.ripple_responsiveness != 0.0 {
            for r in state.ripples() {
                if let Some((c, rad)) = reach {
                    let (dx, dz) = (r.pos.x - c.x, r.pos.y - c.z);
                    let lim = rad + r.radius() + RIPPLE_WIDTH;
                    if dx * dx + dz * dz >= lim * lim {
                        continue;
                    }
                }
                ripples[n] = *r;
                n += 1;
            }
        }
        WaterSetup {
            waves: WaveSet::new(w, state.time),
            ripples,
            n_ripples: n,
            response: w.ripple_responsiveness,
            uv: w.scroll(state.time).0,
            eye: ctx.camera().eye,
            sun: -ctx.ldir,
            sky: f(if light.fog.enabled() {
                light.fog.color
            } else {
                light.ambient_color
            }),
            sun_color: f(light.color),
            sky_fog: {
                let c = light.fog.color;
                [c.r as f32, c.g as f32, c.b as f32]
            },
            gloss,
        }
    }

    /// World point `p` moved by the swell and the ripples, and the
    /// surface's world normal there.
    #[inline]
    fn displace(&self, p: Vec3) -> (Vec3, Vec3) {
        let (h, g) = self.waves.sample(p.x, p.z);
        let (rh, rg) =
            crate::water::ripple_sample(&self.ripples[..self.n_ripples], self.response, p.x, p.z);
        (
            Vec3::new(p.x, p.y + h + rh, p.z),
            crate::water::normal_of(g + rg),
        )
    }

    /// A water vertex's color: lit by the global light from the displaced
    /// normal, then the Fresnel sky and the glint, then fog.
    #[inline]
    fn color(&self, s: &Setup, c: Color, n: Vec3, wp: Vec3, z: f32) -> Color {
        let light = s.light;
        let k = match s.params.shading {
            Shading::Unlit => [1.0, 1.0, 1.0],
            _ => {
                let diffuse = -n.dot(s.ldir);
                let d = (1.0 - light.ambient) * if diffuse > 0.0 { diffuse } else { 0.0 };
                [
                    light.ambient * s.ac[0] + d * s.lc[0],
                    light.ambient * s.ac[1] + d * s.lc[1],
                    light.ambient * s.ac[2] + d * s.lc[2],
                ]
            }
        };
        // Tint × light (+ emissive), in floats to the end.
        let t = s.tint_k;
        let e = s.params.emissive;
        let lit = [
            c.r as f32 * t[0] * k[0] + e.r as f32,
            c.g as f32 * t[1] * k[1] + e.g as f32,
            c.b as f32 * t[2] * k[2] + e.b as f32,
            c.a as f32 * t[3],
        ];
        let to_eye = (self.eye - wp).normalized();
        let mut out = crate::water::shade_f(
            lit,
            n,
            to_eye,
            self.sun,
            self.sky,
            self.sun_color,
            self.gloss,
        );
        if s.fog_on {
            let fog = light.fog;
            let f = ((z - fog.near) / (fog.far - fog.near)).clamp(0.0, 1.0);
            out[0] += (self.sky_fog[0] - out[0]) * f;
            out[1] += (self.sky_fog[1] - out[1]) * f;
            out[2] += (self.sky_fog[2] - out[2]) * f;
        }
        let b = crate::water::to_u8;
        Color::rgba(b(out[0]), b(out[1]), b(out[2]), b(out[3]))
    }
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

/// Does every triangle of a water draw lie outside one plane of the
/// frustum (its vertices free to move `margin` along world Y, the swell
/// and the ripples)? Then clipping leaves nothing of it: a river piece under
/// the camera whose sphere still touches the view. Out of line and only
/// for meshes of up to 256 vertices (the outcodes live on the stack).
#[inline(never)]
fn every_triangle_out(ctx: &TnlContext, mesh: &Mesh, mv: &Mat34, margin: f32) -> bool {
    const MAX: usize = 256;
    if mesh.vertex_count > MAX {
        return false;
    }
    let (lx, ly) = (ctx.lim_x, ctx.lim_y);
    // World Y in view space, and each plane's reach along it.
    let u = Vec3::new(ctx.view.m[0][1], ctx.view.m[1][1], ctx.view.m[2][1]);
    let abs = |v: f32| if v < 0.0 { -v } else { v };
    let reach = [
        abs(u.z) * margin,
        abs(u.x - lx * u.z) * margin,
        abs(-u.x - lx * u.z) * margin,
        abs(u.y - ly * u.z) * margin,
        abs(-u.y - ly * u.z) * margin,
    ];
    let mut codes = [0u8; MAX];
    for (i, code) in codes[..mesh.vertex_count].iter_mut().enumerate() {
        let v = mv.transform_point(mesh.position(i));
        let d = [
            NEAR - v.z,
            v.x - lx * v.z,
            -v.x - lx * v.z,
            v.y - ly * v.z,
            -v.y - ly * v.z,
        ];
        let mut c = (d[0] - reach[0] >= 0.0) as u8;
        for k in 1..5 {
            c |= ((d[k] - reach[k] > 0.0) as u8) << k;
        }
        *code = c;
    }
    (0..mesh.index_count / 3).all(|t| {
        let c = |k: usize| codes[mesh.index(3 * t + k) as usize];
        c(0) & c(1) & c(2) != 0
    })
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
        fog_band: super::fog_split_band(ctx, mesh, params),
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
        water: None,
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

/// `tessellate_with` for a [`Water`] draw: every vertex displaced by the
/// swell and the context's ripples, shaded from the displaced normal with
/// the Fresnel sky and the glint, its UVs scrolled by the flow (the
/// per-vertex tiers; the PC's per-pixel tier takes [`tessellate_surface`],
/// which does the same and hands over the [`SurfaceVert`]s).
#[inline(never)]
pub(super) fn tessellate_water(
    ctx: &TnlContext,
    scratch: &mut TnlScratch,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
    water: &Water,
    emit: &mut dyn FnMut(ScreenTri),
) -> TnlStats {
    let mv = ctx.view.mul(model);
    let bound = match cull(ctx, mesh, model, &mv) {
        Ok(b) => b,
        Err(culled) => return culled,
    };
    if every_triangle_out(ctx, mesh, &mv, super::rsp_water_margin(ctx, water)) {
        return TnlStats {
            culled: 1,
            ..TnlStats::default()
        };
    }
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
        water: Some(WaterSetup::new(ctx, mesh, model, water, params.gloss)),
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
    let mut stats = run(
        mesh,
        model,
        mv,
        params,
        &s,
        bufs,
        (&mut scratch.skin, &mut scratch.world, &mut scratch.viewm),
        emit,
    );
    if let Some(w) = &s.water {
        stats.add(&ripple_rings(ctx, &s.s, w, mesh, model, 0, emit));
    }
    stats
}

/// Segments of a ripple ring ([`tessellate_ripples`]).
pub const RING_SEGMENTS: usize = 10;

/// The per-vertex tiers' ripples: a water mesh's vertices are too far apart
/// for a 0.55 m ripple to show in them (the swell does), so each ripple
/// that reaches the draw also gets a ring on the surface: a band of
/// [`RING_SEGMENTS`] quads at its radius, light, its alpha fading with
/// the ripple. Emitted after the draw's triangles, sorted after them
/// (`depth` 0), never written to the depth buffer by any backend (they are
/// water). The per-pixel tier draws the rings in its shader instead.
/// `tessellate_with` adds them to every water draw; a backend that draws
/// the water surface another way (the 3DS's vertex program) calls this for
/// the rings alone.
pub fn tessellate_ripples(
    ctx: &TnlContext,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
    emit: &mut dyn FnMut(ScreenTri),
) -> TnlStats {
    tessellate_ripples_except(ctx, mesh, model, params, 0, emit)
}

/// [`tessellate_ripples`] without the rings whose bit is set in `skip`
/// (bit `i`: the `i`-th of [`water_rings`]): the N64 draws those on the
/// RSP ([`rsp_ring_mask`]).
pub fn tessellate_ripples_except(
    ctx: &TnlContext,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
    skip: u8,
    emit: &mut dyn FnMut(ScreenTri),
) -> TnlStats {
    let Some(w) = &params.water else {
        return TnlStats::default();
    };
    let s = setup(ctx, mesh, model, params);
    let ws = WaterSetup::new(ctx, mesh, model, w, params.gloss);
    ripple_rings(ctx, &s, &ws, mesh, model, skip, emit)
}

/// Vertices of a ripple ring: its inner and outer edge at each of
/// [`RING_SEGMENTS`] + 1 angles (the first and last coincide).
pub const RING_VERTS: usize = 2 * (RING_SEGMENTS + 1);

/// One ripple ring a water draw owns ([`tessellate_ripples`]): the ripple,
/// the band's radii, the surface's rest height and the lift above it the
/// ring rides at (before the swell), and its color (alpha fading with the
/// ripple).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Ring {
    pub ripple: Ripple,
    pub r0: f32,
    pub r1: f32,
    pub plane: f32,
    pub lift: f32,
    pub color: Color,
}

impl Ring {
    pub const NONE: Ring = Ring {
        ripple: Ripple::NONE,
        r0: 0.0,
        r1: 0.0,
        plane: 0.0,
        lift: 0.0,
        color: Color::rgba(0, 0, 0, 0),
    };
}

/// The rings of `ripples` (pushing with `response`) on `mesh` under
/// `model`: each ripple that falls on the surface and still shows. A
/// surface cut in pieces (the showcase's river) rings each ripple once:
/// from the piece it fell on.
fn rings_of(
    ripples: &[Ripple],
    response: f32,
    plane: f32,
    out: &mut [Ring; MAX_RIPPLES],
    owns: &mut dyn FnMut(&Ripple) -> bool,
) -> usize {
    if ripples.is_empty() {
        return 0;
    }
    let mut n = 0;
    for r in ripples {
        if !owns(r) {
            continue;
        }
        let decay = r.decay();
        let a = 210.0 * decay * (r.strength * response / 0.08).min(1.0);
        if a < 4.0 {
            continue;
        }
        let rad = r.radius();
        let half = 0.07 + 0.05 * r.age;
        out[n] = Ring {
            ripple: *r,
            r0: if rad > half { rad - half } else { 0.0 },
            r1: rad + half,
            plane,
            // Above the surface by the crest it rides on.
            lift: r.strength * response * decay + 0.01,
            color: Color::rgba(232, 244, 255, a as u8),
        };
        n += 1;
    }
    n
}

/// The rings a water draw owns, numbered as [`tessellate_ripples_except`]
/// numbers them.
pub fn water_rings(
    ws: &WaterRsp,
    mesh: &Mesh,
    model: &Mat34,
    out: &mut [Ring; MAX_RIPPLES],
) -> usize {
    rings_of(
        &ws.ripples[..ws.n_ripples],
        ws.response,
        water_plane(mesh, model),
        out,
        &mut |r| over_surface(mesh, model, r.pos),
    )
}

/// A water surface's rest height, where its rings lie: the middle of its
/// bounds under `model` (a flat sheet).
pub fn water_plane(mesh: &Mesh, model: &Mat34) -> f32 {
    match mesh.bounds() {
        Some((c, _)) => model.transform_point(c).y,
        None => model.transform_point(Vec3::ZERO).y,
    }
}

/// [`water_rings`] where `owns` says whether a ripple falls on the surface
/// (a backend that remembers [`ripple_over_surface`]'s answers), on the
/// surface's [`water_plane`] `plane`.
pub fn water_rings_with(
    ws: &WaterRsp,
    plane: f32,
    out: &mut [Ring; MAX_RIPPLES],
    owns: &mut dyn FnMut(&Ripple) -> bool,
) -> usize {
    rings_of(&ws.ripples[..ws.n_ripples], ws.response, plane, out, owns)
}

/// Does a ripple at world `p` (x, z) fall on `mesh` under `model` (seen
/// from above)? Its ring is then this surface's ([`water_rings`]).
pub fn ripple_over_surface(mesh: &Mesh, model: &Mat34, p: Vec2) -> bool {
    over_surface(mesh, model, p)
}

/// A ring's vertices in world space, riding the swell `waves`: vertex `2k`
/// on the inner edge and `2k + 1` on the outer one, at `k / RING_SEGMENTS`
/// turns. Segment `k`'s triangles are `(2k, 2k + 3, 2k + 1)` and
/// `(2k, 2k + 2, 2k + 3)`.
pub fn ring_world(waves: &WaveSet, ring: &Ring) -> [Vec3; RING_VERTS] {
    let r = &ring.ripple;
    core::array::from_fn(|i| {
        let t = (i / 2) as f32 / RING_SEGMENTS as f32;
        let rr = if i % 2 == 0 { ring.r0 } else { ring.r1 };
        let (sn, cs) = (crate::water::sin_turns(t), crate::water::cos_turns(t));
        let (x, z) = (r.pos.x + cs * rr, r.pos.y + sn * rr);
        let (h, _) = waves.sample(x, z);
        Vec3::new(x, ring.plane + h + ring.lift, z)
    })
}

/// A ring's vertices at rest (no swell), in [`ring_world`]'s order: what
/// the N64's water overlay displaces itself.
pub fn ring_flat(ring: &Ring) -> [Vec3; RING_VERTS] {
    let r = &ring.ripple;
    let y = ring.plane + ring.lift;
    core::array::from_fn(|i| {
        let t = (i / 2) as f32 / RING_SEGMENTS as f32;
        let rr = if i % 2 == 0 { ring.r0 } else { ring.r1 };
        let (sn, cs) = (crate::water::sin_turns(t), crate::water::cos_turns(t));
        Vec3::new(r.pos.x + cs * rr, y, r.pos.y + sn * rr)
    })
}

/// Can the N64's RSP draw a ring riding the swell `waves`? It never clips,
/// so the ring's sphere (its outer radius, and the swell's height above
/// and below) must lie past the near plane, inside the RSP band and short
/// of its far limit: then the CPU path would clip none of its triangles.
pub fn ring_on_rsp(ctx: &TnlContext, ring: &Ring, waves: &WaveSet) -> bool {
    let r = &ring.ripple;
    let swell = waves.amp[0].abs() + waves.amp[1].abs() + 0.01;
    let c = ctx
        .view
        .transform_point(Vec3::new(r.pos.x, ring.plane + ring.lift, r.pos.y));
    super::rsp_band_check(ctx, c, ring.r1 * ring.r1 + swell * swell).is_ok()
}

/// Which of a water draw's rings ([`water_rings`]) the N64 draws on the RSP
/// (bit `i`: ring `i`) when the surface itself is there, and how many
/// rings it has; the others stay CPU triangles
/// ([`tessellate_ripples_except`]).
pub fn rsp_ring_mask(
    ctx: &TnlContext,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
) -> (u8, usize) {
    let Some(ws) = water_rsp(ctx, mesh, model, params) else {
        return (0, 0);
    };
    let mut rings = [Ring::NONE; MAX_RIPPLES];
    let n = water_rings(&ws, mesh, model, &mut rings);
    let mut mask = 0;
    for (i, r) in rings[..n].iter().enumerate() {
        if ring_on_rsp(ctx, r, &ws.waves) {
            mask |= 1 << i;
        }
    }
    (mask, n)
}

/// What a water draw on the N64's RSP adds for its rings: the triangles of
/// the rings the RSP draws ([`rsp_ring_mask`]) and the CPU work of the
/// others. The ROM and every host predictor count rings this way.
pub fn rsp_water_rings(
    ctx: &TnlContext,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
) -> (u32, TnlStats) {
    let (mask, _) = rsp_ring_mask(ctx, mesh, model, params);
    let cpu = tessellate_ripples_except(ctx, mesh, model, params, mask, &mut |_| {});
    (mask.count_ones() * 2 * RING_SEGMENTS as u32, cpu)
}

fn ripple_rings(
    ctx: &TnlContext,
    s: &Setup,
    w: &WaterSetup,
    mesh: &Mesh,
    model: &Mat34,
    skip: u8,
    emit: &mut dyn FnMut(ScreenTri),
) -> TnlStats {
    let mut stats = TnlStats::default();
    let mut rings = [Ring::NONE; MAX_RIPPLES];
    let n = rings_of(
        &w.ripples[..w.n_ripples],
        w.response,
        water_plane(mesh, model),
        &mut rings,
        &mut |r| over_surface(mesh, model, r.pos),
    );
    let view = &ctx.view;
    let mut poly_a = [super::NO_VERT; MAX_POLY];
    let mut poly_b = [super::NO_VERT; MAX_POLY];
    for (i, ring) in rings[..n].iter().enumerate() {
        if skip & (1 << i) != 0 {
            continue;
        }
        let pts = ring_world(&w.waves, ring).map(|p| view.transform_point(p));
        for k in 0..RING_SEGMENTS {
            let (a, b, c, d) = (pts[2 * k], pts[2 * k + 1], pts[2 * k + 2], pts[2 * k + 3]);
            stats.tris_in += 2;
            for tri in [[a, d, b], [a, c, d]] {
                ring_tri(
                    s,
                    &tri,
                    ring.color,
                    &mut poly_a,
                    &mut poly_b,
                    &mut stats,
                    emit,
                );
            }
        }
        stats.verts += RING_VERTS as u32;
    }
    stats
}

/// Does world point `p` (x, z) lie over one of `mesh`'s triangles under
/// `model`, seen from above? The owner of a ripple's ring
/// ([`tessellate_ripples`]).
fn over_surface(mesh: &Mesh, model: &Mat34, p: Vec2) -> bool {
    let m = &model.m;
    // World x, z of a vertex as a 2 x 2 map of its own x, z (a model that
    // doesn't tilt the surface): the point goes into model space once,
    // instead of every vertex out of it (the N64 transforms slowly).
    let det = m[0][0] * m[2][2] - m[0][2] * m[2][0];
    let (p, flat) = if m[0][1] == 0.0 && m[2][1] == 0.0 && det != 0.0 {
        let (x, z) = (p.x - m[0][3], p.y - m[2][3]);
        let inv = 1.0 / det;
        (
            Vec2::new(
                (m[2][2] * x - m[0][2] * z) * inv,
                (m[0][0] * z - m[2][0] * x) * inv,
            ),
            true,
        )
    } else {
        (p, false)
    };
    (0..mesh.index_count / 3).any(|t| {
        let v: [Vec3; 3] = core::array::from_fn(|k| {
            let q = mesh.position(mesh.index(3 * t + k) as usize);
            if flat { q } else { model.transform_point(q) }
        });
        let side = |a: Vec3, b: Vec3| (b.x - a.x) * (p.y - a.z) - (b.z - a.z) * (p.x - a.x);
        let s = [side(v[0], v[1]), side(v[1], v[2]), side(v[2], v[0])];
        s.iter().all(|&x| x >= 0.0) || s.iter().all(|&x| x <= 0.0)
    })
}

/// Clip, project and emit one ring triangle (view space; both faces).
fn ring_tri(
    s: &Setup,
    tri: &[Vec3; 3],
    color: Color,
    poly_a: &mut [(Vec3, Color, Vec2); MAX_POLY],
    poly_b: &mut [(Vec3, Color, Vec2); MAX_POLY],
    stats: &mut TnlStats,
    emit: &mut dyn FnMut(ScreenTri),
) {
    let (lim_x, lim_y) = (s.lim_x, s.lim_y);
    if tri.iter().all(|v| v.z <= NEAR) {
        return;
    }
    let fogged = |v: Vec3| s.fog(color, v.z);
    for k in 0..3 {
        poly_a[k] = (tri[k], fogged(tri[k]), Vec2::new(0.5, 0.5));
    }
    let inside = |v: Vec3| {
        v.z > NEAR
            && v.x >= -lim_x * v.z
            && v.x <= lim_x * v.z
            && v.y >= -lim_y * v.z
            && v.y <= lim_y * v.z
    };
    let mut n = 3;
    if !tri.iter().all(|&v| inside(v)) {
        stats.clipped += 1;
        n = clip_frustum(poly_a, poly_b, lim_x, lim_y);
        if n < 3 {
            return;
        }
    }
    let project = |v: Vec3, inv: f32| {
        let k = s.focal * inv;
        Vec2::new(s.center.x + v.x * k, s.center.y - v.y * k)
    };
    for k in 1..n - 1 {
        let mut v = [poly_a[0], poly_a[k], poly_a[k + 1]];
        let mut inv = [1.0 / v[0].0.z, 1.0 / v[1].0.z, 1.0 / v[2].0.z];
        let mut pts = [
            project(v[0].0, inv[0]),
            project(v[1].0, inv[1]),
            project(v[2].0, inv[2]),
        ];
        let area = (pts[1].x - pts[0].x) * (pts[2].y - pts[0].y)
            - (pts[2].x - pts[0].x) * (pts[1].y - pts[0].y);
        if area == 0.0 {
            continue;
        }
        if area < 0.0 {
            // Seen from below or wound the other way: both faces show.
            v.swap(0, 1);
            inv.swap(0, 1);
            pts.swap(0, 1);
        }
        let zn = |inv: f32| 1.0 - NEAR * inv;
        stats.emitted += 1;
        emit(ScreenTri {
            pts,
            colors: [v[0].1, v[1].1, v[2].1],
            // Sorted after the surface's triangles (painter keys put the
            // nearest last).
            depth: 0.0,
            z: [zn(inv[0]), zn(inv[1]), zn(inv[2])],
            uv: [v[0].2, v[1].2, v[2].2],
            w: [v[0].0.z, v[1].0.z, v[2].0.z],
        });
    }
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
        water: params
            .water
            .as_ref()
            .map(|w| WaterSetup::new(ctx, mesh, model, w, params.gloss)),
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
        // Static water has its own copy: the water vertex and nothing of
        // the local lights or the faceting (a smaller body on the N64).
        if s.water.is_some() {
            return joints::<false, false, true, V, K>(mesh, &[*model], &[mv], s, bufs, emit);
        }
        return joints::<false, false, false, V, K>(mesh, &[*model], &[mv], s, bufs, emit);
    }
    let n = mesh.pose(params.anim, skin);
    for j in 0..n {
        world[j] = model.mul(&skin[j]);
        viewm[j] = s.view.mul(&world[j]);
    }
    let (world, viewm) = (&world[..n], &viewm[..n]);
    // Skinned water (rare) shares the lit copies' bodies.
    if mesh.has_blend() {
        joints::<true, true, false, V, K>(mesh, world, viewm, s, bufs, emit)
    } else {
        joints::<true, false, false, V, K>(mesh, world, viewm, s, bufs, emit)
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
            // From the inside vertex (see `render3d::clip_plane`): the
            // neighbor walking the edge the other way gets the same point.
            out[n_out] = if da >= 0.0 {
                let t = da / (da - db);
                V::new(
                    a + (b - a) * t,
                    lerp_color(ca, cb, t),
                    ua + (ub - ua) * t,
                    V::mix(xa, xb, t),
                )
            } else {
                let t = db / (db - da);
                V::new(
                    b + (a - b) * t,
                    lerp_color(cb, ca, t),
                    ub + (ua - ub) * t,
                    V::mix(xb, xa, t),
                )
            };
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
fn joints<
    const SKINNED: bool,
    const SMOOTH: bool,
    const WATER: bool,
    V: Vert,
    K: Sink<V::X> + ?Sized,
>(
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
    // Water is shaded from its displaced normals (never faceted).
    let flat = !WATER && s.params.shading == Shading::Flat && ls.water.is_none();
    let (lim_x, lim_y, focal, center) = (s.lim_x, s.lim_y, s.focal, s.center);
    let mut tag = [u32::MAX; VCACHE];
    let textured = mesh.has_uvs();
    let uv_scroll = ls.water.as_ref().map(|w| w.uv);
    // `WATER` copies always have a water setup, the static lit copy never,
    // a skinned copy may.
    let water = ls.water.as_ref();
    let mut vertex = |i: usize| -> (Vec3, Color, V::X) {
        let slot = i % VCACHE;
        if tag[slot] == i as u32 {
            return (cache_pos[slot], cache_col[slot], cache_x[slot]);
        }
        verts += 1;
        let j = joint(i);
        let local = mesh.position(i);
        // Static draws know at compile time: the lit copy has no water.
        if let Some(w) = water.filter(|_| WATER || SKINNED) {
            // Water: displaced in world space, then viewed.
            let (wp, n) = w.displace(world[j].transform_point(local));
            let p = ls.view.transform_point(wp);
            let col = w.color(s, s.base_color(mesh, i), n, wp, p.z);
            let x = V::make(|| SurfaceVert {
                pos: wp,
                normal: n,
                base: ls.albedo(mesh, i),
            });
            tag[slot] = i as u32;
            cache_pos[slot] = p;
            cache_col[slot] = col;
            cache_x[slot] = x;
            return (p, col, x);
        }
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
            let uv = match (textured, uv_scroll) {
                (true, None) => mesh.uv(idx[k]),
                (true, Some(o)) => mesh.uv(idx[k]) + o,
                (false, _) => Vec2::ZERO,
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
                let span = |a: usize, b: usize| super::split_span(z(a), z(b), s.fog_band);
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
                if worst.0 > DEPTH_SPLIT
                    && top + 2 <= SPLIT_STACK
                    && stats.splits < super::SPLIT_BUDGET
                {
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

/// What the N64's water overlay (`rsp_trino_water`) needs of a water draw:
/// the swell, the ripples that reach it, the texture's scroll and the
/// shading constants, the values [`tessellate_with`](super::tessellate_with)
/// evaluates per vertex.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WaterRsp {
    pub waves: WaveSet,
    /// The ripples that reach the draw, and how strongly they push it.
    pub ripples: [Ripple; MAX_RIPPLES],
    pub n_ripples: usize,
    pub response: f32,
    /// The texture's scroll, added to every UV.
    pub uv: Vec2,
    /// World eye, unit direction toward the light, the reflected sky and
    /// the light's color (0..255), the material's shininess.
    pub eye: Vec3,
    pub sun: Vec3,
    pub sky: [f32; 4],
    pub sun_color: [f32; 4],
    pub gloss: u8,
}

/// [`WaterRsp`] of a water draw (`None` without `params.water`).
pub fn water_rsp(
    ctx: &TnlContext,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
) -> Option<WaterRsp> {
    let w = params.water.as_ref()?;
    let s = WaterSetup::new(ctx, mesh, model, w, params.gloss);
    Some(WaterRsp {
        waves: s.waves,
        ripples: s.ripples,
        n_ripples: s.n_ripples,
        response: s.response,
        uv: s.uv,
        eye: s.eye,
        sun: s.sun,
        sky: s.sky,
        sun_color: s.sun_color,
        gloss: s.gloss,
    })
}

/// [`water_rsp`] with every ripple of the frame (`Caps::max_ripples` of
/// them), not only those that reach one mesh: the same for every draw of a
/// water material in a frame, so a backend computes it once. Ripples that
/// can't reach a vertex add nothing to it either way.
pub fn water_rsp_frame(ctx: &TnlContext, params: &ModelParams) -> Option<WaterRsp> {
    let w = params.water.as_ref()?;
    let s = WaterSetup::within(ctx, None, w, params.gloss);
    Some(WaterRsp {
        waves: s.waves,
        ripples: s.ripples,
        n_ripples: s.n_ripples,
        response: s.response,
        uv: s.uv,
        eye: s.eye,
        sun: s.sun,
        sky: s.sky,
        sun_color: s.sun_color,
        gloss: s.gloss,
    })
}

/// Vertex `i` of a water draw as the CPU path computes it: view-space
/// position (displaced) and color (lit, Fresnel, glint, fog). The N64
/// overlay's oracle.
pub fn water_vertex(
    ctx: &TnlContext,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
    i: usize,
) -> Option<(Vec3, Color)> {
    let w = params.water.as_ref()?;
    let s = setup(ctx, mesh, model, params);
    let ws = WaterSetup::new(ctx, mesh, model, w, params.gloss);
    let (wp, n) = ws.displace(model.transform_point(mesh.position(i)));
    let p = ctx.view.transform_point(wp);
    Some((p, ws.color(&s, s.base_color(mesh, i), n, wp, p.z)))
}

/// Ripple ring decals a water draw gets ([`tessellate_ripples`]): each
/// ripple that reaches it, falls on it and still shows.
pub fn ripple_ring_count(
    ctx: &TnlContext,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
) -> u32 {
    let Some(w) = params.water.as_ref() else {
        return 0;
    };
    let ws = WaterSetup::new(ctx, mesh, model, w, params.gloss);
    let mut rings = [Ring::NONE; MAX_RIPPLES];
    rings_of(
        &ws.ripples[..ws.n_ripples],
        ws.response,
        water_plane(mesh, model),
        &mut rings,
        &mut |r| over_surface(mesh, model, r.pos),
    ) as u32
}
