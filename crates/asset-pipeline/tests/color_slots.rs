//! Color slots end to end: a glTF custom attribute `_SLOT` (float or
//! integer) bakes into the TMDL `SLOT` section, each slot staying with its
//! vertex through the bake's vertex-cache reordering, and a draw's palette
//! recolors exactly those vertices. Files without `_SLOT` bake as before.

mod common;

use std::path::Path;

use common::*;
use trino_asset_pipeline::{ModelOptions, bake_model};
use trino_core::render3d::tessellate;
use trino_core::{
    Camera3, Color, DEFAULT_LIGHT, Mat34, Mesh, ModelParams, ScreenTri, Shading, Vec2, Vec3,
};

/// Slot of each source vertex, in file order (node 0's six, node 1's three,
/// node 2's three). Vertex `k` sits at x = k, so a baked vertex names its
/// source vertex by its position.
const SLOTS: [u8; 12] = [0, 1, 2, 3, 4, 1, 4, 0, 2, 0, 0, 0];

/// `Some(form)` writes `_SLOT` on the first two primitives (`"float"`,
/// `"zeros"` = all-zero floats, `"bad"` = a 5, `"vec2"` = wrong type);
/// `None` writes none. Node 0: two triangles listed high vertices first,
/// so the bake renumbers them. Node 1: one triangle with an unsigned-byte
/// `_SLOT`. Node 2: one triangle without `_SLOT` (zeros in the merge).
fn write_slotted(path: &Path, form: Option<&str>) {
    let mut g = Gltf::new();
    let pos = |xs: std::ops::Range<usize>| -> Vec<f32> {
        xs.flat_map(|k| [k as f32, (k % 2) as f32, 0.0]).collect()
    };
    let p0 = g.f32s("VEC3", &pos(0..6));
    let n0 = g.f32s("VEC3", &[0.0, 0.0, 1.0].repeat(6));
    let i0 = g.u16s("SCALAR", &[5, 3, 4, 0, 1, 2]);
    let p1 = g.f32s("VEC3", &pos(6..9));
    let n1 = g.f32s("VEC3", &[0.0, 0.0, 1.0].repeat(3));
    let p2 = g.f32s("VEC3", &pos(9..12));
    let n2 = g.f32s("VEC3", &[0.0, 0.0, 1.0].repeat(3));
    let slot_attr = |acc: usize| format!(r#", "_SLOT": {acc}"#);
    let (s0, s1) = match form {
        None => (String::new(), String::new()),
        Some(form) => {
            let mut floats: Vec<f32> = SLOTS[..6].iter().map(|&s| s as f32).collect();
            let mut bytes = SLOTS[6..9].to_vec();
            match form {
                "zeros" => {
                    floats = vec![0.0; 6];
                    bytes = vec![0; 3];
                }
                "bad" => floats[2] = 5.0,
                _ => {}
            }
            let a0 = if form == "vec2" {
                g.f32s("VEC2", &[0.0; 12])
            } else {
                g.f32s("SCALAR", &floats)
            };
            (slot_attr(a0), slot_attr(g.u8s("SCALAR", &bytes)))
        }
    };
    g.write(
        path,
        &format!(
            r#""scene": 0, "scenes": [{{"nodes": [0, 1, 2]}}],
"nodes": [{{"mesh": 0}}, {{"mesh": 1}}, {{"mesh": 2}}],
"meshes": [
  {{"primitives": [{{"attributes": {{"POSITION": {p0}, "NORMAL": {n0}{s0}}}, "indices": {i0}}}]}},
  {{"primitives": [{{"attributes": {{"POSITION": {p1}, "NORMAL": {n1}{s1}}}}}]}},
  {{"primitives": [{{"attributes": {{"POSITION": {p2}, "NORMAL": {n2}}}}}]}}
]"#
        ),
    );
}

fn bake(path: &Path) -> Result<Vec<u8>, String> {
    bake_model(path, &ModelOptions::default())
}

fn has_section(blob: &[u8], tag: &[u8; 4]) -> bool {
    blob.windows(4).any(|w| w == tag)
}

#[test]
fn slot_attribute_bakes_per_vertex_through_reordering() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slotted.gltf");
    write_slotted(&path, Some("float"));
    let blob = bake(&path).unwrap();
    assert!(has_section(&blob, b"SLOT"));
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    assert!(mesh.has_slots());
    assert_eq!(mesh.vertex_count, 12);
    let mut moved = 0;
    for i in 0..mesh.vertex_count {
        let source = mesh.position(i).x as usize;
        moved += usize::from(source != i);
        assert_eq!(mesh.slot(i), SLOTS[source], "vertex {i} (source {source})");
    }
    assert!(
        moved > 0,
        "the bake reordered vertices (the case under test)"
    );
}

