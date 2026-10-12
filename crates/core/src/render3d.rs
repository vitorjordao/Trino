//! Software transform & lighting: the engine's whole 3D pipeline (Fase 7).
//!
//! Design (see `docs/adr/0003-software-tnl-3d.md`): the N64 is the ceiling,
//! and for the mesh budgets Trino targets, transforming vertices on the CPU
//! and handing the backends nothing but **screen-space colored triangles**
//! beats integrating three different hardware T&L stacks. Backends only
//! rasterize (`rdpq_triangle` / `C2D_DrawTriangle` / a wgpu vertex-color
//! pipeline); everything above that — model/view transform, perspective,
//! gouraud lighting, backface cull, painter's sort — lives here, in pure
//! deterministic f32, identical on every target.
//!
//! Limits (v1, on purpose): no textures on 3D geometry (vertex colors only).
//!
//! Surface materials ([`ModelParams`]: tint, emissive, flat/gouraud/unlit
//! shading, double-sided), the light (colored directional + ambient) and
//! distance fog are all evaluated here, per vertex — so every target gets
//! them for free. Animated models carry a **rigid skeleton** in their TMDL
//! blob (each vertex follows exactly one joint — the N64-era limb model):
//! [`Mesh::pose`] samples a baked clip into per-joint matrices on the stack.
//!
//! Triangles are clipped against the near plane and a guard-band frustum
//! (1.5x the screen), so geometry that crosses the near plane stays visible
//! and every projected coordinate stays bounded — important for the N64 RDP,
//! whose edge coefficients are fixed-point and overflow on huge offscreen
//! coordinates.
//!
//! Occlusion is **z-buffered**: every vertex carries a normalized depth
//! ([`ScreenTri::z`]) that the PC writes to a wgpu depth buffer, the N64 to
//! the RDP hardware z-buffer and the 3DS to citro2d's per-draw depth (one
//! value per triangle there). Interpenetrating geometry resolves per pixel
//! — something no painter's sort can do. On top of that:
//!
//! - Edges spanning more than [`DEPTH_SPLIT`] units of view depth are
//!   recursively bisected (per-edge rule — neighbors split shared edges at
//!   identical points, no T-junction cracks), which keeps the 3DS's
//!   per-triangle depth honest and the N64's 16-bit z precise.
//! - Backends still batch consecutive `draw_model` calls and sort the batch
//!   far-to-near by [`ScreenTri::depth`] (farthest vertex) — the tie-break
//!   for equal depths and the correct blending order.

use crate::math::{Color, Vec2, Vec3};
use crate::math3d::{Mat34, Quat};
use crate::render::{AnimRef, ClipId, ModelParams, Shading};

/// Mesh blob format ("TMDL", little-endian):
/// `u32 vertex_count, u32 index_count`, then `positions f32*3*v`,
/// `normals f32*3*v`, `colors u8*4*v`, `indices u16*i`.
///
/// Optional **sections** follow, each `tag[4] u32 len payload[len]`;
/// readers skip unknown tags (forward compatible, old blobs stay valid):
///
/// - `BNDS`: bounding sphere `center f32*3, radius f32` in model space,
///   covering every animation frame — lets [`tessellate`] skip whole meshes
///   outside the frustum.
/// - `SKEL`: rigid skeleton. `u32 joint_count`, `u8 joint_of_vertex * v`,
///   then per joint `i32 parent (-1 = root, parents precede children)`,
///   `inverse bind Mat34 (12 f32, row-major)`, `rest t f32*3 q f32*4 s f32*3`.
/// - `ANIM` (after `SKEL`): `u32 clip_count`, then per clip `u32 name hash`
///   ([`ClipId`]), `f32 fps`, `u32 frame_count`, `u32 flags (bit0 = loop)`,
///   then `frame_count * joint_count` samples of `t3 q4 s3` f32,
///   frame-major.
pub const TMDL_MAGIC: &[u8; 4] = b"TMDL";

/// Most joints a skeleton may have. Joint matrices live on the stack during
/// [`tessellate`] (no allocator on consoles); 32 rigid joints is above what
/// N64-era characters used.
pub const MAX_JOINTS: usize = 32;

const JOINT_BYTES: usize = 4 + 12 * 4 + 10 * 4;
const SAMPLE_BYTES: usize = 10 * 4;
const CLIP_HEADER: usize = 16;

/// A parsed (borrowed) mesh — zero-copy over the baked blob.
#[derive(Clone, Copy, Debug)]
pub struct Mesh<'a> {
    positions: &'a [u8],
    normals: &'a [u8],
    colors: &'a [u8],
    indices: &'a [u8],
    pub vertex_count: usize,
    pub index_count: usize,
    bounds: Option<(Vec3, f32)>,
    skeleton: Option<Skeleton<'a>>,
}

/// Rigid skeleton + clips (borrowed from the blob).
#[derive(Clone, Copy, Debug)]
struct Skeleton<'a> {
    joint_of_vertex: &'a [u8],
    joints: &'a [u8],
    joint_count: usize,
    clips: &'a [u8],
    clip_count: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MeshError {
    BadMagic,
    Truncated,
    /// A section is malformed (bad length, joint index out of range, ...).
    BadSection,
    /// More than [`MAX_JOINTS`] joints.
    TooManyJoints,
}

#[inline]
fn rd_u32(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
}

#[inline]
fn rd_f32(d: &[u8], at: usize) -> f32 {
    f32::from_bits(rd_u32(d, at))
}

#[inline]
fn rd_vec3(d: &[u8], at: usize) -> Vec3 {
    Vec3::new(rd_f32(d, at), rd_f32(d, at + 4), rd_f32(d, at + 8))
}

#[inline]
fn rd_quat(d: &[u8], at: usize) -> Quat {
    Quat::new(
        rd_f32(d, at),
        rd_f32(d, at + 4),
        rd_f32(d, at + 8),
        rd_f32(d, at + 12),
    )
}

/// `(t, q, s)` sample at byte offset `at`.
#[inline]
fn rd_trs(d: &[u8], at: usize) -> (Vec3, Quat, Vec3) {
    (rd_vec3(d, at), rd_quat(d, at + 12), rd_vec3(d, at + 28))
}

/// Where a clip time falls: clip byte offset, bracketing frames, blend.
#[derive(Clone, Copy)]
struct ClipSample {
    at: usize,
    f0: usize,
    f1: usize,
    k: f32,
}

/// Interpolate two local transforms (rotations by normalized lerp — the
/// shortest arc, see [`Quat::nlerp`]).
#[inline]
fn lerp_trs(a: (Vec3, Quat, Vec3), b: (Vec3, Quat, Vec3), k: f32) -> (Vec3, Quat, Vec3) {
    (
        a.0 + (b.0 - a.0) * k,
        a.1.nlerp(b.1, k),
        a.2 + (b.2 - a.2) * k,
    )
}

/// Metadata of one baked clip.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ClipInfo {
    pub id: ClipId,
    pub fps: f32,
    pub frame_count: usize,
    pub looping: bool,
}

impl ClipInfo {
    /// Seconds from the first to the last frame.
    pub fn duration(&self) -> f32 {
        if self.frame_count > 1 && self.fps > 0.0 {
            (self.frame_count - 1) as f32 / self.fps
        } else {
            0.0
        }
    }
}

