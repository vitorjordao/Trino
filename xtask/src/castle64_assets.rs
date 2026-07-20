//! Master-asset generator for the castle64 example (the 3D showcase game):
//! vertex-colored GLB models (blocks, articulated player parts, boar, coin,
//! star, doors, shadow) and the HUD sprites (digits + star). Called from
//! `cargo xtask gen-assets` so every generated master stays reproducible
//! from code, like the platformer's assets.
//!
//! The one asset NOT generated here is `door_kaykit.glb` — a real low-poly
//! doorway from KayKit's Dungeon Remastered pack (CC0, see the LICENSE file
//! next to it), used as the castle entrance to exercise the glTF bake on a
//! textured, multi-primitive, real-world model.

use std::path::Path;

use trino_core::Vec3;

type Rgba = [u8; 4];
/// PNG writer callback (borrowed from `main.rs::write_png`).
type PngWriter<'a> = &'a dyn Fn(&Path, u32, u32, &[u8]);

#[derive(Default)]
struct MeshBuf {
    positions: Vec<f32>,
    normals: Vec<f32>,
    colors: Vec<u8>,
    indices: Vec<u16>,
}

impl MeshBuf {
    fn vcount(&self) -> u16 {
        (self.positions.len() / 3) as u16
    }

    /// Quad with 4 CCW-from-outside vertices, one normal, one color.
    fn quad(&mut self, pts: [Vec3; 4], n: Vec3, c: Rgba) {
        let base = self.vcount();
        for p in pts {
            self.positions.extend_from_slice(&[p.x, p.y, p.z]);
            self.normals.extend_from_slice(&[n.x, n.y, n.z]);
            self.colors.extend_from_slice(&c);
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Triangle (a, b, c); winding fixed so the normal faces away from
    /// `out_ref` (a point inside the solid).
    fn tri(&mut self, mut a: Vec3, mut b: Vec3, c: Vec3, out_ref: Vec3, col: Rgba) {
        let mut n = (b - a).cross(c - a);
        let centroid = (a + b + c) * (1.0 / 3.0);
        if n.dot(centroid - out_ref) < 0.0 {
            core::mem::swap(&mut a, &mut b);
            n = -n;
        }
        let n = n.normalized();
        let base = self.vcount();
        for p in [a, b, c] {
            self.positions.extend_from_slice(&[p.x, p.y, p.z]);
            self.normals.extend_from_slice(&[n.x, n.y, n.z]);
            self.colors.extend_from_slice(&col);
        }
        self.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }

    /// Box centered at `center`; face colors ordered -Z,+Z,-X,+X,-Y,+Y.
    fn boxx(&mut self, center: Vec3, size: Vec3, faces: [Rgba; 6]) {
        let normals = [
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        ];
        let half = size * 0.5;
        for (f, n) in normals.iter().enumerate() {
            // Same construction as the engine's sample cube (winding-safe).
            let u = if n.y.abs() > 0.9 {
                Vec3::new(1.0, 0.0, 0.0)
            } else {
                Vec3::new(0.0, 1.0, 0.0).cross(*n).normalized()
            };
            let v = n.cross(u);
            let scale = |d: Vec3| Vec3::new(d.x * half.x, d.y * half.y, d.z * half.z);
            let mut pts = [Vec3::ZERO; 4];
            for (i, (su, sv)) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                .iter()
                .enumerate()
            {
                let p = *n + u * *su + v * *sv;
                pts[i] = center + scale(p);
            }
            self.quad(pts, *n, faces[f]);
        }
    }

    fn boxc(&mut self, center: Vec3, size: Vec3, c: Rgba) {
        self.boxx(center, size, [c; 6]);
    }

    /// Bipyramid: ring of `sides` vertices at `center.y`, apexes at ±h.
    fn bipyramid(
        &mut self,
        center: Vec3,
        radius: f32,
        h: f32,
        sides: u32,
        top: Rgba,
        bottom: Rgba,
    ) {
        let apex_t = center + Vec3::new(0.0, h, 0.0);
        let apex_b = center - Vec3::new(0.0, h, 0.0);
        for i in 0..sides {
            let a0 = i as f32 / sides as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / sides as f32 * std::f32::consts::TAU;
            let p0 = center + Vec3::new(radius * a0.cos(), 0.0, radius * a0.sin());
            let p1 = center + Vec3::new(radius * a1.cos(), 0.0, radius * a1.sin());
            self.tri(p0, p1, apex_t, center, top);
            self.tri(p0, p1, apex_b, center, bottom);
        }
    }

    /// Hand-assembled GLB (JSON + BIN chunks), like `gen_cube_glb`.
    fn write_glb(&self, path: &Path) {
        assert!(self.positions.len() / 3 < u16::MAX as usize);
        let mut bin: Vec<u8> = Vec::new();
        for v in self.positions.iter().chain(self.normals.iter()) {
            bin.extend_from_slice(&v.to_le_bytes());
        }
        let colors_offset = bin.len();
        bin.extend_from_slice(&self.colors);
        let indices_offset = bin.len();
        for i in &self.indices {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }

        let vcount = self.positions.len() / 3;
        let normals_offset = self.positions.len() * 4;
        let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in self.positions.chunks(3) {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        let json = format!(
            r#"{{"asset":{{"version":"2.0","generator":"trino gen-assets"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0,"name":"m"}}],"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1,"COLOR_0":2}},"indices":3}}]}}],"buffers":[{{"byteLength":{}}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":{}}},{{"buffer":0,"byteOffset":{normals_offset},"byteLength":{}}},{{"buffer":0,"byteOffset":{colors_offset},"byteLength":{}}},{{"buffer":0,"byteOffset":{indices_offset},"byteLength":{}}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":{vcount},"type":"VEC3","min":[{},{},{}],"max":[{},{},{}]}},{{"bufferView":1,"componentType":5126,"count":{vcount},"type":"VEC3"}},{{"bufferView":2,"componentType":5121,"normalized":true,"count":{vcount},"type":"VEC4"}},{{"bufferView":3,"componentType":5123,"count":{},"type":"SCALAR"}}]}}"#,
            bin.len(),
            self.positions.len() * 4,
            self.normals.len() * 4,
            self.colors.len(),
            self.indices.len() * 2,
            min[0],
            min[1],
            min[2],
            max[0],
            max[1],
            max[2],
            self.indices.len(),
        );
        let mut json_bytes = json.into_bytes();
        while !json_bytes.len().is_multiple_of(4) {
            json_bytes.push(b' ');
        }
        let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
        let mut glb: Vec<u8> = Vec::with_capacity(total);
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(total as u32).to_le_bytes());
        glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(&json_bytes);
        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        glb.extend_from_slice(&bin);
        std::fs::write(path, glb).unwrap();
    }
}

fn v3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

/// Unit block (1x1x1 at the origin) with top/side/bottom colors.
fn block(dir: &Path, name: &str, top: Rgba, side: Rgba, bottom: Rgba) {
    let mut m = MeshBuf::default();
    m.boxx(Vec3::ZERO, Vec3::ONE, [side, side, side, side, bottom, top]);
    m.write_glb(&dir.join(format!("{name}.glb")));
}

/// Neutral door (frame + pale slab) — the game tints it per destination.
fn door_frame(dir: &Path) {
    let frame: Rgba = [225, 220, 210, 255];
    let slab: Rgba = [245, 242, 235, 255];
    let mut m = MeshBuf::default();
    m.boxc(v3(0.0, 0.9, 0.0), v3(1.1, 1.8, 0.12), slab);
    m.boxc(v3(-0.62, 1.0, 0.0), v3(0.15, 2.0, 0.2), frame);
    m.boxc(v3(0.62, 1.0, 0.0), v3(0.15, 2.0, 0.2), frame);
    m.boxc(v3(0.0, 2.05, 0.0), v3(1.4, 0.15, 0.2), frame);
    m.boxc(v3(0.35, 0.95, 0.08), v3(0.1, 0.1, 0.06), [90, 90, 95, 255]);
    m.write_glb(&dir.join("door_frame.glb"));
}

/// Articulated purple player: torso+head in one mesh; arm and leg meshes
/// have their ORIGIN AT THE PIVOT (shoulder/hip) so rotation.x swings them.
fn player_parts(dir: &Path) {
    let purple: Rgba = [140, 70, 195, 255];
    let sleeve: Rgba = [170, 110, 225, 255];
    let pants: Rgba = [80, 50, 125, 255];
    let skin: Rgba = [245, 200, 160, 255];
    let hair: Rgba = [55, 40, 70, 255];

    let mut torso = MeshBuf::default();
    torso.boxc(v3(0.0, 0.52, 0.0), v3(0.50, 0.36, 0.34), purple);
    torso.boxc(v3(0.0, 0.82, 0.0), v3(0.38, 0.28, 0.34), skin);
    torso.boxc(
        v3(0.0, 0.80, 0.21),
        v3(0.10, 0.08, 0.08),
        [230, 180, 140, 255],
    );
    torso.boxc(v3(0.0, 0.99, 0.0), v3(0.40, 0.10, 0.36), hair);
    torso.boxc(v3(0.0, 0.90, -0.19), v3(0.40, 0.20, 0.06), hair);
    torso.write_glb(&dir.join("player_torso.glb"));

    let mut arm = MeshBuf::default();
    arm.boxc(v3(0.0, -0.16, 0.0), v3(0.13, 0.32, 0.13), sleeve);
    arm.boxc(v3(0.0, -0.36, 0.0), v3(0.11, 0.10, 0.11), skin);
    arm.write_glb(&dir.join("player_arm.glb"));

    let mut leg = MeshBuf::default();
    leg.boxc(v3(0.0, -0.17, 0.0), v3(0.16, 0.34, 0.20), pants);
    leg.boxc(
        v3(0.0, -0.36, 0.03),
        v3(0.17, 0.08, 0.26),
        [45, 35, 55, 255],
    );
    leg.write_glb(&dir.join("player_leg.glb"));
}

/// Patrol boar: low body, snout, tusks, four stub legs.
fn boar(dir: &Path) {
    let hide: Rgba = [105, 75, 55, 255];
    let dark: Rgba = [70, 50, 38, 255];
    let tusk: Rgba = [235, 230, 215, 255];
    let mut m = MeshBuf::default();
    m.boxc(v3(0.0, 0.34, -0.05), v3(0.55, 0.34, 0.62), hide);
    m.boxc(v3(0.0, 0.42, -0.05), v3(0.30, 0.10, 0.55), dark);
    m.boxc(v3(0.0, 0.30, 0.34), v3(0.34, 0.26, 0.22), hide);
    m.boxc(v3(0.0, 0.24, 0.48), v3(0.16, 0.12, 0.10), dark);
    m.boxc(v3(-0.11, 0.20, 0.44), v3(0.04, 0.10, 0.04), tusk);
    m.boxc(v3(0.11, 0.20, 0.44), v3(0.04, 0.10, 0.04), tusk);
    m.boxc(v3(-0.17, 0.09, 0.22), v3(0.10, 0.18, 0.10), dark);
    m.boxc(v3(0.17, 0.09, 0.22), v3(0.10, 0.18, 0.10), dark);
    m.boxc(v3(-0.17, 0.09, -0.30), v3(0.10, 0.18, 0.10), dark);
    m.boxc(v3(0.17, 0.09, -0.30), v3(0.10, 0.18, 0.10), dark);
    m.write_glb(&dir.join("boar.glb"));
}

/// 8x8 HUD digit sprites (classic 3x5 font) + the star icon.
fn hud_sprites(dir: &Path, write_png: PngWriter) {
    const FONT: [[u8; 5]; 10] = [
        [0b111, 0b101, 0b101, 0b101, 0b111],
        [0b010, 0b110, 0b010, 0b010, 0b111],
        [0b111, 0b001, 0b111, 0b100, 0b111],
        [0b111, 0b001, 0b111, 0b001, 0b111],
        [0b101, 0b101, 0b111, 0b001, 0b001],
        [0b111, 0b100, 0b111, 0b001, 0b111],
        [0b111, 0b100, 0b111, 0b101, 0b111],
        [0b111, 0b001, 0b010, 0b010, 0b010],
        [0b111, 0b101, 0b111, 0b101, 0b111],
        [0b111, 0b101, 0b111, 0b001, 0b111],
    ];
    for (d, rows) in FONT.iter().enumerate() {
        let mut rgba = vec![0u8; 8 * 8 * 4];
        for (ry, bits) in rows.iter().enumerate() {
            for rx in 0..3 {
                if bits & (0b100 >> rx) != 0 {
                    for sx in 0..2 {
                        let at = ((1 + ry) * 8 + 1 + rx * 2 + sx) * 4;
                        rgba[at..at + 4].copy_from_slice(&[255, 255, 255, 255]);
                    }
                }
            }
        }
        write_png(&dir.join(format!("digit{d}.png")), 8, 8, &rgba);
    }

    let art = [
        "...##...", "...##...", "########", ".######.", "..####..", ".######.", ".##..##.",
        "........",
    ];
    let mut rgba = Vec::with_capacity(8 * 8 * 4);
    for row in art {
        for ch in row.chars() {
            let c: Rgba = if ch == '#' {
                [255, 220, 70, 255]
            } else {
                [0, 0, 0, 0]
            };
            rgba.extend_from_slice(&c);
        }
    }
    write_png(&dir.join("star.png"), 8, 8, &rgba);
}

/// Regenerate every generated castle64 master (models + HUD sprites).
pub fn gen_all(repo_root: &Path, write_png: PngWriter) {
    let models = repo_root.join("assets/shared/models/castle64");
    let sprites = repo_root.join("assets/shared/sprites/castle64");
    std::fs::create_dir_all(&models).unwrap();
    std::fs::create_dir_all(&sprites).unwrap();

    block(
        &models,
        "block_grass",
        [95, 180, 60, 255],
        [134, 96, 60, 255],
        [90, 64, 40, 255],
    );
    block(
        &models,
        "block_stone",
        [155, 155, 160, 255],
        [125, 125, 130, 255],
        [100, 100, 105, 255],
    );
    block(
        &models,
        "block_brick",
        [190, 85, 60, 255],
        [170, 70, 50, 255],
        [120, 50, 40, 255],
    );
    block(
        &models,
        "block_castle",
        [240, 232, 210, 255],
        [232, 222, 196, 255],
        [200, 190, 170, 255],
    );
    block(
        &models,
        "block_roof",
        [205, 60, 50, 255],
        [185, 50, 45, 255],
        [150, 40, 38, 255],
    );
    block(
        &models,
        "block_lava",
        [255, 145, 30, 255],
        [205, 65, 20, 255],
        [120, 30, 10, 255],
    );

    let mut coin = MeshBuf::default();
    coin.bipyramid(
        Vec3::ZERO,
        0.35,
        0.45,
        4,
        [255, 210, 60, 255],
        [230, 170, 30, 255],
    );
    coin.write_glb(&models.join("coin.glb"));

    let mut star = MeshBuf::default();
    star.bipyramid(
        Vec3::ZERO,
        0.55,
        0.28,
        5,
        [255, 222, 80, 255],
        [255, 190, 40, 255],
    );
    star.write_glb(&models.join("star.glb"));

    player_parts(&models);
    boar(&models);
    door_frame(&models);

    let mut shadow = MeshBuf::default();
    shadow.boxc(Vec3::ZERO, v3(1.0, 0.02, 1.0), [35, 35, 40, 255]);
    shadow.write_glb(&models.join("shadow.glb"));

    hud_sprites(&sprites, write_png);
}
