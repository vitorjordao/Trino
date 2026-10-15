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
//! Textures (v2): a mesh with a `TXUV` section carries one UV per vertex
//! and may name a default texture (`TXID`, a sprite handle); `ModelParams::
//! texture` overrides it per draw. **One texture per draw** — the N64 TMEM
//! holds one ≤ 4 KB texture at a time — sampled with wrap-around (tiling)
//! and modulated by the lit vertex color. UVs are interpolated through
//! clipping and depth subdivision; each [`ScreenTri`] carries per-vertex
//! `uv` and view depth `w`, so every backend rasterizes perspective-correct
//! (RDP with W on N64, clip-space w on PC/3DS).
//!
//! Surface materials ([`ModelParams`]: tint, emissive, flat/gouraud/unlit
//! shading, double-sided, a palette for the mesh's color slots), the light (colored directional + ambient) and
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
//!   identical points, no T-junction cracks) in fogged, textured and
//!   translucent draws ([`split_policy`]). Opaque draws don't need it: the
//!   per-vertex depth is affine in screen space, so the z-buffer gets the
//!   same depth from an unsplit triangle.
//! - Backends still batch consecutive `draw_model` calls and sort the batch
//!   far-to-near by [`ScreenTri::depth`] (farthest vertex) — the tie-break
//!   for equal depths and the correct blending order.

use crate::math::{Color, Vec2, Vec3};
use crate::math3d::{Mat34, Quat};
use crate::render::{AnimRef, ClipId, ModelParams, Shading, SpriteId};

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
/// - `TXUV`: texture coordinates, `f32 u, f32 v` per vertex (0..1 spans the
///   texture once; values outside repeat).
/// - `TXID`: `u32` sprite handle of the default texture.
/// - `SKW2` (after `SKEL`, opt-in smooth skinning): per vertex `u8 joint2,
///   u8 weight2` — the vertex blends from its `SKEL` joint toward `joint2`
///   by `weight2 / 255` (two-bone linear blend). Meshes without it stay
///   rigid (the N64-era look, and cheaper).
/// - `SLOT`: per vertex `u8` color slot, 0..=4. Slot 0 keeps the baked
///   color; slot `s` multiplies the vertex RGB by entry `s - 1` of the
///   draw's [`ModelParams::palette`] (before tint and lighting). Baked only
///   when some vertex has a non-zero slot (glTF attribute `_SLOT`).
pub const TMDL_MAGIC: &[u8; 4] = b"TMDL";

/// Highest color slot a vertex may name (`SLOT`): the palette has
/// [`MAX_SLOT`] entries ([`crate::Palette`]).
pub const MAX_SLOT: u8 = 4;

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
    // Per-vertex records as fixed-size arrays: a fetch is one bounds check
    // plus constant-index byte loads, not one check per byte (the hot T&L
    // loop reads three of them per vertex).
    positions: &'a [[u8; 12]],
    normals: &'a [[u8; 12]],
    colors: &'a [[u8; 4]],
    indices: &'a [[u8; 2]],
    pub vertex_count: usize,
    pub index_count: usize,
    bounds: Option<(Vec3, f32)>,
    skeleton: Option<Skeleton<'a>>,
    uvs: Option<&'a [[u8; 8]]>,
    texture: Option<SpriteId>,
    /// `SKW2`: (joint2, weight2) per vertex.
    blend: Option<&'a [u8]>,
    /// `SLOT`: color slot (0..=[`MAX_SLOT`]) per vertex.
    slots: Option<&'a [u8]>,
    /// Every vertex color has alpha 255 (checked once, at parse).
    opaque: bool,
    /// Every normal has unit length (checked once, at parse): the
    /// model-space lighting path relies on it.
    unit_normals: bool,
    /// Every normal lies on a coordinate axis (boxes): model-space lighting
    /// then holds under non-uniform scale too.
    axis_normals: bool,
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

/// Where each part of a validated TMDL blob lives (byte spans), so a
/// backend can rebuild its [`Mesh`] on every draw without parsing or
/// validating again ([`Mesh::layout`] at register, [`Mesh::from_layout`] per
/// draw). Only valid for the blob it was taken from.
#[derive(Clone, Copy, Debug)]
pub struct MeshLayout {
    positions: Span,
    normals: Span,
    colors: Span,
    indices: Span,
    vertex_count: usize,
    index_count: usize,
    bounds: Option<(Vec3, f32)>,
    skeleton: Option<SkeletonLayout>,
    uvs: Option<Span>,
    texture: Option<SpriteId>,
    blend: Option<Span>,
    slots: Option<Span>,
    opaque: bool,
    unit_normals: bool,
    axis_normals: bool,
}

#[derive(Clone, Copy, Debug)]
struct SkeletonLayout {
    joint_of_vertex: Span,
    joints: Span,
    joint_count: usize,
    clips: Span,
    clip_count: usize,
}

/// A byte range of the blob.
#[derive(Clone, Copy, Debug)]
struct Span {
    at: usize,
    len: usize,
}

impl Span {
    /// `part` must be a sub-slice of `blob` (or empty).
    fn of(blob: &[u8], part: &[u8]) -> Span {
        if part.is_empty() {
            return Span { at: 0, len: 0 };
        }
        Span {
            at: part.as_ptr() as usize - blob.as_ptr() as usize,
            len: part.len(),
        }
    }

