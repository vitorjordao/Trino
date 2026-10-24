//! glTF 2.0 -> TMDL: the 3D model bake, shared by every platform (TMDL is
//! the engine's portable mesh format, parsed no_std by
//! `trino_core::render3d::Mesh`).
//!
//! Real-world low-poly assets (Kenney, Quaternius, ...) are multi-node,
//! multi-primitive and textured, while the engine rasterizes vertex colors
//! only (N64 ceiling). The bake therefore:
//!
//! - walks the default scene and merges **every primitive of every node**,
//!   applying node transforms to positions and normals;
//! - resolves per-vertex colors in priority order: `COLOR_0` when present,
//!   else the material's `baseColorTexture` **sampled at each vertex UV**
//!   (low-poly palette atlases collapse perfectly to flat face colors),
//!   else the material's `baseColorFactor` — always multiplied by the
//!   factor, like glTF specifies;
//! - accepts non-indexed primitives (indices synthesized);
//! - when the file has **animations or a skin**, keeps the node hierarchy as
//!   a rigid skeleton (each vertex follows exactly one joint: the skin's
//!   heaviest influence, or the node that owns the mesh) and samples every
//!   animation into a clip at a fixed rate (`anim_fps`, default 15) —
//!   `trino_core::render3d::Mesh::pose` plays them back on every target;
//! - always appends a bounding sphere covering every animation frame, used
//!   for whole-mesh frustum culling.
//!
//! `*.prim.ron` sources are procedural placeholder meshes instead
//! ([`crate::primitive`]) — same TMDL output, so swapping a placeholder for
//! the real `.glb` keeps the handle.

use std::path::Path;

use trino_core::render3d::{MAX_JOINTS, MAX_SLOT, TMDL_MAGIC};
use trino_core::{AnimRef, ClipId, Mat34, Mesh, Vec3};

/// Column-major glTF node matrix helpers.
fn mat_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for (c, col) in out.iter_mut().enumerate() {
        for (r, cell) in col.iter_mut().enumerate() {
            *cell = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}

fn mat_point(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for (r, o) in out.iter_mut().enumerate() {
        *o = m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r];
    }
    out
}

fn mat_dir(m: &[[f32; 4]; 4], d: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for (r, o) in out.iter_mut().enumerate() {
        *o = m[0][r] * d[0] + m[1][r] * d[1] + m[2][r] * d[2];
    }
    let len = (out[0] * out[0] + out[1] * out[1] + out[2] * out[2]).sqrt();
    if len > 1e-8 {
        for v in &mut out {
            *v /= len;
        }
    }
    out
}

/// glTF `COLOR_0` is **linear** (the spec; Blender exports it so), while
/// the engine's vertex colors are display (sRGB) bytes like texture texels:
/// convert, rounding to the nearest byte (alpha is linear, no curve).
fn color0_to_display(c: [f32; 4]) -> [u8; 4] {
    let srgb = |l: f32| {
        let l = l.clamp(0.0, 1.0);
        if l <= 0.003_130_8 {
            l * 12.92
        } else {
            1.055 * l.powf(1.0 / 2.4) - 0.055
        }
    };
    let byte = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    [
        byte(srgb(c[0])),
        byte(srgb(c[1])),
        byte(srgb(c[2])),
        byte(c[3]),
    ]
}

/// Nearest-neighbor sample of a glTF image at (u, v), repeat wrapping.
fn sample_image(img: &gltf::image::Data, u: f32, v: f32) -> Option<[u8; 4]> {
    let wrap = |t: f32| {
        let f = t - t.floor();
        if f < 0.0 { f + 1.0 } else { f }
    };
    let x = ((wrap(u) * img.width as f32) as u32).min(img.width - 1) as usize;
    let y = ((wrap(v) * img.height as f32) as u32).min(img.height - 1) as usize;
    let at = y * img.width as usize + x;
    use gltf::image::Format;
    Some(match img.format {
        Format::R8 => {
            let g = img.pixels[at];
            [g, g, g, 255]
        }
        Format::R8G8 => {
            let g = img.pixels[at * 2];
            [g, g, g, img.pixels[at * 2 + 1]]
        }
        Format::R8G8B8 => {
            let p = &img.pixels[at * 3..at * 3 + 3];
            [p[0], p[1], p[2], 255]
        }
        Format::R8G8B8A8 => {
            let p = &img.pixels[at * 4..at * 4 + 4];
            [p[0], p[1], p[2], p[3]]
        }
        _ => return None, // 16-bit formats: fall back to the color factor
    })
}

type Mat4 = [[f32; 4]; 4];

const IDENTITY: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// Inverse of a column-major affine matrix (last row 0,0,0,1).
fn affine_inverse(m: &Mat4) -> Mat4 {
    // a = upper 3x3 (a[r][c] = m[c][r]).
    let a = |r: usize, c: usize| m[c][r];
    let det = a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1))
        - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0))
        + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0));
    let inv_det = if det.abs() > 1e-20 { 1.0 / det } else { 0.0 };
    let mut inv = [[0.0f32; 3]; 3]; // inv[r][c]
    inv[0][0] = (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1)) * inv_det;
    inv[0][1] = (a(0, 2) * a(2, 1) - a(0, 1) * a(2, 2)) * inv_det;
    inv[0][2] = (a(0, 1) * a(1, 2) - a(0, 2) * a(1, 1)) * inv_det;
    inv[1][0] = (a(1, 2) * a(2, 0) - a(1, 0) * a(2, 2)) * inv_det;
    inv[1][1] = (a(0, 0) * a(2, 2) - a(0, 2) * a(2, 0)) * inv_det;
    inv[1][2] = (a(0, 2) * a(1, 0) - a(0, 0) * a(1, 2)) * inv_det;
    inv[2][0] = (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0)) * inv_det;
    inv[2][1] = (a(0, 1) * a(2, 0) - a(0, 0) * a(2, 1)) * inv_det;
    inv[2][2] = (a(0, 0) * a(1, 1) - a(0, 1) * a(1, 0)) * inv_det;
    let t = [m[3][0], m[3][1], m[3][2]];
    let mut out = IDENTITY;
    for r in 0..3 {
        for c in 0..3 {
            out[c][r] = inv[r][c];
        }
        out[3][r] = -(inv[r][0] * t[0] + inv[r][1] * t[1] + inv[r][2] * t[2]);
    }
    out
}

/// Translation / rotation (unit quaternion xyzw) / scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trs {
    pub t: [f32; 3],
    pub r: [f32; 4],
    pub s: [f32; 3],
}

impl Trs {
    pub const IDENTITY: Trs = Trs {
        t: [0.0; 3],
        r: [0.0, 0.0, 0.0, 1.0],
        s: [1.0; 3],
    };

