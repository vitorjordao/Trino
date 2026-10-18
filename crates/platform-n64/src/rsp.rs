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
    Mesh, NEAR_PLANE, RSP_FLAT_SEG_TRIS, RSP_MAX_SEGS, RspColor, RspDraw, TnlContext, WaterRsp,
    face_normal, flat_normal, rsp_part_split, rsp_split,
};
use trino_core::{Color, Mat34, Vec3};

use crate::ffi;

/// Bytes per vertex in an RSP segment (overlay `VTX_IN_SIZE`); textured
/// meshes add their UVs ([`VTX_IN_TEX`], `rsp_trino_tex`).
const VTX_IN: usize = 32;
const VTX_IN_TEX: usize = 40;
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
    /// Triangles in all segments.
    tris: u32,
    /// 40-byte vertices with UVs: drawn by `rsp_trino_tex`.
    pub textured: bool,
    /// Local vertex order of segment 0 (global indices), for the oracle.
    pub seg0: Vec<u16>,
    /// Every segment's vertices in order (global indices): a water draw's
    /// ripple deltas follow it.
    pub order: Vec<u16>,
}

impl RspMesh {
    /// Triangles the copy draws.
    pub fn tris(&self) -> u32 {
        self.tris
    }
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

/// The core's split ([`rsp_split`]; [`rsp_part_split`] for joint `part` of a
/// rigid mesh), collected.
fn segments(mesh: &Mesh, part: Option<usize>) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut emit = |verts: &[u16], tris: &[[u8; 3]]| {
        out.push(Segment {
            verts: verts.to_vec(),
            normals: verts.iter().map(|&g| mesh.normal(g as usize)).collect(),
            tris: tris.to_vec(),
        })
    };
    match part {
        Some(j) => rsp_part_split(mesh, j, &mut emit),
        None => rsp_split(mesh, &mut emit),
    };
    out
}

/// A flat copy's segments: [`RSP_FLAT_SEG_TRIS`] faces each, three vertices
/// per face carrying its normal under `ratios` ([`flat_normal`]), so the overlay's
/// per-vertex lighting is the CPU path's per-face lighting.
fn flat_segments(mesh: &Mesh, part: Option<usize>, ratios: [u16; 3]) -> Vec<Segment> {
    let faces: Vec<[usize; 3]> = (0..mesh.index_count / 3)
        .map(|t| core::array::from_fn(|k| mesh.index(3 * t + k) as usize))
        .filter(|t: &[usize; 3]| part.is_none_or(|j| mesh.joint_of_vertex(t[0]) == j))
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
    bake(mesh, &segments(mesh, None))
}

/// Bake joint `j`'s part of a rigid mesh (`rsp_rigid_check`): its triangles
/// in bind pose, drawn with the joint's matrix (`None`: no triangles).
#[cold]
#[inline(never)]
pub fn build_part(mesh: &Mesh, j: usize) -> Option<RspMesh> {
    bake(mesh, &segments(mesh, Some(j)))
}

/// Bake a mesh's flat RSP copy for its `Shading::Flat` draws under scale
/// `ratios` ([`flat_ratios`]; `None` past [`RSP_MAX_SEGS`] segments), or of
/// joint `part`'s triangles for a rigid mesh.
#[cold]
#[inline(never)]
pub fn build_flat(mesh: &Mesh, part: Option<usize>, ratios: [u16; 3]) -> Option<RspMesh> {
    bake(mesh, &flat_segments(mesh, part, ratios))
}

