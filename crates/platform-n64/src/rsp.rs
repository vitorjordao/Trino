//! RSP T&L (`shim/rsp_trino.S`): the CPU side. A mesh the overlay can draw
//! ([`trino_core::render3d::rsp_mesh_supported`]) gets an RSP copy at
//! register — segments of at most `RSP_SEG_VERTS` vertices in uncached
//! RDRAM — and an eligible draw ([`trino_core::render3d::rsp_eligible`])
//! becomes one rspq command carrying its clip-space matrix, light and tint.
//! The layouts here mirror the overlay's; keep them in pairs.

use alloc::vec::Vec;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use trino_core::mem::Blocks;

use trino_core::math3d::to_i32;
use trino_core::render3d::{
    Mesh, NEAR_PLANE, RING_SEGMENTS, RING_VERTS, RSP_FLAT_SEG_TRIS, RSP_MAX_SEGS, Ring, RspColor,
    RspDraw, TnlContext, WaterRsp, face_normal, flat_normal, rsp_part_split, rsp_split,
};
use trino_core::water::RIPPLE_WIDTH;
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
/// Bit in a queued command's word 0 (GitHub #122): a palette draw. Its
/// colored or textured entry is followed by one more, the
/// [`palette_entry`], and the shim sends the overlay's `TrinoCmd_DrawPal`
/// (`trino_rsp_draw_palette`). The shim drops it.
pub const PALETTE: u32 = 0x400;
/// Words the palette command adds after the colored or textured words
/// (overlay `PAL_SIZE` / 4): five entries of four halves, slot 0's white
/// first.
pub const PALETTE_WORDS: usize = 10;
/// The draw command's word the overlay reads as "recolor the slots" (its
/// `DRAW` +0x0C, zero in every other command).
const PALETTE_FLAG: usize = 3;

/// The RSP copies' own region of uncached RDRAM (GitHub #110): one block
/// taken at boot ([`reserve_arena`]), handed out by a free list, so the
/// copies' holes are kept for each other instead of fragmenting the heap
/// between short-lived buffers. A copy that doesn't fit takes the heap
/// (`malloc_uncached`), as every copy did before.
struct Arena {
    /// KSEG1 address of the region.
    base: usize,
    blocks: Blocks<ARENA_RANGES>,
}

/// Free ranges the arena tracks (a free that needs more gives its block up:
/// `Blocks::lost`).
const ARENA_RANGES: usize = 96;

// The N64 runs one thread: the arena is only touched from the renderer's
// register/free paths (never from an interrupt).
static mut ARENA: Option<Arena> = None;

/// Bytes of RSP copies alive now and at most since boot (arena and heap),
/// for the `TRINO_HEAP` lines.
static LIVE: AtomicU32 = AtomicU32::new(0);
static PEAK: AtomicU32 = AtomicU32::new(0);

fn arena() -> &'static mut Option<Arena> {
    // SAFETY: one thread, no reentrancy (see `ARENA`); every borrow ends
    // before the next call.
    unsafe { &mut *core::ptr::addr_of_mut!(ARENA) }
}

/// Take `bytes` of uncached RDRAM for the RSP copies, before any mesh is
/// registered (later copies keep the heap). `false` when the heap has no
/// such block or an arena exists already.
pub fn reserve_arena(bytes: u32) -> bool {
    let slot = arena();
    if slot.is_some() || bytes == 0 {
        return false;
    }
    let p = unsafe { ffi::trino_alloc_uncached(bytes) };
    if p.is_null() {
        return false;
    }
    *slot = Some(Arena {
        base: p as usize,
        blocks: Blocks::new(bytes),
    });
    true
}

/// What the RSP copies hold: (bytes alive, the most alive since boot, the
/// arena's size, its bytes in use, its largest free block).
pub fn copy_stats() -> [u32; 5] {
    let (size, used, largest) = arena().as_ref().map_or((0, 0, 0), |a| {
        (a.blocks.size(), a.blocks.used(), a.blocks.largest_free())
    });
    [
        LIVE.load(Ordering::Relaxed),
        PEAK.load(Ordering::Relaxed),
        size,
        used,
        largest,
    ]
}