    fn matrix(&self) -> Mat4 {
        let [x, y, z, w] = self.r;
        let (xx, yy, zz) = (x * x, y * y, z * z);
        let (xy, xz, yz) = (x * y, x * z, y * z);
        let (wx, wy, wz) = (w * x, w * y, w * z);
        let [sx, sy, sz] = self.s;
        [
            [
                (1.0 - 2.0 * (yy + zz)) * sx,
                2.0 * (xy + wz) * sx,
                2.0 * (xz - wy) * sx,
                0.0,
            ],
            [
                2.0 * (xy - wz) * sy,
                (1.0 - 2.0 * (xx + zz)) * sy,
                2.0 * (yz + wx) * sy,
                0.0,
            ],
            [
                2.0 * (xz + wy) * sz,
                2.0 * (yz - wx) * sz,
                (1.0 - 2.0 * (xx + yy)) * sz,
                0.0,
            ],
            [self.t[0], self.t[1], self.t[2], 1.0],
        ]
    }

    /// Decompose a column-major affine matrix (exact without shear).
    fn from_matrix(m: &Mat4) -> Trs {
        let len = |c: usize| (m[c][0] * m[c][0] + m[c][1] * m[c][1] + m[c][2] * m[c][2]).sqrt();
        let s = [len(0), len(1), len(2)];
        let inv = |v: f32| if v > 1e-12 { 1.0 / v } else { 0.0 };
        // Rotation matrix r[row][col].
        let r = |row: usize, col: usize| m[col][row] * inv(s[col]);
        let trace = r(0, 0) + r(1, 1) + r(2, 2);
        let q = if trace > 0.0 {
            let k = (trace + 1.0).sqrt() * 2.0;
            [
                (r(2, 1) - r(1, 2)) / k,
                (r(0, 2) - r(2, 0)) / k,
                (r(1, 0) - r(0, 1)) / k,
                0.25 * k,
            ]
        } else if r(0, 0) > r(1, 1) && r(0, 0) > r(2, 2) {
            let k = (1.0 + r(0, 0) - r(1, 1) - r(2, 2)).sqrt() * 2.0;
            [
                0.25 * k,
                (r(0, 1) + r(1, 0)) / k,
                (r(0, 2) + r(2, 0)) / k,
                (r(2, 1) - r(1, 2)) / k,
            ]
        } else if r(1, 1) > r(2, 2) {
            let k = (1.0 + r(1, 1) - r(0, 0) - r(2, 2)).sqrt() * 2.0;
            [
                (r(0, 1) + r(1, 0)) / k,
                0.25 * k,
                (r(1, 2) + r(2, 1)) / k,
                (r(0, 2) - r(2, 0)) / k,
            ]
        } else {
            let k = (1.0 + r(2, 2) - r(0, 0) - r(1, 1)).sqrt() * 2.0;
            [
                (r(0, 2) + r(2, 0)) / k,
                (r(1, 2) + r(2, 1)) / k,
                0.25 * k,
                (r(1, 0) - r(0, 1)) / k,
            ]
        };
        Trs {
            t: [m[3][0], m[3][1], m[3][2]],
            r: normalize_quat(q),
            s,
        }
    }
}

fn normalize_quat(q: [f32; 4]) -> [f32; 4] {
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if len > 1e-12 {
        [q[0] / len, q[1] / len, q[2] / len, q[3] / len]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

fn nlerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let b = if dot < 0.0 {
        [-b[0], -b[1], -b[2], -b[3]]
    } else {
        b
    };
    normalize_quat(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t))
}

/// One joint of a rigid skeleton.
#[derive(Clone, Debug)]
pub struct JointData {
    /// Index of the parent joint (always lower), or -1.
    pub parent: i32,
    /// Maps bind-pose model space into the joint's space.
    pub inverse_bind: [[f32; 4]; 3],
    pub rest: Trs,
}

/// A clip sampled at a fixed rate: `frames[f][joint]` local transforms.
#[derive(Clone, Debug)]
pub struct ClipData {
    pub name: String,
    pub fps: f32,
    pub looping: bool,
    pub frames: Vec<Vec<Trs>>,
}

#[derive(Clone, Debug)]
pub struct SkeletonData {
    pub joint_of_vertex: Vec<u8>,
    pub joints: Vec<JointData>,
    pub clips: Vec<ClipData>,
}

/// An in-memory mesh about to become a TMDL blob — what both the glTF bake
/// and the procedural primitives produce.
#[derive(Clone, Debug, Default)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub colors: Vec<[u8; 4]>,
    pub indices: Vec<u32>,
    pub skeleton: Option<SkeletonData>,
    /// Texture coordinates (one per vertex) — written as `TXUV` only when
    /// the model is textured (`texture` set).
    pub uvs: Vec<[f32; 2]>,
    /// Default texture (sprite handle) -> `TXID`; UVs are kept iff set.
    pub texture: Option<u32>,
    /// Smooth skinning: (second joint, weight 0..255) per vertex -> `SKW2`
    /// (written only when some vertex actually blends).
    pub blend: Vec<[u8; 2]>,
    /// Color slot per vertex (0..=4, glTF `_SLOT`) -> `SLOT` (written only
    /// when some vertex has a non-zero slot). Empty = no slots.
    pub slots: Vec<u8>,
}

fn section(blob: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]) {
    blob.extend_from_slice(tag);
    blob.extend_from_slice(&(body.len() as u32).to_le_bytes());
    blob.extend_from_slice(body);
}

