//! PC block compression (BCn, `wgpu::Features::TEXTURE_COMPRESSION_BC`):
//! what the PC bake writes next to a sprite's master when its PC format is
//! `BC1`, `BC3` or `BC7`, and what the PC uploads when the GPU samples it
//! (`trino-runner-pc`; a GPU without BC gets [`BcFormat::decode`]).
//!
//! - **BC1** (4 bits a texel): color, 1-bit alpha (texpresso, cluster fit).
//! - **BC3** (8): BC1 color plus an 8-level alpha block (texpresso).
//! - **BC7** (8): mode 6 only (one subset, RGBA endpoints with p-bits,
//!   16-level indices), encoded here: deterministic, no native code.
//!
//! Every format is decoded with `bcdec_rs`, which knows nothing of these
//! encoders, so a preview is what any BC GPU samples. Blocks are 4×4
//! texels in row-major block order (wgpu's layout); the side lengths must
//! be multiples of 4 (`auto` keeps RGBA8 otherwise).
//!
//! **Not exposed:** BC4 and BC5 sample as (r, 0, 0, 1) and (r, g, 0, 1),
//! and wgpu has no texture swizzle, so neither can mean "gray" or "gray
//! with alpha" without a shader per format. A white mask goes BC3 (its
//! color endpoints are exact), a gray image BC1.

/// A PC block-compressed format, as the manifest names it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum BcFormat {
    Bc1,
    Bc3,
    Bc7,
}

impl BcFormat {
    pub const ALL: [BcFormat; 3] = [BcFormat::Bc1, BcFormat::Bc3, BcFormat::Bc7];

    pub fn name(self) -> &'static str {
        match self {
            BcFormat::Bc1 => "BC1",
            BcFormat::Bc3 => "BC3",
            BcFormat::Bc7 => "BC7",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == name)
    }

    /// The tag in a baked `TBCN` file (`crate::bake`).
    pub fn code(self) -> u32 {
        match self {
            BcFormat::Bc1 => 1,
            BcFormat::Bc3 => 3,
            BcFormat::Bc7 => 7,
        }
    }

    pub fn from_code(code: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.code() == code)
    }

    pub fn block_bytes(self) -> usize {
        match self {
            BcFormat::Bc1 => 8,
            BcFormat::Bc3 | BcFormat::Bc7 => 16,
        }
    }

    pub fn bits_per_texel(self) -> u32 {
        self.block_bytes() as u32 / 2
    }

    /// Can a `width` × `height` texture be stored in BCn? (whole blocks)
    pub fn fits(width: u32, height: u32) -> bool {
        width > 0 && height > 0 && width.is_multiple_of(4) && height.is_multiple_of(4)
    }

    /// Bytes of a `width` × `height` texture in this format.
    pub fn texture_bytes(self, width: u32, height: u32) -> u64 {
        (width as u64 / 4) * (height as u64 / 4) * self.block_bytes() as u64
    }

    /// Encode row-major RGBA8 (`width`, `height` multiples of 4).
    pub fn encode(self, width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        assert!(
            Self::fits(width, height),
            "{width}x{height}: not whole 4x4 blocks"
        );
        assert_eq!(rgba.len(), (width * height * 4) as usize);
        let (w, h) = (width as usize, height as usize);
        match self {
            BcFormat::Bc1 | BcFormat::Bc3 => {
                let format = if self == BcFormat::Bc1 {
                    texpresso::Format::Bc1
                } else {
                    texpresso::Format::Bc3
                };
                let mut out = vec![0u8; format.compressed_size(w, h)];
                let params = texpresso::Params {
                    algorithm: texpresso::Algorithm::ClusterFit,
                    // PSNR weighs channels alike; colors under low alpha
                    // count less (`super::psnr` premultiplies).
                    weights: texpresso::COLOUR_WEIGHTS_UNIFORM,
                    weigh_colour_by_alpha: true,
                };
                format.compress(rgba, w, h, params, &mut out);
                out
            }
            BcFormat::Bc7 => {
                let mut out = Vec::with_capacity(w * h);
                for by in (0..h).step_by(4) {
                    for bx in (0..w).step_by(4) {
                        let block: [[u8; 4]; 16] = core::array::from_fn(|i| {
                            let o = ((by + i / 4) * w + bx + i % 4) * 4;
                            [rgba[o], rgba[o + 1], rgba[o + 2], rgba[o + 3]]
                        });
                        out.extend_from_slice(&bc7_encode_block(&block));
                    }
                }
                out
            }
        }
    }

    /// Decode blocks to row-major RGBA8 (what a BC GPU samples). `None`
    /// when `data` is not `width` × `height` of this format.
    pub fn decode(self, width: u32, height: u32, data: &[u8]) -> Option<Vec<u8>> {
        if !Self::fits(width, height) || data.len() as u64 != self.texture_bytes(width, height) {
            return None;
        }
        let (w, h) = (width as usize, height as usize);
        let mut out = vec![0u8; w * h * 4];
        let pitch = w * 4;
        let mut blocks = data.chunks_exact(self.block_bytes());
        for by in (0..h).step_by(4) {
            for bx in (0..w).step_by(4) {
                let block = blocks.next()?;
                let dst = &mut out[by * pitch + bx * 4..];
                match self {
                    BcFormat::Bc1 => bcdec_rs::bc1(block, dst, pitch),
                    BcFormat::Bc3 => bcdec_rs::bc3(block, dst, pitch),
                    BcFormat::Bc7 => bcdec_rs::bc7(block, dst, pitch),
                }
            }
        }
        Some(out)
    }

    /// What a BC GPU samples from `rgba` in this format.
    pub fn preview(self, width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        self.decode(width, height, &self.encode(width, height, rgba))
            .expect("own encoding decodes")
    }
}