/// `bytes` of uncached RDRAM for a copy: the arena's, else the heap's.
fn alloc_copy(bytes: u32) -> *mut u8 {
    let from_arena = arena().as_mut().and_then(|a| {
        a.blocks
            .alloc(bytes)
            .map(|off| (a.base + off as usize) as *mut u8)
    });
    let p = from_arena.unwrap_or_else(|| unsafe { ffi::trino_alloc_uncached(bytes) } as *mut u8);
    if !p.is_null() {
        let live = LIVE.load(Ordering::Relaxed) + bytes;
        LIVE.store(live, Ordering::Relaxed);
        PEAK.store(PEAK.load(Ordering::Relaxed).max(live), Ordering::Relaxed);
    }
    p
}

/// Give a copy's block back where it came from.
fn free_copy(p: *mut u8, bytes: u32) {
    LIVE.store(LIVE.load(Ordering::Relaxed) - bytes, Ordering::Relaxed);
    if let Some(a) = arena().as_mut() {
        let off = (p as usize).wrapping_sub(a.base);
        if (p as usize) >= a.base && a.blocks.holds(off as u32, bytes) {
            a.blocks.free(off as u32, bytes);
            return;
        }
    }
    unsafe { ffi::trino_free_uncached(p as *mut c_void) }
}

/// A mesh's RSP copy: its segment table in uncached RDRAM.
pub struct RspMesh {
    table: u32,
    /// Bytes of its block (to give it back to the arena).
    bytes: u32,
    segs: u16,
    /// Triangles in all segments.
    tris: u32,
    /// 40-byte vertices with UVs: drawn by `rsp_trino_tex`.
    pub textured: bool,
    /// Local vertex order of segment 0 (global indices), for the oracle.
    pub seg0: Vec<u16>,
}

impl RspMesh {
    /// Triangles the copy draws.
    pub fn tris(&self) -> u32 {
        self.tris
    }

