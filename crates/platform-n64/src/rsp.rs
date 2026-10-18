//! RSP T&L (`shim/rsp_trino.S`): the CPU side. A mesh the overlay can draw
//! ([`trino_core::render3d::rsp_mesh_supported`]) gets an RSP copy at
//! register — segments of at most `RSP_SEG_VERTS` vertices in uncached
//! RDRAM — and an eligible draw ([`trino_core::render3d::rsp_eligible`])
//! becomes one rspq command carrying its clip-space matrix, light and tint.
//! The layouts here mirror the overlay's; keep them in pairs.

use alloc::vec::Vec;
use core::ffi::c_void;

use trino_core::math3d::to_i32;
use trino_core::render3d::{
    Mesh, NEAR_PLANE, RSP_FLAT_SEG_TRIS, RSP_MAX_SEGS, RspColor, RspDraw, TnlContext, face_normal,
    flat_normal, rsp_split,
};
use trino_core::{Color, Mat34, Vec3};

use crate::ffi;

/// Bytes per vertex in an RSP segment (overlay `VTX_IN_SIZE`).
const VTX_IN: usize = 32;
/// Words of the draw command (overlay `DRAW_CMD_SIZE` / 4).
pub const DRAW_WORDS: usize = 27;
/// Words the colored overlay's draw command adds (its `DRAW_CMD_SIZE` / 4 -
/// `DRAW_WORDS`): a pad, the ambient, diffuse and emissive vectors, the fog
/// row and the fog color.
pub const COLOR_WORDS: usize = 13;
/// Bit in a queued draw command's word 0 (above the overlay's cull byte):
/// the draw goes out through the colored overlay, with its
/// [`color_words`] in the next queue entry. The shim drops it.
pub const COLORED: u32 = 0x100;

/// A mesh's RSP copy: its segment table in uncached RDRAM.
pub struct RspMesh {
    table: u32,
    segs: u16,
    /// Local vertex order of segment 0 (global indices), for the oracle.
    pub seg0: Vec<u16>,
}

/// s15.16 of `v`, split into its integer and fractional halves.
fn fixed(v: f32) -> (i16, u16) {
    let x = to_i32(v * 65536.0);
    ((x >> 16) as i16, (x & 0xFFFF) as u16)
}

/// s0.15 of `v` (-1..1).
fn s015(v: f32) -> i16 {
    let x = to_i32(v * 32767.0);
    x.clamp(-32767, 32767) as i16
}

/// `s015(c / 255)` for every tint channel, made at compile time with the
/// same float operations (a division per channel per draw otherwise).
const TINT_S015: [i16; 256] = {
    let mut t = [0i16; 256];
    let mut c = 0;
    while c < 256 {
        let x = (c as f32 / 255.0 * 32767.0) as i32;
        t[c] = if x > 32767 { 32767 } else { x as i16 };
        c += 1;
    }
    t
};

struct Segment {
    verts: Vec<u16>,
    /// Each vertex's normal (its mesh normal, or its face's in a flat copy).
    normals: Vec<Vec3>,
    tris: Vec<[u8; 3]>,
}

/// The core's split ([`rsp_split`]), collected.
fn segments(mesh: &Mesh) -> Vec<Segment> {
    let mut out = Vec::new();
    rsp_split(mesh, &mut |verts, tris| {
        out.push(Segment {
            verts: verts.to_vec(),
            normals: verts.iter().map(|&g| mesh.normal(g as usize)).collect(),
            tris: tris.to_vec(),
        })
    });
    out
}

/// A flat copy's segments: [`RSP_FLAT_SEG_TRIS`] faces each, three vertices
/// per face carrying its normal under `ratios` ([`flat_normal`]), so the overlay's
/// per-vertex lighting is the CPU path's per-face lighting.
fn flat_segments(mesh: &Mesh, ratios: [u16; 3]) -> Vec<Segment> {
    let faces: Vec<[usize; 3]> = (0..mesh.index_count / 3)
        .map(|t| core::array::from_fn(|k| mesh.index(3 * t + k) as usize))
        .collect();
    faces
        .chunks(RSP_FLAT_SEG_TRIS)
        .map(|chunk| {
            let mut s = Segment {
                verts: Vec::new(),
                normals: Vec::new(),
                tris: Vec::new(),
            };
            for (k, &t) in chunk.iter().enumerate() {
                let n = flat_normal(face_normal(mesh, t), ratios);
                for &i in &t {
                    s.verts.push(i as u16);
                    s.normals.push(n);
                }
                let b = (3 * k) as u8;
                s.tris.push([b, b + 1, b + 2]);
            }
            s
        })
        .collect()
}

/// Bake a mesh's RSP copy (`None` if it has too many segments).
pub fn build(mesh: &Mesh) -> Option<RspMesh> {
    bake(mesh, &segments(mesh))
}

