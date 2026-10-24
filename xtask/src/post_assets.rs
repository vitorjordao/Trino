//! The post stack's generated masters (issue #64, `cargo xtask
//! gen-assets`):
//!
//! - `sprites/glow.png`: the engine's halo (`trino_core::Glow::SPRITE`), a
//!   white disc whose alpha falls off smoothly to the edge (the consoles'
//!   bloom; an I4 mask on the N64).
//! - `luts/selftest.png`: a 17³ LUT that rotates the channels, `(r, g, b)
//!   → (g, b, r)` — exact under trilinear filtering, so the `post` self-test
//!   step knows every target's graded color.
//! - `luts/dusk.png`: a 32³ warm evening grade (lifted blacks toward
//!   violet, warm highlights, a little less saturation) for demos.

use std::path::Path;

use crate::castle64_assets::PngWriter;

pub fn gen_all(root: &Path, write_png: PngWriter) {
    let shared = root.join("assets/shared");
    std::fs::create_dir_all(shared.join("sprites")).unwrap();
    std::fs::create_dir_all(shared.join("luts")).unwrap();

    let size = 32u32;
    let mut glow = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let dx = (x as f32 + 0.5 - 16.0) / 16.0;
            let dy = (y as f32 + 0.5 - 16.0) / 16.0;
            let r2 = (dx * dx + dy * dy).min(1.0);
            let a = (1.0 - r2) * (1.0 - r2);
            glow.extend_from_slice(&[255, 255, 255, (a * 255.0 + 0.5) as u8]);
        }
    }
    write_png(&shared.join("sprites/glow.png"), size, size, &glow);

    let strip = |n: u32, f: &dyn Fn([f32; 3]) -> [f32; 3]| {
        let w = n * n;
        let mut rgba = vec![0u8; (w * n * 4) as usize];
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let v = |i: u32| trino_core::Lut::neutral(n as u8, i) as f32 / 255.0;
                    let o = f([v(r), v(g), v(b)]);
                    let i = ((g * w + b * n + r) * 4) as usize;
                    for k in 0..3 {
                        rgba[i + k] = (o[k].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    }
                    rgba[i + 3] = 255;
                }
            }
        }
        (w, n, rgba)
    };
    let (w, h, rotate) = strip(17, &|[r, g, b]| [g, b, r]);
    write_png(&shared.join("luts/selftest.png"), w, h, &rotate);
    let (w, h, dusk) = strip(32, &|c| {
        let luma = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        // Less saturation, then a split tone: shadows toward violet,
        // highlights toward amber, a gentle S-curve.
        let s = c.map(|v| luma + (v - luma) * 0.8);
        let shadow = 1.0 - luma;
        let tone = [
            0.04 * shadow + 0.10 * luma,
            0.01 * shadow + 0.03 * luma,
            0.07 * shadow - 0.06 * luma,
        ];
        let curve = |v: f32| v * v * (3.0 - 2.0 * v) * 0.35 + v * 0.65;
        [0, 1, 2].map(|k| curve(s[k] * 0.92 + tone[k]))
    });
    write_png(&shared.join("luts/dusk.png"), w, h, &dusk);
}