// ---------------------------------------------------------------------------
// BC7 mode 6: one subset, RGBA endpoints of 7 bits plus a p-bit each, 4-bit
// indices (the first one 3 bits, the "anchor").

const BC7_WEIGHTS: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

fn bc7_lerp(e0: [u32; 4], e1: [u32; 4], w: u32) -> [u32; 4] {
    core::array::from_fn(|c| ((64 - w) * e0[c] + w * e1[c] + 32) >> 6)
}

/// Endpoints quantized with their p-bits: the 8-bit values the GPU expands.
fn bc7_quantize(e: [f32; 4], p: u32) -> ([u32; 4], [u32; 4]) {
    let q: [u32; 4] =
        core::array::from_fn(|c| ((e[c] - p as f32) / 2.0).round().clamp(0.0, 127.0) as u32);
    (q, q.map(|v| (v << 1) | p))
}

/// The best index of every texel for two expanded endpoints, and the
/// squared error.
fn bc7_indices(block: &[[u8; 4]; 16], e0: [u32; 4], e1: [u32; 4]) -> (u32, [u8; 16]) {
    let palette: [[u32; 4]; 16] = core::array::from_fn(|k| bc7_lerp(e0, e1, BC7_WEIGHTS[k]));
    let mut err = 0;
    let mut idx = [0u8; 16];
    for (i, t) in block.iter().enumerate() {
        let mut best = (u32::MAX, 0u8);
        for (k, p) in palette.iter().enumerate() {
            let e: u32 = (0..4)
                .map(|c| {
                    let d = p[c] as i32 - t[c] as i32;
                    (d * d) as u32
                })
                .sum();
            if e < best.0 {
                best = (e, k as u8);
            }
        }
        err += best.0;
        idx[i] = best.1;
    }
    (err, idx)
}

/// An encoding: error, 7-bit endpoints, p-bits, indices.
type Bc7Choice = (u32, [[u32; 4]; 2], [u32; 2], [u8; 16]);

/// The best p-bits for two float endpoints.
fn bc7_fit(block: &[[u8; 4]; 16], e0: [f32; 4], e1: [f32; 4]) -> Bc7Choice {
    let mut best: Option<Bc7Choice> = None;
    for p0 in 0..2 {
        for p1 in 0..2 {
            let (q0, x0) = bc7_quantize(e0, p0);
            let (q1, x1) = bc7_quantize(e1, p1);
            let (err, idx) = bc7_indices(block, x0, x1);
            if best.as_ref().is_none_or(|b| err < b.0) {
                best = Some((err, [q0, q1], [p0, p1], idx));
            }
        }
    }
    best.expect("four candidates")
}

/// Least-squares endpoints for fixed indices (`None` when every texel has
/// the same weight).
fn bc7_refit(block: &[[u8; 4]; 16], idx: &[u8; 16]) -> Option<([f32; 4], [f32; 4])> {
    let (mut aa, mut ab, mut bb) = (0f32, 0f32, 0f32);
    let (mut ax, mut bx) = ([0f32; 4], [0f32; 4]);
    for (t, &k) in block.iter().zip(idx) {
        let b = BC7_WEIGHTS[k as usize] as f32 / 64.0;
        let a = 1.0 - b;
        aa += a * a;
        ab += a * b;
        bb += b * b;
        for c in 0..4 {
            ax[c] += a * t[c] as f32;
            bx[c] += b * t[c] as f32;
        }
    }
    let det = aa * bb - ab * ab;
    if det.abs() < 1e-6 {
        return None;
    }
    let e0 = core::array::from_fn(|c| ((ax[c] * bb - bx[c] * ab) / det).clamp(0.0, 255.0));
    let e1 = core::array::from_fn(|c| ((bx[c] * aa - ax[c] * ab) / det).clamp(0.0, 255.0));
    Some((e0, e1))
}