fn put_f32s(out: &mut Vec<u8>, vals: &[f32]) {
    for v in vals {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

fn put_trs(out: &mut Vec<u8>, trs: &Trs) {
    put_f32s(out, &trs.t);
    put_f32s(out, &trs.r);
    put_f32s(out, &trs.s);
}

impl MeshData {
    /// Reorder for the engine's per-draw vertex cache (`trino_core::
    /// render3d`, 32 direct-mapped slots keyed by `index % 32`): triangles
    /// sorted (stably) by their lowest vertex, then vertices renumbered in
    /// first-use order, so a vertex's reuses come while it is still cached.
    /// Exporters do not order for this — Blender's glTF output missed on
    /// every reference (boar: 210 transforms for 140 vertices), and on the
    /// N64 each transform costs ~17 us. Pixels are unchanged (same triangles,
    /// same winding); already-local meshes (quads written as 4 consecutive
    /// vertices) come out identical.
    pub fn optimize_vertex_order(&mut self) {
        let n = self.positions.len();
        let mut tris: Vec<[u32; 3]> = self.indices.as_chunks::<3>().0.to_vec();
        tris.sort_by_key(|t| t[0].min(t[1]).min(t[2]));
        let mut remap = vec![u32::MAX; n];
        let mut order = Vec::with_capacity(n);
        for i in tris.iter().flatten() {
            if remap[*i as usize] == u32::MAX {
                remap[*i as usize] = order.len() as u32;
                order.push(*i as usize);
            }
        }
        // Unreferenced vertices keep their relative order at the end.
        for (i, r) in remap.iter_mut().enumerate() {
            if *r == u32::MAX {
                *r = order.len() as u32;
                order.push(i);
            }
        }
        self.indices = tris.iter().flatten().map(|i| remap[*i as usize]).collect();
        fn permute<T: Copy>(v: &mut Vec<T>, order: &[usize]) {
            if v.len() == order.len() {
                *v = order.iter().map(|&i| v[i]).collect();
            }
        }
        permute(&mut self.positions, &order);
        permute(&mut self.normals, &order);
        permute(&mut self.colors, &order);
        permute(&mut self.uvs, &order);
        permute(&mut self.blend, &order);
        permute(&mut self.slots, &order);
        if let Some(skel) = &mut self.skeleton {
            permute(&mut skel.joint_of_vertex, &order);
        }
    }

    /// Serialize to TMDL (base arrays + `SKEL`/`ANIM` + `BNDS`).
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        if self.positions.is_empty() {
            return Err("mesh has no triangle geometry".into());
        }
        if self.positions.len() > u16::MAX as usize {
            return Err(format!(
                "{} vertices exceed the engine's u16 index space (split the model or decimate it)",
                self.positions.len()
            ));
        }
        let mut blob = Vec::new();
        blob.extend_from_slice(TMDL_MAGIC);
        blob.extend_from_slice(&(self.positions.len() as u32).to_le_bytes());
        blob.extend_from_slice(&(self.indices.len() as u32).to_le_bytes());
        for p in &self.positions {
            put_f32s(&mut blob, p);
        }
        for n in &self.normals {
            put_f32s(&mut blob, n);
        }
        for c in &self.colors {
            blob.extend_from_slice(c);
        }
        for i in &self.indices {
            blob.extend_from_slice(&(*i as u16).to_le_bytes());
        }

        if let Some(skel) = &self.skeleton {
            if skel.joints.len() > MAX_JOINTS {
                return Err(format!(
                    "{} joints exceed the engine limit of {MAX_JOINTS} — merge static parts \
                     into one object or drop helper bones",
                    skel.joints.len()
                ));
            }
            let mut body = Vec::new();
            body.extend_from_slice(&(skel.joints.len() as u32).to_le_bytes());
            body.extend_from_slice(&skel.joint_of_vertex);
            for j in &skel.joints {
                body.extend_from_slice(&j.parent.to_le_bytes());
                for row in &j.inverse_bind {
                    put_f32s(&mut body, row);
                }
                put_trs(&mut body, &j.rest);
            }
            section(&mut blob, b"SKEL", &body);
            if self.blend.iter().any(|b| b[1] != 0) {
                if self.blend.len() != self.positions.len() {
                    return Err("smooth skin: one blend pair per vertex expected".into());
                }
                let body: Vec<u8> = self.blend.iter().flatten().copied().collect();
                section(&mut blob, b"SKW2", &body);
            }
            if !skel.clips.is_empty() {
                let mut body = Vec::new();
                body.extend_from_slice(&(skel.clips.len() as u32).to_le_bytes());
                for c in &skel.clips {
                    body.extend_from_slice(&ClipId::from_name(&c.name).0.to_le_bytes());
                    put_f32s(&mut body, &[c.fps]);
                    body.extend_from_slice(&(c.frames.len() as u32).to_le_bytes());
                    body.extend_from_slice(&(c.looping as u32).to_le_bytes());
                    for frame in &c.frames {
                        for trs in frame {
                            put_trs(&mut body, trs);
                        }
                    }
                }
                section(&mut blob, b"ANIM", &body);
            }
        }

        if let Some(tex) = self.texture {
            if self.uvs.len() != self.positions.len() {
                return Err(format!(
                    "textured model has {} UVs for {} vertices (export TEXCOORD_0)",
                    self.uvs.len(),
                    self.positions.len()
                ));
            }
            let mut body = Vec::with_capacity(self.uvs.len() * 8);
            for uv in &self.uvs {
                put_f32s(&mut body, uv);
            }
            section(&mut blob, b"TXUV", &body);
            section(&mut blob, b"TXID", &tex.to_le_bytes());
        }

        if self.slots.iter().any(|&s| s != 0) {
            if self.slots.len() != self.positions.len() {
                return Err(format!(
                    "{} color slots for {} vertices (one `_SLOT` per vertex expected)",
                    self.slots.len(),
                    self.positions.len()
                ));
            }
            if let Some(s) = self.slots.iter().find(|&&s| s > MAX_SLOT) {
                return Err(format!("color slot {s} is out of range 0..={MAX_SLOT}"));
            }
            section(&mut blob, b"SLOT", &self.slots);
        }

        // Bounds over the bind pose and every sampled frame, evaluated by
        // the engine's own pose code (what the runtime will draw).
        let (center, radius) = {
            let mesh = Mesh::from_tmdl(&blob).map_err(|e| format!("TMDL self-check: {e:?}"))?;
            let mut poses: Vec<Option<AnimRef>> = vec![None];
            for n in 0..mesh.clip_count() {
                let clip = mesh.clip(n).unwrap();
                for f in 0..clip.frame_count {
                    poses.push(Some(AnimRef::new(clip.id, f as f32 / clip.fps.max(1e-6))));
                }
            }
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            let mut posed: Vec<Vec3> = Vec::new();
            let mut skin = [Mat34::IDENTITY; MAX_JOINTS];
            for pose in &poses {
                let joints = mesh.pose(*pose, &mut skin);
                for (i, p) in self.positions.iter().enumerate() {
                    let v = Vec3::new(p[0], p[1], p[2]);
                    let mut candidates = [v; 2];
                    if joints > 0 {
                        let j = self.skeleton.as_ref().unwrap().joint_of_vertex[i] as usize;
                        candidates = [skin[j].transform_point(v); 2];
                        // A blended vertex lies between its two joints'
                        // positions: covering both covers it.
                        if let Some(&[j2, w]) = self.blend.get(i)
                            && w != 0
                        {
                            candidates[1] = skin[j2 as usize].transform_point(v);
                        }
                    }
                    for v in candidates {
                        for (k, c) in [v.x, v.y, v.z].into_iter().enumerate() {
                            lo[k] = lo[k].min(c);
                            hi[k] = hi[k].max(c);
                        }
                        posed.push(v);
                    }
                }
            }
            let c = Vec3::new(
                (lo[0] + hi[0]) * 0.5,
                (lo[1] + hi[1]) * 0.5,
                (lo[2] + hi[2]) * 0.5,
            );
            let r = posed
                .iter()
                .map(|p| {
                    let d = *p - c;
                    (d.x * d.x + d.y * d.y + d.z * d.z).sqrt()
                })
                .fold(0.0f32, f32::max);
            (c, r * 1.001 + 1e-4)
        };
        let mut body = Vec::new();
        put_f32s(&mut body, &[center.x, center.y, center.z, radius]);
        section(&mut blob, b"BNDS", &body);
        Ok(blob)
    }
}

/// Bake options from the manifest's `[models.*]` entry.
#[derive(Clone, Debug)]
pub struct ModelOptions {
    /// Clip sampling rate (frames per second).
    pub anim_fps: f32,
    /// Clip names that play once (clamp) instead of looping.
    pub once: Vec<String>,
    /// Textured model: keep UVs and bake this sprite (logical path) as the
    /// default texture. `None` = vertex colors only (textures are sampled
    /// into vertex colors at bake time).
    pub texture: Option<String>,
    /// Smooth skinning: keep the two strongest joint weights per vertex
    /// (`SKW2`) instead of snapping to the heaviest (rigid, the default).
    pub smooth: bool,
    /// Bake ambient occlusion into the vertex colors ([`crate::ao`]).
    pub ao: Option<crate::ao::AoBake>,
    /// Grade the vertex colors through this `TLUT` blob (the consoles'
    /// color grading, [`crate::lut`]), after the AO.
    pub grading: Option<Vec<u8>>,
}

impl Default for ModelOptions {
    fn default() -> Self {
        ModelOptions {
            anim_fps: 15.0,
            once: Vec::new(),
            texture: None,
            smooth: false,
            ao: None,
            grading: None,
        }
    }
}

/// Convert a model master into a TMDL blob with default options.
pub fn bake_model_tmdl(path: &Path) -> Result<Vec<u8>, String> {
    bake_model(path, &ModelOptions::default())
}

/// Convert a model master (`.gltf`/`.glb`, or a `.prim.ron` placeholder)
/// into a TMDL blob.
pub fn bake_model(path: &Path, opts: &ModelOptions) -> Result<Vec<u8>, String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let mut data = if name.ends_with(".prim.ron") {
        crate::primitive::Primitive::load(path)?.build()
    } else {
        let mut data = load_gltf(path, opts)?;
        data.optimize_vertex_order();
        data
    };
    if let Some(ao) = &opts.ao {
        crate::ao::bake_vertex_ao(&mut data, ao);
    }
    if let Some(blob) = &opts.grading {
        let lut = trino_core::Lut::parse(blob).map_err(|e| err(&format!("grading: {e:?}")))?;
        crate::lut::grade_mesh(&mut data, &lut);
    }
    data.texture = opts.texture.as_deref().map(trino_core::asset_id);
    if data.texture.is_none() {
        data.uvs.clear();
    }
    data.encode().map_err(|e| err(&e))
}