impl<'a> Mesh<'a> {
    pub fn from_tmdl(bytes: &'a [u8]) -> Result<Self, MeshError> {
        if bytes.len() < 12 || &bytes[0..4] != TMDL_MAGIC {
            return Err(MeshError::BadMagic);
        }
        let v = rd_u32(bytes, 4) as usize;
        let i = rd_u32(bytes, 8) as usize;
        let pos_len = v * 12;
        let nrm_len = v * 12;
        let col_len = v * 4;
        let idx_len = i * 2;
        let end = 12 + pos_len + nrm_len + col_len + idx_len;
        if bytes.len() < end {
            return Err(MeshError::Truncated);
        }
        let pos_at = 12;
        let nrm_at = pos_at + pos_len;
        let col_at = nrm_at + nrm_len;
        let idx_at = col_at + col_len;
        let mut mesh = Mesh {
            positions: &bytes[pos_at..pos_at + pos_len],
            normals: &bytes[nrm_at..nrm_at + nrm_len],
            colors: &bytes[col_at..col_at + col_len],
            indices: &bytes[idx_at..idx_at + idx_len],
            vertex_count: v,
            index_count: i,
            bounds: None,
            skeleton: None,
        };

        // Optional sections.
        let mut at = end;
        while bytes.len() >= at + 8 {
            let tag = &bytes[at..at + 4];
            let len = rd_u32(bytes, at + 4) as usize;
            let body_at = at + 8;
            if bytes.len() < body_at + len {
                return Err(MeshError::BadSection);
            }
            let body = &bytes[body_at..body_at + len];
            match tag {
                b"BNDS" if len >= 16 => {
                    mesh.bounds = Some((rd_vec3(body, 0), rd_f32(body, 12)));
                }
                b"SKEL" => mesh.skeleton = Some(Self::parse_skeleton(body, v)?),
                b"ANIM" => {
                    let Some(skel) = mesh.skeleton.as_mut() else {
                        return Err(MeshError::BadSection);
                    };
                    if len < 4 {
                        return Err(MeshError::BadSection);
                    }
                    let count = rd_u32(body, 0) as usize;
                    // Validate every clip's extent once, so lookups can
                    // index without bounds surprises.
                    let mut c_at = 4;
                    for _ in 0..count {
                        if body.len() < c_at + CLIP_HEADER {
                            return Err(MeshError::BadSection);
                        }
                        let frames = rd_u32(body, c_at + 8) as usize;
                        let size = CLIP_HEADER + frames * skel.joint_count * SAMPLE_BYTES;
                        if frames == 0 || body.len() < c_at + size {
                            return Err(MeshError::BadSection);
                        }
                        c_at += size;
                    }
                    skel.clips = &body[4..];
                    skel.clip_count = count;
                }
                _ => {} // unknown section: skip
            }
            at = body_at + len;
        }
        Ok(mesh)
    }

    fn parse_skeleton(body: &'a [u8], v: usize) -> Result<Skeleton<'a>, MeshError> {
        if body.len() < 4 {
            return Err(MeshError::BadSection);
        }
        let j = rd_u32(body, 0) as usize;
        if j == 0 {
            return Err(MeshError::BadSection);
        }
        if j > MAX_JOINTS {
            return Err(MeshError::TooManyJoints);
        }
        let joints_at = 4 + v;
        if body.len() < joints_at + j * JOINT_BYTES {
            return Err(MeshError::BadSection);
        }
        let joint_of_vertex = &body[4..4 + v];
        if joint_of_vertex.iter().any(|&k| k as usize >= j) {
            return Err(MeshError::BadSection);
        }
        Ok(Skeleton {
            joint_of_vertex,
            joints: &body[joints_at..joints_at + j * JOINT_BYTES],
            joint_count: j,
            clips: &[],
            clip_count: 0,
        })
    }

    #[inline]
    pub fn position(&self, i: usize) -> Vec3 {
        rd_vec3(self.positions, i * 12)
    }

    #[inline]
    pub fn normal(&self, i: usize) -> Vec3 {
        rd_vec3(self.normals, i * 12)
    }

    #[inline]
    pub fn color(&self, i: usize) -> Color {
        let at = i * 4;
        Color::rgba(
            self.colors[at],
            self.colors[at + 1],
            self.colors[at + 2],
            self.colors[at + 3],
        )
    }

    #[inline]
    pub fn index(&self, i: usize) -> u16 {
        u16::from_le_bytes([self.indices[i * 2], self.indices[i * 2 + 1]])
    }

    /// Model-space bounding sphere `(center, radius)`, when baked.
    /// Bind-pose bounding box `(min, max)` in model space (`None` for an
    /// empty mesh). Walks every vertex — a tool/load-time query.
    pub fn aabb(&self) -> Option<(Vec3, Vec3)> {
        if self.vertex_count == 0 {
            return None;
        }
        let mut min = self.position(0);
        let mut max = min;
        for i in 1..self.vertex_count {
            let p = self.position(i);
            min = Vec3::new(min.x.min(p.x), min.y.min(p.y), min.z.min(p.z));
            max = Vec3::new(max.x.max(p.x), max.y.max(p.y), max.z.max(p.z));
        }
        Some((min, max))
    }

    pub fn bounds(&self) -> Option<(Vec3, f32)> {
        self.bounds
    }

    /// Joints of the rigid skeleton (0 = static mesh).
    pub fn joint_count(&self) -> usize {
        self.skeleton.map_or(0, |s| s.joint_count)
    }

    /// Number of baked animation clips.
    pub fn clip_count(&self) -> usize {
        self.skeleton.map_or(0, |s| s.clip_count)
    }

    /// Byte offset of clip `n` inside the clips area.
    fn clip_offset(skel: &Skeleton, n: usize) -> usize {
        let mut at = 0;
        for _ in 0..n {
            let frames = rd_u32(skel.clips, at + 8) as usize;
            at += CLIP_HEADER + frames * skel.joint_count * SAMPLE_BYTES;
        }
        at
    }

    /// Metadata of clip `n` (`n < clip_count()`).
    pub fn clip(&self, n: usize) -> Option<ClipInfo> {
        let skel = self.skeleton?;
        if n >= skel.clip_count {
            return None;
        }
        let at = Self::clip_offset(&skel, n);
        Some(ClipInfo {
            id: ClipId(rd_u32(skel.clips, at)),
            fps: rd_f32(skel.clips, at + 4),
            frame_count: rd_u32(skel.clips, at + 8) as usize,
            looping: rd_u32(skel.clips, at + 12) & 1 != 0,
        })
    }

    /// Find a clip by id.
    pub fn find_clip(&self, id: ClipId) -> Option<ClipInfo> {
        (0..self.clip_count())
            .filter_map(|n| self.clip(n))
            .find(|c| c.id == id)
    }

    /// Skinning matrices for `anim` (bind pose when `None` or the clip is
    /// unknown): `out[j]` maps a bind-pose model-space vertex of joint `j`
    /// to its posed model-space position. Returns the joint count (0 for a
    /// static mesh, leaving `out` untouched).
    pub fn pose(&self, anim: Option<AnimRef>, out: &mut [Mat34; MAX_JOINTS]) -> usize {
        let Some(skel) = self.skeleton else {
            return 0;
        };
        let j_count = skel.joint_count;

        // Locate the clip(s) and the two frames of each to interpolate.
        let main = anim.and_then(|a| Self::locate(&skel, a.clip, a.time));
        let blend = anim
            .and_then(|a| a.blend)
            .filter(|_| main.is_some())
            .and_then(|b| {
                let w = b.weight.clamp(0.0, 1.0);
                Self::locate(&skel, b.clip, b.time).map(|s| (s, w))
            });

        // World (model-space) matrix per joint; parents precede children.
        for j in 0..j_count {
            let jat = j * JOINT_BYTES;
            let (t, q, s) = match main {
                Some(m) => {
                    let a = Self::sample_joint(&skel, m, j);
                    match blend {
                        Some((b, w)) => lerp_trs(a, Self::sample_joint(&skel, b, j), w),
                        None => a,
                    }
                }
                None => rd_trs(skel.joints, jat + 4 + 48),
            };
            let local = Mat34::from_trs(t, q, s);
            let parent = rd_u32(skel.joints, jat) as i32;
            out[j] = if parent >= 0 && (parent as usize) < j {
                out[parent as usize].mul(&local)
            } else {
                local
            };
        }
        // Skin = world * inverse bind.
        for (j, m) in out.iter_mut().enumerate().take(j_count) {
            let jat = j * JOINT_BYTES + 4;
            let mut inv = Mat34::IDENTITY;
            for r in 0..3 {
                for c in 0..4 {
                    inv.m[r][c] = rd_f32(skel.joints, jat + (r * 4 + c) * 4);
                }
            }
            *m = m.mul(&inv);
        }
        j_count
    }

    /// Clip byte offset + the two frames around `time` and the blend factor
    /// between them (`None` if the clip is not in this mesh).
    fn locate(skel: &Skeleton, clip: ClipId, time: f32) -> Option<ClipSample> {
        let mut at = 0;
        for _ in 0..skel.clip_count {
            let frames = rd_u32(skel.clips, at + 8) as usize;
            if rd_u32(skel.clips, at) == clip.0 {
                let fps = rd_f32(skel.clips, at + 4);
                let looping = rd_u32(skel.clips, at + 12) & 1 != 0;
                let last = (frames - 1) as f32;
                let mut f = if fps > 0.0 { time * fps } else { 0.0 };
                if looping && last > 0.0 {
                    f %= last;
                    if f < 0.0 {
                        f += last;
                    }
                } else {
                    f = f.clamp(0.0, last);
                }
                let f0 = f as usize;
                let f1 = (f0 + 1).min(frames - 1);
                return Some(ClipSample {
                    at,
                    f0,
                    f1,
                    k: f - f0 as f32,
                });
            }
            at += CLIP_HEADER + frames * skel.joint_count * SAMPLE_BYTES;
        }
        None
    }

    /// Local `(t, q, s)` of joint `j` at a located clip time.
    fn sample_joint(skel: &Skeleton, s: ClipSample, j: usize) -> (Vec3, Quat, Vec3) {
        let base = s.at + CLIP_HEADER;
        let a = rd_trs(
            skel.clips,
            base + (s.f0 * skel.joint_count + j) * SAMPLE_BYTES,
        );
        let b = rd_trs(
            skel.clips,
            base + (s.f1 * skel.joint_count + j) * SAMPLE_BYTES,
        );
        lerp_trs(a, b, s.k)
    }

    /// Joint driving vertex `i` (0 for static meshes).
    #[inline]
    fn joint_of(&self, i: usize) -> usize {
        self.skeleton.map_or(0, |s| s.joint_of_vertex[i] as usize)
    }
}

/// Perspective camera. World space is Y-up, right-handed; the projection
/// flips into the engine's Y-down screen space.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Camera3 {
    pub eye: Vec3,
    pub target: Vec3,
    /// Vertical field of view, radians.
    pub fov_y: f32,
}

