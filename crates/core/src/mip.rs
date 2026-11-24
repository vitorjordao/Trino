//! Mipmap chains (Bongo-Kart #178): the smaller copies of a texture a GPU
//! samples when the texture covers fewer pixels than it has texels — a
//! floor or a sea seen at a grazing angle, which without them shimmers in
//! moiré rings and costs cache misses.
//!
//! One generator for every caller, so the PC's bake (BC levels), the Web's
//! (KTX2 levels), the renderer's uploads of plain RGBA8 and the tests make
//! the same pixels:
//!
//! - a level halves both sides of the one above, down to [`MIN_SIDE`]
//!   texels on the smaller side: every level is whole 4×4 blocks, which is
//!   what BC and Basis encode ([`levels`]);
//! - only textures whose sides are powers of two carry a chain
//!   ([`chainable`]): the halving is exact and tiling textures (the ones
//!   that need mips) are powers of two on every console anyway;
//! - each texel is the mean of its 2×2 parents **in linear light**,
//!   weighted by alpha (a transparent texel's color doesn't bleed into
//!   its neighbors), with alpha the plain mean. Averaging the sRGB codes
//!   instead would darken a fine black-and-white pattern to 50 % gray
//!   (21 % light) where the eye sees 50 % light: far floors would look
//!   darker than near ones.
//!
//! The consoles ignore chains (`Caps::mipmaps`): the N64's TMEM and the
//! 3DS's bakes keep one level, and the PC's console profiles sample level
//! 0 so they keep drawing what the consoles draw.

use crate::access::color::{to_linear, to_srgb};

/// The smaller side of a chain's last level: one 4×4 block.
pub const MIN_SIDE: u32 = 4;

/// Can a `w`×`h` texture carry a chain: both sides powers of two and at
/// least [`MIN_SIDE`].
pub const fn chainable(w: u32, h: u32) -> bool {
    w.is_power_of_two() && h.is_power_of_two() && w >= MIN_SIDE && h >= MIN_SIDE
}

/// Levels of a `w`×`h` texture's chain, level 0 included: halving until the
/// smaller side is [`MIN_SIDE`]. 1 when it can't carry one.
pub const fn levels(w: u32, h: u32) -> u32 {
    if !chainable(w, h) {
        return 1;
    }
    let small = if w < h { w } else { h };
    small.trailing_zeros() - MIN_SIDE.trailing_zeros() + 1
}

/// Size of `level` (0 = the texture).
pub const fn level_size(w: u32, h: u32, level: u32) -> (u32, u32) {
    let (a, b) = (w >> level, h >> level);
    (if a == 0 { 1 } else { a }, if b == 0 { 1 } else { b })
}

/// Bytes of a `w`×`h` RGBA8 chain, every level (`levels(w, h)` of them).
pub const fn rgba8_chain_bytes(w: u32, h: u32) -> usize {
    let mut total = 0usize;
    let mut l = 0;
    while l < levels(w, h) {
        let (lw, lh) = level_size(w, h, l);
        total += lw as usize * lh as usize * 4;
        l += 1;
    }
    total
}

/// Offset of `level` in a chain laid out level after level (RGBA8).
pub const fn rgba8_level_offset(w: u32, h: u32, level: u32) -> usize {
    let mut at = 0usize;
    let mut l = 0;
    while l < level {
        let (lw, lh) = level_size(w, h, l);
        at += lw as usize * lh as usize * 4;
        l += 1;
    }
    at
}

/// One level down: `src` is `w`×`h` RGBA8 (both even), `dst` receives
/// `w/2`×`h/2`. Each texel averages its 2×2 parents in linear light,
/// weighted by their alpha; alpha is their rounded mean. A fully
/// transparent quad averages its colors unweighted (so a cutout's hidden
/// texels keep a color for the bilinear fringe).
pub fn downsample_rgba8(src: &[u8], w: u32, h: u32, dst: &mut [u8]) {
    let (w, h) = (w as usize, h as usize);
    let (dw, dh) = (w / 2, h / 2);
    debug_assert!(src.len() >= w * h * 4 && dst.len() >= dw * dh * 4);
    for y in 0..dh {
        for x in 0..dw {
            let at = |dx: usize, dy: usize| ((y * 2 + dy) * w + x * 2 + dx) * 4;
            let quad = [at(0, 0), at(1, 0), at(0, 1), at(1, 1)];
            let mut alpha = 0u32;
            for &i in &quad {
                alpha += src[i + 3] as u32;
            }
            let o = (y * dw + x) * 4;
            for c in 0..3 {
                let mut sum = 0.0f32;
                if alpha == 0 {
                    for &i in &quad {
                        sum += to_linear(src[i + c]);
                    }
                    sum *= 0.25;
                } else {
                    for &i in &quad {
                        sum += to_linear(src[i + c]) * src[i + 3] as f32;
                    }
                    sum /= alpha as f32;
                }
                dst[o + c] = to_srgb(sum.clamp(0.0, 1.0));
            }
            dst[o + 3] = ((alpha + 2) / 4) as u8;
        }
    }
}