/// Animation clip names of a model master (the TMDL stores only their
/// hashes). Cheap: reads the glTF JSON without decoding buffers.
pub fn clip_names(path: &Path) -> Vec<String> {
    let Ok(gltf) = gltf::Gltf::open(path) else {
        return Vec::new();
    };
    gltf.animations()
        .enumerate()
        .map(|(i, a)| {
            a.name()
                .map(String::from)
                .unwrap_or_else(|| format!("anim{i}"))
        })
        .collect()
}

/// Interpolation-aware sample of a keyframed channel at time `t`.
fn sample_channel<const N: usize>(
    times: &[f32],
    values: &[[f32; N]],
    interp: gltf::animation::Interpolation,
    t: f32,
    lerp: impl Fn([f32; N], [f32; N], f32) -> [f32; N],
) -> [f32; N] {
    use gltf::animation::Interpolation;
    // CUBICSPLINE stores (in-tangent, value, out-tangent) triplets: use the
    // values (the curve passes through them; tangents are dropped).
    let value = |k: usize| match interp {
        Interpolation::CubicSpline => values[k * 3 + 1],
        _ => values[k],
    };
    if times.is_empty() {
        return [0.0; N];
    }
    if t <= times[0] {
        return value(0);
    }
    let last = times.len() - 1;
    if t >= times[last] {
        return value(last);
    }
    let k = times.partition_point(|&x| x <= t).saturating_sub(1);
    let (t0, t1) = (times[k], times[k + 1]);
    match interp {
        Interpolation::Step => value(k),
        _ => {
            let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
            lerp(value(k), value(k + 1), f)
        }
    }
}

/// The primitive's color slots (custom attribute `_SLOT`, one scalar per
/// vertex, float or integer, 0..=4): zeros when it has none. Floats round
/// to the nearest slot (exporters write custom attributes as floats).
fn read_slots<'s>(
    primitive: &gltf::Primitive,
    buf: impl Clone + Fn(gltf::Buffer) -> Option<&'s [u8]>,
    count: usize,
) -> Result<Vec<u8>, String> {
    use gltf::accessor::{DataType, Dimensions, Iter};
    let Some(acc) = primitive.get(&gltf::Semantic::Extras("SLOT".into())) else {
        return Ok(vec![0; count]);
    };
    if acc.dimensions() != Dimensions::Scalar {
        return Err("`_SLOT` must be a SCALAR attribute (one slot per vertex)".into());
    }
    if acc.count() != count {
        return Err(format!(
            "`_SLOT` has {} values for {count} vertices",
            acc.count()
        ));
    }
    fn ints<'s, T: gltf::accessor::Item + Copy + Into<f64>>(
        acc: gltf::Accessor,
        buf: impl Clone + Fn(gltf::Buffer) -> Option<&'s [u8]>,
    ) -> Option<Vec<f64>> {
        Some(Iter::<T>::new(acc, buf)?.map(Into::into).collect())
    }
    let vals = match acc.data_type() {
        DataType::F32 => ints::<f32>(acc, buf),
        DataType::U8 => ints::<u8>(acc, buf),
        DataType::I8 => ints::<i8>(acc, buf),
        DataType::U16 => ints::<u16>(acc, buf),
        DataType::I16 => ints::<i16>(acc, buf),
        DataType::U32 => ints::<u32>(acc, buf),
    }
    .ok_or("`_SLOT` accessor has no readable data")?;
    vals.iter()
        .map(|&v| {
            let r = v.round();
            if (0.0..=MAX_SLOT as f64).contains(&r) {
                Ok(r as u8)
            } else {
                Err(format!("`_SLOT` value {v} is out of range 0..={MAX_SLOT}"))
            }
        })
        .collect()
}

fn node_trs(node: &gltf::Node) -> Trs {
    let (t, r, s) = node.transform().decomposed();
    Trs { t, r, s }
}