/// Encode one 4×4 block (row-major RGBA8) as BC7 mode 6: endpoints along
/// the principal axis of the block's colors, the best p-bits, then two
/// least-squares refits. Deterministic.
pub fn bc7_encode_block(block: &[[u8; 4]; 16]) -> [u8; 16] {
    let px: [[f32; 4]; 16] = block.map(|t| t.map(|v| v as f32));
    let mean: [f32; 4] = core::array::from_fn(|c| px.iter().map(|p| p[c]).sum::<f32>() / 16.0);
    let mut cov = [[0f32; 4]; 4];
    for p in &px {
        for i in 0..4 {
            for j in 0..4 {
                cov[i][j] += (p[i] - mean[i]) * (p[j] - mean[j]);
            }
        }
    }
    // Power iteration from the widest channel's direction.
    let mut axis = [1f32; 4];
    for _ in 0..8 {
        let next: [f32; 4] = core::array::from_fn(|i| (0..4).map(|j| cov[i][j] * axis[j]).sum());
        let len = next.iter().map(|v| v * v).sum::<f32>().sqrt();
        if len < 1e-6 {
            break;
        }
        axis = next.map(|v| v / len);
    }
    let (mut lo, mut hi) = (0f32, 0f32);
    for p in &px {
        let t: f32 = (0..4).map(|c| (p[c] - mean[c]) * axis[c]).sum();
        lo = lo.min(t);
        hi = hi.max(t);
    }
    let e0 = core::array::from_fn(|c| (mean[c] + lo * axis[c]).clamp(0.0, 255.0));
    let e1 = core::array::from_fn(|c| (mean[c] + hi * axis[c]).clamp(0.0, 255.0));
    let mut best = bc7_fit(block, e0, e1);
    for _ in 0..2 {
        let Some((r0, r1)) = bc7_refit(block, &best.3) else {
            break;
        };
        let next = bc7_fit(block, r0, r1);
        if next.0 >= best.0 {
            break;
        }
        best = next;
    }
    let (_, mut q, mut p, mut idx) = best;
    // The anchor (texel 0) stores 3 bits: its index must be below 8.
    if idx[0] >= 8 {
        q.swap(0, 1);
        p.swap(0, 1);
        idx = idx.map(|k| 15 - k);
    }
    let mut bits = 1u128 << 6; // mode 6
    let mut at = 7;
    let mut put = |v: u128, n: u32| {
        bits |= v << at;
        at += n;
    };
    for (a, b) in q[0].iter().zip(&q[1]) {
        put(*a as u128, 7);
        put(*b as u128, 7);
    }
    put(p[0] as u128, 1);
    put(p[1] as u128, 1);
    put(idx[0] as u128, 3);
    for &k in &idx[1..] {
        put(k as u128, 4);
    }
    bits.to_le_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        let mut s = 0x9e37_79b9u32;
        (0..w * h)
            .flat_map(|i| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                let (x, y) = (i % w, i / w);
                let n = s & 31;
                let a = if alpha {
                    (y * 255 / (h - 1)) as u8
                } else {
                    255
                };
                [
                    (x * 8 + n).min(255) as u8,
                    (y * 8 + n).min(255) as u8,
                    (255 - x * 4) as u8,
                    a,
                ]
            })
            .collect()
    }

    #[test]
    fn every_format_round_trips_through_an_independent_decoder() {
        for f in BcFormat::ALL {
            // BC1's alpha is one bit: give it an opaque image.
            let rgba = noise(32, 32, f != BcFormat::Bc1);
            let data = f.encode(32, 32, &rgba);
            assert_eq!(data.len() as u64, f.texture_bytes(32, 32), "{f:?}");
            let out = f.decode(32, 32, &data).unwrap();
            let db = super::super::psnr(&rgba, &out);
            assert!(db > 25.0, "{f:?}: {db:.1} dB");
            assert_eq!(BcFormat::parse(f.name()), Some(f));
            assert_eq!(BcFormat::from_code(f.code()), Some(f));
        }
        assert!(BcFormat::Bc1.decode(32, 32, &[0; 7]).is_none());
        assert!(!BcFormat::fits(30, 32) && BcFormat::fits(4, 8));
    }

    #[test]
    fn bc7_keeps_flat_blocks_and_beats_bc1_on_a_gradient() {
        for c in [
            [0, 0, 0, 255],
            [255, 255, 255, 0],
            [200, 30, 90, 128],
            [17, 128, 250, 255],
        ] {
            let out = BcFormat::Bc7.preview(4, 4, &[c; 16].concat());
            for t in out.as_chunks::<4>().0 {
                for k in 0..4 {
                    assert!((t[k] as i32 - c[k] as i32).abs() <= 1, "{c:?} -> {t:?}");
                }
            }
        }
        let rgba = noise(64, 64, false);
        let db = |f: BcFormat| super::super::psnr(&rgba, &f.preview(64, 64, &rgba));
        let (bc1, bc7) = (db(BcFormat::Bc1), db(BcFormat::Bc7));
        assert!(bc7 > bc1, "BC7 {bc7:.2} dB, BC1 {bc1:.2} dB");
    }

    #[test]
    fn a_white_mask_keeps_its_color_in_bc3() {
        let rgba: Vec<u8> = (0..64u32)
            .flat_map(|i| [255, 255, 255, (i * 4) as u8])
            .collect();
        let out = BcFormat::Bc3.preview(8, 8, &rgba);
        assert!(
            out.as_chunks::<4>()
                .0
                .iter()
                .all(|t| t[..3] == [255, 255, 255])
        );
        let db = super::super::psnr(&rgba, &out);
        // An alpha ramp of 4 a texel: BC3's 8 levels per block.
        assert!(db > 30.0, "{db:.1}");
    }
}
