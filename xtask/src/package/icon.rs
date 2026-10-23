//! Icons for packages, from one square PNG (`[pc] icon`): resized with a
//! box filter (down) or nearest neighbor (up, pixel art stays crisp), and
//! written as a Windows `.ico` and a macOS `.icns` — both formats take PNG
//! payloads, so no `iconutil` or ImageMagick is needed and every OS can
//! build them.

use std::path::Path;

/// An RGBA image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn load(path: &Path) -> Result<Image, String> {
        let (w, h, rgba) = trino_asset_pipeline::decode_png(path)?;
        Ok(Image { w, h, rgba })
    }

    /// `size` × `size`: whole-pixel nearest neighbor when growing, the
    /// average of the covered source pixels (alpha-weighted) when shrinking.
    pub fn resized(&self, size: u32) -> Image {
        let mut rgba = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                if size >= self.w.max(self.h) {
                    let sx = (x * self.w / size).min(self.w - 1);
                    let sy = (y * self.h / size).min(self.h - 1);
                    let i = ((sy * self.w + sx) * 4) as usize;
                    rgba.extend_from_slice(&self.rgba[i..i + 4]);
                    continue;
                }
                let (x0, x1) = (
                    x * self.w / size,
                    ((x + 1) * self.w / size).max(x * self.w / size + 1),
                );
                let (y0, y1) = (
                    y * self.h / size,
                    ((y + 1) * self.h / size).max(y * self.h / size + 1),
                );
                let mut acc = [0u64; 4];
                let mut n = 0u64;
                for sy in y0..y1.min(self.h) {
                    for sx in x0..x1.min(self.w) {
                        let i = ((sy * self.w + sx) * 4) as usize;
                        let a = self.rgba[i + 3] as u64;
                        for (k, sum) in acc.iter_mut().take(3).enumerate() {
                            *sum += self.rgba[i + k] as u64 * a;
                        }
                        acc[3] += a;
                        n += 1;
                    }
                }
                let a = acc[3];
                let c = |k: usize| acc[k].checked_div(a).unwrap_or(0) as u8;
                rgba.extend_from_slice(&[c(0), c(1), c(2), (a / n.max(1)) as u8]);
            }
        }
        Image {
            w: size,
            h: size,
            rgba,
        }
    }

    pub fn png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut e = png::Encoder::new(&mut out, self.w, self.h);
            e.set_color(png::ColorType::Rgba);
            e.set_depth(png::BitDepth::Eight);
            let mut w = e.write_header().expect("png header");
            w.write_image_data(&self.rgba).expect("png data");
        }
        out
    }
}

/// The sizes a Windows icon carries.
pub const ICO_SIZES: [u32; 5] = [16, 32, 48, 64, 256];

/// The PNG of each [`ICO_SIZES`] size (what the `.ico` and the `.exe`'s
/// icon resource hold).
pub fn ico_images(src: &Image) -> Vec<(u32, Vec<u8>)> {
    ICO_SIZES
        .iter()
        .map(|&s| (s, src.resized(s).png()))
        .collect()
}

/// A `.ico` file: `ICONDIR`, one `ICONDIRENTRY` per image, the PNGs.
pub fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, png) in images {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in images {
        out.extend_from_slice(png);
    }
    out
}

/// A macOS `.icns`: PNG entries from 16 to 1024 pixels (`icp4` … `ic10`).
pub fn icns(src: &Image) -> Vec<u8> {
    let entries: [(&[u8; 4], u32); 8] = [
        (b"icp4", 16),
        (b"ic11", 32),
        (b"icp5", 32),
        (b"ic12", 64),
        (b"ic07", 128),
        (b"ic08", 256),
        (b"ic09", 512),
        (b"ic10", 1024),
    ];
    let mut body = Vec::new();
    for (tag, size) in entries {
        let png = src.resized(size).png();
        body.extend_from_slice(tag);
        body.extend_from_slice(&(8 + png.len() as u32).to_be_bytes());
        body.extend_from_slice(&png);
    }
    let mut out = b"icns".to_vec();
    out.extend_from_slice(&(8 + body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

/// The engine's default game icon when the project names none: a 64 × 64
/// gold triangle on a dark rounded tile (Trino's three points).
pub fn default_icon() -> Image {
    let size = 64u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Rounded tile.
            let r = 10.0f32;
            let cx = fx.clamp(r, size as f32 - r);
            let cy = fy.clamp(r, size as f32 - r);
            if (fx - cx).powi(2) + (fy - cy).powi(2) > r * r {
                continue;
            }
            let mut c = [28u8, 30, 44, 255];
            // Triangle: apex (32, 12), base 14..50 at y 50.
            let t = (fy - 12.0) / 38.0;
            if (0.0..=1.0).contains(&t) && (fx - 32.0).abs() <= 18.0 * t {
                c = [255, 196, 40, 255];
            }
            rgba[i..i + 4].copy_from_slice(&c);
        }
    }
    Image {
        w: size,
        h: size,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(n: u32) -> Image {
        let mut rgba = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let on = (x + y) % 2 == 0;
                rgba.extend_from_slice(if on {
                    &[255, 0, 0, 255]
                } else {
                    &[0, 0, 255, 255]
                });
            }
        }
        Image { w: n, h: n, rgba }
    }

    #[test]
    fn resizing_keeps_pixels_up_and_averages_down() {
        let c = checker(2);
        let up = c.resized(8);
        assert_eq!(&up.rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(
            &up.rgba[(4 * 4) as usize..(4 * 4 + 4) as usize],
            &[0, 0, 255, 255]
        );
        let down = checker(4).resized(1);
        assert_eq!(&down.rgba, &[127, 0, 127, 255]);
        // Transparent pixels don't darken the average.
        let half = Image {
            w: 2,
            h: 1,
            rgba: vec![200, 100, 0, 255, 0, 0, 0, 0],
        };
        assert_eq!(&half.resized(1).rgba, &[200, 100, 0, 127]);
    }

    #[test]
    fn ico_directory_points_at_each_png() {
        let imgs = ico_images(&default_icon());
        let file = ico(&imgs);
        assert_eq!(&file[0..6], &[0, 0, 1, 0, 5, 0]);
        for (k, (size, png)) in imgs.iter().enumerate() {
            let e = &file[6 + 16 * k..6 + 16 * (k + 1)];
            assert_eq!(e[0], if *size == 256 { 0 } else { *size as u8 });
            let len = u32::from_le_bytes(e[8..12].try_into().unwrap()) as usize;
            let at = u32::from_le_bytes(e[12..16].try_into().unwrap()) as usize;
            assert_eq!(&file[at..at + len], &png[..]);
            assert_eq!(&png[1..4], b"PNG");
        }
    }

    #[test]
    fn icns_lists_png_entries_with_big_endian_lengths() {
        let file = icns(&default_icon());
        assert_eq!(&file[0..4], b"icns");
        assert_eq!(
            u32::from_be_bytes(file[4..8].try_into().unwrap()) as usize,
            file.len()
        );
        let mut at = 8;
        let mut tags = Vec::new();
        while at < file.len() {
            let len = u32::from_be_bytes(file[at + 4..at + 8].try_into().unwrap()) as usize;
            tags.push(String::from_utf8_lossy(&file[at..at + 4]).into_owned());
            assert_eq!(&file[at + 9..at + 12], b"PNG");
            at += len;
        }
        assert_eq!(at, file.len());
        assert_eq!(
            tags,
            [
                "icp4", "ic11", "icp5", "ic12", "ic07", "ic08", "ic09", "ic10"
            ]
        );
    }
}
