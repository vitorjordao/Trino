//! Emulator golden images: the console self-tests print their framebuffer
//! as hex rows (`TRINO_FBR:<y>:<x0>:<hex RGBA5551>` then
//! `TRINO_FB_END:<w>x<h>:<fnv1a32>`); this rebuilds the image, checks the
//! hash, and compares it pixel-exactly with `tests/golden/<target>/<name>.png`
//! (`--bless` writes it). The emulator is deterministic and so is the
//! engine's T&L, so any difference is a real change — review the PNG.

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Default)]
pub struct FbCapture {
    rows: BTreeMap<(u32, u32), Vec<u16>>,
    end: Option<(u32, u32, u32)>,
}

impl FbCapture {
    /// Take a log line; true if it belonged to the dump.
    pub fn feed(&mut self, line: &str) -> bool {
        if let Some(i) = line.find("TRINO_FBR:") {
            let mut parts = line[i + 10..].trim().splitn(3, ':');
            let (Some(y), Some(x0), Some(hex)) = (parts.next(), parts.next(), parts.next()) else {
                return true;
            };
            let (Ok(y), Ok(x0)) = (y.parse(), x0.parse()) else {
                return true;
            };
            let px = hex
                .as_bytes()
                .chunks(4)
                .filter_map(|c| u16::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok())
                .collect();
            self.rows.insert((y, x0), px);
            return true;
        }
        if let Some(i) = line.find("TRINO_FB_END:") {
            let rest = line[i + 13..].trim();
            let parsed = (|| {
                let (size, hash) = rest.split_once(':')?;
                let (w, h) = size.split_once('x')?;
                Some((
                    w.parse().ok()?,
                    h.parse().ok()?,
                    u32::from_str_radix(hash, 16).ok()?,
                ))
            })();
            self.end = parsed;
            return true;
        }
        false
    }

    /// The RGBA8 image (5-bit channels expanded like the console's
    /// `trino_debug_pixel`), verified against the printed hash.
    pub fn image(&self) -> Result<(u32, u32, Vec<u8>), String> {
        let (w, h, hash) = self.end.ok_or("no TRINO_FB_END line")?;
        let mut px = vec![0u16; (w * h) as usize];
        for (&(y, x0), row) in &self.rows {
            for (i, &p) in row.iter().enumerate() {
                let x = x0 + i as u32;
                if x < w && y < h {
                    px[(y * w + x) as usize] = p;
                }
            }
        }
        let mut fnv: u32 = 2166136261;
        for p in &px {
            for b in [(p >> 8) as u8, *p as u8] {
                fnv = (fnv ^ b as u32).wrapping_mul(16777619);
            }
        }
        if fnv != hash {
            return Err(format!(
                "framebuffer dump hash mismatch ({fnv:08x} vs {hash:08x}): lines lost?"
            ));
        }
        let expand = |c: u16| ((c << 3) | (c >> 2)) as u8;
        let rgba = px
            .iter()
            .flat_map(|&p| {
                [
                    expand((p >> 11) & 31),
                    expand((p >> 6) & 31),
                    expand((p >> 1) & 31),
                    255,
                ]
            })
            .collect();
        Ok((w, h, rgba))
    }
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())
}