    fn get(self, blob: &[u8]) -> &[u8] {
        &blob[self.at..self.at + self.len]
    }
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

#[inline(always)]
fn le_f32(b: [u8; 4]) -> f32 {
    f32::from_bits(u32::from_le_bytes(b))
}

/// A little-endian `f32 * 3` record (constant indices: no bounds checks).
#[inline(always)]
fn vec3_of(c: &[u8; 12]) -> Vec3 {
    Vec3::new(
        le_f32([c[0], c[1], c[2], c[3]]),
        le_f32([c[4], c[5], c[6], c[7]]),
        le_f32([c[8], c[9], c[10], c[11]]),
    )
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
            positions: bytes[pos_at..pos_at + pos_len].as_chunks().0,
            normals: bytes[nrm_at..nrm_at + nrm_len].as_chunks().0,
            colors: bytes[col_at..col_at + col_len].as_chunks().0,
            indices: bytes[idx_at..idx_at + idx_len].as_chunks().0,
            vertex_count: v,
            index_count: i,
            bounds: None,
            skeleton: None,
            uvs: None,
            texture: None,
            blend: None,
            slots: None,
            opaque: true,
            unit_normals: true,
            axis_normals: true,
        };
        mesh.opaque = mesh.colors.iter().all(|c| c[3] == 255);
        mesh.unit_normals = mesh.normals.iter().all(|n| {
            let n = vec3_of(n);
            (n.dot(n) - 1.0).abs() < 2e-3
        });
        mesh.axis_normals = mesh.normals.iter().all(|n| {
            let n = vec3_of(n);
            let zeros = [n.x, n.y, n.z].iter().filter(|c| c.abs() < 1e-6).count();
            zeros == 2
        });

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
                b"TXUV" => {
                    if len < v * 8 {
                        return Err(MeshError::BadSection);
                    }
                    mesh.uvs = Some(body[..v * 8].as_chunks().0);
                }
                b"TXID" if len >= 4 => mesh.texture = Some(SpriteId(rd_u32(body, 0))),
                b"SKW2" => {
                    let Some(skel) = mesh.skeleton else {
                        return Err(MeshError::BadSection);
                    };
                    if len < v * 2
                        || body[..v * 2]
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .any(|&[j, _]| j as usize >= skel.joint_count)
                    {
                        return Err(MeshError::BadSection);
                    }
                    mesh.blend = Some(&body[..v * 2]);
                }
                b"SLOT" => {
                    if len < v || body[..v].iter().any(|&s| s > MAX_SLOT) {
                        return Err(MeshError::BadSection);
                    }
                    mesh.slots = Some(&body[..v]);
                }
                _ => {} // unknown section: skip
            }
            at = body_at + len;
        }
        Ok(mesh)
    }

    /// This mesh's layout inside `blob`, the bytes it was parsed from.
    pub fn layout(&self, blob: &[u8]) -> MeshLayout {
        let span = |part: &[u8]| Span::of(blob, part);
        MeshLayout {
            positions: span(self.positions.as_flattened()),
            normals: span(self.normals.as_flattened()),
            colors: span(self.colors.as_flattened()),
            indices: span(self.indices.as_flattened()),
            vertex_count: self.vertex_count,
            index_count: self.index_count,
            bounds: self.bounds,
            skeleton: self.skeleton.map(|k| SkeletonLayout {
                joint_of_vertex: span(k.joint_of_vertex),
                joints: span(k.joints),
                joint_count: k.joint_count,
                clips: span(k.clips),
                clip_count: k.clip_count,
            }),
            uvs: self.uvs.map(|u| span(u.as_flattened())),
            texture: self.texture,
            blend: self.blend.map(span),
            slots: self.slots.map(span),
            opaque: self.opaque,
            unit_normals: self.unit_normals,
            axis_normals: self.axis_normals,
        }
    }

    /// The mesh [`Mesh::from_tmdl`] parsed from `blob`, rebuilt from its
    /// [`MeshLayout`] without validating again: a draw's mesh for the cost
    /// of a few slice bounds checks. `blob` must be the bytes the layout was
    /// taken from (a shorter blob panics, a different one draws garbage).
    pub fn from_layout(blob: &'a [u8], l: &MeshLayout) -> Mesh<'a> {
        Mesh {
            positions: l.positions.get(blob).as_chunks().0,
            normals: l.normals.get(blob).as_chunks().0,
            colors: l.colors.get(blob).as_chunks().0,
            indices: l.indices.get(blob).as_chunks().0,
            vertex_count: l.vertex_count,
            index_count: l.index_count,
            bounds: l.bounds,
            skeleton: l.skeleton.map(|k| Skeleton {
                joint_of_vertex: k.joint_of_vertex.get(blob),
                joints: k.joints.get(blob),
                joint_count: k.joint_count,
                clips: k.clips.get(blob),
                clip_count: k.clip_count,
            }),
            uvs: l.uvs.map(|u| u.get(blob).as_chunks().0),
            texture: l.texture,
            blend: l.blend.map(|b| b.get(blob)),
            slots: l.slots.map(|s| s.get(blob)),
            opaque: l.opaque,
            unit_normals: l.unit_normals,
            axis_normals: l.axis_normals,
        }
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
        vec3_of(&self.positions[i])
    }

    /// Texture coordinate of vertex `i` (zero when the mesh has no UVs).
    #[inline]
    pub fn uv(&self, i: usize) -> Vec2 {
        match self.uvs {
            Some(uv) => {
                let c = &uv[i];
                Vec2::new(
                    le_f32([c[0], c[1], c[2], c[3]]),
                    le_f32([c[4], c[5], c[6], c[7]]),
                )
            }
            None => Vec2::ZERO,
        }
    }

    pub fn has_uvs(&self) -> bool {
        self.uvs.is_some()
    }

    /// The baked default texture (`TXID`).
    pub fn default_texture(&self) -> Option<SpriteId> {
        self.texture
    }

    /// Texture a draw of this mesh samples: the per-draw override, else the
    /// baked default — `None` for meshes without UVs.
    pub fn texture_for(&self, params: &ModelParams) -> Option<SpriteId> {
        self.uvs?;
        params.texture.or(self.texture)
    }

    #[inline]
    pub fn normal(&self, i: usize) -> Vec3 {
        vec3_of(&self.normals[i])
    }

    #[inline]
    pub fn color(&self, i: usize) -> Color {
        let [r, g, b, a] = self.colors[i];
        Color::rgba(r, g, b, a)
    }

    /// Color slot of vertex `i` (`SLOT`, 0..=[`MAX_SLOT`]); 0 when the mesh
    /// has no slots.
    #[inline]
    pub fn slot(&self, i: usize) -> u8 {
        self.slots.map_or(0, |s| s[i])
    }

    /// Color slots present (`SLOT`): [`ModelParams::palette`] recolors
    /// this mesh.
    pub fn has_slots(&self) -> bool {
        self.slots.is_some()
    }

    #[inline]
    pub fn index(&self, i: usize) -> u16 {
        u16::from_le_bytes(self.indices[i])
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

    /// Smooth skinning data present (`SKW2`)?
    pub fn has_blend(&self) -> bool {
        self.blend.is_some()
    }

    /// Second joint and its weight (0..1) for vertex `i`, if blended.
    #[inline]
    pub fn blend_of(&self, i: usize) -> Option<(usize, f32)> {
        let b = self.blend?;
        let w = b[i * 2 + 1];
        (w != 0).then(|| (b[i * 2] as usize, w as f32 * (1.0 / 255.0)))
    }

    /// Joint driving vertex `i` (0 for static meshes).
    #[inline]
    fn joint_of(&self, i: usize) -> usize {
        self.skeleton.map_or(0, |s| s.joint_of_vertex[i] as usize)
    }

    #[cfg(test)]
    fn joint_of_first(&self, j: usize) -> Option<usize> {
        (0..self.vertex_count).find(|&i| self.joint_of(i) == j)
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
    /// Texture coordinates (zero for untextured draws).
    pub uv: [Vec2; 3],
    /// View-space depth per vertex (> 0): perspective-correct texturing
    /// (RDP W on N64, clip-space w on PC/3DS).
    pub w: [f32; 3],
}

/// Sort key of a batch triangle in painter order: farthest first
/// ([`ScreenTri::depth`] descending), ties in submission order (`index`).
/// Backends sort these 8-byte keys (plus a texture group in the high word
/// of a pair) instead of the 88-byte triangles: on the N64 moving the
/// triangles themselves cost more than the whole rest of the sort, and the
/// explicit tie-break makes the order identical on every target, whatever
/// the sort algorithm.
#[inline]
pub fn painter_key(depth: f32, index: usize) -> u64 {
    // View depths are > NEAR > 0, where IEEE-754 bit patterns order like
    // the values; inverting them makes "farther" sort first.
    (((!depth.to_bits()) as u64) << 32) | index as u64
}

/// Index back out of a [`painter_key`].
#[inline]
pub fn painter_index(key: u64) -> usize {
    (key & 0xFFFF_FFFF) as usize
}

const NEAR: f32 = 0.05;
/// The near plane's view depth (hardware T&L paths project like [`tessellate`]).
pub const NEAR_PLANE: f32 = NEAR;
/// Guard band: side planes sit at 1.5x the screen so clipped coordinates stay
/// bounded (RDP fixed-point safety) without visible clipping at the borders.
const GUARD: f32 = 1.5;
/// A triangle clipped by up to 5 planes gains at most one vertex per plane.
const MAX_POLY: usize = 8;
/// Edges spanning more than this much view depth are bisected before
/// projection, for the draws [`split_policy`] picks: painter sorting (the
/// blending order) is only as good as how depth-uniform each triangle is,
/// and per-vertex fog and perspective-free texture paths need the extra
/// vertices. The rule looks at one edge at a time, so neighbors split
/// shared edges identically (no cracks).
pub const DEPTH_SPLIT: f32 = 3.0;

/// The light direction in model space, for a draw whose world normals are
/// a rotation of its unit model normals. With orthogonal columns,
/// `M = R·S` (rotation, per-axis scale `S`, no shear), and `normalize(M·n)`
/// is `R·n` when the scale is uniform, or when every normal lies on an axis
/// (castle64's stretched blocks): then `(R·n)·l = n·(Rᵀ·l)`, and `Rᵀ·l` is
/// each column's unit vector dotted with `l`. One transform per draw instead
/// of a normal transform, a square root and a division per vertex. `None`
/// for shear, for non-uniform scale with free normals, or when the normals
/// aren't unit length.
fn model_space_light(mesh: &Mesh, model: &Mat34, ldir: Vec3) -> Option<Vec3> {
    if !mesh.unit_normals {
        return None;
    }
    let m = &model.m;
    let col = |c: usize| Vec3::new(m[0][c], m[1][c], m[2][c]);
    let (a, b, c) = (col(0), col(1), col(2));
    let (la, lb, lc) = (a.dot(a), b.dot(b), c.dot(c));
    let big = la.max(lb).max(lc);
    // The core's sin/cos are approximations (~1e-4), so its rotation
    // matrices are orthonormal only to ~1e-4; 3e-3 still keeps the lighting
    // error under a quarter of a color step.
    let tol = big * 3e-3;
    let near = |x: f32, y: f32| (x - y).abs() <= tol;
    let orthogonal = la.min(lb).min(lc) > 0.0
        && a.dot(b).abs() <= tol
        && a.dot(c).abs() <= tol
        && b.dot(c).abs() <= tol;
    let uniform = near(la, lb) && near(la, lc);
    if !orthogonal || !(uniform || mesh.axis_normals) {
        return None;
    }
    let unit = |v: Vec3, len2: f32| v.dot(ldir) / crate::math3d::sqrt(len2);
    Some(Vec3::new(unit(a, la), unit(b, lb), unit(c, lc)))
}

/// Vertices per RSP mesh segment: the N64 overlay transforms a segment into
/// DMEM, then draws its triangles (local u8 indices).
pub const RSP_SEG_VERTS: usize = 32;
/// Triangles per RSP mesh segment (the overlay's index space).
pub const RSP_SEG_TRIS: usize = 64;
/// Segments per RSP mesh (the overlay's segment table).
pub const RSP_MAX_SEGS: usize = 16;
/// How far beyond the screen an RSP draw may reach, as a multiple of the
/// half screen (the CPU path's guard band is 1.5). The overlay does not
/// clip: it only needs every vertex past NEAR and inside its fixed-point
/// ranges — screen coordinates are s13.2 (±8192 px: 4× a 320-wide screen
/// is ±640) and the RDP scissors the rest. A wider band keeps props at the
/// screen edges on the fast path instead of CPU-clipping them for pixels
/// nobody sees (~3–4 ms per prop on the N64).
pub const RSP_BAND: f32 = 4.0;
/// Farthest view depth (including the bounding radius) an RSP draw may
/// reach: the overlay's s15.16 clip coordinates go to ~RSP_BAND × the
/// depth (|x| < 32768).
pub const RSP_MAX_DEPTH: f32 = 6_000.0;
/// Largest model axis scale an RSP draw may use (its matrix entries are
/// s15.16 too).
pub const RSP_MAX_SCALE: f32 = 4_000.0;

/// What [`rsp_split`] hands each segment to: its vertices (global indices,
/// local order) and triangles (local indices).
pub type RspSegmentFn<'a> = dyn FnMut(&[u16], &[[u8; 3]]) + 'a;

/// Split a mesh's triangles, in source order, into the N64 overlay's
/// segments: at most [`RSP_SEG_VERTS`] distinct vertices and
/// [`RSP_SEG_TRIS`] triangles each. `emit` gets each segment's vertices
/// (global indices, in local order) and its triangles (local indices).
/// Returns the segment count. No allocation: the PC (work baseline) and the
/// N64 (the RSP copy) run the same split.
pub fn rsp_split(mesh: &Mesh, emit: &mut RspSegmentFn) -> usize {
    let mut verts = [0u16; RSP_SEG_VERTS];
    let mut tris = [[0u8; 3]; RSP_SEG_TRIS];
    let (mut nv, mut nt, mut segs) = (0usize, 0usize, 0usize);
    for t in mesh.indices.as_flattened().as_chunks::<6>().0 {
        let idx = [
            u16::from_le_bytes([t[0], t[1]]),
            u16::from_le_bytes([t[2], t[3]]),
            u16::from_le_bytes([t[4], t[5]]),
        ];
        let new = (0..3)
            .filter(|&k| !verts[..nv].contains(&idx[k]) && !idx[..k].contains(&idx[k]))
            .count();
        if nv + new > RSP_SEG_VERTS || nt == RSP_SEG_TRIS {
            emit(&verts[..nv], &tris[..nt]);
            segs += 1;
            (nv, nt) = (0, 0);
        }
        for k in 0..3 {
            let l = match verts[..nv].iter().position(|&v| v == idx[k]) {
                Some(l) => l,
                None => {
                    verts[nv] = idx[k];
                    nv += 1;
                    nv - 1
                }
            };
            tris[nt][k] = l as u8;
        }
        nt += 1;
    }
    if nt > 0 {
        emit(&verts[..nv], &tris[..nt]);
        segs += 1;
    }
    segs
}

/// Can the N64's RSP T&L path hold this mesh at all? Static (no skeleton,
/// no smooth skinning), untextured, opaque vertex colors, unit normals,
/// coordinates inside the overlay's s15.16 range, and at most
/// [`RSP_MAX_SEGS`] segments ([`rsp_split`]). Walks the mesh: call it once;
/// the N64 renderer bakes an RSP copy only for these.
pub fn rsp_mesh_supported(mesh: &Mesh) -> bool {
    mesh.joint_count() == 0
        && !mesh.has_uvs()
        && !mesh.has_blend()
        && mesh.opaque
        && mesh.unit_normals
        && mesh.vertex_count > 0
        && mesh.positions.iter().all(|p| {
            let p = vec3_of(p);
            p.x.abs() < 16_384.0 && p.y.abs() < 16_384.0 && p.z.abs() < 16_384.0
        })
        && (1..=RSP_MAX_SEGS).contains(&rsp_split(mesh, &mut |_, _| {}))
}

/// What the N64's RSP T&L path needs for one draw ([`rsp_eligible`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RspDraw {
    /// Unit vector toward the light in model space (zero for `Unlit`).
    pub light: Vec3,
    /// Ambient term (1.0 for `Unlit`).
    pub ambient: f32,
    /// Back-face culling (always on: double-sided draws stay on the CPU).
    pub cull_back: bool,
}

/// Whether the N64 backend may send this draw to its RSP overlay
/// (`crates/platform-n64/shim/rsp_trino.S`) instead of [`tessellate`]: the
/// overlay has no clipping, splitting, fog, colored light, emissive, flat
/// shading or textures, so the draw must need none of them. Its whole
/// bounding sphere must be inside the guard-band frustum and past the near
/// plane, so no triangle could have been clipped. A palette on a mesh with
/// color slots stays on the CPU too (the overlay draws baked colors). Only for meshes
/// [`rsp_mesh_supported`] accepted (checked once; this runs per draw). It
/// lives in the core so host tests can predict the console's work split
/// exactly (`perf_budget`).
pub fn rsp_eligible(
    ctx: &TnlContext,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
) -> Option<RspDraw> {
    rsp_eligible_mv(ctx, mesh, model, &ctx.view.mul(model), params)
}

/// [`rsp_eligible`] with the draw's `view · model` product already made
/// (`mv`): a backend that needs it for the draw itself computes it once
/// (on the N64 a 3×4 product is ~8 µs). Same arithmetic, same answers.
pub fn rsp_eligible_mv(
    ctx: &TnlContext,
    mesh: &Mesh,
    model: &Mat34,
    mv: &Mat34,
    params: &ModelParams,
) -> Option<RspDraw> {
    if !ctx.white
        || ctx.fog_on
        || params.tint.a < 255
        || params.emissive.r | params.emissive.g | params.emissive.b != 0
        || params.texture.is_some()
        // Back faces through the overlay were never verified: CPU path.
        || params.double_sided
        // The overlay reads baked vertex colors only.
        || (params.palette.is_some() && mesh.has_slots())
    {
        return None;
    }
    let (c, r) = mesh.bounds()?;
    let c = mv.transform_point(c);
    let s = model.axis_scales_sq();
    let r2 = r * r * s.x.max(s.y).max(s.z);
    let inside = |d: f32, k: f32| d > 0.0 && d * d > r2 * k;
    let abs = |v: f32| if v < 0.0 { -v } else { v };
    // Wholly outside the guard band: the CPU path culls it for ~13 µs —
    // never ship invisible work to the overlay.
    let outside = |d: f32, k: f32| d < 0.0 && d * d > r2 * k;
    let (gx, gy) = (ctx.lim_x, ctx.lim_y);
    if outside(gx * c.z - abs(c.x), gx * gx + 1.0) || outside(gy * c.z - abs(c.y), gy * gy + 1.0) {
        return None;
    }
    let band = RSP_BAND / GUARD;
    let (lx, ly) = (ctx.lim_x * band, ctx.lim_y * band);
    if !(inside(c.z - NEAR, 1.0)
        && inside(lx * c.z - abs(c.x), lx * lx + 1.0)
        && inside(ly * c.z - abs(c.y), ly * ly + 1.0))
    {
        return None;
    }
    // The overlay's clip coordinates are s15.16 (|v| < 32768): x reaches
    // ~RSP_BAND × the view depth inside the band. Keep the whole sphere
    // under RSP_MAX_DEPTH, and the matrix entries (axis scale) in range.
    if !inside(RSP_MAX_DEPTH - c.z, 1.0) || s.x.max(s.y).max(s.z) >= RSP_MAX_SCALE * RSP_MAX_SCALE {
        return None;
    }
    let cull_back = true;
    match params.shading {
        Shading::Unlit => Some(RspDraw {
            light: Vec3::ZERO,
            ambient: 1.0,
            cull_back,
        }),
        Shading::Gouraud => Some(RspDraw {
            light: model_space_light(mesh, model, ctx.ldir)? * -1.0,
            ambient: ctx.light.ambient,
            cull_back,
        }),
        Shading::Flat => None,
    }
}

impl TnlContext {
    /// View matrix, focal length (pixels) and screen size: what a hardware
    /// T&L path needs to build its own projection.
    pub fn projection(&self) -> (Mat34, f32, Vec2) {
        (self.view, self.focal, self.screen)
    }
}

/// Whether a draw gets [`DEPTH_SPLIT`] bisection. Every target z-buffers
/// with per-vertex depth `1 - NEAR/z`, which is affine in screen space, so
/// an unsplit opaque triangle gets the same per-pixel depth: splitting it
/// buys nothing but triangles (castle64's lava sea: 12 → 198). Splits stay
/// where the vertices carry something that is not affine on screen or
/// where draw order matters:
/// - fog (per-vertex, by view depth);
/// - textured draws (conservative: their alpha may blend);
/// - translucent draws (tint or vertex alpha < 255: PC blends, and the
///   painter order is the blending order).
#[inline]
pub fn split_policy(ctx: &TnlContext, mesh: &Mesh, params: &ModelParams) -> bool {
    ctx.fog_on || mesh.has_uvs() || params.tint.a < 255 || !mesh.opaque
}
/// Subdivision work stack; overflow emits the triangle unsplit (graceful).
const SPLIT_STACK: usize = 48;

/// Deterministic work counters of [`tessellate`] — what the T&L actually did
/// for one draw (or, summed, for a frame). Identical on every target (same
/// code, same f32), so a host test can pin a scene's console workload
/// without an emulator: the profiling side of the "N64 is the ceiling"
/// rule. Backends sum them into [`crate::FrameStats`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct TnlStats {
    /// Draws rejected whole by the bounding-sphere frustum test.
    pub culled: u32,
    /// Vertex transforms (+ lighting) computed — vertex-cache misses.
    pub verts: u32,
    /// Source triangles considered.
    pub tris_in: u32,
    /// Source triangles that needed near/guard-band clipping.
    pub clipped: u32,
    /// Bisections made by the depth-span rule ([`DEPTH_SPLIT`]).
    pub splits: u32,
    /// Triangles dropped as back faces (or degenerate): source triangles
    /// tested before any clipping/splitting, plus the rare split piece that
    /// rounds to zero area.
    pub backfaced: u32,
    /// Screen triangles handed to the backend.
    pub emitted: u32,
    /// Draws a backend sent whole to a hardware T&L path instead (the N64's
    /// RSP overlay, see [`rsp_eligible`]); their work is not in the fields
    /// above.
    pub rsp_draws: u32,
    /// Source triangles of those draws.
    pub rsp_tris: u32,
}