impl Default for Camera3 {
    fn default() -> Self {
        Camera3 {
            eye: Vec3::new(0.0, 0.0, -5.0),
            target: Vec3::ZERO,
            fov_y: 60.0 * core::f32::consts::PI / 180.0,
        }
    }
}

impl Camera3 {
    pub fn view(&self) -> Mat34 {
        Mat34::look_at(self.eye, self.target, Vec3::new(0.0, 1.0, 0.0))
    }
}

/// Distance fog blended into vertex colors (the N64 classic: hides the far
/// clip and sells depth). Disabled when `far <= near`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Fog {
    pub color: Color,
    /// View depth where fog starts / reaches full strength.
    pub near: f32,
    pub far: f32,
}

impl Fog {
    pub const NONE: Fog = Fog {
        color: Color::BLACK,
        near: 0.0,
        far: 0.0,
    };

    pub fn enabled(&self) -> bool {
        self.far > self.near
    }
}

/// Directional light + ambient floor + fog — the whole lighting model.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Light {
    /// Direction the light travels (world space); normalized by the caller
    /// or in [`tessellate`].
    pub dir: Vec3,
    /// 0..1 base intensity every face receives.
    pub ambient: f32,
    /// Color of the directional light.
    pub color: Color,
    /// Color of the ambient floor.
    pub ambient_color: Color,
    pub fog: Fog,
}

/// The engine's default key light: from above-left-behind the camera.
pub const DEFAULT_LIGHT: Light = Light {
    dir: Vec3::new(0.4, -0.8, 0.45),
    ambient: 0.35,
    color: Color::WHITE,
    ambient_color: Color::WHITE,
    fog: Fog::NONE,
};

/// One screen-space triangle ready for a backend rasterizer.
#[derive(Clone, Copy, Debug)]
pub struct ScreenTri {
    pub pts: [Vec2; 3],
    pub colors: [Color; 3],
    /// Painter sort key: view-space depth of the triangle's **farthest**
    /// vertex (larger = farther). Farthest-vertex keys make a surface that
    /// extends behind an object draw before the object standing on it —
    /// centroid keys painted floors over players and walls over doors.
    pub depth: f32,
    /// Per-vertex normalized depth for z-buffered rasterizers:
    /// `1 - NEAR/z` — 0.0 at the near plane rising toward 1.0 at infinity
    /// (hyperbolic: more precision close up, where it matters). PC writes it
    /// to the wgpu depth buffer, the N64 to the RDP z-buffer; the 3DS uses
    /// the per-triangle average (citro2d has one depth per draw). Painter
    /// order (the batch sort) stays as the tie-break for equal depths.
    pub z: [f32; 3],
}

const NEAR: f32 = 0.05;
/// Guard band: side planes sit at 1.5x the screen so clipped coordinates stay
/// bounded (RDP fixed-point safety) without visible clipping at the borders.
const GUARD: f32 = 1.5;
/// A triangle clipped by up to 5 planes gains at most one vertex per plane.
const MAX_POLY: usize = 8;
/// Edges spanning more than this much view depth are bisected before
/// projection: painter sorting is only as good as how depth-uniform each
/// triangle is. The rule looks at one edge at a time, so neighbors split
/// shared edges identically (no cracks).
pub const DEPTH_SPLIT: f32 = 3.0;
/// Subdivision work stack; overflow emits the triangle unsplit (graceful).
const SPLIT_STACK: usize = 48;