fn read_png(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("png too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(info.buffer_size());
    if info.color_type != png::ColorType::Rgba {
        return Err(format!("{}: expected RGBA", path.display()));
    }
    Ok((info.width, info.height, buf))
}

/// How far two frames are apart: mean of the per-pixel largest channel
/// difference, and the fraction of pixels differing by more than
/// [`FAR`] (a different color, not a rounding or edge-coverage step).
pub fn distance(a: &[u8], b: &[u8]) -> (f32, f32) {
    let diffs: Vec<u8> = a
        .chunks(4)
        .zip(b.chunks(4))
        .map(|(a, b)| (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap())
        .collect();
    let n = diffs.len().max(1) as f32;
    let mean = diffs.iter().map(|&d| d as f32).sum::<f32>() / n;
    let far = diffs.iter().filter(|&&d| d > FAR).count() as f32 / n;
    (mean, far)
}

/// A channel difference above this is a different color.
pub const FAR: u8 = 32;

/// Like [`check`], within a tolerance: for emulators whose GPU output
/// depends on the host renderer (Azahar's OpenGL backend differs between a
/// desktop GPU and CI's software rasterizer at triangle edges). Fails above
/// `max_mean` mean difference or `max_far` fraction of far pixels.
pub fn check_close(
    root: &Path,
    target: &str,
    name: &str,
    img: (u32, u32, Vec<u8>),
    bless: bool,
    max_mean: f32,
    max_far: f32,
) -> Result<String, String> {
    let dir = root.join("tests/golden").join(target);
    let path = dir.join(format!("{name}.png"));
    let (w, h, rgba) = img;
    if bless {
        write_png(&path, w, h, &rgba)?;
        return Ok(format!("blessed {}", path.display()));
    }
    if !path.exists() {
        return Err(format!(
            "{} missing — run with --bless once and commit it",
            path.display()
        ));
    }
    let (gw, gh, golden) = read_png(&path)?;
    let actual = dir.join("actual").join(format!("{name}.png"));
    if (gw, gh) != (w, h) {
        write_png(&actual, w, h, &rgba)?;
        return Err(format!("{} differs in size", path.display()));
    }
    let (mean, far) = distance(&rgba, &golden);
    let msg = format!(
        "{}: mean diff {mean:.2}, {:.2}% far pixels (limits {max_mean}, {:.1}%)",
        path.display(),
        far * 100.0,
        max_far * 100.0
    );
    if mean <= max_mean && far <= max_far {
        return Ok(msg);
    }
    write_png(&actual, w, h, &rgba)?;
    Err(format!("{msg}; actual written to {}", actual.display()))
}

/// Compare (or with `bless`, write) `tests/golden/<target>/<name>.png`.
/// On mismatch the actual image goes to `.../actual/<name>.png`.
pub fn check(
    root: &Path,
    target: &str,
    name: &str,
    img: (u32, u32, Vec<u8>),
    bless: bool,
) -> Result<String, String> {
    let dir = root.join("tests/golden").join(target);
    let path = dir.join(format!("{name}.png"));
    let (w, h, rgba) = img;
    if bless {
        write_png(&path, w, h, &rgba)?;
        return Ok(format!("blessed {}", path.display()));
    }
    if !path.exists() {
        return Err(format!(
            "{} missing — run with --bless once and commit it",
            path.display()
        ));
    }
    let (gw, gh, golden) = read_png(&path)?;
    let differ = if (gw, gh) != (w, h) {
        usize::MAX
    } else {
        rgba.chunks(4)
            .zip(golden.chunks(4))
            .filter(|(a, b)| a != b)
            .count()
    };
    if differ == 0 {
        return Ok(format!("{} matches", path.display()));
    }
    let actual = dir.join("actual").join(format!("{name}.png"));
    write_png(&actual, w, h, &rgba)?;
    Err(format!(
        "{} differs ({} pixels); actual written to {}",
        path.display(),
        if differ == usize::MAX {
            "size".to_string()
        } else {
            differ.to_string()
        },
        actual.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuilds_and_verifies_a_dump() {
        let px: [u16; 3] = [0xf801, 0x07c1, 0x003f];
        let mut fnv: u32 = 2166136261;
        for p in px {
            for b in [(p >> 8) as u8, p as u8] {
                fnv = (fnv ^ b as u32).wrapping_mul(16777619);
            }
        }
        let mut c = FbCapture::default();
        assert!(c.feed("[ares] TRINO_FBR:0:0:f80107c1"));
        assert!(c.feed("TRINO_FBR:0:2:003f"));
        assert!(!c.feed("TRINO_STEP:x"));
        assert!(c.feed(&format!("TRINO_FB_END:3x1:{fnv:08x}")));
        let (w, h, rgba) = c.image().unwrap();
        assert_eq!((w, h), (3, 1));
        assert_eq!(&rgba[..4], &[255, 0, 0, 255]);
        assert_eq!(&rgba[4..8], &[0, 255, 0, 255]);
        assert_eq!(&rgba[8..12], &[0, 0, 255, 255]);
        let mut bad = FbCapture::default();
        bad.feed("TRINO_FBR:0:0:f801");
        bad.feed(&format!("TRINO_FB_END:3x1:{fnv:08x}"));
        assert!(bad.image().is_err(), "lost rows are detected");
    }
}