fn bake(mesh: &Mesh, segs: &[Segment]) -> Option<RspMesh> {
    if segs.is_empty() || segs.len() > RSP_MAX_SEGS {
        return None;
    }
    let textured = mesh.has_uvs();
    let stride = if textured { VTX_IN_TEX } else { VTX_IN };
    let seg_bytes = |s: &Segment| (s.verts.len() * stride + s.tris.len() * 3).div_ceil(8) * 8;
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
        // +12: the segment's first vertex in the copy's order (water ripple deltas).
        let first: usize = segs[..n].iter().map(|s| s.verts.len()).sum();
        blob[entry + 12..entry + 16].copy_from_slice(&(first as u32).to_be_bytes());
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
            if textured {
                // s5.10 repeats (rsp_mesh_check keeps them within 32).
                let t = mesh.uv(g);
                let q = |v: f32| (to_i32(v * 1024.0).clamp(-32767, 32767) as i16).to_be_bytes();
                blob[at + 32..at + 34].copy_from_slice(&q(t.x));
                blob[at + 34..at + 36].copy_from_slice(&q(t.y));
            }
            at += stride;
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
        tris: segs.iter().map(|s| s.tris.len() as u32).sum(),
        textured,
        seg0: segs[0].verts.clone(),
        order: segs.iter().flat_map(|s| s.verts.iter().copied()).collect(),
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
    // The RDP triangle: shade + z (0xCD), or shade + texture + z (0xCF).
    w[2] = ((mesh.segs as u32) << 16) | if mesh.textured { 0xCF00 } else { 0xCD00 };
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

/// Words the textured overlay's command adds after the color words (its
/// `DRAW` +0xA0..0xB0): the texture scale and the S/T offset.
pub const TEX_WORDS: usize = 4;
/// Bit in a queued command's word 0: a textured draw (`rsp_trino_tex`),
/// its color and texture words and its binding in the next queue entry
/// ([`tex_entry`]). The shim drops it.
pub const TEXTURED: u32 = 0x200;
/// Where a textured entry keeps the texture binding: the sprite, the
/// detail sprite (0: none) and the `trino_3d_texture_ex` flags.
pub const TEX_BIND: usize = COLOR_WORDS + TEX_WORDS;

/// The textured overlay's extra words: [`color_words`], then S, T per u, v
/// repeat (`size / 32` as s15.16: integer parts, fractional parts; the
/// overlay reads u, v as s5.10 and writes S, T as s10.5) and the S, T
/// offset in texels (s10.5), then `bind` ([`TEX_BIND`]).
#[cold]
#[inline(never)]
pub fn tex_entry(
    c: &RspColor,
    mv: &Mat34,
    size: [f32; 2],
    offset: [f32; 2],
    bind: [u32; 3],
) -> [u32; DRAW_WORDS] {
    let pair = |a: i16, b: i16| ((a as u16 as u32) << 16) | b as u16 as u32;
    let mut e = [0u32; DRAW_WORDS];
    e[..COLOR_WORDS].copy_from_slice(&color_words(c, mv));
    let (s, t) = (fixed(size[0] / 32.0), fixed(size[1] / 32.0));
    let q = |v: f32| to_i32(v * 32.0).clamp(-32768, 32767) as i16;
    e[COLOR_WORDS] = pair(s.0, t.0);
    e[COLOR_WORDS + 1] = pair(s.1 as i16, t.1 as i16);
    e[COLOR_WORDS + 2] = pair(q(offset[0]), q(offset[1]));
    e[TEX_BIND..TEX_BIND + 3].copy_from_slice(&bind);
    e
}

/// Bytes of the water block (`rsp_trino_water`'s WBLK).
pub const WATER_BLOCK: usize = 0xE0;
/// Bytes of one vertex's ripple deltas (`RIP_IN`).
pub const RIPPLE_DELTA: usize = 8;
/// Where a water entry keeps the water block's and the ripple deltas'
/// addresses (after the color and texture words) and then its binding:
/// the sprite, the detail sprite (0: none), the `trino_3d_water` flags and
/// scroll.
pub const WATER_PTRS: usize = TEX_BIND;
pub const WATER_BIND: usize = TEX_BIND + 2;

/// The water overlay's per-draw block (WBLK, big-endian halves): see
/// `rsp_trino.S`. `model` is the draw's model matrix, `w` its
/// [`WaterRsp`] terms; fog per the context.
#[cold]
#[inline(never)]
pub fn water_block(ctx: &TnlContext, model: &Mat34, w: &WaterRsp) -> [u8; WATER_BLOCK] {
    let mut b = [0u8; WATER_BLOCK];
    let mut put = |at: usize, v: i16| b[at..at + 2].copy_from_slice(&v.to_be_bytes());
    // World rows in lanes 5..7 of each column.
    for c in 0..4 {
        for r in 0..3 {
            let (i, f) = fixed(model.m[r][c]);
            put(c * 6 + r * 2, i);
            put(0x18 + c * 6 + r * 2, f as i16);
        }
    }
    // UP: what one world unit of height adds to clip x, y, z, w, the fog
    // factor and world y.
    let (view, _, _) = ctx.projection();
    let cm = clip_matrix_mv(ctx, &view);
    let fog = ctx.light().fog;
    let fog_up = if fog.enabled() {
        view.m[2][1] / (fog.far - fog.near)
    } else {
        0.0
    };
    let up = [
        cm[0][1], cm[1][1], cm[2][1], cm[3][1], fog_up, 0.0, 1.0, 0.0,
    ];
    for (lane, v) in up.iter().enumerate() {
        let (i, f) = fixed(*v);
        put(0x30 + lane * 2, i);
        put(0x40 + lane * 2, f as i16);
    }
    // The swell's phases: lanes sin0, cos0, sin1, cos1.
    let wv = &w.waves;
    let lane_of = |k: usize| k / 2;
    for k in 0..4 {
        let i = lane_of(k);
        let quarter = if k % 2 == 1 { 0.25 } else { 0.0 };
        let ax = fixed(wv.k[i] * wv.dir[i].x);
        let bz = fixed(wv.k[i] * wv.dir[i].y);
        let c = fixed(quarter - wv.phase[i]);
        put(0x50 + k * 2, ax.0);
        put(0x58 + k * 2, ax.1 as i16);
        put(0x60 + k * 2, bz.0);
        put(0x68 + k * 2, bz.1 as i16);
        put(0x70 + k * 2, c.0);
        put(0x78 + k * 2, c.1 as i16);
    }
    // Height per sine (lanes 0, 2), half the gradient per cosine (1, 3).
    let tau = core::f32::consts::TAU;
    for i in 0..2 {
        let g = wv.amp[i] * tau * wv.k[i] * 0.5;
        put(0x80 + i * 4, s015(wv.amp[i]));
        put(0x88 + i * 4 + 2, s015(g * wv.dir[i].x));
        put(0x90 + i * 4 + 2, s015(g * wv.dir[i].y));
    }
    // a, b from the screen math, the view's rotation, the sun in view.
    let (_, focal, screen) = ctx.projection();
    let (fx, fy) = (focal / (screen.x * 0.5), focal / (screen.y * 0.5));
    put(0x98, to_i32(8192.0 / fx) as i16);
    put(0x9A, to_i32(-8192.0 / fy) as i16);
    for c in 0..3 {
        for r in 0..3 {
            put(0xA0 + c * 8 + r * 2, s015(view.m[r][c]));
        }
    }
    let sun = view.transform_dir(w.sun);
    put(0xB8, s015(sun.x));
    put(0xBA, s015(sun.y));
    put(0xBC, s015(sun.z));
    let byte = |v: f32| ((v.clamp(0.0, 255.0) as u16) << 7) as i16;
    for i in 0..3 {
        put(0xC0 + i * 2, byte(w.sky[i]));
        put(0xC8 + i * 2, byte(w.sun_color[i] * w.gloss as f32 / 255.0));
    }
    put(0xC6, byte(255.0));
    put(0xD0, (w.gloss > 0) as i16);
    put(0xD2, (w.gloss > 128) as i16);
    b
}

/// The ripples' height and half gradient at world `(x, z)` (s0.15 each,
/// `RIP_IN`'s layout): what the CPU adds to a water vertex the RSP swells.
#[inline]
pub fn ripple_delta(w: &WaterRsp, x: f32, z: f32) -> [u8; RIPPLE_DELTA] {
    let (h, g) = trino_core::water::ripple_sample(&w.ripples[..w.n_ripples], w.response, x, z);
    let mut d = [0u8; RIPPLE_DELTA];
    d[0..2].copy_from_slice(&s015(h).to_be_bytes());
    d[2..4].copy_from_slice(&s015(g.x * 0.5).to_be_bytes());
    d[4..6].copy_from_slice(&s015(g.y * 0.5).to_be_bytes());
    d
}
