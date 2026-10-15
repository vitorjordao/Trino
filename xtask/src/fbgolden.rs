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
    /// The dump's end line arrived (the next rows start a new image).
    pub fn done(&self) -> bool {
        self.end.is_some()
    }

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

    /// Console log lines for an RGBA5551 framebuffer, as the self-tests print
    /// them (rows split in two chunks, like the ROM's line-length cap does).
    fn dump(w: u32, h: u32, px: &[u16]) -> Vec<String> {
        let mut lines = Vec::new();
        let mut fnv: u32 = 2166136261;
        for p in px {
            for b in [(p >> 8) as u8, *p as u8] {
                fnv = (fnv ^ b as u32).wrapping_mul(16777619);
            }
        }
        for y in 0..h {
            let row = &px[(y * w) as usize..((y + 1) * w) as usize];
            let half = (w / 2) as usize;
            for (x0, chunk) in [(0, &row[..half]), (half, &row[half..])] {
                let hex: String = chunk.iter().map(|p| format!("{p:04x}")).collect();
                lines.push(format!("[emu] TRINO_FBR:{y}:{x0}:{hex}"));
            }
        }
        lines.push(format!("TRINO_FB_END:{w}x{h}:{fnv:08x}"));
        lines
    }

    fn capture(lines: &[String]) -> FbCapture {
        let mut c = FbCapture::default();
        for l in lines {
            assert!(c.feed(l), "{l}");
        }
        assert!(
            !c.feed("TRINO_STEP:unrelated"),
            "other lines are not the dump"
        );
        c
    }

    /// 4x2: red, green, blue, white / black, grey ramp (RGBA5551, alpha bit set).
    const PX: [u16; 8] = [
        0xf801, 0x07c1, 0x003f, 0xffff, 0x0001, 0x4211, 0x8421, 0xc631,
    ];

    #[test]
    fn framebuffer_dumps_round_trip_through_bless_and_check() {
        let root = tempfile::tempdir().unwrap();
        let c = capture(&dump(4, 2, &PX));
        assert!(c.done());
        let img = c.image().unwrap();
        assert_eq!((img.0, img.1), (4, 2));
        assert_eq!(
            &img.2[..16],
            &[
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255
            ]
        );

        // No golden yet: a clear error, then --bless writes it.
        let err = check(root.path(), "n64", "frame", img.clone(), false).unwrap_err();
        assert!(err.contains("run with --bless"), "{err}");
        let msg = check(root.path(), "n64", "frame", img.clone(), true).unwrap();
        assert!(msg.starts_with("blessed"), "{msg}");
        assert!(root.path().join("tests/golden/n64/frame.png").is_file());
        let ok = check(root.path(), "n64", "frame", img.clone(), false).unwrap();
        assert!(ok.ends_with("matches"), "{ok}");

        // One pixel off: pixel-exact check fails and writes the actual image.
        let mut px = PX;
        px[5] ^= 0x0800;
        let changed = capture(&dump(4, 2, &px)).image().unwrap();
        let err = check(root.path(), "n64", "frame", changed, false).unwrap_err();
        assert!(err.contains("differs (1 pixels)"), "{err}");
        assert!(
            root.path()
                .join("tests/golden/n64/actual/frame.png")
                .is_file()
        );

        // A different size is reported as such.
        let small = capture(&dump(2, 1, &PX[..2])).image().unwrap();
        let err = check(root.path(), "n64", "frame", small, false).unwrap_err();
        assert!(err.contains("differs (size"), "{err}");
    }

    #[test]
    fn lost_or_garbled_dump_lines_are_detected() {
        let lines = dump(4, 2, &PX);
        // A dropped row chunk: the hash no longer matches.
        let partial: Vec<String> = lines.iter().skip(1).cloned().collect();
        let err = capture(&partial).image().unwrap_err();
        assert!(err.contains("hash mismatch"), "{err}");
        // Malformed lines still belong to the dump but carry nothing.
        let mut c = FbCapture::default();
        assert!(c.feed("TRINO_FBR:x:0:ffff"));
        assert!(c.feed("TRINO_FBR:0"));
        assert!(c.feed("TRINO_FB_END:garbage"));
        assert!(!c.done(), "an unparsable end line is no end");
        assert!(c.image().unwrap_err().contains("no TRINO_FB_END"));
    }

    #[test]
    fn tolerant_goldens_accept_small_drift_and_reject_far_pixels() {
        let root = tempfile::tempdir().unwrap();
        let (w, h) = (8u32, 8u32);
        let base: Vec<u8> = (0..w * h)
            .flat_map(|i| [i as u8 * 3, 100, 200, 255])
            .collect();
        let msg = check_close(
            root.path(),
            "3ds",
            "hub",
            (w, h, base.clone()),
            true,
            2.0,
            0.02,
        )
        .unwrap();
        assert!(msg.starts_with("blessed"));

        // +2 on one channel everywhere: mean 2, no far pixels.
        let near: Vec<u8> = base
            .chunks(4)
            .flat_map(|p| [p[0], p[1] + 2, p[2], p[3]])
            .collect();
        assert_eq!(distance(&near, &base), (2.0, 0.0));
        let ok = check_close(root.path(), "3ds", "hub", (w, h, near), false, 2.0, 0.02).unwrap();
        assert!(ok.contains("mean diff 2.00"), "{ok}");

        // Two pixels of a different color: 2/64 > 2% far pixels.
        let mut far = base.clone();
        far[0] = far[0].wrapping_add(100);
        far[4] = far[4].wrapping_add(100);
        let (_, frac) = distance(&far, &base);
        assert!((frac - 2.0 / 64.0).abs() < 1e-6);
        let err =
            check_close(root.path(), "3ds", "hub", (w, h, far), false, 2.0, 0.02).unwrap_err();
        assert!(err.contains("actual written to"), "{err}");
        assert!(
            root.path()
                .join("tests/golden/3ds/actual/hub.png")
                .is_file()
        );

        let err = check_close(
            root.path(),
            "3ds",
            "hub",
            (4, 4, vec![0; 64]),
            false,
            2.0,
            0.02,
        )
        .unwrap_err();
        assert!(err.contains("differs in size"), "{err}");
        let err =
            check_close(root.path(), "3ds", "none", (w, h, base), false, 2.0, 0.02).unwrap_err();
        assert!(err.contains("--bless"), "{err}");
    }
}