/// Fill a whole RGBA8 chain in place: `chain` holds level 0 (`w`×`h`) at
/// its start and is [`rgba8_chain_bytes`] long; every later level is made
/// from the one before it. Returns `false` (and writes nothing) when the
/// size can't carry a chain or `chain` is the wrong length.
pub fn fill_rgba8_chain(chain: &mut [u8], w: u32, h: u32) -> bool {
    if !chainable(w, h) || chain.len() != rgba8_chain_bytes(w, h) {
        return false;
    }
    for l in 1..levels(w, h) {
        let (pw, ph) = level_size(w, h, l - 1);
        let (prev, next) = chain.split_at_mut(rgba8_level_offset(w, h, l));
        let src = &prev[rgba8_level_offset(w, h, l - 1)..];
        downsample_rgba8(src, pw, ph, next);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chains_stop_at_one_block() {
        assert_eq!(levels(256, 256), 7, "256 .. 4");
        assert_eq!(levels(64, 16), 3, "64x16, 32x8, 16x4");
        assert_eq!(levels(4, 4), 1);
        assert_eq!(levels(100, 100), 1, "not a power of two");
        assert_eq!(levels(2, 2), 1, "smaller than a block");
        assert_eq!(level_size(64, 16, 2), (16, 4));
        assert_eq!(rgba8_chain_bytes(8, 8), (64 + 16) * 4);
        assert_eq!(rgba8_level_offset(8, 8, 1), 256);
        assert_eq!(rgba8_chain_bytes(6, 6), 6 * 6 * 4, "no chain: level 0");
    }

    #[test]
    fn a_checker_averages_in_linear_light() {
        // Black and white texels: the mean light is half, which is sRGB
        // code 188 (code 128 would be ~22 % light).
        let mut chain = vec![0u8; rgba8_chain_bytes(8, 8)];
        for y in 0..8 {
            for x in 0..8 {
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                chain[(y * 8 + x) * 4..][..4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        assert!(fill_rgba8_chain(&mut chain, 8, 8));
        let l1 = &chain[rgba8_level_offset(8, 8, 1)..];
        for p in l1.as_chunks::<4>().0 {
            assert_eq!(*p, [188, 188, 188, 255]);
        }
    }

    #[test]
    fn transparent_texels_do_not_tint_their_neighbors() {
        // Red opaque beside green fully transparent: the color stays red,
        // alpha halves.
        let src = [
            255, 0, 0, 255, 0, 255, 0, 0, //
            255, 0, 0, 255, 0, 255, 0, 0,
        ];
        let mut dst = [0u8; 4];
        downsample_rgba8(&src, 2, 2, &mut dst);
        assert_eq!(dst, [255, 0, 0, 128]);
        // All transparent: the plain mean keeps a color.
        let clear = [10, 20, 30, 0, 10, 20, 30, 0, 10, 20, 30, 0, 10, 20, 30, 0];
        downsample_rgba8(&clear, 2, 2, &mut dst);
        assert_eq!(dst, [10, 20, 30, 0]);
    }

    #[test]
    fn a_flat_texture_keeps_its_color_on_every_level() {
        let (w, h) = (32, 8);
        let mut chain = vec![0u8; rgba8_chain_bytes(w, h)];
        for p in chain[..(w * h * 4) as usize].as_chunks_mut::<4>().0 {
            *p = [37, 140, 201, 255];
        }
        assert!(fill_rgba8_chain(&mut chain, w, h));
        for p in chain.as_chunks::<4>().0 {
            assert_eq!(*p, [37, 140, 201, 255]);
        }
        assert!(!fill_rgba8_chain(&mut chain[..16], w, h), "wrong length");
        assert!(!fill_rgba8_chain(&mut [0u8; 36 * 4], 6, 6), "not chainable");
    }
}