fn load_gltf(path: &Path, opts: &ModelOptions) -> Result<MeshData, String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let (doc, buffers, images) = gltf::import(path).map_err(|e| err(&e))?;
    let buf = |b: gltf::Buffer| buffers.get(b.index()).map(|d| &d.0[..]);

    // Scene graph: parents, world rest transforms, preorder (parents first).
    let scene = doc.default_scene().or_else(|| doc.scenes().next());
    let node_count = doc.nodes().count();
    let mut parent: Vec<Option<usize>> = vec![None; node_count];
    let mut world: Vec<Mat4> = vec![IDENTITY; node_count];
    let mut preorder: Vec<usize> = Vec::new();
    if let Some(scene) = &scene {
        let mut work: Vec<(gltf::Node, Option<usize>, Mat4)> =
            scene.nodes().map(|n| (n, None, IDENTITY)).collect();
        work.reverse();
        while let Some((node, par, parent_world)) = work.pop() {
            let i = node.index();
            parent[i] = par;
            world[i] = mat_mul(parent_world, node.transform().matrix());
            preorder.push(i);
            let mut kids: Vec<_> = node.children().map(|c| (c, Some(i), world[i])).collect();
            kids.reverse();
            work.extend(kids);
        }
    }

    // Skeleton source: a skin (first one used by a mesh node) or, for
    // animated files without skins, every scene node as a rigid joint.
    let animated = doc.animations().next().is_some();
    let skin = preorder
        .iter()
        .filter_map(|&i| doc.nodes().nth(i).and_then(|n| n.skin()))
        .next();
    let mut joint_nodes: Vec<usize> = Vec::new();
    if let Some(skin) = &skin {
        let in_skin: std::collections::HashSet<usize> = skin.joints().map(|j| j.index()).collect();
        joint_nodes = preorder
            .iter()
            .copied()
            .filter(|i| in_skin.contains(i))
            .collect();
    } else if animated {
        joint_nodes = preorder.clone();
    }
    let rigged = !joint_nodes.is_empty();
    if joint_nodes.len() > MAX_JOINTS {
        return Err(err(&format!(
            "{} joints exceed the engine limit of {MAX_JOINTS} — merge static parts into one \
             object or drop helper bones",
            joint_nodes.len()
        )));
    }
    let joint_of_node: std::collections::HashMap<usize, usize> = joint_nodes
        .iter()
        .enumerate()
        .map(|(j, &n)| (n, j))
        .collect();
    let nearest_joint = |mut n: usize| -> Option<usize> {
        loop {
            if let Some(&j) = joint_of_node.get(&n) {
                return Some(j);
            }
            n = parent[n]?;
        }
    };
    // Static product of non-joint nodes between a joint and its nearest
    // joint ancestor (the whole ancestor chain for roots).
    let pre: Vec<Mat4> = joint_nodes
        .iter()
        .map(|&n| {
            let mut m = IDENTITY;
            let mut p = parent[n];
            while let Some(a) = p {
                if joint_of_node.contains_key(&a) {
                    break;
                }
                m = mat_mul(doc.nodes().nth(a).unwrap().transform().matrix(), m);
                p = parent[a];
            }
            m
        })
        .collect();
    let local = |j: usize, trs: &Trs| Trs::from_matrix(&mat_mul(pre[j], trs.matrix()));

    let mut data = MeshData::default();
    let mut joint_of_vertex: Vec<u8> = Vec::new();
    // (second joint, weight) per vertex — only kept with `opts.smooth`.
    let mut blend_of_vertex: Vec<[u8; 2]> = Vec::new();
    // A static joint for unskinned meshes outside the rig (skinned files).
    let mut needs_static_joint = false;
    let static_joint = joint_nodes.len();

    let mesh_nodes: Vec<usize> = if preorder.is_empty() {
        Vec::new()
    } else {
        preorder
            .iter()
            .copied()
            .filter(|&i| doc.nodes().nth(i).unwrap().mesh().is_some())
            .collect()
    };
    let mut instanced: Vec<(gltf::Mesh, Option<usize>)> = mesh_nodes
        .iter()
        .map(|&i| (doc.nodes().nth(i).unwrap().mesh().unwrap(), Some(i)))
        .collect();
    if instanced.is_empty() {
        instanced = doc.meshes().map(|m| (m, None)).collect();
    }
    if instanced.is_empty() {
        return Err(err(&"glTF contains no mesh"));
    }

    for (mesh, node_index) in &instanced {
        let node = node_index.and_then(|i| doc.nodes().nth(i));
        let skinned = rigged && skin.is_some() && node.as_ref().is_some_and(|n| n.skin().is_some());
        let xform = match node_index {
            Some(i) if !skinned => world[*i],
            _ => IDENTITY, // skinned vertices are already in bind space
        };
        let owner = node_index.and_then(nearest_joint);
        for primitive in mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                continue; // engine rasterizes triangles only
            }
            let reader = primitive.reader(buf);
            let prim_pos: Vec<[f32; 3]> = match reader.read_positions() {
                Some(p) => p.collect(),
                None => continue,
            };
            let prim_nrm: Vec<[f32; 3]> = reader
                .read_normals()
                .ok_or_else(|| err(&"primitive has no NORMAL (export with normals)"))?
                .collect();
            if prim_nrm.len() != prim_pos.len() {
                return Err(err(&"attribute counts do not match POSITION count"));
            }

            // Vertex colors: COLOR_0 > baseColorTexture@UV > baseColorFactor.
            let pbr = primitive.material().pbr_metallic_roughness();
            let factor = pbr.base_color_factor();
            let fmul = |c: [u8; 4]| {
                [
                    (c[0] as f32 * factor[0]) as u8,
                    (c[1] as f32 * factor[1]) as u8,
                    (c[2] as f32 * factor[2]) as u8,
                    (c[3] as f32 * factor[3]) as u8,
                ]
            };
            // Textured models keep UVs and sample at runtime; the vertex
            // color is then COLOR_0 or the factor (the texture modulates it).
            let textured = opts.texture.is_some();
            let tex_set = pbr.base_color_texture().map_or(0, |i| i.tex_coord());
            let prim_uv: Vec<[f32; 2]> = reader
                .read_tex_coords(tex_set)
                .map(|t| t.into_f32().collect())
                .unwrap_or_default();
            let prim_col: Vec<[u8; 4]> = if let Some(c) = reader.read_colors(0) {
                c.into_rgba_f32().map(color0_to_display).collect()
            } else if textured {
                vec![fmul([255, 255, 255, 255]); prim_pos.len()]
            } else {
                let tex = pbr.base_color_texture().and_then(|info| {
                    let image = images.get(info.texture().source().index())?;
                    let uvs: Vec<[f32; 2]> = reader
                        .read_tex_coords(info.tex_coord())?
                        .into_f32()
                        .collect();
                    Some((image, uvs))
                });
                match tex {
                    Some((image, uvs)) if uvs.len() == prim_pos.len() => uvs
                        .iter()
                        .map(|uv| {
                            fmul(sample_image(image, uv[0], uv[1]).unwrap_or([255, 255, 255, 255]))
                        })
                        .collect(),
                    _ => vec![fmul([255, 255, 255, 255]); prim_pos.len()],
                }
            };
            if prim_col.len() != prim_pos.len() {
                return Err(err(&"attribute counts do not match POSITION count"));
            }
            let prim_slots = read_slots(&primitive, buf, prim_pos.len()).map_err(|e| err(&e))?;

            // Joint per vertex.
            if rigged {
                if skinned {
                    let skin = skin.as_ref().unwrap();
                    let skin_nodes: Vec<usize> = skin.joints().map(|j| j.index()).collect();
                    let js: Vec<[u16; 4]> = reader
                        .read_joints(0)
                        .ok_or_else(|| err(&"skinned primitive has no JOINTS_0"))?
                        .into_u16()
                        .collect();
                    let ws: Vec<[f32; 4]> = reader
                        .read_weights(0)
                        .ok_or_else(|| err(&"skinned primitive has no WEIGHTS_0"))?
                        .into_f32()
                        .collect();
                    for (j4, w4) in js.iter().zip(ws.iter()) {
                        // Rigid binding: the heaviest influence wins.
                        let mut order = [0usize, 1, 2, 3];
                        order.sort_by(|&a, &b| w4[b].total_cmp(&w4[a]));
                        let k = order[0];
                        let node = skin_nodes
                            .get(j4[k] as usize)
                            .ok_or_else(|| err(&"JOINTS_0 index out of range"))?;
                        joint_of_vertex.push(joint_of_node[node] as u8);
                        // Smooth skinning (opt-in): keep the runner-up too,
                        // renormalized over the two.
                        let k2 = order[1];
                        let (w1, w2) = (w4[k], w4[k2]);
                        let blend = if opts.smooth && w2 > 0.0 && w1 + w2 > 0.0 {
                            let node2 = skin_nodes
                                .get(j4[k2] as usize)
                                .ok_or_else(|| err(&"JOINTS_0 index out of range"))?;
                            let w = (w2 / (w1 + w2) * 255.0).round() as u8;
                            [joint_of_node[node2] as u8, w]
                        } else {
                            [0, 0]
                        };
                        blend_of_vertex.push(blend);
                    }
                } else {
                    let j = match owner {
                        Some(j) => j,
                        None => {
                            needs_static_joint = true;
                            static_joint
                        }
                    };
                    joint_of_vertex.extend(std::iter::repeat_n(j as u8, prim_pos.len()));
                    blend_of_vertex.extend(std::iter::repeat_n([0u8, 0], prim_pos.len()));
                }
            }

            let base = data.positions.len() as u32;
            data.positions
                .extend(prim_pos.iter().map(|p| mat_point(&xform, *p)));
            data.normals
                .extend(prim_nrm.iter().map(|n| mat_dir(&xform, *n)));
            data.colors.extend(prim_col);
            data.slots.extend(prim_slots);
            if prim_uv.len() == prim_pos.len() {
                data.uvs.extend(prim_uv);
            } else {
                data.uvs
                    .extend(std::iter::repeat_n([0.0, 0.0], prim_pos.len()));
            }
            match reader.read_indices() {
                Some(idx) => data.indices.extend(idx.into_u32().map(|i| base + i)),
                None => data.indices.extend(base..data.positions.len() as u32),
            }
        }
    }

    if !rigged {
        return Ok(data);
    }

    // Joints: parent, inverse bind, rest pose.
    let ibms: Option<Vec<Mat4>> = skin.as_ref().map(|s| {
        let reader = s.reader(buf);
        let nodes: Vec<usize> = s.joints().map(|j| j.index()).collect();
        let mats: Vec<Mat4> = reader
            .read_inverse_bind_matrices()
            .map(|m| m.collect())
            .unwrap_or_else(|| vec![IDENTITY; nodes.len()]);
        // Reorder from skin order to our preorder.
        joint_nodes
            .iter()
            .map(|n| {
                let k = nodes.iter().position(|x| x == n).unwrap();
                mats.get(k).copied().unwrap_or(IDENTITY)
            })
            .collect()
    });
    let to_rows =
        |m: &Mat4| -> [[f32; 4]; 3] { std::array::from_fn(|r| std::array::from_fn(|c| m[c][r])) };
    let mut joints: Vec<JointData> = joint_nodes
        .iter()
        .enumerate()
        .map(|(j, &n)| {
            let parent_joint = parent[n].and_then(nearest_joint).map_or(-1, |p| p as i32);
            let ibm = match &ibms {
                Some(m) => m[j],
                None => affine_inverse(&world[n]),
            };
            JointData {
                parent: parent_joint,
                inverse_bind: to_rows(&ibm),
                rest: local(j, &node_trs(&doc.nodes().nth(n).unwrap())),
            }
        })
        .collect();
    if needs_static_joint {
        joints.push(JointData {
            parent: -1,
            inverse_bind: to_rows(&IDENTITY),
            rest: Trs::IDENTITY,
        });
    }
    if joints.len() > MAX_JOINTS {
        return Err(err(&format!(
            "{} joints exceed the engine limit of {MAX_JOINTS}",
            joints.len()
        )));
    }

    // Clips: sample every channel at the fixed rate.
    let fps = opts.anim_fps.max(1.0);
    let mut clips = Vec::new();
    for (ai, anim) in doc.animations().enumerate() {
        let name = anim
            .name()
            .map(String::from)
            .unwrap_or_else(|| format!("anim{ai}"));
        // Per joint: (times, values) for t / r / s.
        type Track<const N: usize> =
            Option<(Vec<f32>, Vec<[f32; N]>, gltf::animation::Interpolation)>;
        let mut tracks: Vec<(Track<3>, Track<4>, Track<3>)> =
            vec![(None, None, None); joint_nodes.len()];
        // Clips start at their first keyframe: exporters offset the
        // timeline (Blender's starts at frame 1 = 1/24 s), and sampling from
        // t = 0 put a held duplicate of the first pose into every loop — a
        // visible hitch at the seam.
        let mut start = f32::MAX;
        let mut end = 0.0f32;
        for channel in anim.channels() {
            let Some(&j) = joint_of_node.get(&channel.target().node().index()) else {
                continue; // animates a node outside the rig
            };
            let reader = channel.reader(buf);
            let Some(times) = reader.read_inputs().map(|i| i.collect::<Vec<f32>>()) else {
                continue;
            };
            start = start.min(times.first().copied().unwrap_or(0.0));
            end = end.max(times.last().copied().unwrap_or(0.0));
            let interp = channel.sampler().interpolation();
            use gltf::animation::util::ReadOutputs;
            match reader.read_outputs() {
                Some(ReadOutputs::Translations(v)) => {
                    tracks[j].0 = Some((times, v.collect(), interp))
                }
                Some(ReadOutputs::Rotations(v)) => {
                    tracks[j].1 = Some((times, v.into_f32().collect(), interp))
                }
                Some(ReadOutputs::Scales(v)) => tracks[j].2 = Some((times, v.collect(), interp)),
                _ => {} // morph weights: not supported (rigid skeleton only)
            }
        }
        if start > end {
            start = 0.0; // no channel targets the rig
        }
        let duration = end - start;
        let frame_count = ((duration * fps).round() as usize).max(1) + 1;
        let lerp3 =
            |a: [f32; 3], b: [f32; 3], t: f32| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
        let mut frames = Vec::with_capacity(frame_count);
        for f in 0..frame_count {
            let t = start + (f as f32 / fps).min(duration);
            let mut frame: Vec<Trs> = joint_nodes
                .iter()
                .enumerate()
                .map(|(j, &n)| {
                    let rest = node_trs(&doc.nodes().nth(n).unwrap());
                    let (tt, rt, st) = &tracks[j];
                    let trs = Trs {
                        t: tt
                            .as_ref()
                            .map_or(rest.t, |(ts, vs, i)| sample_channel(ts, vs, *i, t, lerp3)),
                        r: rt.as_ref().map_or(rest.r, |(ts, vs, i)| {
                            normalize_quat(sample_channel(ts, vs, *i, t, nlerp))
                        }),
                        s: st
                            .as_ref()
                            .map_or(rest.s, |(ts, vs, i)| sample_channel(ts, vs, *i, t, lerp3)),
                    };
                    local(j, &trs)
                })
                .collect();
            if needs_static_joint {
                frame.push(Trs::IDENTITY);
            }
            frames.push(frame);
        }
        let looping = !opts.once.iter().any(|o| o == &name);
        clips.push(ClipData {
            name,
            fps,
            looping,
            frames,
        });
    }

    if opts.smooth {
        if needs_static_joint {
            // Unskinned meshes in a skinned file: no blend for those.
            blend_of_vertex.resize(joint_of_vertex.len(), [0, 0]);
        }
        data.blend = blend_of_vertex;
    }
    data.skeleton = Some(SkeletonData {
        joint_of_vertex,
        joints,
        clips,
    });
    Ok(data)
}

