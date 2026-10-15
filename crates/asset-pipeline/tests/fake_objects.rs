//! Fake objects (`*.prim.ron` placeholders), every shape end to end: the
//! RON a user or an agent writes → `bake_all` → TMDL → `Mesh`, with the
//! dimensions, pivots, colors and validation rules a scene relies on.

use trino_asset_pipeline::{Pivot, Platform, Primitive, Shape, bake_all, load_dir};
use trino_core::{Mesh, asset_id};

/// Expected bind-pose size (x, y, z) of each default shape.
fn expected_size(shape: &Shape) -> [f32; 3] {
    match *shape {
        Shape::Box { size } | Shape::Ramp { size } => size,
        Shape::Sphere { radius } => [2.0 * radius; 3],
        Shape::Cylinder { radius, height }
        | Shape::Cone { radius, height }
        | Shape::Capsule { radius, height } => [2.0 * radius, height, 2.0 * radius],
        Shape::Plane { size } => [size[0], 0.0, size[1]],
        Shape::Torus { radius, tube } => [2.0 * (radius + tube), 2.0 * tube, 2.0 * (radius + tube)],
    }
}

#[test]
fn every_shape_bakes_from_its_ron_file_at_its_declared_size() {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path().join("assets");
    let mut manifest = String::from("version = 1\n");
    let shapes = Shape::all_defaults();
    let kinds: Vec<&str> = shapes.iter().map(Shape::kind).collect();
    assert_eq!(
        kinds,
        [
            "box", "sphere", "cylinder", "cone", "capsule", "plane", "ramp", "torus"
        ]
    );
    for shape in &shapes {
        let prim = Primitive {
            shape: shape.clone(),
            pivot: Pivot::Bottom,
            ..Primitive::default()
        };
        let text = prim.to_ron();
        assert!(text.starts_with("// Trino placeholder model"), "{text}");
        let file = format!("models/{}.prim.ron", shape.kind());
        let path = assets.join("shared").join(&file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        assert_eq!(Primitive::load(&path).unwrap(), prim, "RON round trip");
        manifest.push_str(&format!("[models.{}]\nfile = \"{file}\"\n", shape.kind()));
    }
    std::fs::write(assets.join("manifest.toml"), manifest).unwrap();
    let out = dir.path().join("out");
    let report = bake_all(&assets, Platform::Pc, &out).unwrap();
    assert_eq!(report.entries.len(), 8);
    let loaded = load_dir(&out, None).unwrap();
    for shape in &shapes {
        let model = loaded
            .models
            .iter()
            .find(|m| m.id == asset_id(&format!("models/{}", shape.kind())))
            .unwrap();
        let mesh = Mesh::from_tmdl(&model.tmdl).unwrap();
        assert!(mesh.index_count > 0 && mesh.index_count.is_multiple_of(3));
        let (min, max) = mesh.aabb().unwrap();
        let want = expected_size(shape);
        let got = [max.x - min.x, max.y - min.y, max.z - min.z];
        for axis in 0..3 {
            // Round shapes are polygons: their flats sit inside the circle.
            assert!(
                got[axis] <= want[axis] + 1e-4 && got[axis] >= want[axis] * 0.85 - 1e-4,
                "{}: axis {axis} is {} (want {})",
                shape.kind(),
                got[axis],
                want[axis]
            );
        }
        // Bottom pivot: the shape sits on y = 0. The torus's tube is a
        // polygon whose lowest edge is a flat, so it can float a hair above.
        let lift = match shape {
            Shape::Torus { tube, .. } => tube * 0.15,
            _ => 1e-4,
        };
        assert!(
            min.y > -1e-4 && min.y < lift,
            "{}: bottom pivot at y = {}",
            shape.kind(),
            min.y
        );
        // A bounding sphere for frustum culling, around the whole mesh.
        let (c, r) = mesh.bounds().unwrap();
        for i in 0..mesh.vertex_count {
            let p = mesh.position(i);
            assert!((p - c).dot(p - c).sqrt() <= r + 1e-3, "{}", shape.kind());
        }
    }
}

#[test]
fn segments_smoothing_and_colors_shape_the_mesh() {
    let sphere = |segments: u32, smooth: bool| Primitive {
        shape: Shape::Sphere { radius: 1.0 },
        segments,
        smooth,
        ..Primitive::default()
    };
    let verts = |p: &Primitive| p.build().positions.len();
    // Segments clamp to 3..=48.
    assert_eq!(verts(&sphere(1000, true)), verts(&sphere(48, true)));
    assert_eq!(verts(&sphere(0, true)), verts(&sphere(3, true)));
    assert!(verts(&sphere(24, true)) > verts(&sphere(6, true)));
    // Faceted shapes cannot share vertices between faces.
    assert!(verts(&sphere(12, false)) > verts(&sphere(12, true)));

    // A top color blends the vertex colors bottom → top.
    let column = Primitive {
        shape: Shape::Cylinder {
            radius: 0.5,
            height: 2.0,
        },
        color: [0, 0, 0, 255],
        top_color: Some([200, 100, 0, 255]),
        ..Primitive::default()
    };
    let blob = column.build().encode().unwrap();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    for i in 0..mesh.vertex_count {
        let (y, c) = (mesh.position(i).y, mesh.color(i));
        let t = (y + 1.0) / 2.0;
        assert!(
            (c.r as f32 - 200.0 * t).abs() <= 1.0 && (c.g as f32 - 100.0 * t).abs() <= 1.0,
            "y {y}: {c:?}"
        );
    }
    // Centered pivot: symmetric about y = 0.
    let (min, max) = mesh.aabb().unwrap();
    assert!((min.y + 1.0).abs() < 1e-5 && (max.y - 1.0).abs() < 1e-5);
}

#[test]
fn invalid_dimensions_are_rejected_with_the_field_name() {
    for (text, needle) in [
        ("(shape: Box(size: (1.0, 0.0, 1.0)))", "box `y` must be > 0"),
        (
            "(shape: Ramp(size: (1.0, 1.0, -2.0)))",
            "ramp `z` must be > 0",
        ),
        ("(shape: Plane(size: (0.0, 1.0)))", "plane `x` must be > 0"),
        ("(shape: Plane(size: (1.0, 0.0)))", "plane `z` must be > 0"),
        ("(shape: Sphere(radius: 0.0))", "sphere `radius`"),
        (
            "(shape: Cylinder(radius: 1.0, height: -1.0))",
            "cylinder `height`",
        ),
        ("(shape: Cone(radius: -1.0, height: 1.0))", "cone `radius`"),
        (
            "(shape: Capsule(radius: 1.0, height: 1.0))",
            "at least 2 × radius",
        ),
        (
            "(shape: Capsule(radius: -1.0, height: 1.0))",
            "capsule `radius`",
        ),
        ("(shape: Torus(radius: 1.0, tube: 0.0))", "torus `tube`"),
        ("(shape: Torus(radius: inf, tube: 1.0))", "torus `radius`"),
        ("(shape: Pyramid(size: 1.0))", "primitive parse error"),
    ] {
        let err = Primitive::from_ron(text).unwrap_err();
        assert!(err.contains(needle), "{text}: {err}");
    }
    let dir = tempfile::tempdir().unwrap();
    let err = Primitive::load(&dir.path().join("none.prim.ron")).unwrap_err();
    assert!(err.contains("cannot read"), "{err}");
}
