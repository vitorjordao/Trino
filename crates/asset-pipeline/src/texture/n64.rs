//! The N64's texture formats on the host: TMEM use and, where the
//! conversion is known exactly, what the RDP samples (`mksprite` at the
//! libdragon commit pinned in `docker/n64/Dockerfile`).
//!
//! - **RGBA5551** truncates like the 3DS's (`>> 3`, alpha ≥ 128).
//! - **I4/I8**: an all-gray master keeps its red channel as the intensity;
//!   any other is Rec. 601 luma × alpha stretched to the full range. The
//!   RDP samples an intensity texel as (i, i, i, i), so the build stages a
//!   mask's coverage as its intensity ([`stage`]): a font atlas becomes its
//!   alpha, a gray sprite with alpha its color premultiplied by it.
//!   Otherwise a white glow would be a solid disc (`mksprite` ignores alpha
//!   on gray input).
//! - **CI4/CI8** are exact when the master has at most 16/256 colors after
//!   the RGBA5551 truncation; otherwise `mksprite` quantizes with exoquant,
//!   which is not modeled here (no preview, no PSNR).

use std::borrow::Cow;

use super::{N3dsFormat, Source, x4};

/// An N64 texture format, as the manifest names it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum N64Format {
    Ci4,
    Ci8,
    I4,
    I8,
    Rgba5551,
}

impl N64Format {
    pub const ALL: [N64Format; 5] = [
        N64Format::I4,
        N64Format::Ci4,
        N64Format::I8,
        N64Format::Ci8,
        N64Format::Rgba5551,
    ];

    pub fn name(self) -> &'static str {
        match self {
            N64Format::Ci4 => "CI4",
            N64Format::Ci8 => "CI8",
            N64Format::I4 => "I4",
            N64Format::I8 => "I8",
            N64Format::Rgba5551 => "RGBA5551",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == name)
    }

    pub fn bits_per_texel(self) -> u32 {
        match self {
            N64Format::Ci4 | N64Format::I4 => 4,
            N64Format::Ci8 | N64Format::I8 => 8,
            N64Format::Rgba5551 => 16,
        }
    }

    /// Bytes in the ROM: texels plus the palette (16 or 256 RGBA5551).
    pub fn bytes(self, width: u32, height: u32) -> u64 {
        let tlut = match self {
            N64Format::Ci4 => 32,
            N64Format::Ci8 => 512,
            _ => 0,
        };
        (width as u64 * height as u64 * self.bits_per_texel() as u64).div_ceil(8) + tlut
    }

    /// TMEM bytes when loaded whole (rows padded to 8 bytes; rdpq loads a
    /// palette 4 times into TMEM's upper half) and the bytes available to
    /// texels (a palette leaves half of the 4 KB).
    pub fn tmem(self, width: u32, height: u32) -> (u64, u64) {
        let rows = (width * self.bits_per_texel() / 8).next_multiple_of(8) as u64 * height as u64;
        match self {
            N64Format::Ci4 => (rows + 128, 2048),
            N64Format::Ci8 => (rows + 2048, 2048),
            _ => (rows, 4096),
        }
    }

    /// TMEM bytes the texels take when loaded whole and the room they
    /// have: 4 KB, or the lower 2 KB beside a palette — the limit the
    /// shim's resident text atlas (`trino_text_begin`) and a 3D draw
    /// hold a texture to.
    pub fn tmem_texels(self, width: u32, height: u32) -> (u64, u64) {
        let (used, room) = self.tmem(width, height);
        let palette = match self {
            N64Format::Ci4 => 128,
            N64Format::Ci8 => 2048,
            _ => 0,
        };
        (used - palette, room)
    }

    /// Does the whole texture fit TMEM (what a 3D draw needs; 2D blits
    /// tile bigger sprites)?
    pub fn fits_tmem(self, width: u32, height: u32) -> bool {
        let (texels, room) = self.tmem_texels(width, height);
        texels <= room
    }

    /// What the RDP samples from `rgba` (row-major RGBA8), or why it is not
    /// modeled.
    pub fn preview(self, width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
        let quantized = N3dsFormat::Rgba5551.preview(width, height, rgba);
        match self {
            N64Format::Rgba5551 => Ok(quantized),
            N64Format::Ci4 | N64Format::Ci8 => {
                let max = if self == N64Format::Ci4 { 16 } else { 256 };
                let mut colors: Vec<&[u8; 4]> = quantized.as_chunks::<4>().0.iter().collect();
                colors.sort_unstable();
                colors.dedup();
                if colors.len() <= max {
                    Ok(quantized)
                } else {
                    Err(format!(
                        "{} colors for a {max}-color palette: mksprite quantizes (not modeled)",
                        colors.len()
                    ))
                }
            }
            N64Format::I4 | N64Format::I8 => Ok(intensity(rgba)
                .into_iter()
                .flat_map(|i| {
                    let i = if self == N64Format::I4 { x4(i >> 4) } else { i };
                    [i; 4]
                })
                .collect()),
        }
    }
}