/// Bake a mesh's flat RSP copy for its `Shading::Flat` draws under scale
/// `ratios` ([`flat_ratios`]; `None` past [`RSP_MAX_SEGS`] segments).
#[cold]
#[inline(never)]
pub fn build_flat(mesh: &Mesh, ratios: [u16; 3]) -> Option<RspMesh> {
    bake(mesh, &flat_segments(mesh, ratios))
}

fn bake(mesh: &Mesh, segs: &[Segment]) -> Option<RspMesh> {
    if segs.is_empty() || segs.len() > RSP_MAX_SEGS {
        return None;
    }
    let seg_bytes = |s: &Segment| (s.verts.len() * VTX_IN + s.tris.len() * 3).div_ceil(8) * 8;
    let table_bytes = segs.len() * 16;
    let total = table_bytes + segs.iter().map(seg_bytes).sum::<usize>();
    let base = unsafe { ffi::trino_alloc_uncached(total as u32) } as *mut u8;
    if base.is_null() {
        return None;
    }
    let phys = unsafe { ffi::trino_phys_addr(base as *const c_void) };
    let mut blob: Vec<u8> = alloc::vec![0u8; total];
    let mut at = table_bytes;
    for (n, s) in segs.iter().enumerate() {
        let entry = n * 16;
        blob[entry..entry + 4].copy_from_slice(&(phys + at as u32).to_be_bytes());
        blob[entry + 4..entry + 6].copy_from_slice(&(seg_bytes(s) as u16).to_be_bytes());
        blob[entry + 6..entry + 8].copy_from_slice(&(s.verts.len() as u16).to_be_bytes());
        blob[entry + 8..entry + 10].copy_from_slice(&(s.tris.len() as u16).to_be_bytes());
        for (&g, &nrm) in s.verts.iter().zip(&s.normals) {
            let g = g as usize;
            let (p, c) = (mesh.position(g), mesh.color(g));
            let (xi, xf) = fixed(p.x);
            let (yi, yf) = fixed(p.y);
            let (zi, zf) = fixed(p.z);
            let halves = [
                xi as u16,
                yi as u16,
                zi as u16,
                1,
                xf,
                yf,
                zf,
                0,
                s015(nrm.x) as u16,
                s015(nrm.y) as u16,
                s015(nrm.z) as u16,
                0,
            ];
            for (k, v) in halves.iter().enumerate() {
                blob[at + k * 2..at + k * 2 + 2].copy_from_slice(&v.to_be_bytes());
            }
            blob[at + 24..at + 28].copy_from_slice(&[c.r, c.g, c.b, c.a]);
            at += VTX_IN;
        }
        for t in &s.tris {
            blob[at..at + 3].copy_from_slice(t);
            at += 3;
        }
        at = at.div_ceil(8) * 8;
    }
    // Uncached memory: word writes, no cache writeback needed.
    for (k, w) in blob.as_chunks::<4>().0.iter().enumerate() {
        unsafe {
            (base as *mut u32)
                .add(k)
                .write_volatile(u32::from_be_bytes(*w))
        };
    }
    Some(RspMesh {
        table: phys,
        segs: segs.len() as u16,
        seg0: segs[0].verts.clone(),
    })
}

/// The clip-space matrix the overlay applies: x and y normalized to the
/// screen (|x| <= 2w is on screen), z = view depth - NEAR, w = view depth / 2,
/// so z / 2w = 1 - NEAR / depth like the CPU path's z-buffer value. The
/// half on w: the RSP's 32-bit reciprocal (vrcph/vrcpl) of an s15.16 value
/// comes out as 1 / 2w in s15.16 (measured by the oracle).
///
/// `mv` is the draw's `view · model` product, which the eligibility test
/// already needed ([`trino_core::render3d::rsp_eligible_mv`]).
pub fn clip_matrix_mv(ctx: &TnlContext, mv: &Mat34) -> [[f32; 4]; 4] {
    let (_, focal, screen) = ctx.projection();
    let mv = mv.m;
    let (cx, cy) = (screen.x * 0.5, screen.y * 0.5);
    let row = |r: [f32; 4], k: f32| [r[0] * k, r[1] * k, r[2] * k, r[3] * k];
    let mut z = mv[2];
    z[3] -= NEAR_PLANE;
    [
        row(mv[0], focal / cx),
        row(mv[1], -focal / cy),
        z,
        row(mv[2], 0.5),
    ]
}