/// Linear blend of two already-lit vertex colors (gouraud interpolation for
/// clip-generated vertices).
fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color::rgba(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b), mix(a.a, b.a))
}

/// Clip a view-space polygon against `dist(v) >= 0` (Sutherland-Hodgman).
/// Returns the new vertex count.
fn clip_plane(
    verts: &[(Vec3, Color); MAX_POLY],
    n_in: usize,
    out: &mut [(Vec3, Color); MAX_POLY],
    dist: impl Fn(Vec3) -> f32,
) -> usize {
    let mut n_out = 0;
    for i in 0..n_in {
        let (a, ca) = verts[i];
        let (b, cb) = verts[(i + 1) % n_in];
        let (da, db) = (dist(a), dist(b));
        if da >= 0.0 {
            if n_out == MAX_POLY {
                return n_out;
            }
            out[n_out] = (a, ca);
            n_out += 1;
        }
        if (da >= 0.0) != (db >= 0.0) {
            if n_out == MAX_POLY {
                return n_out;
            }
            let t = da / (da - db);
            out[n_out] = (a + (b - a) * t, lerp_color(ca, cb, t));
            n_out += 1;
        }
    }
    n_out
}

/// Transform, light, clip, depth-subdivide, project and cull `mesh`,
/// handing each visible triangle to `emit`. Triangles are **not** sorted
/// here: backends batch the triangles of consecutive `draw_model` calls and
/// depth-sort the whole batch (see the module docs). The callback API exists
/// because clipping + subdivision have no static output bound — backends
/// push into a growable buffer.
///
/// `params` is the surface material: tint, emissive, [`Shading`],
/// double-sided and the animation pose for skinned meshes.
pub fn tessellate(
    mesh: &Mesh,
    model: &Mat34,
    camera: &Camera3,
    light: &Light,
    params: &ModelParams,
    screen: Vec2,
    emit: &mut dyn FnMut(ScreenTri),
) {
    let view = camera.view();
    let mv = view.mul(model);
    // fov -> focal length in pixels (vertical).
    let half_fov = camera.fov_y * 0.5;
    let focal = crate::math3d::cos(half_fov) / crate::math3d::sin(half_fov) * screen.y * 0.5;
    let center = screen * 0.5;
    // Guard-band frustum half-extents per unit of view depth.
    let lim_x = center.x * GUARD / focal;
    let lim_y = center.y * GUARD / focal;

    // Whole-mesh frustum cull from the baked bounding sphere: a mesh that
    // cannot produce a single visible triangle costs one transform instead
    // of one per vertex. Output is unchanged (only invisible meshes skip).
    if let Some((c, r)) = mesh.bounds() {
        // sqrt-free (the core's sqrt is software on consoles): compare
        // squared distances against the squared, scale-adjusted radius.
        let c = mv.transform_point(c);
        let s = model.axis_scales_sq();
        let r2 = r * r * s.x.max(s.y).max(s.z);
        // Plane `lim*z - v >= 0` (normal length sqrt(lim²+1)): outside when
        // the signed distance is below -r.
        let outside = |d: f32, lim: f32| d < 0.0 && d * d > r2 * (lim * lim + 1.0);
        // Entirely behind the near plane: c.z + r <= NEAR.
        let behind = c.z <= NEAR && (NEAR - c.z) * (NEAR - c.z) >= r2;
        if behind
            || outside(lim_x * c.z - c.x, lim_x)
            || outside(lim_x * c.z + c.x, lim_x)
            || outside(lim_y * c.z - c.y, lim_y)
            || outside(lim_y * c.z + c.y, lim_y)
        {
            return;
        }
    }

    let setup = Setup {
        focal,
        center,
        lim_x,
        lim_y,
        ldir: light.dir.normalized(),
        light,
        params,
        lc: [
            light.color.r as f32 / 255.0,
            light.color.g as f32 / 255.0,
            light.color.b as f32 / 255.0,
        ],
        ac: [
            light.ambient_color.r as f32 / 255.0,
            light.ambient_color.g as f32 / 255.0,
            light.ambient_color.b as f32 / 255.0,
        ],
        white: light.color == Color::WHITE && light.ambient_color == Color::WHITE,
        has_emissive: params.emissive.r | params.emissive.g | params.emissive.b != 0,
        fog_on: light.fog.enabled(),
    };

    if mesh.joint_count() == 0 {
        // Static mesh: one "joint" (the model matrix) and no joint arrays —
        // the common case stays as cheap as before skeletons existed.
        tessellate_joints::<false>(mesh, &[*model], &[mv], &setup, emit);
    } else {
        let mut skin = [Mat34::IDENTITY; MAX_JOINTS];
        let joints = mesh.pose(params.anim, &mut skin);
        let mut world = [Mat34::IDENTITY; MAX_JOINTS];
        let mut viewm = [Mat34::IDENTITY; MAX_JOINTS];
        for j in 0..joints {
            world[j] = model.mul(&skin[j]);
            viewm[j] = view.mul(&world[j]);
        }
        tessellate_joints::<true>(mesh, &world[..joints], &viewm[..joints], &setup, emit);
    }
}

/// Per-draw constants shared by the static and skinned paths.
struct Setup<'a> {
    focal: f32,
    center: Vec2,
    lim_x: f32,
    lim_y: f32,
    ldir: Vec3,
    light: &'a Light,
    params: &'a ModelParams,
    /// Light / ambient colors as 0..1 factors. White yields exactly 1.0,
    /// keeping the default path bit-identical to the single-intensity model.
    lc: [f32; 3],
    ac: [f32; 3],
    /// Both light colors white: the single-intensity fast path applies.
    white: bool,
    has_emissive: bool,
    fog_on: bool,
}

impl Setup<'_> {
    /// Final vertex color from a world-space normal and the view depth.
    #[inline(always)]
    fn shade(&self, c: Color, n: Vec3, z: f32) -> Color {
        let light = self.light;
        let tint = self.params.tint;
        let k = match self.params.shading {
            Shading::Unlit => [1.0, 1.0, 1.0],
            Shading::Gouraud | Shading::Flat => {
                let diffuse = -n.dot(self.ldir);
                let diffuse = if diffuse > 0.0 { diffuse } else { 0.0 };
                let d = (1.0 - light.ambient) * diffuse;
                if self.white {
                    // White light: one intensity (identical result, less work).
                    let i = light.ambient + d;
                    return self.finish(c, tint, [i, i, i], z);
                }
                [
                    light.ambient * self.ac[0] + d * self.lc[0],
                    light.ambient * self.ac[1] + d * self.lc[1],
                    light.ambient * self.ac[2] + d * self.lc[2],
                ]
            }
        };
        self.finish(c, tint, k, z)
    }

    /// Tint × light, then emissive and fog.
    #[inline(always)]
    fn finish(&self, c: Color, tint: Color, k: [f32; 3], z: f32) -> Color {
        let light = self.light;
        let mul = |v: u8, t: u8, k: f32| (v as f32 * (t as f32 / 255.0) * k) as u8;
        let mut out = Color::rgba(
            mul(c.r, tint.r, k[0]),
            mul(c.g, tint.g, k[1]),
            mul(c.b, tint.b, k[2]),
            (c.a as f32 * (tint.a as f32 / 255.0)) as u8,
        );
        let e = self.params.emissive;
        if self.has_emissive {
            out.r = out.r.saturating_add(e.r);
            out.g = out.g.saturating_add(e.g);
            out.b = out.b.saturating_add(e.b);
        }
        let fog = light.fog;
        if self.fog_on {
            let f = ((z - fog.near) / (fog.far - fog.near)).clamp(0.0, 1.0);
            let a = out.a;
            out = lerp_color(out, fog.color, f);
            out.a = a;
        }
        out
    }
}

