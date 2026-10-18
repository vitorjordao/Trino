//! Texture formats on the host: what each format keeps of a master, how
//! many bytes it takes, how far it is from the master, and which one a
//! texture gets ([`choose`]: a declared format, or `auto`'s smallest that
//! keeps `quality`/`min_psnr` — the one decision every bake, `texture
//! inspect|report` and the editor share). Per platform: the 3DS below,
//! PC BCn in [`BcFormat`], the N64's `mksprite` model in [`n64`].
//!
//! The 3DS bake hands sprites to `tex3ds` (the `.t3x` the shim loads); this
//! module reproduces the result in Rust so the PC can show it without
//! devkitPro:
//! - [`N3dsFormat::preview`] decodes what the PICA200 samples, which the
//!   PC's 3DS profile draws instead of the master (`TRINO_SIM=3ds` shows
//!   ETC1 blocks and 4-bit banding like Azahar);
//! - [`psnr`] compares a preview with its master (`cargo xtask texture
//!   inspect`);
//! - [`decode_tex3ds_raw`] reads `tex3ds -r` output (8×8 Morton tiles), so
//!   tests and `texture inspect` can check the encoder against the real one.
//!
//! The rules follow `tex3ds` v2.3 (checked against its raw output): 16-bit
//! formats truncate (`>> 3`, `>> 4`), RGBA5551 keeps alpha ≥ 128, a fully
//! transparent texel becomes 0 in every channel, and luminance is
//! Rec. 709 of the linearized color, re-encoded to sRGB. The PICA200
//! expands n-bit channels by bit replication. ETC1 is encoded here (the
//! same block format, a search close to `tex3ds`'s medium quality), so a
//! preview's blocks look like Azahar's without being byte-identical.
//!
//! **Channel semantics** (the same on every target, the shim enforces them):
//! `L8` is gray and opaque, `LA8` gray with alpha, `A8` **white** with
//! alpha — a mask tinted at draw time (fonts, glows). The PICA200 itself
//! samples A8 as black; the shim draws such textures with the vertex or
//! tint color instead.

mod auto;
mod bc;
pub mod n64;

pub use auto::{
    Candidate, Choice, Content, Quality, Source, TextureSettings, candidates, choose, db, formats,
    measure,
};
pub use bc::{BcFormat, bc7_encode_block};
pub use n64::N64Format;

/// A 3DS (PICA200) texture format, as the manifest names it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum N3dsFormat {
    Rgba8,
    Rgb565,
    Rgba5551,
    Rgba4,
    La8,
    L8,
    A8,
    Etc1,
    Etc1a4,
}

impl N3dsFormat {
    pub const ALL: [N3dsFormat; 9] = [
        N3dsFormat::Rgba8,
        N3dsFormat::Rgb565,
        N3dsFormat::Rgba5551,
        N3dsFormat::Rgba4,
        N3dsFormat::La8,
        N3dsFormat::L8,
        N3dsFormat::A8,
        N3dsFormat::Etc1,
        N3dsFormat::Etc1a4,
    ];