impl TnlStats {
    /// Field-wise sum (per-frame totals).
    pub fn add(&mut self, o: &TnlStats) {
        self.culled += o.culled;
        self.verts += o.verts;
        self.tris_in += o.tris_in;
        self.clipped += o.clipped;
        self.splits += o.splits;
        self.backfaced += o.backfaced;
        self.emitted += o.emitted;
        self.rsp_draws += o.rsp_draws;
        self.rsp_tris += o.rsp_tris;
    }
}

/// Linear blend of two already-lit vertex colors (gouraud interpolation for
/// clip-generated vertices).
fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color::rgba(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b), mix(a.a, b.a))
}

/// A view-space vertex on its way to the screen: position, lit color, UV.
type ClipVert = (Vec3, Color, Vec2);

const NO_VERT: ClipVert = (Vec3::ZERO, Color::WHITE, Vec2::ZERO);

/// Clip a view-space polygon against `dist(v) >= 0` (Sutherland-Hodgman).
/// Returns the new vertex count.
fn clip_plane(
    verts: &[ClipVert; MAX_POLY],
    n_in: usize,
    out: &mut [ClipVert; MAX_POLY],
    dist: impl Fn(Vec3) -> f32,
) -> usize {
    let mut n_out = 0;
    for i in 0..n_in {
        let (a, ca, ua) = verts[i];
        let (b, cb, ub) = verts[(i + 1) % n_in];
        let (da, db) = (dist(a), dist(b));
        if da >= 0.0 {
            if n_out == MAX_POLY {
                return n_out;
            }
            out[n_out] = (a, ca, ua);
            n_out += 1;
        }
        if (da >= 0.0) != (db >= 0.0) {
            if n_out == MAX_POLY {
                return n_out;
            }
            let t = da / (da - db);
            out[n_out] = (a + (b - a) * t, lerp_color(ca, cb, t), ua + (ub - ua) * t);
            n_out += 1;
        }
    }
    n_out
}

