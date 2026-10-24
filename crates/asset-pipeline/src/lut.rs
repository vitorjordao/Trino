//! Color-grading LUTs (issue #64, ADR 0033): `[luts.<name>]` masters baked
//! to `TLUT` blobs (`trino_core::post::Lut`) for the PC and the Web, which
//! grade at run time, and applied by the bake to the colors of the console
//! assets that name one (`grading = "luts/<name>"` on a model or a sprite,
//! `bake_grading = true` on a scene), the consoles' grading at zero cost.
//!
//! A master is either:
//! - a **strip PNG** `n²×n`: `n` slices of blue side by side, red along x
//!   inside a slice, green down y (the layout most tools export; `cargo
//!   xtask lut neutral` writes the identity to start from), or
//! - an Adobe/Resolve **`.cube`** file (`LUT_3D_SIZE n`, `n³` lines of
//!   `r g b` in 0..1, red fastest; `DOMAIN_MIN`/`DOMAIN_MAX` must be the
//!   defaults).

use std::path::Path;

use trino_core::post::{LUT_MAX, LUT_MIN, Lut};

/// A LUT master's bytes as a `TLUT` blob.
pub fn bake(path: &Path) -> Result<Vec<u8>, String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => {
            let (w, h, rgba) = crate::bake::decode_png(path)?;
            from_strip(w, h, &rgba).map_err(|e| format!("{}: {e}", path.display()))
        }
        "cube" => {
            let text =
                std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            from_cube(&text).map_err(|e| format!("{}: {e}", path.display()))
        }
        _ => Err(format!(
            "{}: a LUT is a strip `.png` (n²×n) or a `.cube` file",
            path.display()
        )),
    }
}

fn check_size(n: u32) -> Result<u8, String> {
    if n < LUT_MIN as u32 || n > LUT_MAX as u32 {
        return Err(format!(
            "a LUT has {LUT_MIN}..={LUT_MAX} texels per axis, this one {n}"
        ));
    }
    Ok(n as u8)
}

/// A strip image (`n²×n` RGBA8) as a `TLUT` blob.
pub fn from_strip(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    if w != h * h {
        return Err(format!(
            "a strip LUT is n²×n pixels (n slices of n×n side by side), this one {w}×{h}"
        ));
    }
    let n = check_size(h)?;
    let n32 = n as u32;
    let mut out = Lut::header(n).to_vec();
    out.reserve(Lut::texel_bytes(n));
    for b in 0..n32 {
        for g in 0..n32 {
            for r in 0..n32 {
                let i = ((g * w + b * n32 + r) * 4) as usize;
                out.extend_from_slice(&rgba[i..i + 3]);
            }
        }
    }
    Ok(out)
}

/// A `TLUT` blob as a strip image (`n²×n` RGBA8): what [`from_strip`]
/// reads back.
pub fn to_strip(lut: &Lut<'_>) -> (u32, u32, Vec<u8>) {
    let n = lut.size() as u32;
    let w = n * n;
    let mut rgba = vec![0u8; (w * n * 4) as usize];
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let t = lut.texel(r, g, b);
                let i = ((g * w + b * n + r) * 4) as usize;
                rgba[i..i + 3].copy_from_slice(&t);
                rgba[i + 3] = 255;
            }
        }
    }
    (w, n, rgba)
}

/// The identity LUT of `size` as a `TLUT` blob.
pub fn neutral(size: u8) -> Vec<u8> {
    let n = size as u32;
    let mut out = Lut::header(size).to_vec();
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                out.extend([
                    Lut::neutral(size, r),
                    Lut::neutral(size, g),
                    Lut::neutral(size, b),
                ]);
            }
        }
    }
    out
}

/// An Adobe/Resolve `.cube` 3D LUT as a `TLUT` blob.
pub fn from_cube(text: &str) -> Result<Vec<u8>, String> {
    let mut size = None;
    let mut values: Vec<[f32; 3]> = Vec::new();
    for (no, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let first = words.next().unwrap_or_default();
        match first {
            "TITLE" => {}
            "LUT_3D_SIZE" => {
                let n = words
                    .next()
                    .and_then(|v| v.parse::<u32>().ok())
                    .ok_or_else(|| format!("line {}: LUT_3D_SIZE needs a number", no + 1))?;
                size = Some(check_size(n)?);
            }
            "LUT_1D_SIZE" => return Err("a 1D LUT can't grade colors: use LUT_3D_SIZE".into()),
            "DOMAIN_MIN" | "DOMAIN_MAX" => {
                let want = if first == "DOMAIN_MIN" { 0.0 } else { 1.0 };
                let v: Vec<f32> = words.filter_map(|w| w.parse().ok()).collect();
                if v.len() != 3 || v.iter().any(|x| (x - want).abs() > 1e-6) {
                    return Err(format!(
                        "line {}: only the default {first} is supported",
                        no + 1
                    ));
                }
            }
            _ => {
                let mut v = [0.0f32; 3];
                let parts: Vec<&str> = core::iter::once(first).chain(words).collect();
                if parts.len() != 3 {
                    return Err(format!("line {}: expected `r g b`", no + 1));
                }
                for (k, p) in parts.iter().enumerate() {
                    v[k] = p
                        .parse()
                        .map_err(|_| format!("line {}: `{p}` is not a number", no + 1))?;
                }
                values.push(v);
            }
        }
    }
    let n = size.ok_or("no LUT_3D_SIZE")?;
    let want = (n as usize).pow(3);
    if values.len() != want {
        return Err(format!(
            "{} values for LUT_3D_SIZE {n} (want {want})",
            values.len()
        ));
    }
    let mut out = Lut::header(n).to_vec();
    for v in values {
        out.extend(v.map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8));
    }
    Ok(out)
}