/// Direct-mapped per-draw vertex cache: a vertex shared by several
/// triangles is transformed (and, unless flat-shaded, lit) once. Fixed size,
/// on the stack — no allocator on consoles. Results are bit-identical to
/// recomputing (same inputs, same operations).
const VCACHE: usize = 32;

struct VertexCache {
    tag: [u32; VCACHE],
    pos: [Vec3; VCACHE],
    col: [Color; VCACHE],
}

/// `SKINNED` is a const parameter so the static path compiles without any
/// per-vertex joint lookup.
fn tessellate_joints<const SKINNED: bool>(
    mesh: &Mesh,
    world: &[Mat34],
    viewm: &[Mat34],
    s: &Setup,
    emit: &mut dyn FnMut(ScreenTri),
) {
    let joint = |i: usize| if SKINNED { mesh.joint_of(i) } else { 0 };
    let flat = s.params.shading == Shading::Flat;
    let (lim_x, lim_y, focal, center) = (s.lim_x, s.lim_y, s.focal, s.center);
    let mut cache = VertexCache {
        tag: [u32::MAX; VCACHE],
        pos: [Vec3::ZERO; VCACHE],
        col: [Color::WHITE; VCACHE],
    };
    // (view-space position, shaded color — unused when flat).
    let mut vertex = |i: usize| -> (Vec3, Color) {
        let slot = i % VCACHE;
        if cache.tag[slot] == i as u32 {
            return (cache.pos[slot], cache.col[slot]);
        }
        let j = joint(i);
        let p = viewm[j].transform_point(mesh.position(i));
        let col = match s.params.shading {
            Shading::Flat => Color::WHITE,
            Shading::Unlit => s.shade(mesh.color(i), Vec3::ZERO, p.z),
            Shading::Gouraud => {
                let n = world[j].transform_dir(mesh.normal(i)).normalized();
                s.shade(mesh.color(i), n, p.z)
            }
        };
        cache.tag[slot] = i as u32;
        cache.pos[slot] = p;
        cache.col[slot] = col;
        (p, col)
    };

    let tri_count = mesh.index_count / 3;
    for t in 0..tri_count {
        let idx = [
            mesh.index(t * 3) as usize,
            mesh.index(t * 3 + 1) as usize,
            mesh.index(t * 3 + 2) as usize,
        ];
        let v = [vertex(idx[0]), vertex(idx[1]), vertex(idx[2])];
        let vs = [v[0].0, v[1].0, v[2].0];
        // Entirely behind the near plane: gone.
        if vs[0].z <= NEAR && vs[1].z <= NEAR && vs[2].z <= NEAR {
            continue;
        }
        let colors = if flat {
            // One light value per triangle, from the world-space face normal.
            let p: [Vec3; 3] = core::array::from_fn(|k| {
                world[joint(idx[k])].transform_point(mesh.position(idx[k]))
            });
            let n = (p[1] - p[0]).cross(p[2] - p[0]).normalized();
            [
                s.shade(mesh.color(idx[0]), n, vs[0].z),
                s.shade(mesh.color(idx[1]), n, vs[1].z),
                s.shade(mesh.color(idx[2]), n, vs[2].z),
            ]
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
        let mut poly_a = [(Vec3::ZERO, Color::WHITE); MAX_POLY];
        let mut poly_b = [(Vec3::ZERO, Color::WHITE); MAX_POLY];
        for k in 0..3 {
            poly_a[k] = (vs[k], colors[k]);
        }
        let mut n_poly = 3;
        if !(inside(vs[0]) && inside(vs[1]) && inside(vs[2])) {
            // Clip against near plane + the 4 guard-band side planes.
            n_poly = clip_plane(&poly_a, n_poly, &mut poly_b, |v| v.z - NEAR);
            n_poly = clip_plane(&poly_b, n_poly, &mut poly_a, |v| lim_x * v.z - v.x);
            n_poly = clip_plane(&poly_a, n_poly, &mut poly_b, |v| lim_x * v.z + v.x);
            n_poly = clip_plane(&poly_b, n_poly, &mut poly_a, |v| lim_y * v.z - v.y);
            n_poly = clip_plane(&poly_a, n_poly, &mut poly_b, |v| lim_y * v.z + v.y);
            poly_a = poly_b;
            if n_poly < 3 {
                continue;
            }
        }

        let project = |v: Vec3| {
            Vec2::new(
                center.x + v.x * focal / v.z,
                center.y - v.y * focal / v.z, // world Y-up -> screen Y-down
            )
        };
        // Fan-triangulate the clipped polygon, then depth-subdivide each
        // fan triangle (DFS with a fixed stack): any edge spanning more
        // than DEPTH_SPLIT of view depth is bisected, recursively, so every
        // emitted triangle is depth-uniform enough for painter sorting.
        for k in 1..n_poly - 1 {
            let mut stack = [[(Vec3::ZERO, Color::WHITE); 3]; SPLIT_STACK];
            stack[0] = [poly_a[0], poly_a[k], poly_a[k + 1]];
            let mut top = 1usize;
            while top > 0 {
                top -= 1;
                let tri = stack[top];
                // Worst depth-spanning edge (cyclic: 0-1, 1-2, 2-0).
                let span = |a: usize, b: usize| {
                    let d = tri[a].0.z - tri[b].0.z;
                    if d < 0.0 { -d } else { d }
                };
                let (s01, s12, s20) = (span(0, 1), span(1, 2), span(2, 0));
                let worst = if s01 >= s12 && s01 >= s20 {
                    (s01, 0)
                } else if s12 >= s20 {
                    (s12, 1)
                } else {
                    (s20, 2)
                };
                if worst.0 > DEPTH_SPLIT && top + 2 <= SPLIT_STACK {
                    // Rotate so the split edge is (a, b); bisect keeping the
                    // CCW winding: (a, m, c) + (m, b, c).
                    let (a, b, c) = match worst.1 {
                        0 => (tri[0], tri[1], tri[2]),
                        1 => (tri[1], tri[2], tri[0]),
                        _ => (tri[2], tri[0], tri[1]),
                    };
                    let mid = ((a.0 + b.0) * 0.5, lerp_color(a.1, b.1, 0.5));
                    stack[top] = [a, mid, c];
                    stack[top + 1] = [mid, b, c];
                    top += 2;
                    continue;
                }
                let [(mut v0, mut c0), (mut v1, mut c1), (v2, c2)] = tri;
                let mut pts = [project(v0), project(v1), project(v2)];
                // Backface cull: front faces are CCW in world space, which
                // lands as positive signed area in Y-down screen space.
                let area = (pts[1].x - pts[0].x) * (pts[2].y - pts[0].y)
                    - (pts[2].x - pts[0].x) * (pts[1].y - pts[0].y);
                if area <= 0.0 {
                    if !s.params.double_sided || area == 0.0 {
                        continue;
                    }
                    // Double-sided: emit the back face with flipped winding.
                    core::mem::swap(&mut v0, &mut v1);
                    core::mem::swap(&mut c0, &mut c1);
                    pts.swap(0, 1);
                }
                let depth = v0.z.max(v1.z).max(v2.z);
                let zn = |v: Vec3| 1.0 - NEAR / v.z;
                emit(ScreenTri {
                    pts,
                    colors: [c0, c1, c2],
                    depth,
                    z: [zn(v0), zn(v1), zn(v2)],
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unit cube TMDL built in memory (24 verts, 36 indices, per-face
    /// normals/colors) — mirrors what the asset pipeline bakes.
    pub fn cube_tmdl() -> std::vec::Vec<u8> {
        // (normal, color) per face; 4 verts per face.
        let faces: [(Vec3, [u8; 4]); 6] = [
            (Vec3::new(0.0, 0.0, -1.0), [230, 80, 80, 255]),
            (Vec3::new(0.0, 0.0, 1.0), [80, 230, 80, 255]),
            (Vec3::new(-1.0, 0.0, 0.0), [80, 80, 230, 255]),
            (Vec3::new(1.0, 0.0, 0.0), [230, 230, 80, 255]),
            (Vec3::new(0.0, -1.0, 0.0), [230, 80, 230, 255]),
            (Vec3::new(0.0, 1.0, 0.0), [80, 230, 230, 255]),
        ];
        let mut positions: std::vec::Vec<f32> = std::vec::Vec::new();
        let mut normals: std::vec::Vec<f32> = std::vec::Vec::new();
        let mut colors: std::vec::Vec<u8> = std::vec::Vec::new();
        let mut indices: std::vec::Vec<u16> = std::vec::Vec::new();
        for (f, (n, c)) in faces.iter().enumerate() {
            // Build the face quad from the normal's basis.
            let u = if n.y.abs() > 0.9 {
                Vec3::new(1.0, 0.0, 0.0)
            } else {
                Vec3::new(0.0, 1.0, 0.0).cross(*n).normalized()
            };
            let v = n.cross(u);
            let base = (f * 4) as u16;
            for (su, sv) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
                let p = *n * 0.5 + u * su + v * sv;
                positions.extend_from_slice(&[p.x, p.y, p.z]);
                normals.extend_from_slice(&[n.x, n.y, n.z]);
                colors.extend_from_slice(c);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        let mut blob = std::vec::Vec::new();
        blob.extend_from_slice(TMDL_MAGIC);
        blob.extend_from_slice(&(positions.len() as u32 / 3).to_le_bytes());
        blob.extend_from_slice(&(indices.len() as u32).to_le_bytes());
        for p in &positions {
            blob.extend_from_slice(&p.to_le_bytes());
        }
        for n in &normals {
            blob.extend_from_slice(&n.to_le_bytes());
        }
        blob.extend_from_slice(&colors);
        for i in &indices {
            blob.extend_from_slice(&i.to_le_bytes());
        }
        blob
    }

    #[test]
    fn tmdl_roundtrip() {
        let blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        assert_eq!(mesh.vertex_count, 24);
        assert_eq!(mesh.index_count, 36);
        assert_eq!(mesh.color(0), Color::rgba(230, 80, 80, 255));
        assert!((mesh.normal(4).z - 1.0).abs() < 1e-6);
    }

    #[test]
    fn tmdl_rejects_garbage() {
        assert!(matches!(Mesh::from_tmdl(b"nope"), Err(MeshError::BadMagic)));
        let mut blob = cube_tmdl();
        blob.truncate(40);
        assert!(matches!(Mesh::from_tmdl(&blob), Err(MeshError::Truncated)));
    }

    /// Roda o tessellate coletando os triângulos emitidos.
    fn collect(
        mesh: &Mesh,
        model: &Mat34,
        camera: &Camera3,
        tint: Color,
    ) -> std::vec::Vec<ScreenTri> {
        let mut v = std::vec::Vec::new();
        tessellate(
            mesh,
            model,
            camera,
            &DEFAULT_LIGHT,
            &ModelParams::tinted(tint),
            Vec2::new(320.0, 240.0),
            &mut |t| v.push(t),
        );
        v
    }

    fn collect_with(
        mesh: &Mesh,
        model: &Mat34,
        light: &Light,
        params: &ModelParams,
    ) -> std::vec::Vec<ScreenTri> {
        let mut v = std::vec::Vec::new();
        tessellate(
            mesh,
            model,
            &Camera3::default(),
            light,
            params,
            Vec2::new(320.0, 240.0),
            &mut |t| v.push(t),
        );
        v
    }

    fn section(blob: &mut std::vec::Vec<u8>, tag: &[u8; 4], body: &[u8]) {
        blob.extend_from_slice(tag);
        blob.extend_from_slice(&(body.len() as u32).to_le_bytes());
        blob.extend_from_slice(body);
    }

    fn f32s(out: &mut std::vec::Vec<u8>, vals: &[f32]) {
        for v in vals {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    /// Cube with a 2-joint skeleton: joint 0 (root) owns the bottom half of
    /// the vertices, joint 1 (child) the top half; one looping clip "lift"
    /// raising joint 1 by 2 units over one second.
    fn skinned_cube() -> std::vec::Vec<u8> {
        let mut blob = cube_tmdl();
        let mesh_blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&mesh_blob).unwrap();
        let mut skel = std::vec::Vec::new();
        skel.extend_from_slice(&2u32.to_le_bytes());
        for i in 0..mesh.vertex_count {
            skel.push(if mesh.position(i).y > 0.0 { 1 } else { 0 });
        }
        let ident = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let rest = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        for parent in [-1i32, 0] {
            skel.extend_from_slice(&parent.to_le_bytes());
            f32s(&mut skel, &ident);
            f32s(&mut skel, &rest);
        }
        section(&mut blob, b"SKEL", &skel);
        let mut anim = std::vec::Vec::new();
        anim.extend_from_slice(&2u32.to_le_bytes());
        anim.extend_from_slice(&ClipId::from_name("lift").0.to_le_bytes());
        f32s(&mut anim, &[2.0]); // fps
        anim.extend_from_slice(&3u32.to_le_bytes()); // frames
        anim.extend_from_slice(&1u32.to_le_bytes()); // loop
        for f in 0..3 {
            for j in 0..2 {
                let y = if j == 1 { f as f32 } else { 0.0 };
                f32s(&mut anim, &[0.0, y, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
            }
        }
        // "slide": one-shot, joint 1 moves along +X by 4 per frame.
        anim.extend_from_slice(&ClipId::from_name("slide").0.to_le_bytes());
        f32s(&mut anim, &[1.0]); // fps
        anim.extend_from_slice(&2u32.to_le_bytes()); // frames
        anim.extend_from_slice(&0u32.to_le_bytes()); // once
        for f in 0..2 {
            for j in 0..2 {
                let x = if j == 1 { 4.0 * f as f32 } else { 0.0 };
                f32s(&mut anim, &[x, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
            }
        }
        section(&mut blob, b"ANIM", &anim);
        blob
    }

    #[test]
    fn cube_in_front_produces_visible_tris() {
        let blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        let out = collect(&mesh, &Mat34::IDENTITY, &Camera3::default(), Color::WHITE);
        // A cube facing the camera: between 1 and 3 visible faces = 2..6
        // tris (the unit cube's edges never trip the depth subdivision).
        assert!((2..=6).contains(&out.len()), "visible tris: {}", out.len());
        for tri in &out {
            for p in tri.pts {
                assert!((0.0..320.0).contains(&p.x) && (0.0..240.0).contains(&p.y));
            }
        }
    }

    #[test]
    fn ground_crossing_the_near_plane_is_clipped_not_dropped() {
        // A big flat "ground" passing under the camera used to vanish
        // entirely (any vertex behind the near plane dropped the triangle).
        let blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        // 40x1x40 slab whose top face is just below the camera and extends
        // far behind it.
        let model = Mat34::from_rotation_scale_translation(
            Vec3::ZERO,
            Vec3::new(40.0, 1.0, 40.0),
            Vec3::new(0.0, -1.5, 0.0),
        );
        let camera = Camera3 {
            eye: Vec3::new(0.0, 0.0, -5.0),
            target: Vec3::ZERO,
            ..Default::default()
        };
        let out = collect(&mesh, &model, &camera, Color::WHITE);
        assert!(!out.is_empty(), "near-crossing ground vanished");
        // Depth subdivision: the receding floor must arrive as depth-thin
        // strips, not one giant triangle pair.
        assert!(out.len() > 6, "floor not subdivided: {} tris", out.len());
        // Guard band keeps every projected coordinate bounded (the N64 RDP
        // works in fixed point; huge offscreen coordinates overflow it).
        for tri in &out {
            for p in tri.pts {
                assert!(
                    p.x.abs() <= 320.0 * 2.0 && p.y.abs() <= 240.0 * 2.0,
                    "unbounded coord {p:?}"
                );
            }
        }
    }

    #[test]
    fn farther_mesh_sorts_behind_a_nearer_one() {
        // depth = farthest vertex: every triangle of a cube 6 units behind
        // another must carry a strictly larger sort key.
        let blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        let camera = Camera3::default();
        let near = collect(&mesh, &Mat34::IDENTITY, &camera, Color::WHITE);
        let far_model =
            Mat34::from_rotation_scale_translation(Vec3::ZERO, Vec3::ONE, Vec3::new(0.0, 0.0, 6.0));
        let far = collect(&mesh, &far_model, &camera, Color::WHITE);
        assert!(!near.is_empty() && !far.is_empty());
        let near_max = near.iter().map(|t| t.depth).fold(0.0f32, f32::max);
        let far_min = far.iter().map(|t| t.depth).fold(f32::MAX, f32::min);
        assert!(
            near_max < far_min,
            "near max {near_max} vs far min {far_min}"
        );
    }

    #[test]
    fn behind_camera_is_dropped() {
        let blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        let camera = Camera3 {
            eye: Vec3::new(0.0, 0.0, -5.0),
            target: Vec3::new(0.0, 0.0, -10.0),
            ..Default::default()
        };
        let out = collect(&mesh, &Mat34::IDENTITY, &camera, Color::WHITE);
        assert!(out.is_empty());
    }

    #[test]
    fn tint_multiplies_vertex_colors() {
        let blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        let camera = Camera3::default();
        let white = collect(&mesh, &Mat34::IDENTITY, &camera, Color::WHITE);
        let dark = collect(&mesh, &Mat34::IDENTITY, &camera, Color::rgb(128, 128, 128));
        assert_eq!(white.len(), dark.len());
        assert!(!white.is_empty());
        // Every emitted channel is (rounding aside) halved by the 50% tint.
        for (w, d) in white.iter().zip(dark.iter()) {
            for (cw, cd) in w.colors.iter().zip(d.colors.iter()) {
                assert!((cd.r as i32 - cw.r as i32 / 2).abs() <= 2);
                assert!((cd.g as i32 - cw.g as i32 / 2).abs() <= 2);
                assert!((cd.b as i32 - cw.b as i32 / 2).abs() <= 2);
                assert_eq!(cd.a, cw.a, "alpha tinted only by tint.a");
            }
        }
    }

    #[test]
    fn lighting_darkens_faces_away_from_the_light() {
        let blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        // Light straight down: the top face (+Y) is lit, the bottom is
        // ambient-only.
        let light = Light {
            dir: Vec3::new(0.0, -1.0, 0.0),
            ambient: 0.2,
            ..DEFAULT_LIGHT
        };
        let model = Mat34::IDENTITY;
        // Top face verts are indices 20..24 (face 5), bottom 16..20.
        let shade_of = |i: usize| {
            let n = model.transform_dir(mesh.normal(i)).normalized();
            let d = (-n.dot(light.dir.normalized())).max(0.0);
            light.ambient + (1.0 - light.ambient) * d
        };
        assert!(shade_of(20) > 0.9);
        assert!(shade_of(16) < 0.3);
    }

    #[test]
    fn unknown_sections_are_skipped_and_bad_ones_rejected() {
        let mut blob = cube_tmdl();
        section(&mut blob, b"ZZZZ", &[1, 2, 3]);
        assert!(Mesh::from_tmdl(&blob).is_ok());
        let mut bad = cube_tmdl();
        bad.extend_from_slice(b"BNDS");
        bad.extend_from_slice(&100u32.to_le_bytes()); // longer than the blob
        assert_eq!(Mesh::from_tmdl(&bad).unwrap_err(), MeshError::BadSection);
    }

    #[test]
    fn bounds_cull_skips_meshes_outside_the_frustum() {
        let mut blob = cube_tmdl();
        let mut b = std::vec::Vec::new();
        f32s(&mut b, &[0.0, 0.0, 0.0, 0.87]);
        section(&mut blob, b"BNDS", &b);
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        assert_eq!(mesh.bounds(), Some((Vec3::ZERO, 0.87)));
        let camera = Camera3::default();
        // Visible: identical output with and without bounds.
        let plain_blob = cube_tmdl();
        let plain = Mesh::from_tmdl(&plain_blob).unwrap();
        assert_eq!(
            collect(&mesh, &Mat34::IDENTITY, &camera, Color::WHITE).len(),
            collect(&plain, &Mat34::IDENTITY, &camera, Color::WHITE).len()
        );
        // Far to the side: culled before any per-triangle work.
        let side = Mat34::from_rotation_scale_translation(
            Vec3::ZERO,
            Vec3::ONE,
            Vec3::new(40.0, 0.0, 0.0),
        );
        assert!(collect(&mesh, &side, &camera, Color::WHITE).is_empty());
    }

    #[test]
    fn unlit_ignores_the_light_and_emissive_adds() {
        let mesh_blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&mesh_blob).unwrap();
        let dark = Light {
            ambient: 0.0,
            dir: Vec3::new(0.0, 0.0, -1.0), // travels toward the camera: lights nothing visible
            ..DEFAULT_LIGHT
        };
        let unlit = ModelParams {
            shading: Shading::Unlit,
            ..ModelParams::DEFAULT
        };
        for tri in collect_with(&mesh, &Mat34::IDENTITY, &dark, &unlit) {
            for c in tri.colors {
                assert!(c.r.max(c.g).max(c.b) >= 229, "unlit got darkened: {c:?}");
            }
        }
        let glow = ModelParams {
            emissive: Color::rgb(0, 0, 100),
            ..ModelParams::DEFAULT
        };
        let lit = collect_with(&mesh, &Mat34::IDENTITY, &dark, &ModelParams::DEFAULT);
        let glowing = collect_with(&mesh, &Mat34::IDENTITY, &dark, &glow);
        for (a, b) in lit.iter().zip(glowing.iter()) {
            assert_eq!(b.colors[0].b, a.colors[0].b.saturating_add(100));
        }
    }

    #[test]
    fn flat_shading_gives_one_color_per_triangle() {
        let mesh_blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&mesh_blob).unwrap();
        let flat = ModelParams {
            shading: Shading::Flat,
            ..ModelParams::DEFAULT
        };
        let model =
            Mat34::from_rotation_scale_translation(Vec3::new(0.4, 0.7, 0.0), Vec3::ONE, Vec3::ZERO);
        let out = collect_with(&mesh, &model, &DEFAULT_LIGHT, &flat);
        assert!(!out.is_empty());
        for tri in out {
            assert_eq!(tri.colors[0], tri.colors[1]);
            assert_eq!(tri.colors[1], tri.colors[2]);
        }
    }

    #[test]
    fn fog_pulls_far_geometry_toward_the_fog_color() {
        let mesh_blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&mesh_blob).unwrap();
        let foggy = Light {
            fog: Fog {
                color: Color::rgb(255, 0, 255),
                near: 1.0,
                far: 2.0, // the cube sits ~4.5 away: fully fogged
            },
            ..DEFAULT_LIGHT
        };
        for tri in collect_with(&mesh, &Mat34::IDENTITY, &foggy, &ModelParams::DEFAULT) {
            for c in tri.colors {
                assert_eq!((c.r, c.g, c.b), (255, 0, 255));
                assert_eq!(c.a, 255, "fog keeps alpha");
            }
        }
    }

    #[test]
    fn double_sided_draws_back_faces() {
        let mesh_blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&mesh_blob).unwrap();
        let one = collect_with(
            &mesh,
            &Mat34::IDENTITY,
            &DEFAULT_LIGHT,
            &ModelParams::DEFAULT,
        );
        let both = collect_with(
            &mesh,
            &Mat34::IDENTITY,
            &DEFAULT_LIGHT,
            &ModelParams {
                double_sided: true,
                ..ModelParams::DEFAULT
            },
        );
        assert!(both.len() > one.len(), "{} vs {}", both.len(), one.len());
        for tri in both {
            let p = tri.pts;
            let area =
                (p[1].x - p[0].x) * (p[2].y - p[0].y) - (p[2].x - p[0].x) * (p[1].y - p[0].y);
            assert!(area > 0.0, "flipped back faces must arrive front-facing");
        }
    }

    #[test]
    fn skeleton_parses_and_clips_are_listed() {
        let blob = skinned_cube();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        assert_eq!(mesh.joint_count(), 2);
        assert_eq!(mesh.clip_count(), 2);
        let clip = mesh.find_clip(ClipId::from_name("lift")).unwrap();
        assert!(clip.looping);
        assert_eq!(clip.frame_count, 3);
        assert!((clip.duration() - 1.0).abs() < 1e-6);
        assert!(mesh.find_clip(ClipId::from_name("nope")).is_none());
    }

    #[test]
    fn crossfade_interpolates_between_clips() {
        let blob = skinned_cube();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        let lift = ClipId::from_name("lift");
        let slide = ClipId::from_name("slide");
        let pose = |a: AnimRef| {
            let mut m = [Mat34::IDENTITY; MAX_JOINTS];
            mesh.pose(Some(a), &mut m);
            (m[1].m[0][3], m[1].m[1][3])
        };
        // lift @0.5 s = frame 1 (y = 1); slide @1 s = last frame (x = 4).
        let base = AnimRef::new(lift, 0.5);
        assert_eq!(pose(base), (0.0, 1.0));
        assert_eq!(pose(base.blend_to(slide, 1.0, 0.0)), (0.0, 1.0));
        assert_eq!(pose(base.blend_to(slide, 1.0, 1.0)), (4.0, 0.0));
        let (x, y) = pose(base.blend_to(slide, 1.0, 0.25));
        assert!((x - 1.0).abs() < 1e-5 && (y - 0.75).abs() < 1e-5, "{x} {y}");
        // Weights clamp; an unknown blend clip leaves the main clip alone.
        assert_eq!(pose(base.blend_to(slide, 1.0, 7.0)), (4.0, 0.0));
        assert_eq!(pose(base.blend_to(ClipId(1), 0.0, 0.5)), (0.0, 1.0));
    }

    #[test]
    fn pose_moves_only_the_joint_owned_vertices() {
        let blob = skinned_cube();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        let mut m = [Mat34::IDENTITY; MAX_JOINTS];
        // Bind pose: identity.
        assert_eq!(mesh.pose(None, &mut m), 2);
        assert_eq!(m[1], Mat34::IDENTITY);
        // t = 0.25 s at 2 fps = frame 0.5: joint 1 raised by 0.5.
        let anim = Some(AnimRef::new(ClipId::from_name("lift"), 0.25));
        mesh.pose(anim, &mut m);
        assert!((m[1].m[1][3] - 0.5).abs() < 1e-5);
        assert_eq!(m[0], Mat34::IDENTITY);
        // Looping wraps: t = 1.25 s == t = 0.25 s.
        let mut w = [Mat34::IDENTITY; MAX_JOINTS];
        mesh.pose(Some(AnimRef::new(ClipId::from_name("lift"), 1.25)), &mut w);
        assert!((w[1].m[1][3] - 0.5).abs() < 1e-4);
        // The posed draw differs from the bind-pose draw.
        let camera = Camera3::default();
        let bind = collect(&mesh, &Mat34::IDENTITY, &camera, Color::WHITE);
        let mut posed = std::vec::Vec::new();
        tessellate(
            &mesh,
            &Mat34::IDENTITY,
            &camera,
            &DEFAULT_LIGHT,
            &ModelParams {
                anim,
                ..ModelParams::DEFAULT
            },
            Vec2::new(320.0, 240.0),
            &mut |t| posed.push(t),
        );
        let top = |tris: &[ScreenTri]| {
            tris.iter()
                .flat_map(|t| t.pts)
                .map(|p| p.y)
                .fold(f32::MAX, f32::min)
        };
        assert!(
            top(&posed) < top(&bind) - 1.0,
            "joint 1 should rise on screen"
        );
    }

    #[test]
    fn too_many_joints_is_an_error() {
        let mut blob = cube_tmdl();
        let mut skel = std::vec::Vec::new();
        skel.extend_from_slice(&((MAX_JOINTS + 1) as u32).to_le_bytes());
        section(&mut blob, b"SKEL", &skel);
        assert_eq!(
            Mesh::from_tmdl(&blob).unwrap_err(),
            MeshError::TooManyJoints
        );
    }
}