    /// The manifest spelling (`formats = { 3ds = "ETC1" }`).
    pub fn name(self) -> &'static str {
        match self {
            N3dsFormat::Rgba8 => "RGBA8",
            N3dsFormat::Rgb565 => "RGB565",
            N3dsFormat::Rgba5551 => "RGBA5551",
            N3dsFormat::Rgba4 => "RGBA4",
            N3dsFormat::La8 => "LA8",
            N3dsFormat::L8 => "L8",
            N3dsFormat::A8 => "A8",
            N3dsFormat::Etc1 => "ETC1",
            N3dsFormat::Etc1a4 => "ETC1A4",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == name)
    }

    /// `tex3ds -f` spelling.
    pub fn tex3ds(self) -> &'static str {
        match self {
            N3dsFormat::Rgba8 => "rgba8",
            N3dsFormat::Rgb565 => "rgb565",
            N3dsFormat::Rgba5551 => "rgba5551",
            N3dsFormat::Rgba4 => "rgba4",
            N3dsFormat::La8 => "la8",
            N3dsFormat::L8 => "l8",
            N3dsFormat::A8 => "a8",
            N3dsFormat::Etc1 => "etc1",
            N3dsFormat::Etc1a4 => "etc1a4",
        }
    }

    pub fn bits_per_texel(self) -> u32 {
        match self {
            N3dsFormat::Rgba8 => 32,
            N3dsFormat::Rgb565 | N3dsFormat::Rgba5551 | N3dsFormat::Rgba4 | N3dsFormat::La8 => 16,
            N3dsFormat::L8 | N3dsFormat::A8 | N3dsFormat::Etc1a4 => 8,
            N3dsFormat::Etc1 => 4,
        }
    }

    /// Does the format keep an alpha channel?
    pub fn has_alpha(self) -> bool {
        !matches!(self, N3dsFormat::Rgb565 | N3dsFormat::L8 | N3dsFormat::Etc1)
    }

    /// Bytes of texture data in linear memory: the PICA200 samples
    /// power-of-two textures of 8..=1024 texels a side, which `tex3ds`
    /// pads the master to.
    pub fn texture_bytes(self, width: u32, height: u32) -> u64 {
        let side = |n: u32| n.max(8).next_power_of_two() as u64;
        side(width) * side(height) * self.bits_per_texel() as u64 / 8
    }

    /// What the PICA200 samples from this format, as RGBA8 (row-major,
    /// `width * height * 4` bytes in, the same out).
    pub fn preview(self, width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        assert_eq!(rgba.len(), (width * height * 4) as usize);
        // tex3ds reads a fully transparent texel as 0 in every channel.
        let src: Vec<[u8; 4]> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                if p[3] == 0 {
                    [0; 4]
                } else {
                    [p[0], p[1], p[2], p[3]]
                }
            })
            .collect();
        let out: Vec<[u8; 4]> = match self {
            N3dsFormat::Rgba8 => src,
            N3dsFormat::Rgb565 => src
                .iter()
                .map(|p| [x5(p[0] >> 3), x6(p[1] >> 2), x5(p[2] >> 3), 255])
                .collect(),
            N3dsFormat::Rgba5551 => src
                .iter()
                .map(|p| {
                    let a = if p[3] >= 128 { 255 } else { 0 };
                    [x5(p[0] >> 3), x5(p[1] >> 3), x5(p[2] >> 3), a]
                })
                .collect(),
            N3dsFormat::Rgba4 => src
                .iter()
                .map(|p| [x4(p[0] >> 4), x4(p[1] >> 4), x4(p[2] >> 4), x4(p[3] >> 4)])
                .collect(),
            N3dsFormat::La8 => src
                .iter()
                .map(|p| {
                    let l = luminance(*p);
                    [l, l, l, p[3]]
                })
                .collect(),
            N3dsFormat::L8 => src
                .iter()
                .map(|p| {
                    let l = luminance(*p);
                    [l, l, l, 255]
                })
                .collect(),
            N3dsFormat::A8 => src.iter().map(|p| [255, 255, 255, p[3]]).collect(),
            N3dsFormat::Etc1 | N3dsFormat::Etc1a4 => {
                let alpha = self == N3dsFormat::Etc1a4;
                let mut out = vec![[0u8; 4]; src.len()];
                for by in (0..height).step_by(4) {
                    for bx in (0..width).step_by(4) {
                        let block = block_at(&src, width, height, bx, by);
                        let word = etc1_encode_block(&block);
                        let colors = etc1_decode_block(word);
                        for (i, c) in colors.iter().enumerate() {
                            let (x, y) = (bx + (i as u32 & 3), by + (i as u32 >> 2));
                            if x < width && y < height {
                                let p = &mut out[(y * width + x) as usize];
                                let a = src[(y * width + x) as usize][3];
                                *p = [c[0], c[1], c[2], if alpha { x4(a >> 4) } else { 255 }];
                            }
                        }
                    }
                }
                out
            }
        };
        out.into_iter().flatten().collect()
    }
}

/// n-bit to 8-bit by bit replication (what the PICA200 does).
fn x4(v: u8) -> u8 {
    (v << 4) | v
}
fn x5(v: u8) -> u8 {
    (v << 3) | (v >> 2)
}
fn x6(v: u8) -> u8 {
    (v << 2) | (v >> 4)
}