#[test]
fn files_without_slots_bake_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain.gltf");
    let zeros = dir.path().join("zeros.gltf");
    write_slotted(&plain, None);
    write_slotted(&zeros, Some("zeros"));
    let blob = bake(&plain).unwrap();
    assert!(!has_section(&blob, b"SLOT"));
    assert!(!Mesh::from_tmdl(&blob).unwrap().has_slots());
    // An all-zero `_SLOT` writes nothing either: the same bytes.
    assert_eq!(bake(&zeros).unwrap(), blob);
}

#[test]
fn bad_slot_attributes_fail_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    for (form, needle) in [
        ("bad", "`_SLOT` value 5 is out of range 0..=4"),
        ("vec2", "`_SLOT` must be a SCALAR attribute"),
    ] {
        let path = dir.path().join(format!("{form}.gltf"));
        write_slotted(&path, Some(form));
        let err = bake(&path).unwrap_err();
        assert!(err.contains(needle), "{form}: {err}");
        assert!(
            err.contains(&format!("{form}.gltf")),
            "names the file: {err}"
        );
    }
}

#[test]
fn a_palette_recolors_the_baked_slots() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slotted.gltf");
    write_slotted(&path, Some("float"));
    let blob = bake(&path).unwrap();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    // No COLOR_0 or material: every vertex is white, so an unlit vertex in
    // slot s shows exactly palette[s - 1].
    let palette = [
        Color::rgb(200, 40, 40),
        Color::rgb(40, 200, 40),
        Color::rgb(40, 40, 200),
        Color::rgb(90, 60, 30),
    ];
    let camera = Camera3 {
        eye: Vec3::new(5.5, 0.5, 20.0),
        target: Vec3::new(5.5, 0.5, 0.0),
        ..Camera3::default()
    };
    let draw = |palette| {
        // Double-sided: half of these triangles face away.
        let params = ModelParams {
            shading: Shading::Unlit,
            double_sided: true,
            palette,
            ..ModelParams::DEFAULT
        };
        let mut tris: Vec<ScreenTri> = Vec::new();
        tessellate(
            &mesh,
            &Mat34::IDENTITY,
            &camera,
            &DEFAULT_LIGHT,
            &params,
            Vec2::new(320.0, 240.0),
            &mut |t| tris.push(t),
        );
        tris
    };
    let plain = draw(None);
    let colored = draw(Some(palette));
    assert_eq!(plain.len(), 4);
    assert_eq!(colored.len(), 4);
    assert!(plain.iter().all(|t| t.colors == [Color::WHITE; 3]));
    let mut seen = [false; 5];
    // Map each screen vertex back to its source through the projection.
    let (x0, x1) = (screen_x(&camera, 0.0), screen_x(&camera, 1.0));
    for t in &colored {
        for k in 0..3 {
            let source = ((t.pts[k].x - x0) / (x1 - x0)).round() as usize;
            let slot = SLOTS[source];
            seen[slot as usize] = true;
            let want = if slot == 0 {
                Color::WHITE
            } else {
                palette[slot as usize - 1]
            };
            assert_eq!(t.colors[k], want, "source vertex {source}, slot {slot}");
        }
    }
    assert_eq!(seen, [true; 5], "every slot drawn");
}

/// Screen x of the world point (x, 0.5, 0) under `camera` (320x240).
fn screen_x(camera: &Camera3, x: f32) -> f32 {
    let v = camera.view().transform_point(Vec3::new(x, 0.5, 0.0));
    let half = camera.fov_y * 0.5;
    let focal = trino_core::math3d::cos(half) / trino_core::math3d::sin(half) * 120.0;
    160.0 + v.x * focal / v.z
}