/// Grade a mesh's vertex colors (display sRGB, as the bake stores them).
pub fn grade_mesh(data: &mut crate::model::MeshData, lut: &Lut<'_>) {
    for c in &mut data.colors {
        let g = lut.apply_color(trino_core::Color::rgba(c[0], c[1], c[2], c[3]));
        *c = [g.r, g.g, g.b, g.a];
    }
}

/// Grade RGBA8 pixels in place (alpha kept).
pub fn grade_rgba(rgba: &mut [u8], lut: &Lut<'_>) {
    for p in rgba.as_chunks_mut::<4>().0 {
        let g = lut.apply_color(trino_core::Color::rgba(p[0], p[1], p[2], p[3]));
        p[..3].copy_from_slice(&[g.r, g.g, g.b]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_round_trip_and_the_neutral_one_is_the_identity() {
        let blob = neutral(16);
        let lut = Lut::parse(&blob).unwrap();
        let (w, h, rgba) = to_strip(&lut);
        assert_eq!((w, h), (256, 16));
        assert_eq!(from_strip(w, h, &rgba).unwrap(), blob);
        // A red ramp along x of the first slice, green down y.
        assert_eq!(&rgba[..4], &[0, 0, 0, 255]);
        let x15 = (15 * 4) as usize;
        assert_eq!(&rgba[x15..x15 + 3], &[255, 0, 0]);
        let row15 = (15 * 256 * 4) as usize;
        assert_eq!(&rgba[row15..row15 + 3], &[0, 255, 0]);
        let last_slice = (15 * 16 * 4) as usize;
        assert_eq!(&rgba[last_slice..last_slice + 3], &[0, 0, 255]);
        assert!(
            from_strip(90, 10, &vec![0; 3600])
                .unwrap_err()
                .contains("n²×n")
        );
        assert!(
            from_strip(1, 1, &[0; 4])
                .unwrap_err()
                .contains("texels per axis")
        );
    }

    #[test]
    fn cube_files_parse_like_the_strip() {
        let mut text = String::from("TITLE \"x\"\n# comment\nLUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\n");
        for i in 0..8u32 {
            let (r, g, b) = (i & 1, (i >> 1) & 1, (i >> 2) & 1);
            text.push_str(&format!("{r}.0 {g}.0 {b}.0\n"));
        }
        assert_eq!(from_cube(&text).unwrap(), neutral(2));
        assert!(
            from_cube("LUT_3D_SIZE 2\n0 0 0\n")
                .unwrap_err()
                .contains("1 values")
        );
        assert!(from_cube("LUT_1D_SIZE 16\n").is_err());
        assert!(from_cube("DOMAIN_MAX 2 2 2\n").is_err());
        assert!(from_cube("0 0\n").is_err());
    }

    #[test]
    fn grading_vertex_colors_equals_grading_the_frame_for_unlit_colors() {
        // A warm LUT: red up, blue down.
        let n = 9u8;
        let mut b = Lut::header(n).to_vec();
        for bl in 0..n as u32 {
            for g in 0..n as u32 {
                for r in 0..n as u32 {
                    let v = |i| Lut::neutral(n, i) as f32;
                    b.extend([
                        (v(r) * 1.1).min(255.0) as u8,
                        v(g) as u8,
                        (v(bl) * 0.8) as u8,
                    ]);
                }
            }
        }
        let lut = Lut::parse(&b).unwrap();
        let mut mesh = crate::model::MeshData {
            colors: vec![[200, 120, 40, 255], [10, 20, 250, 128]],
            ..Default::default()
        };
        grade_mesh(&mut mesh, &lut);
        let mut rgba = vec![200, 120, 40, 255, 10, 20, 250, 128];
        grade_rgba(&mut rgba, &lut);
        assert_eq!(mesh.colors.concat(), rgba);
        assert_eq!(mesh.colors[1][3], 128, "alpha kept");
        assert!(mesh.colors[0][0] > 200 && mesh.colors[1][2] < 250);
    }
}