    /// Give its uncached RDRAM back (the renderer's grave calls it once no
    /// frame in flight can read it). `table` is the block's physical
    /// address: KSEG1 (`| 0xA000_0000`) is the pointer `malloc_uncached`
    /// returned.
    pub fn free(self) {
        free_copy((self.table | 0xA000_0000) as *mut u8, self.bytes)
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
    let base = alloc_copy(total as u32);
    if base.is_null() {
        return None;
    }
    let phys = unsafe { ffi::trino_phys_addr(base as *const c_void) };
    // Uncached memory: word writes, no cache writeback needed.
    let put = |at: usize, bytes: &[u8]| {
        for (k, w) in bytes.as_chunks::<4>().0.iter().enumerate() {
            unsafe {
                (base.add(at) as *mut u32)
                    .add(k)
                    .write_volatile(u32::from_be_bytes(*w))
            };
        }
    };
    // Staged one segment at a time (a few KB): a whole-copy staging `Vec`
    // asked the heap for a second block as big as the copy (45 KB for the
    // micro scenes' flat copy) while the copy was held (GitHub #110).
    let mut table = alloc::vec![0u8; table_bytes];
    let mut blob: Vec<u8> = Vec::with_capacity(segs.iter().map(seg_bytes).max().unwrap_or(0));
    let mut seg_at = table_bytes;
    for (n, s) in segs.iter().enumerate() {
        let entry = n * 16;
        table[entry..entry + 4].copy_from_slice(&(phys + seg_at as u32).to_be_bytes());
        table[entry + 4..entry + 6].copy_from_slice(&(seg_bytes(s) as u16).to_be_bytes());
        table[entry + 6..entry + 8].copy_from_slice(&(s.verts.len() as u16).to_be_bytes());
        table[entry + 8..entry + 10].copy_from_slice(&(s.tris.len() as u16).to_be_bytes());
        blob.clear();
        blob.resize(seg_bytes(s), 0);
        let mut at = 0;
        for (vi, (&g, &nrm)) in s.verts.iter().zip(&s.normals).enumerate() {
            let g = g as usize;
            let (p, c) = (mesh.position(g), mesh.color(g));
            // The normal row's fourth half: the vertex's color slot as an
            // offset into the palette command's table (8 bytes an entry,
            // slot 0 white). The light's fourth lane is zero, so `n · l`
            // never reads it; zero without slots, as before.
            let slot = (mesh.slot(g) as u16) * 8;
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
                slot,
            ];
            for (k, v) in halves.iter().enumerate() {
                blob[at + k * 2..at + k * 2 + 2].copy_from_slice(&v.to_be_bytes());
            }
            blob[at + 24..at + 28].copy_from_slice(&[c.r, c.g, c.b, c.a]);
            // The pad: the next vertex's color, so the overlay's paired
            // loop loads both colors of a pair at once (`luv`).
            if let Some(&n) = s.verts.get(vi + 1) {
                let c = mesh.color(n as usize);
                blob[at + 28..at + 32].copy_from_slice(&[c.r, c.g, c.b, c.a]);
            }
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
        put(seg_at, &blob);
        seg_at += blob.len();
    }
    put(0, &table);
    Some(RspMesh {
        table: phys,
        bytes: total as u32,
        segs: segs.len() as u16,
        tris: segs.iter().map(|s| s.tris.len() as u32).sum(),
        textured,
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
    let seg = (mesh.table, mesh.segs as u32, mesh.textured);
    draw_words_at(ctx, mv, d, tint, seg, cull, debug_phys)
}

/// [`draw_words`] for segments that aren't an [`RspMesh`]: `seg` is the
/// segment table's RSP address, its segment count and whether its
/// vertices are textured (a ripple ring's, [`ring_segment`]).
pub fn draw_words_at(
    ctx: &TnlContext,
    mv: &Mat34,
    d: &RspDraw,
    tint: Color,
    seg: (u32, u32, bool),
    cull: u32,
    debug_phys: u32,
) -> [u32; DRAW_WORDS] {
    let m = clip_matrix_mv(ctx, mv);
    let mut w = [0u32; DRAW_WORDS];
    w[0] = cull;
    w[1] = seg.0;
    // The RDP triangle: shade + z (0xCD), or shade + texture + z (0xCF).
    w[2] = (seg.1 << 16) | if seg.2 { 0xCF00 } else { 0xCD00 };
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
/// fog: `(depth - near) / (far - near)` as `w FS + FO` of the clip w
/// (half the view depth, [`clip_matrix_mv`]): `FS = 2 / (far - near)` and
/// `FO = -near / (far - near)` (s15.16: integer, fraction each), two
/// unused words, and its color. Without fog both are zero, and so is the
/// blend. (`_mv`: the draw's `view · model`, which the factor no longer
/// needs.)
#[cold]
#[inline(never)]
pub fn color_words(c: &RspColor, _mv: &Mat34) -> [u32; COLOR_WORDS] {
    let pair = |a: i16, b: i16| ((a as u16 as u32) << 16) | b as u16 as u32;
    let byte = |v: u8| ((v as u16) << 7) as i16;
    let e = |i: usize| byte(c.emissive[i]);
    let f = c.fog;
    let (fs, fo, fog) = match f.on {
        true => {
            let s = 1.0 / (f.far - f.near);
            (fixed(2.0 * s), fixed(-f.near * s), f.color)
        }
        false => ((0, 0), (0, 0), [0; 3]),
    };
    [
        0,
        pair(s015(c.ambient[0]), s015(c.ambient[1])),
        pair(s015(c.ambient[2]), 0),
        pair(s015(c.diffuse[0]), s015(c.diffuse[1])),
        pair(s015(c.diffuse[2]), 0),
        pair(e(0), e(1)),
        pair(e(2), 0),
        pair(fs.0, fs.1 as i16),
        pair(fo.0, fo.1 as i16),
        0,
        0,
        pair(byte(fog[0]), byte(fog[1])),
        pair(byte(fog[2]), 0),
    ]
}

/// Turn a colored or textured command (`w`, its entry made) into a
/// palette command: the overlay's flag word, the queue bit.
pub fn mark_palette(w: &mut [u32; DRAW_WORDS]) {
    w[0] |= PALETTE;
    w[PALETTE_FLAG] = 1;
}

/// The palette command's table (overlay `PAL`, after the colored or
/// textured words) in a queue entry: entry `s` multiplies a vertex in slot
/// `s` (entry 0 white, then `p[0..4]` for slots 1..=4). Each channel is
/// `255 · 257` scaled (`c · 257`, a u16 the overlay multiplies a `u8 << 7`
/// color by with `vmudl`, then rounds to a whole u8): the CPU's
/// `(c · p + 127) / 255` within one unit (exact for 65 512 of the 65 536
/// pairs). Alpha keeps 65 535, which leaves it unchanged.
#[cold]
#[inline(never)]
pub fn palette_entry(p: &[Color; 4]) -> [u32; DRAW_WORDS] {
    // Slot 0's white, then two words for each of the four entries.
    const _: () = assert!(2 + 2 * 4 == PALETTE_WORDS);
    let half = |v: u8| v as u32 * 257;
    let mut e = [0u32; DRAW_WORDS];
    e[0] = 0xFFFF_FFFF;
    e[1] = 0xFFFF_FFFF;
    for (k, c) in p.iter().enumerate() {
        e[2 + 2 * k] = (half(c.r) << 16) | half(c.g);
        e[3 + 2 * k] = (half(c.b) << 16) | 0xFFFF;
    }
    e
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
pub const WATER_BLOCK: usize = 0x240;
/// Where a water block's per-frame part starts: the swell's phases, then
/// the ripples and their count ([`patch_water`]).
pub const WATER_FRAME_PART: usize = 0x180;
/// Where a water entry keeps the water block's address (after the color
/// and texture words; the next word is unused) and then its binding: the
/// sprite, the detail sprite (0: none), the `trino_3d_water` flags and
/// scroll.
pub const WATER_PTRS: usize = TEX_BIND;
pub const WATER_BIND: usize = TEX_BIND + 2;

/// The water overlay's per-draw block (WBLK, big-endian halves): see
/// `rsp_trino.S`. The overlay transforms two vertices per pass, so every
/// row of four lanes is written twice (lanes 4..7 repeat 0..3). `model` is
/// the draw's model matrix, `w` its [`WaterRsp`] terms; fog per the
/// context. A ring draw's block (`ring`: [`ring_segment`]s in world space)
/// keeps the vertex colors and has no ripples. The swell's phases and the
/// ripples are this frame's: [`patch_water`] rewrites them.
#[cold]
#[inline(never)]
pub fn water_block(ctx: &TnlContext, model: &Mat34, w: &WaterRsp, ring: bool) -> [u8; WATER_BLOCK] {
    let mut b = [0u8; WATER_BLOCK];
    // Lane `lane` (0..4) of the row at `at`, in both halves.
    let mut put = |at: usize, lane: usize, v: i16| {
        for half in [0, 8] {
            let o = at + half + lane * 2;
            b[o..o + 2].copy_from_slice(&v.to_be_bytes());
        }
    };
    // The world x and z rows of each column (lanes 0, 1).
    for c in 0..4 {
        for (lane, r) in [0, 2].into_iter().enumerate() {
            let (i, f) = fixed(model.m[r][c]);
            put(c * 0x20, lane, i);
            put(c * 0x20 + 0x10, lane, f as i16);
        }
    }
    // UP: what one world unit of height adds to clip x, y, z, w.
    let (view, _, _) = ctx.projection();
    let cm = clip_matrix_mv(ctx, &view);
    for (lane, row) in cm.iter().enumerate() {
        let (i, f) = fixed(row[1]);
        put(0x80, lane, i);
        put(0x90, lane, f as i16);
    }
    // The swell's phase terms: lanes sin0, cos0, sin1, cos1.
    let wv = &w.waves;
    for k in 0..4 {
        let i = k / 2;
        let ax = fixed(wv.k[i] * wv.dir[i].x);
        let bz = fixed(wv.k[i] * wv.dir[i].y);
        put(0xA0, k, ax.0);
        put(0xB0, k, ax.1 as i16);
        put(0xC0, k, bz.0);
        put(0xD0, k, bz.1 as i16);
    }
    // Height per sine (lanes 0, 2), half the gradient per cosine (1, 3).
    let tau = core::f32::consts::TAU;
    for i in 0..2 {
        let g = wv.amp[i] * tau * wv.k[i] * 0.5;
        put(0xE0, 2 * i, s015(wv.amp[i]));
        put(0xF0, 2 * i + 1, s015(g * wv.dir[i].x));
        put(0x100, 2 * i + 1, s015(g * wv.dir[i].y));
    }
    // a, b from the screen math (lanes 0, 1), the glint flags (2, 3) and,
    // in the second half, the ring flag (lane 6).
    let (_, focal, screen) = ctx.projection();
    let (fx, fy) = (focal / (screen.x * 0.5), focal / (screen.y * 0.5));
    put(0x110, 0, to_i32(8192.0 / fx) as i16);
    put(0x110, 1, to_i32(-8192.0 / fy) as i16);
    put(0x110, 2, (w.gloss > 0) as i16);
    put(0x110, 3, (w.gloss > 128) as i16);
    // The view's rotation columns, the sun in view.
    for c in 0..3 {
        for r in 0..3 {
            put(0x120 + c * 0x10, r, s015(view.m[r][c]));
        }
    }
    let sun = view.transform_dir(w.sun);
    put(0x150, 0, s015(sun.x));
    put(0x150, 1, s015(sun.y));
    put(0x150, 2, s015(sun.z));
    let byte = |v: f32| ((v.clamp(0.0, 255.0) as u16) << 7) as i16;
    for i in 0..3 {
        put(0x160, i, byte(w.sky[i]));
        put(0x170, i, byte(w.sun_color[i] * w.gloss as f32 / 255.0));
    }
    put(0x160, 3, byte(255.0));
    b[0x11C..0x11E].copy_from_slice(&(ring as i16).to_be_bytes());
    b[0x11E..0x120].fill(0);
    patch_water(&mut b, w, !ring);
    b
}

/// Rewrite the parts of a water block made by [`water_block`] that change
/// while the camera, the light and the draw stay (from
/// [`WATER_FRAME_PART`]): the swell's phases and (`ripples`, not for a
/// ring) the ripples, one per lane: x, z (s15.16), 128 × radius (s15.16),
/// strength × response × decay (s0.15) and each band's d² bounds, as
/// `water::ripple_sample` reads them; then their count.
pub fn patch_water(b: &mut [u8; WATER_BLOCK], w: &WaterRsp, ripples: bool) {
    let mut put = |at: usize, lane: usize, v: i16| {
        for half in [0, 8] {
            let o = at + half + lane * 2;
            b[o..o + 2].copy_from_slice(&v.to_be_bytes());
        }
    };
    for k in 0..4 {
        let quarter = if k % 2 == 1 { 0.25 } else { 0.0 };
        let c = fixed(quarter - w.waves.phase[k / 2]);
        put(0x180, k, c.0);
        put(0x190, k, c.1 as i16);
    }
    let n = if ripples { w.n_ripples } else { 0 };
    for (lane, r) in w.ripples[..n].iter().enumerate() {
        let (xi, xf) = fixed(r.pos.x);
        let (zi, zf) = fixed(r.pos.y);
        let (ri, rf) = fixed(128.0 * r.radius());
        put(0x1A0, lane, xi);
        put(0x1B0, lane, xf as i16);
        put(0x1C0, lane, zi);
        put(0x1D0, lane, zf as i16);
        put(0x1E0, lane, ri);
        put(0x1F0, lane, rf as i16);
        put(0x200, lane, s015(r.strength * w.response * r.decay()));
        // The band d^2 can touch, as integers rounded outward: the overlay
        // skips a pair of vertices whose d^2 lies outside every one.
        let rad = r.radius();
        let (outer, inner) = (rad + RIPPLE_WIDTH, rad - RIPPLE_WIDTH);
        let bound = |v: f32| to_i32(v).clamp(-1, 32766) as i16;
        put(0x210, lane, bound(outer * outer) + 1);
        put(
            0x220,
            lane,
            if inner > 0.0 {
                bound(inner * inner)
            } else {
                -1
            },
        );
    }
    for lane in n..4 {
        for row in 0..9 {
            put(0x1A0 + row * 0x10, lane, 0);
        }
    }
    b[0x230..0x232].copy_from_slice(&(n as i16).to_be_bytes());
}

/// Bytes of one ripple ring's segment ([`ring_segment`]): its textured
/// vertices, then the triangles' indices padded to 8.
pub const RING_SEG_BYTES: usize = RING_VERTS * VTX_IN_TEX + (2 * RING_SEGMENTS * 3).div_ceil(8) * 8;

/// The angles of a ring's vertices ([`trino_core::render3d::ring_flat`]'s:
/// `k / RING_SEGMENTS` turns): cosine and sine as s1.15, made once.
pub fn ring_unit() -> [[i32; 2]; RING_SEGMENTS + 1] {
    core::array::from_fn(|k| {
        let t = k as f32 / RING_SEGMENTS as f32;
        [
            to_i32(trino_core::water::cos_turns(t) * 32768.0),
            to_i32(trino_core::water::sin_turns(t) * 32768.0),
        ]
    })
}

/// A ring segment's triangle indices ([`trino_core::render3d::ring_world`]'s
/// order), padded to 8.
const RING_INDICES: [u8; RING_SEG_BYTES - RING_VERTS * VTX_IN_TEX] = {
    let mut t = [0u8; RING_SEG_BYTES - RING_VERTS * VTX_IN_TEX];
    let mut k = 0;
    while k < RING_SEGMENTS {
        let b = 2 * k as u8;
        let at = 6 * k;
        t[at] = b;
        t[at + 1] = b + 3;
        t[at + 2] = b + 1;
        t[at + 3] = b;
        t[at + 4] = b + 2;
        t[at + 5] = b + 3;
        k += 1;
    }
    t
};

/// A ripple ring as one segment of a water draw with a ring block
/// ([`water_block`]): [`trino_core::render3d::ring_flat`]'s vertices in
/// world space, at rest (the overlay adds the swell), in fixed point from
/// the angles `unit` ([`ring_unit`]); each the ring's color with the
/// texel at uv (0.5, 0.5) under it, as the CPU path's ring triangles; then
/// its triangles. Written as words straight into `out` (the cached arena;
/// [`RING_SEG_BYTES`], 8-aligned): this runs per ring per frame.
///
/// # Safety
/// `out` must be valid for [`RING_SEG_BYTES`] bytes and 4-aligned.
pub unsafe fn ring_segment(out: *mut u8, ring: &Ring, unit: &[[i32; 2]; RING_SEGMENTS + 1]) {
    let fx = |v: f32| to_i32(v * 65536.0);
    let (cx, cz) = (fx(ring.ripple.pos.x), fx(ring.ripple.pos.y));
    let y = fx(ring.plane + ring.lift) as u32;
    let radii = [fx(ring.r0) as i64, fx(ring.r1) as i64];
    let c = ring.color;
    let color = u32::from_be_bytes([c.r, c.g, c.b, c.a]);
    let w = out as *mut u32;
    for i in 0..RING_VERTS {
        let [cs, sn] = unit[i / 2];
        let r = radii[i % 2];
        let x = (cx + ((cs as i64 * r) >> 15) as i32) as u32;
        let z = (cz + ((sn as i64 * r) >> 15) as i32) as u32;
        let words = [
            (x & 0xFFFF_0000) | y >> 16,
            (z & 0xFFFF_0000) | 1,
            x << 16 | (y & 0xFFFF),
            z << 16,
            0,
            0,
            color,
            0,
            // u = v = 0.5 (s5.10).
            0x0200_0200,
            0,
        ];
        for (k, v) in words.iter().enumerate() {
            unsafe { w.add(i * VTX_IN_TEX / 4 + k).write(v.to_be()) };
        }
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            RING_INDICES.as_ptr(),
            out.add(RING_VERTS * VTX_IN_TEX),
            RING_INDICES.len(),
        )
    };
}

/// The segment table entry of a [`ring_segment`] at RSP address `phys`.
pub fn ring_entry(phys: u32) -> [u8; 16] {
    let mut e = [0u8; 16];
    e[0..4].copy_from_slice(&phys.to_be_bytes());
    e[4..6].copy_from_slice(&(RING_SEG_BYTES as u16).to_be_bytes());
    e[6..8].copy_from_slice(&(RING_VERTS as u16).to_be_bytes());
    e[8..10].copy_from_slice(&((2 * RING_SEGMENTS) as u16).to_be_bytes());
    e
}