/// Bake the scene `decl.file` (relative to the project root, the parent of
/// `assets_root`) into a `TSCN` blob, resolving prefabs, materials and
/// `Auto` colliders exactly like the editor and `scene export`.
pub fn bake_scene(assets_root: &Path, file: &str) -> Result<Vec<u8>, String> {
    bake_scene_for(assets_root, file, crate::Platform::Pc)
}

/// [`bake_scene`] for one platform or hardware tier: each entity's variant
/// for `target` (`platforms` in the scene; `n64_expanded` falls back to
/// `n64`) is applied. Colliders always come from the PC models, so gameplay
/// is the same on every target.
pub fn bake_scene_for(
    assets_root: &Path,
    file: &str,
    target: impl Into<trino_scene::Target>,
) -> Result<Vec<u8>, String> {
    bake_scene_with_bounds(
        assets_root,
        file,
        manifest_model_bounds(assets_root),
        target,
    )
}

/// [`bake_scene`] with model bounds the caller already has (the bake loop
/// collects them while baking models — no second model bake).
pub fn bake_scene_with_bounds(
    assets_root: &Path,
    file: &str,
    bounds: std::collections::BTreeMap<String, ([f32; 3], [f32; 3])>,
    target: impl Into<trino_scene::Target>,
) -> Result<Vec<u8>, String> {
    let project = assets_root.parent().unwrap_or(Path::new(""));
    let scene = trino_scene::Scene::load(&project.join(file))?;
    let mut lib = trino_scene::Library::load(project);
    if let Some(e) = lib.errors.first() {
        return Err(format!("scene `{file}`: library error: {e}"));
    }
    lib.model_bounds = bounds;
    lib.impostors = crate::impostor::baked(assets_root);
    let (blob, warnings) = trino_scene::bake::to_tscn_for(&scene, &lib, target.into());
    if let Some(w) = warnings.first() {
        return Err(format!("scene `{file}`: {w}"));
    }
    Ok(blob)
}