/// Everything [`tessellate`] derives from the camera, the light and the
/// screen, computed once instead of per draw: the view matrix (two
/// normalizations), the focal length (the core's software `sin`/`cos`), the
/// guard-band extents, the normalized light direction and the light color
/// factors. Backends keep one and rebuild it on `set_camera` / `set_light`
/// (and on a screen-size change); [`tessellate_with`] takes it by reference.
/// The values are the same expressions [`tessellate`] used to evaluate per
/// draw, so the output is bit-identical.
#[derive(Clone, Copy, Debug)]
pub struct TnlContext {
    camera: Camera3,
    light: Light,
    screen: Vec2,
    view: Mat34,
    focal: f32,
    center: Vec2,
    lim_x: f32,
    lim_y: f32,
    ldir: Vec3,
    /// Light / ambient colors as 0..1 factors. White yields exactly 1.0,
    /// keeping the default path bit-identical to the single-intensity model.
    lc: [f32; 3],
    ac: [f32; 3],
    /// Both light colors white: the single-intensity fast path applies.
    white: bool,
    fog_on: bool,
}

impl TnlContext {
    pub fn new(camera: &Camera3, light: &Light, screen: Vec2) -> TnlContext {
        // fov -> focal length in pixels (vertical).
        let half_fov = camera.fov_y * 0.5;
        let focal = crate::math3d::cos(half_fov) / crate::math3d::sin(half_fov) * screen.y * 0.5;
        let center = screen * 0.5;
        TnlContext {
            camera: *camera,
            light: *light,
            screen,
            view: camera.view(),
            focal,
            center,
            // Guard-band frustum half-extents per unit of view depth.
            lim_x: center.x * GUARD / focal,
            lim_y: center.y * GUARD / focal,
            ldir: light.dir.normalized(),
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
            fog_on: light.fog.enabled(),
        }
    }

    /// Still valid for this camera, light and screen? Backends that can't
    /// hook every change (a resize) check this before reusing a context.
    pub fn matches(&self, camera: &Camera3, light: &Light, screen: Vec2) -> bool {
        self.camera == *camera && self.light == *light && self.screen == screen
    }
}

