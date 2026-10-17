//! `trino5x7`: the engine's default pixel font, drawn here as ASCII art and
//! written to `assets/shared/fonts/trino5x7.png` by `cargo xtask gen-assets`
//! (original artwork — no third-party font data, same license as the repo).
//!
//! Grid: 16 columns of 6x10 cells, printable ASCII from U+0020. Glyphs are
//! 5x7 on rows 0..7, descenders use rows 7..9, row 9 is line spacing. The
//! manifest declares it as a proportional PNG grid (`cell = [6, 10]`).

use std::path::Path;

pub const CELL_W: u32 = 6;
pub const CELL_H: u32 = 10;
const COLS: u32 = 16;

/// Rows of each glyph, `#` = ink. Fewer than 9 rows = no descender.
#[rustfmt::skip]
const GLYPHS: [(char, &[&str]); 95] = [
    (' ', &[]),
    ('!', &["..#..", "..#..", "..#..", "..#..", "..#..", ".....", "..#.."]),
    ('"', &[".#.#.", ".#.#.", ".#.#."]),
    ('#', &[".#.#.", ".#.#.", "#####", ".#.#.", "#####", ".#.#.", ".#.#."]),
    ('$', &["..#..", ".####", "#.#..", ".###.", "..#.#", "####.", "..#.."]),
    ('%', &["##...", "##..#", "...#.", "..#..", ".#...", "#..##", "...##"]),
    ('&', &[".##..", "#..#.", "#.#..", ".#...", "#.#.#", "#..#.", ".##.#"]),
    ('\'', &["..#..", "..#..", ".#..."]),
    ('(', &["...#.", "..#..", ".#...", ".#...", ".#...", "..#..", "...#."]),
    (')', &[".#...", "..#..", "...#.", "...#.", "...#.", "..#..", ".#..."]),
    ('*', &[".....", "..#..", "#.#.#", ".###.", "#.#.#", "..#..", "....."]),
    ('+', &[".....", "..#..", "..#..", "#####", "..#..", "..#..", "....."]),
    (',', &[".....", ".....", ".....", ".....", ".##..", "..#..", ".#..."]),
    ('-', &[".....", ".....", ".....", "#####", ".....", ".....", "....."]),
    ('.', &[".....", ".....", ".....", ".....", ".....", ".##..", ".##.."]),
    ('/', &[".....", "....#", "...#.", "..#..", ".#...", "#....", "....."]),
    ('0', &[".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###."]),
    ('1', &["..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###."]),
    ('2', &[".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####"]),
    ('3', &["#####", "...#.", "..#..", "...#.", "....#", "#...#", ".###."]),
    ('4', &["...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#."]),
    ('5', &["#####", "#....", "####.", "....#", "....#", "#...#", ".###."]),
    ('6', &["..##.", ".#...", "#....", "####.", "#...#", "#...#", ".###."]),
    ('7', &["#####", "....#", "...#.", "..#..", ".#...", ".#...", ".#..."]),
    ('8', &[".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###."]),
    ('9', &[".###.", "#...#", "#...#", ".####", "....#", "...#.", ".##.."]),
    (':', &[".....", ".##..", ".##..", ".....", ".##..", ".##..", "....."]),
    (';', &[".....", ".##..", ".##..", ".....", ".##..", "..#..", ".#..."]),
    ('<', &["...#.", "..#..", ".#...", "#....", ".#...", "..#..", "...#."]),
    ('=', &[".....", ".....", "#####", ".....", "#####", ".....", "....."]),
    ('>', &[".#...", "..#..", "...#.", "....#", "...#.", "..#..", ".#..."]),
    ('?', &[".###.", "#...#", "....#", "...#.", "..#..", ".....", "..#.."]),
    ('@', &[".###.", "#...#", "....#", ".##.#", "#.#.#", "#.#.#", ".###."]),
    ('A', &[".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"]),
    ('B', &["####.", "#...#", "#...#", "####.", "#...#", "#...#", "####."]),
    ('C', &[".###.", "#...#", "#....", "#....", "#....", "#...#", ".###."]),
    ('D', &["###..", "#..#.", "#...#", "#...#", "#...#", "#..#.", "###.."]),
    ('E', &["#####", "#....", "#....", "####.", "#....", "#....", "#####"]),
    ('F', &["#####", "#....", "#....", "####.", "#....", "#....", "#...."]),
    ('G', &[".###.", "#...#", "#....", "#.###", "#...#", "#...#", ".####"]),
    ('H', &["#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"]),
    ('I', &[".###.", "..#..", "..#..", "..#..", "..#..", "..#..", ".###."]),
    ('J', &["..###", "...#.", "...#.", "...#.", "...#.", "#..#.", ".##.."]),
    ('K', &["#...#", "#..#.", "#.#..", "##...", "#.#..", "#..#.", "#...#"]),
    ('L', &["#....", "#....", "#....", "#....", "#....", "#....", "#####"]),
    ('M', &["#...#", "##.##", "#.#.#", "#.#.#", "#...#", "#...#", "#...#"]),
    ('N', &["#...#", "#...#", "##..#", "#.#.#", "#..##", "#...#", "#...#"]),
    ('O', &[".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."]),
    ('P', &["####.", "#...#", "#...#", "####.", "#....", "#....", "#...."]),
    ('Q', &[".###.", "#...#", "#...#", "#...#", "#.#.#", "#..#.", ".##.#"]),
    ('R', &["####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#"]),
    ('S', &[".####", "#....", "#....", ".###.", "....#", "....#", "####."]),
    ('T', &["#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#.."]),
    ('U', &["#...#", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."]),
    ('V', &["#...#", "#...#", "#...#", "#...#", "#...#", ".#.#.", "..#.."]),
    ('W', &["#...#", "#...#", "#...#", "#.#.#", "#.#.#", "#.#.#", ".#.#."]),
    ('X', &["#...#", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "#...#"]),
    ('Y', &["#...#", "#...#", ".#.#.", "..#..", "..#..", "..#..", "..#.."]),
    ('Z', &["#####", "....#", "...#.", "..#..", ".#...", "#....", "#####"]),
    ('[', &[".###.", ".#...", ".#...", ".#...", ".#...", ".#...", ".###."]),
    ('\\', &[".....", "#....", ".#...", "..#..", "...#.", "....#", "....."]),
    (']', &[".###.", "...#.", "...#.", "...#.", "...#.", "...#.", ".###."]),
    ('^', &["..#..", ".#.#.", "#...#"]),
    ('_', &[".....", ".....", ".....", ".....", ".....", ".....", "#####"]),
    ('`', &[".#...", "..#..", "...#."]),
    ('a', &[".....", ".....", ".###.", "....#", ".####", "#...#", ".####"]),
    ('b', &["#....", "#....", "#.##.", "##..#", "#...#", "#...#", "####."]),
    ('c', &[".....", ".....", ".###.", "#....", "#....", "#...#", ".###."]),
    ('d', &["....#", "....#", ".##.#", "#..##", "#...#", "#...#", ".####"]),
    ('e', &[".....", ".....", ".###.", "#...#", "#####", "#....", ".###."]),
    ('f', &["..##.", ".#..#", ".#...", "###..", ".#...", ".#...", ".#..."]),
    ('g', &[".....", ".....", ".####", "#...#", "#...#", ".####", "....#", "#...#", ".###."]),
    ('h', &["#....", "#....", "#.##.", "##..#", "#...#", "#...#", "#...#"]),
    ('i', &["..#..", ".....", ".##..", "..#..", "..#..", "..#..", ".###."]),
    ('j', &["...#.", ".....", "..##.", "...#.", "...#.", "...#.", "...#.", "#..#.", ".##.."]),
    ('k', &["#....", "#....", "#..#.", "#.#..", "##...", "#.#..", "#..#."]),
    ('l', &[".##..", "..#..", "..#..", "..#..", "..#..", "..#..", ".###."]),
    ('m', &[".....", ".....", "##.#.", "#.#.#", "#.#.#", "#...#", "#...#"]),
    ('n', &[".....", ".....", "#.##.", "##..#", "#...#", "#...#", "#...#"]),
    ('o', &[".....", ".....", ".###.", "#...#", "#...#", "#...#", ".###."]),
    ('p', &[".....", ".....", "####.", "#...#", "#...#", "####.", "#....", "#....", "#...."]),
    ('q', &[".....", ".....", ".####", "#...#", "#...#", ".####", "....#", "....#", "....#"]),
    ('r', &[".....", ".....", "#.##.", "##..#", "#....", "#....", "#...."]),
    ('s', &[".....", ".....", ".###.", "#....", ".###.", "....#", "####."]),
    ('t', &[".#...", ".#...", "###..", ".#...", ".#...", ".#..#", "..##."]),
    ('u', &[".....", ".....", "#...#", "#...#", "#...#", "#..##", ".##.#"]),
    ('v', &[".....", ".....", "#...#", "#...#", "#...#", ".#.#.", "..#.."]),
    ('w', &[".....", ".....", "#...#", "#...#", "#.#.#", "#.#.#", ".#.#."]),
    ('x', &[".....", ".....", "#...#", ".#.#.", "..#..", ".#.#.", "#...#"]),
    ('y', &[".....", ".....", "#...#", "#...#", "#...#", ".####", "....#", "#...#", ".###."]),
    ('z', &[".....", ".....", "#####", "...#.", "..#..", ".#...", "#####"]),
    ('{', &["...#.", "..#..", "..#..", ".#...", "..#..", "..#..", "...#."]),
    ('|', &["..#..", "..#..", "..#..", "..#..", "..#..", "..#..", "..#.."]),
    ('}', &[".#...", "..#..", "..#..", "...#.", "..#..", "..#..", ".#..."]),
    ('~', &[".....", ".....", ".#...", "#.#.#", "...#.", ".....", "....."]),
];

/// The atlas as RGBA8 (white ink, transparent background).
pub fn atlas() -> (u32, u32, Vec<u8>) {
    let rows = (GLYPHS.len() as u32).div_ceil(COLS);
    let (w, h) = (COLS * CELL_W, rows * CELL_H);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for (i, (ch, art)) in GLYPHS.iter().enumerate() {
        assert_eq!(
            *ch as u32,
            32 + i as u32,
            "glyph table out of order at {ch:?}"
        );
        assert!(art.len() <= 9, "{ch:?}: at most 9 rows");
        let (cx, cy) = ((i as u32 % COLS) * CELL_W, (i as u32 / COLS) * CELL_H);
        for (y, row) in art.iter().enumerate() {
            assert_eq!(row.len(), 5, "{ch:?}: rows are 5 wide");
            for (x, px) in row.bytes().enumerate() {
                if px == b'#' {
                    let at = (((cy + y as u32) * w + cx + x as u32) * 4) as usize;
                    rgba[at..at + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
    }
    (w, h, rgba)
}

// Self-test skinned strip (smooth skinning on consoles): the same fixture
// as crates/asset-pipeline/tests/animated_model.rs.
fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.len();
        let v = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= n {
                out.push(T[(v >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A skinned strip: two joints (root at y=0, tip at y=1), three vertices
/// weighted 100% root, 50/50 and 100% tip.
pub fn write_skinned_gltf(path: &std::path::Path) {
    let mut buf: Vec<u8> = Vec::new();
    let f = |buf: &mut Vec<u8>, vals: &[f32]| {
        for v in vals {
            buf.extend_from_slice(&v.to_le_bytes());
        }
    };
    f(&mut buf, &[0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.2, 0.5, 0.0]); // 0..36 positions
    f(&mut buf, &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]); // 36..72 normals
    for i in [0u16, 2, 1, 0] {
        buf.extend_from_slice(&i.to_le_bytes()); // 72..80 indices (+pad)
    }
    // 80..104 JOINTS_0 (u16 x4 per vertex): v0 [0,1], v1 [1,0], v2 [0,1]
    for j in [[0u16, 1, 0, 0], [1, 0, 0, 0], [0, 1, 0, 0]] {
        for x in j {
            buf.extend_from_slice(&x.to_le_bytes());
        }
    }
    // 104..152 WEIGHTS_0: v0 root only, v1 tip only, v2 half/half
    f(
        &mut buf,
        &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0],
    );
    // 152..280 inverse bind matrices (column-major): root identity, tip -1 y.
    f(
        &mut buf,
        &[
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
    );
    f(
        &mut buf,
        &[
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 1.0,
        ],
    );
    let json = format!(
        r#"{{
  "asset": {{"version": "2.0"}},
  "scene": 0,
  "scenes": [{{"nodes": [0, 2]}}],
  "nodes": [
    {{"name": "root", "children": [1]}},
    {{"name": "tip", "translation": [0.0, 1.0, 0.0]}},
    {{"name": "strip", "mesh": 0, "skin": 0}}
  ],
  "skins": [{{"joints": [0, 1], "inverseBindMatrices": 5}}],
  "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0, "NORMAL": 1, "JOINTS_0": 3, "WEIGHTS_0": 4}}, "indices": 2}}]}}],
  "buffers": [{{"byteLength": {len}, "uri": "data:application/octet-stream;base64,{data}"}}],
  "bufferViews": [
    {{"buffer": 0, "byteOffset": 0, "byteLength": 36}},
    {{"buffer": 0, "byteOffset": 36, "byteLength": 36}},
    {{"buffer": 0, "byteOffset": 72, "byteLength": 6}},
    {{"buffer": 0, "byteOffset": 80, "byteLength": 24}},
    {{"buffer": 0, "byteOffset": 104, "byteLength": 48}},
    {{"buffer": 0, "byteOffset": 152, "byteLength": 128}}
  ],
  "accessors": [
    {{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0,0,0], "max": [0.2,1,0]}},
    {{"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"}},
    {{"bufferView": 2, "componentType": 5123, "count": 3, "type": "SCALAR"}},
    {{"bufferView": 3, "componentType": 5123, "count": 3, "type": "VEC4"}},
    {{"bufferView": 4, "componentType": 5126, "count": 3, "type": "VEC4"}},
    {{"bufferView": 5, "componentType": 5126, "count": 2, "type": "MAT4"}}
  ]
}}"#,
        len = buf.len(),
        data = b64(&buf)
    );
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, json).unwrap();
}

/// A white card of three 0.4 x 0.6 quads side by side (x from -0.6 to 0.6,
/// facing -Z), in color slots 0, 1 and 4 (a float `_SLOT`, what Blender
/// exports): `castle64::selftest::palette` probes each column. No COLOR_0
/// and no material, so every vertex is white.
pub fn write_slots_gltf(path: &std::path::Path) {
    let (mut pos, mut slots, mut idx) = (Vec::new(), Vec::new(), Vec::new());
    for (q, (x0, slot)) in [(-0.6f32, 0.0f32), (-0.2, 1.0), (0.2, 4.0)]
        .into_iter()
        .enumerate()
    {
        let x1 = x0 + 0.4;
        // Front faces toward -Z: (+x,-y), (-x,-y), (-x,+y), (+x,+y).
        for (x, y) in [(x1, -0.3f32), (x0, -0.3), (x0, 0.3), (x1, 0.3)] {
            pos.extend_from_slice(&[x, y, 0.0]);
            slots.push(slot);
        }
        let b = q as u16 * 4;
        idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    let mut buf: Vec<u8> = Vec::new();
    for v in &pos {
        buf.extend_from_slice(&v.to_le_bytes()); // 0..144 positions
    }
    for _ in 0..12 {
        for v in [0.0f32, 0.0, -1.0] {
            buf.extend_from_slice(&v.to_le_bytes()); // 144..288 normals
        }
    }
    for i in &idx {
        buf.extend_from_slice(&i.to_le_bytes()); // 288..324 indices
    }
    for v in &slots {
        buf.extend_from_slice(&v.to_le_bytes()); // 324..372 _SLOT
    }
    let json = format!(
        r#"{{
  "asset": {{"version": "2.0"}},
  "scene": 0,
  "scenes": [{{"nodes": [0]}}],
  "nodes": [{{"name": "card", "mesh": 0}}],
  "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0, "NORMAL": 1, "_SLOT": 3}}, "indices": 2}}]}}],
  "buffers": [{{"byteLength": {len}, "uri": "data:application/octet-stream;base64,{data}"}}],
  "bufferViews": [
    {{"buffer": 0, "byteOffset": 0, "byteLength": 144}},
    {{"buffer": 0, "byteOffset": 144, "byteLength": 144}},
    {{"buffer": 0, "byteOffset": 288, "byteLength": 36}},
    {{"buffer": 0, "byteOffset": 324, "byteLength": 48}}
  ],
  "accessors": [
    {{"bufferView": 0, "componentType": 5126, "count": 12, "type": "VEC3", "min": [-0.6,-0.3,0], "max": [0.6,0.3,0]}},
    {{"bufferView": 1, "componentType": 5126, "count": 12, "type": "VEC3"}},
    {{"bufferView": 2, "componentType": 5123, "count": 18, "type": "SCALAR"}},
    {{"bufferView": 3, "componentType": 5126, "count": 12, "type": "SCALAR"}}
  ]
}}
"#,
        len = buf.len(),
        data = b64(&buf)
    );
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, json).unwrap();
}

/// Ambient-occlusion probe (`[models.selftest_ao]` with a baked `ao`, and
/// the same file as `[models.selftest_ao_raw]` without): a 1 m wide white
/// panel facing -Z, split into 8 columns so its colors can vary across it,
/// between two fins that stick out 0.5 m toward the viewer at its left and
/// right edges. The baked AO darkens the panel next to the fins and leaves
/// its middle open (`castle64::selftest::ambient_occlusion`).
pub fn write_ao_gltf(path: &std::path::Path) {
    let (mut pos, mut nrm, mut idx): (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<u16>) =
        (Vec::new(), Vec::new(), Vec::new());
    let (h, cols) = (0.35f32, 8u16);
    for c in 0..=cols {
        let x = -0.5 + c as f32 / cols as f32;
        for y in [-h, h] {
            pos.push([x, y, 0.0]);
            nrm.push([0.0, 0.0, -1.0]);
        }
    }
    for c in 0..cols {
        let (b, n) = (c * 2, c * 2 + 2);
        // (x1,-y), (x0,-y), (x0,+y), (x1,+y): front faces toward -Z.
        idx.extend_from_slice(&[n, b, b + 1, n, b + 1, n + 1]);
    }
    for (x, nx) in [(-0.5f32, 1.0f32), (0.5, -1.0)] {
        let b = pos.len() as u16;
        for (z, y) in [(-0.5f32, -h), (0.0, -h), (0.0, h), (-0.5, h)] {
            pos.push([x, y, z]);
            nrm.push([nx, 0.0, 0.0]);
        }
        idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    let mut buf: Vec<u8> = Vec::new();
    for v in pos.iter().chain(&nrm).flatten() {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    for i in &idx {
        buf.extend_from_slice(&i.to_le_bytes());
    }
    let vbytes = pos.len() * 12;
    let json = format!(
        r#"{{
  "asset": {{"version": "2.0"}},
  "scene": 0,
  "scenes": [{{"nodes": [0]}}],
  "nodes": [{{"name": "ao-probe", "mesh": 0}}],
  "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0, "NORMAL": 1}}, "indices": 2}}]}}],
  "buffers": [{{"byteLength": {len}, "uri": "data:application/octet-stream;base64,{data}"}}],
  "bufferViews": [
    {{"buffer": 0, "byteOffset": 0, "byteLength": {vbytes}}},
    {{"buffer": 0, "byteOffset": {vbytes}, "byteLength": {vbytes}}},
    {{"buffer": 0, "byteOffset": {ioff}, "byteLength": {ibytes}}}
  ],
  "accessors": [
    {{"bufferView": 0, "componentType": 5126, "count": {count}, "type": "VEC3", "min": [-0.5,-{h},-0.5], "max": [0.5,{h},0]}},
    {{"bufferView": 1, "componentType": 5126, "count": {count}, "type": "VEC3"}},
    {{"bufferView": 2, "componentType": 5123, "count": {icount}, "type": "SCALAR"}}
  ]
}}
"#,
        len = buf.len(),
        data = b64(&buf),
        ioff = vbytes * 2,
        ibytes = idx.len() * 2,
        count = pos.len(),
        icount = idx.len(),
    );
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, json).unwrap();
}

pub fn gen_all(repo_root: &Path, write_png: crate::castle64_assets::PngWriter) {
    write_skinned_gltf(&repo_root.join("assets/shared/models/proxies/selftest-strip.gltf"));
    write_slots_gltf(&repo_root.join("assets/shared/models/proxies/selftest-slots.gltf"));
    write_ao_gltf(&repo_root.join("assets/shared/models/proxies/selftest-ao.gltf"));
    let (w, h, rgba) = atlas();
    let path = repo_root.join("assets/shared/fonts/trino5x7.png");
    std::fs::create_dir_all(path.parent().unwrap()).expect("create fonts dir");
    write_png(&path, w, h, &rgba);

    // Self-test texture: 16x16, one solid color per quadrant (TL red, TR
    // green, BL blue, BR white) — `castle64::selftest::textured_3d` probes
    // each quadrant on screen, catching flipped or unbound textures.
    let mut tex = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16 {
        for x in 0..16 {
            tex.extend_from_slice(match (x < 8, y < 8) {
                (true, true) => &[255, 0, 0, 255],
                (false, true) => &[0, 255, 0, 255],
                (true, false) => &[0, 0, 255, 255],
                (false, false) => &[255, 255, 255, 255],
            });
        }
    }
    let path = repo_root.join("assets/shared/sprites/selftest_quadrants.png");
    write_png(&path, 16, 16, &tex);

    // Detail layer (selftest::texture_detail): white left half, gray right
    // half — multiplied over the quadrants it halves the right column.
    let mut detail = Vec::with_capacity(8 * 8 * 4);
    for _y in 0..8 {
        for x in 0..8 {
            let v = if x < 4 { 255 } else { 128 };
            detail.extend_from_slice(&[v, v, v, 255]);
        }
    }
    write_png(
        &repo_root.join("assets/shared/sprites/selftest_detail.png"),
        8,
        8,
        &detail,
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn atlas_is_consistent() {
        let (w, h, rgba) = super::atlas();
        assert_eq!((w, h), (96, 60));
        assert_eq!(rgba.len(), (w * h * 4) as usize);
    }
}