fn srgb_to_linear(c: u8) -> f64 {
    let c = c as f64 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(l: f64) -> f64 {
    if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

/// `tex3ds`'s luminance: Rec. 709 weights on linear light, back to sRGB
/// (within 1 of its output).
pub fn luminance(p: [u8; 4]) -> u8 {
    let l = 0.2126 * srgb_to_linear(p[0])
        + 0.7152 * srgb_to_linear(p[1])
        + 0.0722 * srgb_to_linear(p[2]);
    (linear_to_srgb(l) * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The 4×4 block at (`bx`, `by`), edges clamped, row-major.
fn block_at(src: &[[u8; 4]], width: u32, height: u32, bx: u32, by: u32) -> [[u8; 3]; 16] {
    let mut b = [[0u8; 3]; 16];
    for (i, t) in b.iter_mut().enumerate() {
        let x = (bx + (i as u32 & 3)).min(width - 1);
        let y = (by + (i as u32 >> 2)).min(height - 1);
        let p = src[(y * width + x) as usize];
        *t = [p[0], p[1], p[2]];
    }
    b
}

// ---------------------------------------------------------------------------
// ETC1 (the standard 64-bit block; the 3DS stores it little-endian in 8×8
// tiles of four blocks).

const ETC1_TABLES: [[i32; 4]; 8] = [
    [2, 8, -2, -8],
    [5, 17, -5, -17],
    [9, 29, -9, -29],
    [13, 42, -13, -42],
    [18, 60, -18, -60],
    [24, 80, -24, -80],
    [33, 106, -33, -106],
    [47, 183, -47, -183],
];

/// Is texel `i` (row-major in the block) in the second sub-block?
fn second_half(i: usize, flip: bool) -> bool {
    let (x, y) = (i & 3, i >> 2);
    if flip { y >= 2 } else { x >= 2 }
}

/// Decode one ETC1 block (the standard big-endian word) to 16 row-major
/// colors.
#[allow(clippy::needless_range_loop)] // per channel, like the bit layout
pub fn etc1_decode_block(word: u64) -> [[u8; 3]; 16] {
    let hi = (word >> 32) as u32;
    let diff = hi & 2 != 0;
    let flip = hi & 1 != 0;
    let tables = [((hi >> 5) & 7) as usize, ((hi >> 2) & 7) as usize];
    let mut base = [[0i32; 3]; 2];
    for c in 0..3 {
        let shift = 24 - 8 * c as u32;
        if diff {
            let v = ((hi >> (shift + 3)) & 31) as i32;
            let d = (((hi >> shift) & 7) as i32) << 29 >> 29;
            let v2 = (v + d) & 31;
            base[0][c] = (v << 3) | (v >> 2);
            base[1][c] = (v2 << 3) | (v2 >> 2);
        } else {
            let v1 = ((hi >> (shift + 4)) & 15) as i32;
            let v2 = ((hi >> shift) & 15) as i32;
            base[0][c] = v1 * 17;
            base[1][c] = v2 * 17;
        }
    }
    let lo = word as u32;
    let mut out = [[0u8; 3]; 16];
    for (i, o) in out.iter_mut().enumerate() {
        let (x, y) = (i & 3, i >> 2);
        let bit = x * 4 + y;
        let index = (((lo >> (16 + bit)) & 1) << 1 | ((lo >> bit) & 1)) as usize;
        let half = second_half(i, flip) as usize;
        let m = ETC1_TABLES[tables[half]][index];
        for c in 0..3 {
            o[c] = (base[half][c] + m).clamp(0, 255) as u8;
        }
    }
    out
}

/// The best modifier column for each texel of a half and the squared error.
fn half_error(
    block: &[[u8; 3]; 16],
    flip: bool,
    half: bool,
    base: [i32; 3],
    table: usize,
) -> (u32, [u8; 16]) {
    let mut err = 0u32;
    let mut idx = [0u8; 16];
    for (i, t) in block.iter().enumerate() {
        if second_half(i, flip) != half {
            continue;
        }
        let mut best = (u32::MAX, 0u8);
        for (k, m) in ETC1_TABLES[table].iter().enumerate() {
            let mut e = 0u32;
            for c in 0..3 {
                let d = (base[c] + m).clamp(0, 255) - t[c] as i32;
                e += (d * d) as u32;
            }
            if e < best.0 {
                best = (e, k as u8);
            }
        }
        err += best.0;
        idx[i] = best.1;
    }
    (err, idx)
}

/// The best (error, table, indices) for a half with a fixed base color.
fn best_table(
    block: &[[u8; 3]; 16],
    flip: bool,
    half: bool,
    base: [i32; 3],
) -> (u32, usize, [u8; 16]) {
    let mut best = (u32::MAX, 0, [0u8; 16]);
    for table in 0..8 {
        let (e, idx) = half_error(block, flip, half, base, table);
        if e < best.0 {
            best = (e, table, idx);
        }
    }
    best
}

fn half_average(block: &[[u8; 3]; 16], flip: bool, half: bool) -> [f32; 3] {
    let mut sum = [0u32; 3];
    for (i, t) in block.iter().enumerate() {
        if second_half(i, flip) == half {
            for c in 0..3 {
                sum[c] += t[c] as u32;
            }
        }
    }
    sum.map(|s| s as f32 / 8.0)
}

/// Encoded base-color candidates around a half's average: the rounded
/// average shifted along the gray axis (a block's modifiers are gray too).
fn etc1_bases(avg: [f32; 3], bits: u32) -> Vec<[i32; 3]> {
    let max = (1 << bits) - 1;
    let q = avg.map(|a| (a * max as f32 / 255.0).round() as i32);
    (-2..=2).map(|k| q.map(|v| (v + k).clamp(0, max))).collect()
}

fn expand(v: [i32; 3], bits: u32) -> [i32; 3] {
    if bits == 4 {
        v.map(|c| c * 17)
    } else {
        v.map(|c| (c << 3) | (c >> 2))
    }
}

/// An encoding candidate: error, differential, flip, encoded base colors,
/// tables, per-texel modifier columns.
type Etc1Choice = (u32, bool, bool, [[i32; 3]; 2], [usize; 2], [u8; 16]);

/// Encode one block (16 row-major colors) to the standard ETC1 word:
/// both flips, individual and differential modes, five base candidates per
/// half and every table. Deterministic.
#[allow(clippy::needless_range_loop)] // per channel, like the bit layout
pub fn etc1_encode_block(block: &[[u8; 3]; 16]) -> u64 {
    let mut best: Option<Etc1Choice> = None;
    for flip in [false, true] {
        let avgs = [
            half_average(block, flip, false),
            half_average(block, flip, true),
        ];
        // Individual: each half on its own, 4 bits a channel.
        let mut halves = Vec::new();
        for (h, avg) in avgs.iter().enumerate() {
            let mut hb = (u32::MAX, [0; 3], 0, [0u8; 16]);
            for cand in etc1_bases(*avg, 4) {
                let (e, t, idx) = best_table(block, flip, h == 1, expand(cand, 4));
                if e < hb.0 {
                    hb = (e, cand, t, idx);
                }
            }
            halves.push(hb);
        }
        let mut idx = halves[0].3;
        for (i, v) in idx.iter_mut().enumerate() {
            if second_half(i, flip) {
                *v = halves[1].3[i];
            }
        }
        let e = halves[0].0 + halves[1].0;
        if best.as_ref().is_none_or(|b| e < b.0) {
            best = Some((
                e,
                false,
                flip,
                [halves[0].1, halves[1].1],
                [halves[0].2, halves[1].2],
                idx,
            ));
        }
        // Differential: 5 bits a channel, the second half within -4..=3.
        for c0 in etc1_bases(avgs[0], 5) {
            let (e0, t0, idx0) = best_table(block, flip, false, expand(c0, 5));
            let target = etc1_bases(avgs[1], 5)[2];
            let mut hb = (u32::MAX, [0; 3], 0, [0u8; 16]);
            for k in -2..=2 {
                let c1: [i32; 3] = core::array::from_fn(|c| {
                    (target[c] + k).clamp(c0[c] - 4, c0[c] + 3).clamp(0, 31)
                });
                let (e1, t1, idx1) = best_table(block, flip, true, expand(c1, 5));
                if e1 < hb.0 {
                    hb = (e1, c1, t1, idx1);
                }
            }
            let e = e0 + hb.0;
            if best.as_ref().is_none_or(|b| e < b.0) {
                let mut idx = idx0;
                for (i, v) in idx.iter_mut().enumerate() {
                    if second_half(i, flip) {
                        *v = hb.3[i];
                    }
                }
                best = Some((e, true, flip, [c0, hb.1], [t0, hb.2], idx));
            }
        }
    }
    let (_, diff, flip, bases, tables, idx) = best.expect("a candidate");
    let mut hi = 0u32;
    for c in 0..3 {
        let shift = 24 - 8 * c as u32;
        if diff {
            let d = (bases[1][c] - bases[0][c]) as u32 & 7;
            hi |= ((bases[0][c] as u32) << (shift + 3)) | (d << shift);
        } else {
            hi |= ((bases[0][c] as u32) << (shift + 4)) | ((bases[1][c] as u32) << shift);
        }
    }
    hi |= (tables[0] as u32) << 5 | (tables[1] as u32) << 2;
    hi |= (diff as u32) << 1 | flip as u32;
    let mut lo = 0u32;
    for (i, &k) in idx.iter().enumerate() {
        let bit = (i & 3) * 4 + (i >> 2);
        lo |= ((k as u32 >> 1) & 1) << (16 + bit) | (k as u32 & 1) << bit;
    }
    (hi as u64) << 32 | lo as u64
}

// ---------------------------------------------------------------------------
// tex3ds raw output.

/// Decode `tex3ds -r -z none` output (its 4-byte header included) of a
/// `width` × `height` power-of-two texture to row-major RGBA8, with the
/// channel semantics above (A8 white). `None` when the size is wrong.
pub fn decode_tex3ds_raw(
    format: N3dsFormat,
    width: u32,
    height: u32,
    raw: &[u8],
) -> Option<Vec<u8>> {
    let data = raw.get(4..)?;
    let texels = (width * height) as usize;
    if data.len() < texels * format.bits_per_texel() as usize / 8 {
        return None;
    }
    let mut out = vec![0u8; texels * 4];
    let tiles_x = width / 8;
    let mut put = |x: u32, y: u32, c: [u8; 4]| {
        let o = ((y * width + x) * 4) as usize;
        out[o..o + 4].copy_from_slice(&c);
    };
    let morton = |i: u32| {
        let (mut x, mut y) = (0, 0);
        for b in 0..3 {
            x |= ((i >> (2 * b)) & 1) << b;
            y |= ((i >> (2 * b + 1)) & 1) << b;
        }
        (x, y)
    };
    match format {
        N3dsFormat::Etc1 | N3dsFormat::Etc1a4 => {
            let alpha = format == N3dsFormat::Etc1a4;
            let stride = if alpha { 16 } else { 8 };
            for (n, chunk) in data.chunks_exact(stride).take(texels / 16).enumerate() {
                let n = n as u32;
                let (tile, sub) = (n / 4, n % 4);
                let bx = (tile % tiles_x) * 8 + (sub & 1) * 4;
                let by = (tile / tiles_x) * 8 + (sub >> 1) * 4;
                let (a, c) = if alpha {
                    chunk.split_at(8)
                } else {
                    (&[][..], chunk)
                };
                let word = u64::from_le_bytes(c.try_into().ok()?);
                let alphas = if alpha {
                    u64::from_le_bytes(a.try_into().ok()?)
                } else {
                    u64::MAX
                };
                for (i, col) in etc1_decode_block(word).iter().enumerate() {
                    let (x, y) = (i as u32 & 3, i as u32 >> 2);
                    let av = x4(((alphas >> (4 * (x * 4 + y))) & 15) as u8);
                    put(bx + x, by + y, [col[0], col[1], col[2], av]);
                }
            }
        }
        _ => {
            let bpp = (format.bits_per_texel() / 8) as usize;
            for (n, px) in data.chunks_exact(bpp).take(texels).enumerate() {
                let n = n as u32;
                let (tile, i) = (n / 64, n % 64);
                let (x, y) = morton(i);
                let (x, y) = ((tile % tiles_x) * 8 + x, (tile / tiles_x) * 8 + y);
                let v16 = || u16::from_le_bytes([px[0], px[1]]);
                let c = match format {
                    N3dsFormat::Rgba8 => [px[3], px[2], px[1], px[0]],
                    N3dsFormat::Rgb565 => {
                        let v = v16();
                        [
                            x5((v >> 11) as u8),
                            x6((v >> 5) as u8 & 63),
                            x5(v as u8 & 31),
                            255,
                        ]
                    }
                    N3dsFormat::Rgba5551 => {
                        let v = v16();
                        let a = if v & 1 != 0 { 255 } else { 0 };
                        [
                            x5((v >> 11) as u8),
                            x5((v >> 6) as u8 & 31),
                            x5((v >> 1) as u8 & 31),
                            a,
                        ]
                    }
                    N3dsFormat::Rgba4 => {
                        let v = v16();
                        [
                            x4((v >> 12) as u8),
                            x4((v >> 8) as u8 & 15),
                            x4((v >> 4) as u8 & 15),
                            x4(v as u8 & 15),
                        ]
                    }
                    N3dsFormat::La8 => [px[1], px[1], px[1], px[0]],
                    N3dsFormat::L8 => [px[0], px[0], px[0], 255],
                    N3dsFormat::A8 => [255, 255, 255, px[0]],
                    N3dsFormat::Etc1 | N3dsFormat::Etc1a4 => unreachable!(),
                };
                put(x, y, c);
            }
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Quality.

/// PSNR in dB of `got` against `master` (both RGBA8, the same size), on
/// colors premultiplied by alpha plus the alpha channel, so the color of an
/// invisible texel doesn't count. `f64::INFINITY` when identical.
pub fn psnr(master: &[u8], got: &[u8]) -> f64 {
    assert_eq!(master.len(), got.len());
    let mut sum = 0f64;
    for (m, g) in master
        .as_chunks::<4>()
        .0
        .iter()
        .zip(got.as_chunks::<4>().0.iter())
    {
        for c in 0..3 {
            let a = m[c] as f64 * m[3] as f64 / 255.0;
            let b = g[c] as f64 * g[3] as f64 / 255.0;
            sum += (a - b) * (a - b);
        }
        let d = m[3] as f64 - g[3] as f64;
        sum += d * d;
    }
    let n = master.len() as f64;
    if sum == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (255.0f64 * 255.0 / (sum / n)).log10()
}

// ---------------------------------------------------------------------------
// `cargo xtask texture inspect` and `texture report`.

/// One platform's view of a texture: its format and why, bytes, quality
/// against the master and its share of the platform's texture memory.
#[derive(Clone, Debug, PartialEq)]
pub struct TextureReport {
    pub platform: crate::Platform,
    pub format: String,
    pub width: u32,
    pub height: u32,
    /// Bytes of texel data (and palette) as the platform stores it.
    pub bytes: u64,
    /// PSNR in dB against the master ([`psnr`]): `Some(INFINITY)` when
    /// lossless, `None` when this module does not model the encoder.
    pub psnr: Option<f64>,
    /// The memory the share is of: `TMEM` (N64), `texture memory` (3DS
    /// linear memory, `Caps::N3DS.texture_memory_bytes`) or `VRAM` (PC,
    /// no budget).
    pub memory: &'static str,
    /// Bytes counted against that memory, and its size.
    pub memory_bytes: u64,
    pub memory_budget: Option<u64>,
    /// Notes: why PSNR is unknown, a TMEM overflow.
    pub note: Option<String>,
    /// Picked by `auto` ([`auto::choose`]).
    pub auto: bool,
    /// Why this format: `auto`'s weighing, "declared" or the default.
    pub reason: String,
    /// The PSNR it had to keep (`auto`, or a declared `min_psnr`).
    pub threshold: Option<f64>,
    /// What the master holds (`white mask`, `opaque color`, ...).
    pub content: &'static str,
    /// Every format `auto` measured (empty for a declared format).
    pub candidates: Vec<Candidate>,
}

impl TextureReport {
    /// Percentage of the platform's texture memory, when it has a budget.
    pub fn share_percent(&self) -> Option<f64> {
        self.memory_budget
            .map(|b| self.memory_bytes as f64 * 100.0 / b as f64)
    }
}

/// A sprite's or font's settings and master, the way the bake reads them.
pub struct Master {
    pub source: Source,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The settings of `logical` (`sprites/x`, `fonts/x`) for `platform` and
/// its master (that platform's override resolved like the bake).
pub fn master(
    assets_root: &std::path::Path,
    manifest: &crate::Manifest,
    logical: &str,
    platform: crate::Platform,
) -> Result<(TextureSettings, Master), String> {
    let (kind, name) = logical
        .split_once('/')
        .ok_or_else(|| format!("`{logical}`: expected sprites/<name> or fonts/<name>"))?;
    match kind {
        "sprites" => {
            let decl = manifest
                .sprites
                .get(name)
                .ok_or_else(|| format!("sprite `{logical}` is not in the manifest"))?;
            let source = crate::resolve_source(assets_root, platform, &decl.file)?;
            let (width, height, rgba) = crate::bake::decode_png(&source)?;
            Ok((
                decl.settings(platform),
                Master {
                    source: Source::Sprite,
                    width,
                    height,
                    rgba,
                },
            ))
        }
        "fonts" => {
            let decl = manifest
                .fonts
                .get(name)
                .ok_or_else(|| format!("font `{logical}` is not in the manifest"))?;
            let source = crate::resolve_source(assets_root, platform, &decl.file)?;
            let font =
                crate::bake_font(&source, decl).map_err(|e| format!("font `{logical}`: {e}"))?;
            Ok((
                decl.settings(platform),
                Master {
                    source: Source::FontAtlas,
                    width: font.width,
                    height: font.height,
                    rgba: font.rgba,
                },
            ))
        }
        other => Err(format!(
            "`{logical}`: no textures under `{other}/` (sprites, fonts)"
        )),
    }
}

/// Inspect a sprite or font atlas (`sprites/x`, `fonts/x`) of the project
/// at `assets_root` on each of `platforms`, deciding its format like the
/// bake.
pub fn inspect(
    assets_root: &std::path::Path,
    logical: &str,
    platforms: &[crate::Platform],
) -> Result<Vec<TextureReport>, String> {
    let manifest = crate::Manifest::load(&assets_root.join("manifest.toml"))?;
    platforms
        .iter()
        .map(|&platform| {
            let (settings, m) = master(assets_root, &manifest, logical, platform)?;
            let choice = choose(
                platform, logical, m.source, &settings, m.width, m.height, &m.rgba,
            )?;
            Ok(report(platform, choice, m.width, m.height))
        })
        .collect()
}

/// One texture of a project in `texture report`.
#[derive(Clone, Debug)]
pub struct TextureEntry {
    pub logical: String,
    /// Each platform's report, or why its bake would fail.
    pub platforms: Vec<(crate::Platform, Result<TextureReport, String>)>,
}

/// Every sprite and font atlas of the project at `assets_root` on each of
/// `platforms`, in manifest order (`texture report` sorts them).
pub fn report_project(
    assets_root: &std::path::Path,
    platforms: &[crate::Platform],
) -> Result<Vec<TextureEntry>, String> {
    let manifest = crate::Manifest::load(&assets_root.join("manifest.toml"))?;
    let logicals = manifest
        .sprites
        .keys()
        .map(|n| format!("sprites/{n}"))
        .chain(manifest.fonts.keys().map(|n| format!("fonts/{n}")));
    Ok(logicals
        .map(|logical| {
            let platforms = platforms
                .iter()
                .map(|&platform| {
                    let r = master(assets_root, &manifest, &logical, platform).and_then(
                        |(settings, m)| {
                            choose(
                                platform, &logical, m.source, &settings, m.width, m.height, &m.rgba,
                            )
                            .map(|c| report(platform, c, m.width, m.height))
                        },
                    );
                    (platform, r)
                })
                .collect();
            TextureEntry { logical, platforms }
        })
        .collect())
}

/// A [`Choice`] as a report line with the platform's memory.
pub fn report(platform: crate::Platform, choice: Choice, width: u32, height: u32) -> TextureReport {
    let chosen = choice
        .candidates
        .iter()
        .find(|c| c.format == choice.format)
        .cloned();
    let (memory, memory_bytes, memory_budget) = match platform {
        crate::Platform::Pc => ("VRAM", choice.bytes, None),
        crate::Platform::N3ds => (
            "texture memory",
            choice.bytes,
            Some(trino_core::Caps::N3DS.texture_memory_bytes as u64),
        ),
        crate::Platform::N64 => {
            let f = N64Format::parse(&choice.format).expect("an N64 format");
            ("TMEM", f.tmem(width, height).0, Some(4096))
        }
    };
    TextureReport {
        platform,
        format: choice.format,
        width,
        height,
        bytes: choice.bytes,
        psnr: choice.psnr,
        memory,
        memory_bytes,
        memory_budget,
        note: chosen.and_then(|c| c.note),
        auto: choice.auto,
        reason: choice.reason,
        threshold: choice.threshold,
        content: choice.content.describe(),
        candidates: if choice.auto {
            choice.candidates
        } else {
            Vec::new()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_sizes() {
        for f in N3dsFormat::ALL {
            assert_eq!(N3dsFormat::parse(f.name()), Some(f));
        }
        assert_eq!(
            N3dsFormat::parse("etc1"),
            None,
            "manifest names are upper case"
        );
        assert_eq!(N3dsFormat::Etc1.texture_bytes(256, 256), 32 * 1024);
        assert_eq!(N3dsFormat::Rgba8.texture_bytes(256, 256), 256 * 1024);
        // Padded to a power of two and at least 8.
        assert_eq!(N3dsFormat::L8.texture_bytes(4, 20), 8 * 32);
    }

    #[test]
    fn etc1_encodes_flat_blocks_closely() {
        for c in [[0, 0, 0], [255, 255, 255], [200, 30, 90], [17, 128, 250]] {
            let block = [c; 16];
            let out = etc1_decode_block(etc1_encode_block(&block));
            for o in out {
                for k in 0..3 {
                    assert!((o[k] as i32 - c[k] as i32).abs() <= 6, "{c:?} -> {o:?}");
                }
            }
        }
    }

    #[test]
    fn etc1_two_color_block_uses_both_halves() {
        // Red left, blue right: needs two base colors (flip 0).
        let block: [[u8; 3]; 16] =
            core::array::from_fn(|i| if i & 3 < 2 { [255, 0, 0] } else { [0, 0, 255] });
        let out = etc1_decode_block(etc1_encode_block(&block));
        assert!(out[0][0] > 200 && out[0][2] < 60, "{:?}", out[0]);
        assert!(out[3][2] > 200 && out[3][0] < 60, "{:?}", out[3]);
    }

    #[test]
    fn channel_semantics() {
        let px = [200u8, 100, 50, 77];
        let p = |f: N3dsFormat| f.preview(1, 1, &px);
        assert_eq!(p(N3dsFormat::Rgba8), px);
        assert_eq!(p(N3dsFormat::A8), [255, 255, 255, 77]);
        let l = luminance([200, 100, 50, 255]);
        assert_eq!(p(N3dsFormat::L8), [l, l, l, 255]);
        assert_eq!(p(N3dsFormat::La8), [l, l, l, 77]);
        assert_eq!(p(N3dsFormat::Rgba4), [0xcc, 0x66, 0x33, 0x44]);
        assert_eq!(p(N3dsFormat::Rgba5551)[3], 0, "alpha < 128 drops");
        assert_eq!(p(N3dsFormat::Rgb565)[3], 255);
        assert_eq!(p(N3dsFormat::Etc1)[3], 255);
        assert_eq!(p(N3dsFormat::Etc1a4)[3], 0x44);
        // A fully transparent texel is 0 everywhere (then L8 is black).
        assert_eq!(
            N3dsFormat::L8.preview(1, 1, &[255, 255, 255, 0]),
            [0, 0, 0, 255]
        );
    }

    #[test]
    fn psnr_is_infinite_when_equal_and_ignores_invisible_color() {
        let a = [10u8, 20, 30, 255, 99, 99, 99, 0];
        let b = [10u8, 20, 30, 255, 0, 0, 0, 0];
        assert!(psnr(&a, &b).is_infinite());
        let c = [12u8, 20, 30, 255, 0, 0, 0, 0];
        let v = psnr(&a, &c);
        assert!(v > 50.0 && v.is_finite(), "{v}");
    }
}