/// [`bake_scene_for`] as a console bake does it (issue #64): with
/// `bake_grading` on the scene's manifest entry, its environment's colors
/// graded through the LUT its `post.grading` names.
pub fn bake_console_scene(
    assets_root: &Path,
    manifest: &crate::Manifest,
    decl: &crate::manifest::BlobDecl,
    platform: crate::Platform,
    target: impl Into<trino_scene::Target>,
) -> Result<Vec<u8>, String> {
    let Some(lut) = scene_grading(assets_root, manifest, decl, platform)? else {
        return bake_scene_for(assets_root, &decl.file, target);
    };
    let lut = trino_core::Lut::parse(&lut).map_err(|e| format!("{}: {e:?}", decl.file))?;
    let project = assets_root.parent().unwrap_or(Path::new(""));
    let mut scene = trino_scene::Scene::load(&project.join(&decl.file))?;
    scene.environment = scene.environment.graded(&lut);
    let mut lib = trino_scene::Library::load(project);
    if let Some(e) = lib.errors.first() {
        return Err(format!("scene `{}`: library error: {e}", decl.file));
    }
    lib.model_bounds = manifest_model_bounds(assets_root);
    lib.impostors = crate::impostor::baked(assets_root);
    let (blob, warnings) = trino_scene::bake::to_tscn_for(&scene, &lib, target.into());
    let file = &decl.file;
    if let Some(w) = warnings.first() {
        return Err(format!("scene `{file}`: {w}"));
    }
    Ok(blob)
}

/// The LUT a console bake grades scene `decl` through: `bake_grading` on,
/// a console, and the scene's `post.grading` (an error when it names
/// none).
pub fn scene_grading(
    assets_root: &Path,
    manifest: &crate::Manifest,
    decl: &crate::manifest::BlobDecl,
    platform: crate::Platform,
) -> Result<Option<Vec<u8>>, String> {
    if !decl.bake_grading || !matches!(platform, crate::Platform::N64 | crate::Platform::N3ds) {
        return Ok(None);
    }
    let project = assets_root.parent().unwrap_or(Path::new(""));
    let scene = trino_scene::Scene::load(&project.join(&decl.file))?;
    let grading = scene
        .environment
        .post
        .as_ref()
        .and_then(|p| p.grading.clone())
        .ok_or_else(|| {
            format!(
                "scene `{}`: bake_grading needs `environment.post.grading` (a `luts/<name>`)",
                decl.file
            )
        })?;
    manifest.lut_blob(assets_root, platform, &grading).map(Some)
}

/// Bind-pose AABB of a baked TMDL blob.
pub fn tmdl_bounds(blob: &[u8]) -> Option<([f32; 3], [f32; 3])> {
    let (min, max) = trino_core::render3d::Mesh::from_tmdl(blob).ok()?.aabb()?;
    Some(([min.x, min.y, min.z], [max.x, max.y, max.z]))
}

