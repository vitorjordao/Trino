//! RSP T&L (`shim/rsp_trino.S`): the CPU side. A mesh the overlay can draw
//! ([`trino_core::render3d::rsp_mesh_supported`]) gets an RSP copy at
//! register — segments of at most `RSP_SEG_VERTS` vertices in uncached
//! RDRAM — and an eligible draw ([`trino_core::render3d::rsp_eligible`])
//! becomes one rspq command carrying its clip-space matrix, light and tint.
//! The layouts here mirror the overlay's; keep them in pairs.

use alloc::vec::Vec;
use core::ffi::c_void;

use trino_core::render3d::{Mesh, NEAR_PLANE, RSP_MAX_SEGS, RspDraw, TnlContext, rsp_split};
use trino_core::{Color, Mat34, Vec3};

use crate::ffi;

/// Bytes per vertex in an RSP segment (overlay `VTX_IN_SIZE`).
const VTX_IN: usize = 32;
/// Words of the draw command (overlay `DRAW_CMD_SIZE` / 4).
pub const DRAW_WORDS: usize = 27;

/// A mesh's RSP copy: its segment table in uncached RDRAM.
pub struct RspMesh {
    table: u32,
    segs: u16,
    /// The uncached block (freed only for runtime batches, never while the
    /// RSP may still read it: see `N64Renderer::retire`).
    base: *mut u8,
    /// Local vertex order of segment 0 (global indices), for the oracle.
    pub seg0: Vec<u16>,
}

/// s15.16 of `v`, split into its integer and fractional halves.
fn fixed(v: f32) -> (i16, u16) {
    let x = (v * 65536.0) as i32;
    ((x >> 16) as i16, (x & 0xFFFF) as u16)
}

/// s0.15 of `v` (-1..1).
fn s015(v: f32) -> i16 {
    let x = (v * 32767.0) as i32;
    x.clamp(-32767, 32767) as i16
}

struct Segment {
    verts: Vec<u16>,
    tris: Vec<[u8; 3]>,
}

/// The core's split ([`rsp_split`]), collected.
fn segments(mesh: &Mesh) -> Vec<Segment> {
    let mut out = Vec::new();
    rsp_split(mesh, &mut |verts, tris| {
        out.push(Segment {
            verts: verts.to_vec(),
            tris: tris.to_vec(),
        })
    });
    out
}

/// Bake a mesh's RSP copy (`None` if it has too many segments).
pub fn build(mesh: &Mesh) -> Option<RspMesh> {
    let segs = segments(mesh);
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
        for &g in &s.verts {
            let g = g as usize;
            let (p, nrm, c) = (mesh.position(g), mesh.normal(g), mesh.color(g));
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
        base,
        seg0: segs[0].verts.clone(),
    })
}

impl RspMesh {
    /// Free the uncached block.
    ///
    /// # Safety
    /// Nothing the RSP still has queued may read it (retire it for two
    /// frames first: `N64Renderer::retire`).
    pub unsafe fn free(self) {
        unsafe { ffi::trino_free_uncached(self.base as *mut c_void) };
    }
}

/// The clip-space matrix the overlay applies: x and y normalized to the
/// screen (|x| <= 2w is on screen), z = view depth - NEAR, w = view depth / 2,
/// so z / 2w = 1 - NEAR / depth like the CPU path's z-buffer value. The
/// half on w: the RSP's 32-bit reciprocal (vrcph/vrcpl) of an s15.16 value
/// comes out as 1 / 2w in s15.16 (measured by the oracle).
pub fn clip_matrix(ctx: &TnlContext, model: &Mat34) -> [[f32; 4]; 4] {
    let (view, focal, screen) = ctx.projection();
    let mv = view.mul(model).m;
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

/// The 27-word draw command (overlay `DRAW` layout).
pub fn draw_words(
    ctx: &TnlContext,
    model: &Mat34,
    d: &RspDraw,
    tint: Color,
    mesh: &RspMesh,
    cull: u32,
    debug_phys: u32,
) -> [u32; DRAW_WORDS] {
    let m = clip_matrix(ctx, model);
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
    let t = |c: u8| s015(c as f32 / 255.0);
    w[22] = pair(t(tint.r), t(tint.g));
    w[23] = pair(t(tint.b), 0);
    w[24] = pair(s015(d.ambient), s015(1.0 - d.ambient));
    w[25] = pair(t(tint.a), 0);
    w[26] = debug_phys;
    w
}