/// mksprite's intensity of each texel.
fn intensity(rgba: &[u8]) -> Vec<u8> {
    let px = rgba.as_chunks::<4>().0;
    if px.iter().all(|p| p[0] == p[1] && p[1] == p[2]) {
        return px.iter().map(|p| p[0]).collect();
    }
    let f: Vec<f32> = px
        .iter()
        .map(|p| {
            (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
                * (p[3] as f32 / 255.0)
        })
        .collect();
    let (min, max) = f
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    f.iter()
        .map(|&v| {
            if max > min {
                ((v - min) / (max - min) * 255.0) as u8
            } else {
                0
            }
        })
        .collect()
}

/// What the N64 build hands `mksprite` for `format`. The RDP samples an
/// I4/I8 texel as (i, i, i, i), and `mksprite` takes a gray master's red
/// channel as i whatever its alpha, so a white glow would be a solid disc.
/// For those formats a font atlas is staged as its coverage
/// ([`intensity_from_alpha`]); an all-gray sprite with alpha takes its
/// color or its coverage as the intensity, whichever previews closer to
/// the master (a white glow: coverage; a premultiplied one: either). A
/// colored master, an opaque one and every other format go as they are
/// (`mksprite` already weighs a colored master's luma by its alpha).
pub fn stage(source: Source, format: N64Format, rgba: &[u8]) -> Cow<'_, [u8]> {
    if !matches!(format, N64Format::I4 | N64Format::I8) {
        return Cow::Borrowed(rgba);
    }
    let px = rgba.as_chunks::<4>().0;
    match source {
        Source::FontAtlas => Cow::Owned(intensity_from_alpha(rgba)),
        Source::Sprite
            if px.iter().all(|p| p[0] == p[1] && p[1] == p[2]) && px.iter().any(|p| p[3] < 255) =>
        {
            // Gray input previews independently of the image size.
            let n = px.len() as u32;
            let close = |input: &[u8]| {
                format
                    .preview(n, 1, input)
                    .map_or(f64::NEG_INFINITY, |p| super::psnr(rgba, &p))
            };
            let coverage = intensity_from_alpha(rgba);
            if close(&coverage) > close(rgba) {
                Cow::Owned(coverage)
            } else {
                Cow::Borrowed(rgba)
            }
        }
        Source::Sprite => Cow::Borrowed(rgba),
    }
}

/// A white-on-transparent atlas as the N64 stages it for I4/I8: the
/// coverage becomes the intensity, so antialiased edges stay soft.
pub fn intensity_from_alpha(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[3]; 4])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_tmem() {
        assert_eq!(N64Format::Ci4.bytes(32, 32), 512 + 32);
        assert_eq!(N64Format::Rgba5551.tmem(64, 64), (8192, 4096));
        assert!(!N64Format::Rgba5551.fits_tmem(64, 64));
        assert!(N64Format::Ci8.fits_tmem(32, 32) && !N64Format::Ci8.fits_tmem(64, 64));
        // A 6-texel-wide I4 row (3 bytes) pads to 8.
        assert_eq!(N64Format::I4.tmem(6, 2).0, 16);
    }

    #[test]
    fn intensity_follows_mksprite() {
        // All gray: the red channel, whatever the alpha.
        let gray = [10u8, 10, 10, 255, 200, 200, 200, 0];
        assert_eq!(
            N64Format::I8.preview(2, 1, &gray).unwrap(),
            [10, 10, 10, 10, 200, 200, 200, 200]
        );
        // Color: luma × alpha, stretched to 0..=255.
        let color = [255u8, 0, 0, 255, 0, 0, 255, 255, 0, 255, 0, 128];
        let i = N64Format::I8.preview(3, 1, &color).unwrap();
        // Lumas 76.2, 29.1 and 75.1 (green at half alpha): blue is the
        // darkest, red the brightest.
        assert_eq!((i[0], i[4], i[8]), (255, 0, 249));
        assert_eq!(N64Format::I4.preview(2, 1, &gray).unwrap()[4], 0xcc);
        assert_eq!(intensity_from_alpha(&[255, 255, 255, 77]), [77; 4]);
    }

    #[test]
    fn masks_are_staged_with_their_coverage_as_intensity() {
        // A white glow: mksprite alone would keep 255 under every alpha.
        let glow = [255u8, 255, 255, 64, 255, 255, 255, 255, 0, 0, 0, 0];
        assert_eq!(N64Format::I8.preview(3, 1, &glow).unwrap()[0], 255);
        let staged = stage(Source::Sprite, N64Format::I8, &glow);
        assert_eq!(
            &staged[..],
            [64, 64, 64, 64, 255, 255, 255, 255, 0, 0, 0, 0]
        );
        // Already premultiplied (color = coverage): kept as it is.
        let soft = [64u8, 64, 64, 64, 200, 200, 200, 200];
        assert!(matches!(
            stage(Source::Sprite, N64Format::I4, &soft),
            Cow::Borrowed(_)
        ));
        // Opaque gray, colored masters and other formats go unchanged.
        let opaque = [90u8, 90, 90, 255];
        assert!(matches!(
            stage(Source::Sprite, N64Format::I4, &opaque),
            Cow::Borrowed(_)
        ));
        let color = [255u8, 0, 0, 128];
        assert!(matches!(
            stage(Source::Sprite, N64Format::I8, &color),
            Cow::Borrowed(_)
        ));
        assert!(matches!(
            stage(Source::Sprite, N64Format::Ci4, &glow),
            Cow::Borrowed(_)
        ));
        // A font atlas is its alpha, whatever its color.
        assert_eq!(
            &stage(Source::FontAtlas, N64Format::I4, &color)[..],
            [128; 4]
        );
    }

    #[test]
    fn palettes_are_exact_only_when_the_colors_fit() {
        let two: Vec<u8> = (0..16)
            .flat_map(|i| {
                if i % 2 == 0 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 255, 255]
                }
            })
            .collect();
        assert_eq!(N64Format::Ci4.preview(4, 4, &two).unwrap(), two);
        let many: Vec<u8> = (0..64u32)
            .flat_map(|i| [(i * 4) as u8, 0, 0, 255])
            .collect();
        assert!(
            N64Format::Ci4
                .preview(8, 8, &many)
                .unwrap_err()
                .contains("not modeled")
        );
        assert!(N64Format::Ci8.preview(8, 8, &many).is_ok());
    }
}