/// Bind-pose bounding box `(min, max)` of every model in
/// `<assets_root>/manifest.toml`, keyed by logical path (`models/<name>`)
/// — what `Auto` scene colliders use. Models that fail to bake are skipped
/// (the bake itself reports them).
pub fn manifest_model_bounds(
    assets_root: &Path,
) -> std::collections::BTreeMap<String, ([f32; 3], [f32; 3])> {
    let mut out = std::collections::BTreeMap::new();
    let Ok(manifest) = crate::Manifest::load(&assets_root.join("manifest.toml")) else {
        return out;
    };
    for (name, decl) in &manifest.models {
        let Ok(source) = crate::resolve_source(assets_root, crate::Platform::Pc, &decl.file) else {
            continue;
        };
        let Ok(blob) = bake_model(&source, &decl.options()) else {
            continue;
        };
        if let Some(b) = tmdl_bounds(&blob) {
            out.insert(format!("models/{name}"), b);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_order_follows_first_use() {
        // Two quads whose triangles were exported interleaved (q0a, q1a,
        // q0b, q1b) with scattered vertex numbers.
        let mut m = MeshData {
            positions: (0..8).map(|i| [i as f32, 0.0, 0.0]).collect(),
            normals: vec![[0.0, 1.0, 0.0]; 8],
            colors: (0..8).map(|i| [i as u8, 0, 0, 255]).collect(),
            indices: vec![0, 4, 2, 1, 5, 3, 0, 2, 6, 1, 3, 7],
            ..MeshData::default()
        };
        let before: Vec<[f32; 3]> = m.indices.iter().map(|&i| m.positions[i as usize]).collect();
        m.optimize_vertex_order();
        // Each quad's triangles are now adjacent, numbered in first-use order.
        assert_eq!(m.indices, vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
        // Same triangles (and windings), same per-vertex data.
        let after: Vec<[f32; 3]> = m.indices.iter().map(|&i| m.positions[i as usize]).collect();
        let mut tb: Vec<_> = before.chunks(3).map(|t| format!("{t:?}")).collect();
        let mut ta: Vec<_> = after.chunks(3).map(|t| format!("{t:?}")).collect();
        tb.sort();
        ta.sort();
        assert_eq!(ta, tb);
        for (p, c) in m.positions.iter().zip(&m.colors) {
            assert_eq!(p[0] as u8, c[0]);
        }
        // An already-local mesh is left exactly as it was.
        let local = m.clone();
        m.optimize_vertex_order();
        assert_eq!(m.indices, local.indices);
        assert_eq!(m.positions, local.positions);
    }

    #[test]
    fn color0_is_linear_and_round_trips_every_byte() {
        // What Blender writes for a color painted as sRGB (200, 120, 80):
        // linear values, quantized to 16-bit normalized.
        let blender = [
            37_852.0 / 65535.0,
            12_309.0 / 65535.0,
            5_257.0 / 65535.0,
            1.0,
        ];
        assert_eq!(color0_to_display(blender), [200, 120, 80, 255]);
        // Every display byte survives sRGB -> linear f32 -> display (what
        // the generators in xtask write and the bake reads).
        for b in 0..=255u8 {
            let c = b as f64 / 255.0;
            let l = if c <= 0.040_45 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            } as f32;
            let a = b as f32 / 255.0;
            assert_eq!(color0_to_display([l, l, l, a]), [b, b, b, b], "byte {b}");
        }
    }

    #[test]
    fn textured_gltf_keeps_texcoords() {
        // A real-world textured asset (KayKit, CC0): with `texture` the
        // bake keeps TEXCOORD_0 instead of sampling the atlas into colors.
        let door = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/shared/models/castle64/door_kaykit.glb");
        let opts = ModelOptions {
            texture: Some("sprites/brick".into()),
            ..ModelOptions::default()
        };
        let blob = bake_model(&door, &opts).unwrap();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        assert!(mesh.has_uvs());
        assert_eq!(
            mesh.default_texture(),
            Some(trino_core::SpriteId::from_path("sprites/brick"))
        );
        let distinct = (0..mesh.vertex_count)
            .map(|i| mesh.uv(i))
            .filter(|uv| uv.x != 0.0 || uv.y != 0.0)
            .count();
        assert!(distinct > mesh.vertex_count / 2, "UVs were read");
        // Untextured bake of the same file: no UV section at all.
        let plain = bake_model(&door, &ModelOptions::default()).unwrap();
        assert!(!Mesh::from_tmdl(&plain).unwrap().has_uvs());
        assert!(plain.len() < blob.len());
    }

    #[test]
    fn affine_inverse_undoes_the_matrix() {
        let trs = Trs {
            t: [1.0, 2.0, -3.0],
            r: normalize_quat([0.3, -0.2, 0.5, 0.8]),
            s: [2.0, 1.0, 0.5],
        };
        let m = trs.matrix();
        let id = mat_mul(m, affine_inverse(&m));
        for c in 0..4 {
            for r in 0..4 {
                assert!((id[c][r] - IDENTITY[c][r]).abs() < 1e-4, "{id:?}");
            }
        }
    }

    #[test]
    fn trs_matrix_round_trips() {
        let trs = Trs {
            t: [1.0, 2.0, -3.0],
            r: normalize_quat([0.3, -0.2, 0.5, 0.8]),
            s: [2.0, 1.5, 0.5],
        };
        let back = Trs::from_matrix(&trs.matrix());
        for i in 0..3 {
            assert!((back.t[i] - trs.t[i]).abs() < 1e-4);
            assert!((back.s[i] - trs.s[i]).abs() < 1e-4);
        }
        let dot: f32 = (0..4).map(|i| back.r[i] * trs.r[i]).sum();
        assert!(dot.abs() > 0.9999, "{back:?} vs {trs:?}");
    }

    #[test]
    fn engine_trs_matches_pipeline_trs() {
        // The core's Mat34::from_trs (row-major) must agree with the bake's
        // column-major matrix — they describe the same joints.
        let trs = Trs {
            t: [1.0, 2.0, -3.0],
            r: normalize_quat([0.3, -0.2, 0.5, 0.8]),
            s: [2.0, 1.5, 0.5],
        };
        let a = trs.matrix();
        let b = Mat34::from_trs(
            Vec3::new(trs.t[0], trs.t[1], trs.t[2]),
            trino_core::Quat::new(trs.r[0], trs.r[1], trs.r[2], trs.r[3]),
            Vec3::new(trs.s[0], trs.s[1], trs.s[2]),
        );
        for (r, row) in b.m.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                assert!((a[c][r] - v).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn encode_appends_bounds_covering_the_mesh() {
        let data = crate::primitive::Primitive::default().build();
        let blob = data.encode().unwrap();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        let (c, r) = mesh.bounds().unwrap();
        for i in 0..mesh.vertex_count {
            let d = mesh.position(i) - c;
            assert!((d.x * d.x + d.y * d.y + d.z * d.z).sqrt() <= r);
        }
    }
}