/// Clip the triangle in `poly_a` against the near plane and the 4
/// guard-band side planes; the result is back in `poly_a` (`poly_b` is
/// scratch). Returns the vertex count. Out of line and cold: a few percent
/// of triangles need it, and inlined five times it was most of the T&L's
/// code size.
#[cold]
#[inline(never)]
fn clip_frustum(
    poly_a: &mut [ClipVert; MAX_POLY],
    poly_b: &mut [ClipVert; MAX_POLY],
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

/// Transform, light, clip, depth-subdivide, project and cull `mesh`,
/// handing each visible triangle to `emit`. Triangles are **not** sorted
/// here: backends batch the triangles of consecutive `draw_model` calls and
/// depth-sort the whole batch (see the module docs). The callback API exists
/// because clipping + subdivision have no static output bound — backends
/// push into a growable buffer.
///
/// `params` is the surface material: tint, emissive, [`Shading`],
/// double-sided and the animation pose for skinned meshes.
///
/// Returns what the draw cost in [`TnlStats`] (callers may ignore it).
/// Builds a [`TnlContext`] per call; backends drawing many meshes per frame
/// keep one and call [`tessellate_with`].
pub fn tessellate(
    mesh: &Mesh,
    model: &Mat34,
    camera: &Camera3,
    light: &Light,
    params: &ModelParams,
    screen: Vec2,
    emit: &mut dyn FnMut(ScreenTri),
) -> TnlStats {
    let ctx = TnlContext::new(camera, light, screen);
    let mut scratch = TnlScratch::new();
    tessellate_with(&ctx, &mut scratch, mesh, model, params, emit)
}

/// Working memory of one [`tessellate_with`] call — the vertex cache, the
/// clipping polygons, the depth-split stack and the skinned path's joint
/// matrices, ~9 KB — owned by the caller and reused across draws without
/// being cleared. Initializing it on every draw was most of a visible
/// draw's fixed cost on the N64: thousands of stores through an 8 KB
/// direct-mapped data cache before the first vertex. Every slot is written
/// before it is read (only the vertex-cache tags are reset per draw), so
/// reuse changes no output. Backends keep one (boxed: the N64 main stack is
/// 64 KB).
pub struct TnlScratch {
    tri: TriScratch,
    skin: [Mat34; MAX_JOINTS],
    world: [Mat34; MAX_JOINTS],
    viewm: [Mat34; MAX_JOINTS],
}

impl TnlScratch {
    pub const fn new() -> TnlScratch {
        TnlScratch {
            tri: TriScratch {
                cache_pos: [Vec3::ZERO; VCACHE],
                cache_col: [Color::WHITE; VCACHE],
                poly_a: [NO_VERT; MAX_POLY],
                poly_b: [NO_VERT; MAX_POLY],
                stack: [[NO_VERT; 3]; SPLIT_STACK],
            },
            skin: [Mat34::IDENTITY; MAX_JOINTS],
            world: [Mat34::IDENTITY; MAX_JOINTS],
            viewm: [Mat34::IDENTITY; MAX_JOINTS],
        }
    }
}

/// The per-triangle half of [`TnlScratch`] (vertex cache payload, clip
/// polygons, split stack).
struct TriScratch {
    cache_pos: [Vec3; VCACHE],
    cache_col: [Color; VCACHE],
    poly_a: [ClipVert; MAX_POLY],
    poly_b: [ClipVert; MAX_POLY],
    stack: [[ClipVert; 3]; SPLIT_STACK],
}

impl Default for TnlScratch {
    fn default() -> Self {
        Self::new()
    }
}

/// [`tessellate`] with the per-frame constants precomputed and the
/// caller's scratch memory.
pub fn tessellate_with(
    ctx: &TnlContext,
    scratch: &mut TnlScratch,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
    emit: &mut dyn FnMut(ScreenTri),
) -> TnlStats {
    let view = ctx.view;
    let mv = view.mul(model);
    let (lim_x, lim_y) = (ctx.lim_x, ctx.lim_y);

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
            return TnlStats {
                culled: 1,
                ..TnlStats::default()
            };
        }
    }

    let setup = Setup {
        focal: ctx.focal,
        center: ctx.center,
        lim_x,
        lim_y,
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
    };

    if mesh.joint_count() == 0 {
        // Static mesh: one "joint" (the model matrix) and no joint arrays —
        // the common case stays as cheap as before skeletons existed.
        tessellate_joints::<false, false>(mesh, &[*model], &[mv], &setup, &mut scratch.tri, emit)
    } else {
        // Only the first `joints` matrices are written (pose included) and
        // read.
        let joints = mesh.pose(params.anim, &mut scratch.skin);
        for j in 0..joints {
            scratch.world[j] = model.mul(&scratch.skin[j]);
            scratch.viewm[j] = view.mul(&scratch.world[j]);
        }
        let (world, viewm) = (&scratch.world[..joints], &scratch.viewm[..joints]);
        let tri = &mut scratch.tri;
        if mesh.has_blend() {
            tessellate_joints::<true, true>(mesh, world, viewm, &setup, tri, emit)
        } else {
            tessellate_joints::<true, false>(mesh, world, viewm, &setup, tri, emit)
        }
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
    /// `params.tint` as 0..1 factors: the same quotients `finish` took per
    /// vertex (4 divisions), once per draw.
    tint_k: [f32; 4],
    /// Apply the [`DEPTH_SPLIT`] rule (see [`split_policy`]).
    split: bool,
    /// The light direction in model space, when the static path may light
    /// model-space normals directly ([`model_space_light`]).
    local_ldir: Option<Vec3>,
    /// The mesh's slots and the draw's palette, indexed by slot (entry 0
    /// white: slot 0 multiplies by 255, which keeps the color exactly).
    /// `None` unless the draw has a palette and the mesh has slots: the
    /// default path reads `mesh.color` as before.
    palette: Option<(&'a [u8], [Color; 5])>,
}

/// `a * b / 255` per RGB channel, rounded (`(a*b + 127) / 255`, exact for
/// b = 255); alpha stays `c`'s. The palette multiply ([`crate::Palette`]).
#[inline(always)]
fn palette_mul(c: Color, p: Color) -> Color {
    let mul = |a: u8, b: u8| ((a as u32 * b as u32 + 127) / 255) as u8;
    Color::rgba(mul(c.r, p.r), mul(c.g, p.g), mul(c.b, p.b), c.a)
}

impl Setup<'_> {
    /// Vertex `i`'s color before tint and lighting: the baked color, times
    /// its palette entry when the draw recolors slots. One predictable
    /// branch per vertex when unused.
    #[inline(always)]
    fn base_color(&self, mesh: &Mesh, i: usize) -> Color {
        let c = mesh.color(i);
        match &self.palette {
            None => c,
            Some((slots, pal)) => palette_mul(c, pal[(slots[i] as usize).min(MAX_SLOT as usize)]),
        }
    }

    /// Final vertex color from a world-space normal and the view depth.
    #[inline(always)]
    fn shade(&self, c: Color, n: Vec3, z: f32) -> Color {
        self.shade_with(c, || -n.dot(self.ldir), z)
    }

    /// [`Setup::shade`] with the raw diffuse term (`-n · l`) supplied by
    /// the caller (the model-space lighting path), evaluated only when the
    /// shading mode is lit.
    #[inline(always)]
    fn shade_with(&self, c: Color, diffuse: impl FnOnce() -> f32, z: f32) -> Color {
        let light = self.light;
        let k = match self.params.shading {
            Shading::Unlit => [1.0, 1.0, 1.0],
            Shading::Gouraud | Shading::Flat => {
                let diffuse = diffuse();
                let diffuse = if diffuse > 0.0 { diffuse } else { 0.0 };
                let d = (1.0 - light.ambient) * diffuse;
                if self.white {
                    // White light: one intensity (identical result, less work).
                    let i = light.ambient + d;
                    return self.finish(c, [i, i, i], z);
                }
                [
                    light.ambient * self.ac[0] + d * self.lc[0],
                    light.ambient * self.ac[1] + d * self.lc[1],
                    light.ambient * self.ac[2] + d * self.lc[2],
                ]
            }
        };
        self.finish(c, k, z)
    }

    /// Tint × light, then emissive and fog.
    #[inline(always)]
    fn finish(&self, c: Color, k: [f32; 3], z: f32) -> Color {
        let light = self.light;
        let t = self.tint_k;
        // v * (tint / 255) * k, with the quotient from `Setup`: the same
        // operations in the same order as dividing here, so bit-identical.
        let mul = |v: u8, t: f32, k: f32| (v as f32 * t * k) as u8;
        let mut out = Color::rgba(
            mul(c.r, t[0], k[0]),
            mul(c.g, t[1], k[1]),
            mul(c.b, t[2], k[2]),
            (c.a as f32 * t[3]) as u8,
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

/// `SKINNED` / `SMOOTH` are const parameters so the static and rigid paths
/// compile without any per-vertex joint or blend lookup. Out of line: each
/// path is its own function, so the static one (most draws) stays compact
/// in the N64's 16 KB instruction cache instead of being one 60 KB body
/// with the skinned paths.
#[inline(never)]
fn tessellate_joints<const SKINNED: bool, const SMOOTH: bool>(
    mesh: &Mesh,
    world: &[Mat34],
    viewm: &[Mat34],
    s: &Setup,
    scratch: &mut TriScratch,
    emit: &mut dyn FnMut(ScreenTri),
) -> TnlStats {
    let TriScratch {
        cache_pos,
        cache_col,
        poly_a,
        poly_b,
        stack,
    } = scratch;
    let mut stats = TnlStats::default();
    let mut verts = 0u32;
    let joint = |i: usize| if SKINNED { mesh.joint_of(i) } else { 0 };
    let flat = s.params.shading == Shading::Flat;
    let (lim_x, lim_y, focal, center) = (s.lim_x, s.lim_y, s.focal, s.center);
    // Only the tags start empty: the payload (`cache_pos` / `cache_col`,
    // in the caller's scratch) is written on every miss.
    let mut tag = [u32::MAX; VCACHE];
    let textured = mesh.has_uvs();
    // (view-space position, shaded color — unused when flat).
    let mut vertex = |i: usize| -> (Vec3, Color) {
        let slot = i % VCACHE;
        if tag[slot] == i as u32 {
            return (cache_pos[slot], cache_col[slot]);
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
        let col = match s.params.shading {
            Shading::Flat => Color::WHITE,
            Shading::Unlit => s.shade(s.base_color(mesh, i), Vec3::ZERO, p.z),
            Shading::Gouraud if !SKINNED && s.local_ldir.is_some() => {
                // Rotation + uniform scale: light the model-space normal
                // with the light turned into model space (Setup).
                let ll = s.local_ldir.unwrap_or(Vec3::ZERO);
                let nl = mesh.normal(i);
                s.shade_with(s.base_color(mesh, i), || -nl.dot(ll), p.z)
            }
            Shading::Gouraud => {
                let nl = mesh.normal(i);
                let mut n = world[j].transform_dir(nl);
                if let Some((j2, w)) = blend {
                    n = n + (world[j2].transform_dir(nl) - n) * w;
                }
                s.shade(s.base_color(mesh, i), n.normalized(), p.z)
            }
        };
        tag[slot] = i as u32;
        cache_pos[slot] = p;
        cache_col[slot] = col;
        (p, col)
    };

    // `poly_a` / `poly_b` / `stack` come from the caller's scratch and are
    // never cleared: initializing them per triangle (3.8 KB of stores, most
    // of the VR4300's 8 KB data cache) was once the N64's single largest
    // T&L cost, and per draw still most of a draw's fixed cost. Every slot
    // is written before it is read, so reuse changes no output.
    // Whole triangles of the index buffer: iterating them needs no bounds
    // checks at all.
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
        // Entirely behind the near plane: gone.
        if vs[0].z <= NEAR && vs[1].z <= NEAR && vs[2].z <= NEAR {
            continue;
        }
        // Back faces go before any clipping, depth splitting or flat
        // lighting. With the eye at the view-space origin, the projected
        // (Y-down) area of a triangle is -f² · p0·((p1-p0)×(p2-p0)) / (z0 z1
        // z2), and every clipped or split piece lies in the same plane, so
        // this sign is the one the per-piece screen test below finds — it
        // just used to find it after clipping and bisecting pieces that were
        // all thrown away (castle64's throne: 592 culled pieces from 276
        // source triangles).
        if !s.params.double_sided && (vs[1] - vs[0]).cross(vs[2] - vs[0]).dot(vs[0]) >= 0.0 {
            stats.backfaced += 1;
            continue;
        }
        let colors = if flat {
            // One light value per triangle, from the world-space face normal.
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
            [
                s.shade(s.base_color(mesh, idx[0]), n, vs[0].z),
                s.shade(s.base_color(mesh, idx[1]), n, vs[1].z),
                s.shade(s.base_color(mesh, idx[2]), n, vs[2].z),
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
        for k in 0..3 {
            let uv = if textured {
                mesh.uv(idx[k])
            } else {
                Vec2::ZERO
            };
            poly_a[k] = (vs[k], colors[k], uv);
        }
        let mut n_poly = 3;
        if !(inside(vs[0]) && inside(vs[1]) && inside(vs[2])) {
            stats.clipped += 1;
            n_poly = clip_frustum(poly_a, poly_b, lim_x, lim_y);
            if n_poly < 3 {
                continue;
            }
        }

        // One division per vertex: `inv` = 1 / view depth serves the
        // projection and the z-buffer depth (`1 - NEAR / z`).
        let project = |v: Vec3, inv: f32| {
            let k = focal * inv;
            Vec2::new(
                center.x + v.x * k,
                center.y - v.y * k, // world Y-up -> screen Y-down
            )
        };
        // Fan-triangulate the clipped polygon, then depth-subdivide each
        // fan triangle (DFS with a fixed stack): any edge spanning more
        // than DEPTH_SPLIT of view depth is bisected, recursively, so every
        // emitted triangle is depth-uniform enough for painter sorting.
        for k in 1..n_poly - 1 {
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
                    // Rotate so the split edge is (a, b); bisect keeping the
                    // CCW winding: (a, m, c) + (m, b, c).
                    let (a, b, c) = match worst.1 {
                        0 => (tri[0], tri[1], tri[2]),
                        1 => (tri[1], tri[2], tri[0]),
                        _ => (tri[2], tri[0], tri[1]),
                    };
                    let mid = (
                        (a.0 + b.0) * 0.5,
                        lerp_color(a.1, b.1, 0.5),
                        (a.2 + b.2) * 0.5,
                    );
                    stack[top] = [a, mid, c];
                    stack[top + 1] = [mid, b, c];
                    top += 2;
                    stats.splits += 1;
                    continue;
                }
                let [
                    (mut v0, mut c0, mut u0),
                    (mut v1, mut c1, mut u1),
                    (v2, c2, u2),
                ] = tri;
                let mut inv = [1.0 / v0.z, 1.0 / v1.z, 1.0 / v2.z];
                let mut pts = [
                    project(v0, inv[0]),
                    project(v1, inv[1]),
                    project(v2, inv[2]),
                ];
                // Backface cull: front faces are CCW in world space, which
                // lands as positive signed area in Y-down screen space.
                let area = (pts[1].x - pts[0].x) * (pts[2].y - pts[0].y)
                    - (pts[2].x - pts[0].x) * (pts[1].y - pts[0].y);
                if area <= 0.0 {
                    if !s.params.double_sided || area == 0.0 {
                        stats.backfaced += 1;
                        continue;
                    }
                    // Double-sided: emit the back face with flipped winding.
                    core::mem::swap(&mut v0, &mut v1);
                    core::mem::swap(&mut c0, &mut c1);
                    core::mem::swap(&mut u0, &mut u1);
                    pts.swap(0, 1);
                    inv.swap(0, 1);
                }
                let depth = v0.z.max(v1.z).max(v2.z);
                let zn = |inv: f32| 1.0 - NEAR * inv;
                stats.emitted += 1;
                emit(ScreenTri {
                    pts,
                    colors: [c0, c1, c2],
                    depth,
                    z: [zn(inv[0]), zn(inv[1]), zn(inv[2])],
                    uv: [u0, u1, u2],
                    w: [v0.z, v1.z, v2.z],
                });
            }
        }
    }
    stats.verts = verts;
    stats
}

/// Normalized `[u0, v0, u1, v1]` of a sprite region (pixels) inside a
/// `tex_w` x `tex_h` texture; the whole texture for `None`.
pub fn region_uv(region: Option<crate::math::Rect>, tex_w: f32, tex_h: f32) -> [f32; 4] {
    match region {
        Some(r) if tex_w > 0.0 && tex_h > 0.0 => [
            r.pos.x / tex_w,
            r.pos.y / tex_h,
            (r.pos.x + r.size.x) / tex_w,
            (r.pos.y + r.size.y) / tex_h,
        ],
        _ => [0.0, 0.0, 1.0, 1.0],
    }
}

/// Camera-facing quad ("billboard") as two screen triangles: a sprite in 3D
/// space — coins, smoke, sparks, distant trees. `params.size` is in world
/// units, `params.rotation` rolls the quad in screen space (radians), `uv`
/// is the sprite sub-rectangle `[u0, v0, u1, v1]` (normalized; flipbook
/// frames — backends derive it from `params.region`). Colors are the tint
/// with the light's distance fog (billboards are unlit). Returns `None`
/// when the quad is behind the near plane or entirely off screen.
pub fn billboard(
    camera: &Camera3,
    light: &Light,
    screen: Vec2,
    pos: Vec3,
    params: &crate::render::BillboardParams,
    uv: [f32; 4],
) -> Option<[ScreenTri; 2]> {
    billboard_with(&TnlContext::new(camera, light, screen), pos, params, uv)
}

/// [`billboard`] with the per-frame constants precomputed (the view matrix
/// and focal length cost two normalizations and a software sin/cos per
/// call otherwise). Same expressions, so the same triangles.
pub fn billboard_with(
    ctx: &TnlContext,
    pos: Vec3,
    params: &crate::render::BillboardParams,
    uv: [f32; 4],
) -> Option<[ScreenTri; 2]> {
    let (size, rotation, tint) = (params.size, params.rotation, params.tint);
    let (light, screen) = (&ctx.light, ctx.screen);
    let c = ctx.view.transform_point(pos);
    // Keep the whole quad comfortably in front of the near plane: its
    // corners share the center's depth (screen-aligned), so no clipping.
    if c.z <= NEAR * 2.0 {
        return None;
    }
    let (focal, center) = (ctx.focal, ctx.center);
    let (sn, cs) = if rotation.to_bits() == 0 {
        // sin/cos of +0.0 are exactly 0 and 1 (see math3d's tests).
        (0.0, 1.0)
    } else {
        (crate::math3d::sin(rotation), crate::math3d::cos(rotation))
    };
    let (hw, hh) = (size.x * 0.5, size.y * 0.5);
    // Corners in view space (x right, y up): TL, TR, BR, BL.
    let corner = |x: f32, y: f32| {
        let (rx, ry) = (x * cs - y * sn, x * sn + y * cs);
        Vec2::new(
            center.x + (c.x + rx) * focal / c.z,
            center.y - (c.y + ry) * focal / c.z,
        )
    };
    let pts = [
        corner(-hw, hh),
        corner(hw, hh),
        corner(hw, -hh),
        corner(-hw, -hh),
    ];
    let (min_x, max_x) = pts
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.x), b.max(p.x)));
    let (min_y, max_y) = pts
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.y), b.max(p.y)));
    if max_x < 0.0 || max_y < 0.0 || min_x > screen.x || min_y > screen.y {
        return None;
    }
    // Bounded coordinates (RDP fixed point): very close, huge quads skip.
    let limit = screen.x.max(screen.y) * 4.0;
    if min_x < -limit || min_y < -limit || max_x > limit || max_y > limit {
        return None;
    }
    let mut color = tint;
    let fog = light.fog;
    if fog.enabled() {
        let f = ((c.z - fog.near) / (fog.far - fog.near)).clamp(0.0, 1.0);
        color = lerp_color(tint, fog.color, f);
        color.a = tint.a;
    }
    let z = 1.0 - NEAR / c.z;
    let uvs = [
        Vec2::new(uv[0], uv[1]),
        Vec2::new(uv[2], uv[1]),
        Vec2::new(uv[2], uv[3]),
        Vec2::new(uv[0], uv[3]),
    ];
    let tri = |a: usize, b: usize, d: usize| ScreenTri {
        pts: [pts[a], pts[b], pts[d]],
        colors: [color; 3],
        depth: c.z,
        z: [z; 3],
        uv: [uvs[a], uvs[b], uvs[d]],
        w: [c.z; 3],
    };
    Some([tri(0, 1, 2), tri(0, 2, 3)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn painter_keys_sort_far_first_then_by_submission() {
        let depths = [2.0f32, 9.5, 0.06, 9.5, 300.0];
        let mut keys: std::vec::Vec<u64> = depths
            .iter()
            .enumerate()
            .map(|(i, d)| painter_key(*d, i))
            .collect();
        keys.sort_unstable();
        let order: std::vec::Vec<usize> = keys.into_iter().map(painter_index).collect();
        // 300 first; the two 9.5s keep submission order; nearest last.
        assert_eq!(order, [4, 1, 3, 0, 2]);
    }

    #[test]
    fn billboards_face_the_camera_and_cull() {
        let cam = Camera3::default(); // at z = -5 looking at the origin
        let screen = Vec2::new(320.0, 240.0);
        let [a, b] = billboard(
            &cam,
            &DEFAULT_LIGHT,
            screen,
            Vec3::ZERO,
            &crate::render::BillboardParams::DEFAULT,
            [0.0, 0.0, 1.0, 1.0],
        )
        .unwrap();
        // Centered on screen, front-facing (positive area), one depth.
        let area = |t: &ScreenTri| {
            let p = t.pts;
            (p[1].x - p[0].x) * (p[2].y - p[0].y) - (p[2].x - p[0].x) * (p[1].y - p[0].y)
        };
        assert!(area(&a) > 0.0 && area(&b) > 0.0);
        let cx = (a.pts[0].x + a.pts[1].x) * 0.5;
        assert!((cx - 160.0).abs() < 1e-3);
        assert_eq!(a.uv[0], Vec2::new(0.0, 0.0), "top-left corner samples v0");
        assert!(a.pts[0].y < a.pts[2].y, "v0 is the top of the screen quad");
        assert!((a.w[0] - 5.0).abs() < 1e-4 && a.z[0] == a.z[2]);
        // Behind the camera / off screen: nothing.
        let behind = billboard(
            &cam,
            &DEFAULT_LIGHT,
            screen,
            Vec3::new(0.0, 0.0, -10.0),
            &crate::render::BillboardParams::DEFAULT,
            [0.0, 0.0, 1.0, 1.0],
        );
        assert!(behind.is_none());
        let aside = billboard(
            &cam,
            &DEFAULT_LIGHT,
            screen,
            Vec3::new(50.0, 0.0, 0.0),
            &crate::render::BillboardParams::DEFAULT,
            [0.0, 0.0, 1.0, 1.0],
        );
        assert!(aside.is_none());
    }

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
    fn split_policy_splits_only_fogged_textured_or_translucent_draws() {
        let (cube, tex) = (cube_tmdl(), textured_cube());
        let (cube, tex) = (
            Mesh::from_tmdl(&cube).unwrap(),
            Mesh::from_tmdl(&tex).unwrap(),
        );
        let screen = Vec2::new(320.0, 240.0);
        let plain = TnlContext::new(&Camera3::default(), &DEFAULT_LIGHT, screen);
        let fogged = TnlContext::new(
            &Camera3::default(),
            &Light {
                fog: Fog {
                    color: Color::BLACK,
                    near: 1.0,
                    far: 9.0,
                },
                ..DEFAULT_LIGHT
            },
            screen,
        );
        let opaque = ModelParams::DEFAULT;
        let glass = ModelParams::tinted(Color::rgba(255, 255, 255, 100));
        assert!(!split_policy(&plain, &cube, &opaque));
        assert!(split_policy(&fogged, &cube, &opaque));
        assert!(split_policy(&plain, &cube, &glass));
        assert!(split_policy(&plain, &tex, &opaque));
        // Vertex alpha counts too.
        let mut blob = cube_tmdl();
        let colors_at = 12 + 24 * 12 * 2;
        blob[colors_at + 3] = 10;
        assert!(split_policy(
            &plain,
            &Mesh::from_tmdl(&blob).unwrap(),
            &opaque
        ));
    }

    #[test]
    fn model_space_light_matches_world_space_lighting() {
        let blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        let rot = Vec3::new(0.3, 0.9, -0.2);
        let at = Vec3::new(0.2, -0.1, 3.0);
        let uniform = Mat34::from_rotation_scale_translation(rot, Vec3::new(1.5, 1.5, 1.5), at);
        let l = DEFAULT_LIGHT.dir.normalized();
        assert!(model_space_light(&mesh, &uniform, l).is_some());
        // Non-uniform scale: fine for a box (axis-aligned normals, like
        // castle64's blocks), not for free normals (the world path then).
        let stretched = Mat34::from_rotation_scale_translation(rot, Vec3::new(1.5, 1.5, 1.6), at);
        assert!(model_space_light(&mesh, &stretched, l).is_some());
        let free = Mesh {
            axis_normals: false,
            ..mesh
        };
        assert!(model_space_light(&free, &stretched, l).is_none());
        // The same cube with normals 1% long is lit in world space (they
        // are renormalized there): the colors must agree within rounding.
        let mut long = blob.clone();
        let normals_at = 12 + 24 * 12;
        for k in 0..24 * 3 {
            let at = normals_at + k * 4;
            let v = f32::from_le_bytes(long[at..at + 4].try_into().unwrap()) * 1.01;
            long[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        let world = Mesh::from_tmdl(&long).unwrap();
        assert!(model_space_light(&world, &uniform, l).is_none());
        for model in [uniform, stretched] {
            let a = collect(&mesh, &model, &Camera3::default(), Color::WHITE);
            let b = collect(&world, &model, &Camera3::default(), Color::WHITE);
            assert_eq!(a.len(), b.len());
            for (x, y) in a.iter().zip(&b) {
                for (p, q) in x.colors.iter().zip(&y.colors) {
                    for (u, v) in [(p.r, q.r), (p.g, q.g), (p.b, q.b)] {
                        assert!(u.abs_diff(v) <= 1, "{p:?} vs {q:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn rsp_eligibility_follows_what_the_overlay_can_draw() {
        let mut blob = cube_tmdl();
        let mut b = std::vec::Vec::new();
        f32s(&mut b, &[0.0, 0.0, 0.0, 0.87]);
        section(&mut blob, b"BNDS", &b);
        let cube = Mesh::from_tmdl(&blob).unwrap();
        let screen = Vec2::new(320.0, 240.0);
        let ctx = TnlContext::new(&Camera3::default(), &DEFAULT_LIGHT, screen);
        let at = |z: f32| {
            Mat34::from_rotation_scale_translation(
                Vec3::ZERO,
                Vec3::new(1.0, 1.0, 1.0),
                Vec3::new(0.0, 0.0, z),
            )
        };
        let plain = ModelParams::DEFAULT;
        // In front of the camera (eye at z = -5), whole sphere inside.
        let d = rsp_eligible(&ctx, &cube, &at(0.0), &plain).expect("eligible");
        assert!(d.cull_back && d.ambient == DEFAULT_LIGHT.ambient);
        // Crossing the near plane (clipping needed): CPU path.
        assert!(rsp_eligible(&ctx, &cube, &at(-4.6), &plain).is_none());
        // What the overlay lacks: CPU path.
        for p in [
            ModelParams {
                shading: Shading::Flat,
                ..plain
            },
            ModelParams::tinted(Color::rgba(255, 255, 255, 200)),
            ModelParams {
                emissive: Color::rgb(20, 0, 0),
                ..plain
            },
        ] {
            assert!(rsp_eligible(&ctx, &cube, &at(0.0), &p).is_none(), "{p:?}");
        }
        let fog = Light {
            fog: Fog {
                color: Color::BLACK,
                near: 1.0,
                far: 9.0,
            },
            ..DEFAULT_LIGHT
        };
        let fogged = TnlContext::new(&Camera3::default(), &fog, screen);
        assert!(rsp_eligible(&fogged, &cube, &at(0.0), &plain).is_none());
        // Fixed-point range: too far, or a huge scale, stays on the CPU.
        assert!(rsp_eligible(&ctx, &cube, &at(20_000.0), &plain).is_none());
        let huge = Mat34::from_rotation_scale_translation(
            Vec3::ZERO,
            Vec3::new(5_000.0, 5_000.0, 5_000.0),
            Vec3::new(0.0, 0.0, 30_000.0),
        );
        assert!(rsp_eligible(&ctx, &cube, &huge, &plain).is_none());
        // Back faces through the overlay are unverified: CPU path.
        let both = ModelParams {
            double_sided: true,
            ..plain
        };
        assert!(rsp_eligible(&ctx, &cube, &at(0.0), &both).is_none());
        // Unlit: no light, full ambient.
        let u = ModelParams {
            shading: Shading::Unlit,
            ..plain
        };
        assert_eq!(
            rsp_eligible(&ctx, &cube, &at(0.0), &u).unwrap().ambient,
            1.0
        );
        // A textured mesh never has an RSP copy.
        assert!(!rsp_mesh_supported(
            &Mesh::from_tmdl(&textured_cube()).unwrap()
        ));
    }

    #[test]
    fn layout_rebuilds_the_same_mesh() {
        for blob in [cube_tmdl(), skinned_cube(), textured_cube()] {
            let parsed = Mesh::from_tmdl(&blob).unwrap();
            let rebuilt = Mesh::from_layout(&blob, &parsed.layout(&blob));
            assert_eq!(format!("{parsed:?}"), format!("{rebuilt:?}"));
        }
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

    /// The unit cube with planar UVs (u = x + 0.5, v = y + 0.5) and a
    /// default texture.
    fn textured_cube() -> std::vec::Vec<u8> {
        let mut blob = cube_tmdl();
        let mesh_blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&mesh_blob).unwrap();
        let mut uv = std::vec::Vec::new();
        for i in 0..mesh.vertex_count {
            let p = mesh.position(i);
            f32s(&mut uv, &[p.x + 0.5, p.y + 0.5]);
        }
        section(&mut blob, b"TXUV", &uv);
        section(&mut blob, b"TXID", &SpriteId(42).0.to_le_bytes());
        blob
    }

    #[test]
    fn textured_meshes_carry_uvs_and_view_depth() {
        let blob = textured_cube();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        assert!(mesh.has_uvs());
        assert_eq!(mesh.default_texture(), Some(SpriteId(42)));
        assert_eq!(mesh.texture_for(&ModelParams::DEFAULT), Some(SpriteId(42)));
        let over = ModelParams {
            texture: Some(SpriteId(7)),
            ..ModelParams::DEFAULT
        };
        assert_eq!(mesh.texture_for(&over), Some(SpriteId(7)));
        // A mesh without UVs never samples a texture.
        let plain = cube_tmdl();
        assert_eq!(Mesh::from_tmdl(&plain).unwrap().texture_for(&over), None);

        let tris = collect(&mesh, &Mat34::IDENTITY, &Camera3::default(), Color::WHITE);
        assert!(!tris.is_empty());
        for t in &tris {
            for k in 0..3 {
                let ok = |x: f32| (-1e-4..=1.0001).contains(&x);
                assert!(ok(t.uv[k].x) && ok(t.uv[k].y), "{:?}", t.uv[k]);
                // Camera at z = -5 looking at a unit cube: depth ~4.5..5.5.
                assert!((4.4..5.6).contains(&t.w[k]), "w = {}", t.w[k]);
                // z and w agree: z = 1 - NEAR / w.
                assert!((t.z[k] - (1.0 - NEAR / t.w[k])).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn clipping_interpolates_uvs() {
        // A long textured slab crossing the near plane: clipped vertices
        // get UVs between their edge endpoints, never outside 0..1 here.
        let blob = textured_cube();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        let model = Mat34::from_rotation_scale_translation(
            Vec3::ZERO,
            Vec3::new(1.0, 1.0, 40.0),
            Vec3::new(0.0, -1.5, 0.0),
        );
        let camera = Camera3 {
            eye: Vec3::new(0.0, 0.0, 0.0),
            target: Vec3::new(0.0, -0.2, 1.0),
            fov_y: 1.0,
        };
        let tris = collect(&mesh, &model, &camera, Color::WHITE);
        assert!(!tris.is_empty());
        for t in &tris {
            for k in 0..3 {
                assert!(t.w[k] >= NEAR * 0.999, "clipped at the near plane");
                assert!((-1e-4..=1.0001).contains(&t.uv[k].x), "{:?}", t.uv[k]);
            }
        }
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
        // Translucent, so the split policy applies.
        let out = collect(&mesh, &model, &camera, Color::rgba(255, 255, 255, 128));
        assert!(!out.is_empty(), "near-crossing ground vanished");
        // Depth subdivision: the receding floor must arrive as depth-thin
        // strips, not one giant triangle pair.
        assert!(out.len() > 6, "floor not subdivided: {} tris", out.len());
        // Opaque, the same floor is clipped but not subdivided: the
        // z-buffer gets the same depth from the whole triangle.
        let opaque = collect(&mesh, &model, &camera, Color::WHITE);
        assert!(!opaque.is_empty() && opaque.len() < out.len());
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
    fn back_faces_are_dropped_before_clipping_and_splitting() {
        // A long floor slab seen from above: its top and near side face the
        // camera, the bottom, far side and both ends do not (8 triangles).
        // The bottom spans the same depth as the top, so splitting it first
        // would cull a pile of pieces; it must cost one test per triangle.
        let blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        let slab = crate::Transform3 {
            position: Vec3::new(0.0, -1.5, 6.0),
            rotation: Vec3::ZERO,
            scale: Vec3::new(12.0, 1.0, 16.0),
        }
        .matrix();
        let mut tris = std::vec::Vec::new();
        // Translucent: split_policy keeps the depth splits on.
        let stats = tessellate(
            &mesh,
            &slab,
            &Camera3::default(),
            &DEFAULT_LIGHT,
            &ModelParams::tinted(Color::rgba(255, 255, 255, 200)),
            Vec2::new(320.0, 240.0),
            &mut |t| tris.push(t),
        );
        assert_eq!(stats.tris_in, 12);
        assert_eq!(stats.backfaced, 8, "{stats:?}");
        assert!(stats.splits > 0, "the top still splits: {stats:?}");
        assert_eq!(stats.emitted as usize, tris.len());
        for t in &tris {
            let p = t.pts;
            let area =
                (p[1].x - p[0].x) * (p[2].y - p[0].y) - (p[2].x - p[0].x) * (p[1].y - p[0].y);
            assert!(area > 0.0, "only front faces reach the backend");
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
    fn smooth_skinning_blends_two_joints() {
        // The skinned cube with every top vertex (joint 1) blended 50%
        // back toward joint 0: lifting joint 1 by 1 moves them by 0.5.
        let rigid_blob = skinned_cube();
        let mut smooth_blob = skinned_cube();
        let mesh = Mesh::from_tmdl(&rigid_blob).unwrap();
        let mut skw2 = std::vec::Vec::new();
        for i in 0..mesh.vertex_count {
            if mesh.joint_of(i) == 1 {
                skw2.extend_from_slice(&[0, 128]);
            } else {
                skw2.extend_from_slice(&[0, 0]);
            }
        }
        section(&mut smooth_blob, b"SKW2", &skw2);
        let smooth = Mesh::from_tmdl(&smooth_blob).unwrap();
        assert!(smooth.has_blend() && !mesh.has_blend());
        let top = mesh.joint_of_first(1).unwrap();
        let (j2, w) = smooth.blend_of(top).unwrap();
        assert_eq!(j2, 0);
        assert!((w - 128.0 / 255.0).abs() < 1e-6);
        // Same pose (lift 1 unit), drawn rigid vs smooth: the top edge of
        // the smooth one sits lower on screen (larger y).
        let params = ModelParams {
            anim: Some(AnimRef::new(ClipId::from_name("lift"), 0.5)),
            ..ModelParams::DEFAULT
        };
        let top_y = |m: &Mesh| {
            let mut tris = std::vec::Vec::new();
            tessellate(
                m,
                &Mat34::IDENTITY,
                &Camera3::default(),
                &DEFAULT_LIGHT,
                &params,
                Vec2::new(320.0, 240.0),
                &mut |t| tris.push(t),
            );
            tris.iter()
                .flat_map(|t| t.pts)
                .map(|p| p.y)
                .fold(f32::MAX, f32::min)
        };
        let (r, sm) = (top_y(&mesh), top_y(&smooth));
        assert!(sm > r + 1.0, "smooth top {sm} vs rigid {r}");
        // A blend toward a joint that does not exist is rejected.
        let mut bad = skinned_cube();
        let mut b = std::vec::Vec::new();
        for _ in 0..mesh.vertex_count {
            b.extend_from_slice(&[9, 255]);
        }
        section(&mut bad, b"SKW2", &b);
        assert_eq!(Mesh::from_tmdl(&bad).unwrap_err(), MeshError::BadSection);
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