/// Rewrite the translation column of a draw command made by
/// [`draw_words`] for a `view · model` whose translation column is now
/// `mt` (a camera pan: everything else in the command stays). The same
/// operations as [`clip_matrix_mv`] and [`draw_words`] on that column, so
/// the words equal a full rebuild.
pub fn patch_translation(ctx: &TnlContext, w: &mut [u32; DRAW_WORDS], mt: [f32; 3]) {
    let (_, focal, screen) = ctx.projection();
    let (cx, cy) = (screen.x * 0.5, screen.y * 0.5);
    let col = [
        mt[0] * (focal / cx),
        mt[1] * (-focal / cy),
        mt[2] - NEAR_PLANE,
        mt[2] * 0.5,
    ];
    let pair = |a: i16, b: i16| ((a as u16 as u32) << 16) | b as u16 as u32;
    let f: [(i16, u16); 4] = core::array::from_fn(|row| fixed(col[row]));
    w[10] = pair(f[0].0, f[1].0);
    w[11] = pair(f[2].0, f[3].0);
    w[18] = pair(f[0].1 as i16, f[1].1 as i16);
    w[19] = pair(f[2].1 as i16, f[3].1 as i16);
}

/// The 27-word draw command (overlay `DRAW` layout); `mv` is the draw's
/// `view · model` product.
pub fn draw_words(
    ctx: &TnlContext,
    mv: &Mat34,
    d: &RspDraw,
    tint: Color,
    mesh: &RspMesh,
    cull: u32,
    debug_phys: u32,
) -> [u32; DRAW_WORDS] {
    let m = clip_matrix_mv(ctx, mv);
    let mut w = [0u32; DRAW_WORDS];
    w[0] = cull;
    w[1] = mesh.table;
    w[2] = ((mesh.segs as u32) << 16) | 0xCD00;
    let pair = |a: i16, b: i16| ((a as u16 as u32) << 16) | b as u16 as u32;
    for col in 0..4 {
        let f: [(i16, u16); 4] = core::array::from_fn(|row| fixed(m[row][col]));
        w[4 + col * 2] = pair(f[0].0, f[1].0);
        w[5 + col * 2] = pair(f[2].0, f[3].0);
        w[12 + col * 2] = pair(f[0].1 as i16, f[1].1 as i16);
        w[13 + col * 2] = pair(f[2].1 as i16, f[3].1 as i16);
    }
    let l: Vec3 = d.light;
    w[20] = pair(s015(l.x), s015(l.y));
    w[21] = pair(s015(l.z), 0);
    let t = |c: u8| TINT_S015[c as usize];
    w[22] = pair(t(tint.r), t(tint.g));
    w[23] = pair(t(tint.b), 0);
    w[24] = pair(s015(d.ambient), s015(1.0 - d.ambient));
    w[25] = pair(t(tint.a), 0);
    w[26] = debug_phys;
    w
}

/// [`color_words`] in a queue entry (the renderer queues it right after
/// the command).
#[cold]
#[inline(never)]
pub fn color_entry(c: &RspColor, mv: &Mat34) -> [u32; DRAW_WORDS] {
    let mut e = [0u32; DRAW_WORDS];
    e[..COLOR_WORDS].copy_from_slice(&color_words(c, mv));
    e
}

/// The colored overlay's extra words (its `DRAW` +0x6C..0xA0): the
/// per-channel ambient and diffuse terms (s0.15), the emissive color in
/// the overlay's `luv` layout (a byte << 7), each `r, g, b, 0`, then the
/// fog: a fifth clip-matrix row giving `(depth - near) / (far - near)`
/// (s15.16, one entry per column of `mv`, the draw's `view · model`) and
/// its color. Without fog the row is zero, and so is the blend.
#[cold]
#[inline(never)]
pub fn color_words(c: &RspColor, mv: &Mat34) -> [u32; COLOR_WORDS] {
    let pair = |a: i16, b: i16| ((a as u16 as u32) << 16) | b as u16 as u32;
    let byte = |v: u8| ((v as u16) << 7) as i16;
    let e = |i: usize| byte(c.emissive[i]);
    let f = c.fog;
    let (row, fog) = match f.on {
        true => {
            let s = 1.0 / (f.far - f.near);
            let z = mv.m[2];
            let row: [(i16, u16); 4] = [
                fixed(z[0] * s),
                fixed(z[1] * s),
                fixed(z[2] * s),
                fixed((z[3] - f.near) * s),
            ];
            (row, f.color)
        }
        false => ([(0, 0); 4], [0; 3]),
    };
    [
        0,
        pair(s015(c.ambient[0]), s015(c.ambient[1])),
        pair(s015(c.ambient[2]), 0),
        pair(s015(c.diffuse[0]), s015(c.diffuse[1])),
        pair(s015(c.diffuse[2]), 0),
        pair(e(0), e(1)),
        pair(e(2), 0),
        pair(row[0].0, row[1].0),
        pair(row[2].0, row[3].0),
        pair(row[0].1 as i16, row[1].1 as i16),
        pair(row[2].1 as i16, row[3].1 as i16),
        pair(byte(fog[0]), byte(fog[1])),
        pair(byte(fog[2]), 0),
    ]
}
