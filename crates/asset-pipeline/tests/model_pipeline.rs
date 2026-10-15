//! glTF → TMDL → `trino_core::Mesh` → `pose` → `tessellate`, end to end:
//! what a model master turns into on screen. Covers the repo's cube, vertex
//! color resolution (COLOR_0 / texture / factor), textured UVs, every
//! animation channel and interpolation, skins with static parts, and the
//! bake's error messages.

mod common;

use std::path::{Path, PathBuf};

use common::*;
use trino_asset_pipeline::model::{ClipData, JointData, SkeletonData, Trs};
use trino_asset_pipeline::{
    MeshData, ModelOptions, bake_model, bake_model_tmdl, clip_names, manifest_model_bounds,
};
use trino_core::render3d::tessellate;
use trino_core::{
    AnimRef, Camera3, ClipId, DEFAULT_LIGHT, MAX_JOINTS, Mat34, Mesh, ModelParams, ScreenTri,
    SpriteId, TnlStats, Vec2, Vec3,
};

const SCREEN: Vec2 = Vec2::new(320.0, 240.0);

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn draw(mesh: &Mesh, camera: &Camera3, params: &ModelParams) -> (Vec<ScreenTri>, TnlStats) {
    let mut tris = Vec::new();
    let stats = tessellate(
        mesh,
        &Mat34::IDENTITY,
        camera,
        &DEFAULT_LIGHT,
        params,
        SCREEN,
        &mut |t| tris.push(t),
    );
    (tris, stats)
}

fn near(a: Vec3, b: Vec3) -> bool {
    let d = a - b;
    d.dot(d) < 1e-6
}

#[test]
fn the_cube_master_bakes_and_tessellates_its_visible_faces() {
    let path = repo().join("assets/shared/models/cube.glb");
    let blob = bake_model_tmdl(&path).unwrap();
    assert_eq!(blob, bake_model(&path, &ModelOptions::default()).unwrap());
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    assert_eq!(mesh.index_count, 36, "12 triangles");
    assert_eq!(mesh.joint_count(), 0);
    let (min, max) = mesh.aabb().unwrap();
    assert_eq!(
        trino_asset_pipeline::model::tmdl_bounds(&blob),
        Some(([min.x, min.y, min.z], [max.x, max.y, max.z]))
    );
    assert!(min.x < 0.0 && max.x > 0.0, "centered: {min:?} {max:?}");

    // Off-axis, from outside: three faces face the camera, three don't.
    let camera = Camera3 {
        eye: Vec3::new(3.0, 4.0, -5.0),
        ..Camera3::default()
    };
    let (tris, stats) = draw(&mesh, &camera, &ModelParams::DEFAULT);
    assert_eq!(stats.tris_in, 12);
    assert_eq!(stats.emitted, 6);
    assert_eq!(stats.backfaced, 6);
    assert_eq!(tris.len(), 6);
    for t in &tris {
        for p in t.pts {
            assert!(p.x > 0.0 && p.x < SCREEN.x && p.y > 0.0 && p.y < SCREEN.y);
        }
        assert!(t.z.iter().all(|&z| z > 0.0 && z < 1.0));
    }
    // Double-sided draws keep the back faces too.
    let (_, both) = draw(
        &mesh,
        &camera,
        &ModelParams {
            double_sided: true,
            ..ModelParams::DEFAULT
        },
    );
    assert_eq!(both.emitted, 12);

    // Looking away: the bounding sphere culls the whole draw.
    let away = Camera3 {
        eye: Vec3::new(0.0, 0.0, -5.0),
        target: Vec3::new(0.0, 0.0, -10.0),
        ..Camera3::default()
    };
    let (tris, stats) = draw(&mesh, &away, &ModelParams::DEFAULT);
    assert!(tris.is_empty());
    assert_eq!(stats.culled, 1);
}

/// Three one-triangle meshes on three nodes: COLOR_0 (linear), a
/// baseColorFactor, and a 2x1 red|blue baseColorTexture sampled at UVs.
fn write_color_sources(path: &Path) {
    let mut g = Gltf::new();
    let (pos, nrm) = triangle(&mut g);
    let col = g.f32s(
        "VEC4",
        &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.5, 0.5, 0.5, 0.5],
    );
    let uv = g.f32s("VEC2", &[0.25, 0.5, 0.25, 0.5, 0.75, 0.5]);
    let png = png_bytes(2, 1, |x, _| {
        if x == 0 {
            [255, 0, 0, 255]
        } else {
            [0, 0, 255, 255]
        }
    });
    g.write(
        path,
        &format!(
            r#""scene": 0, "scenes": [{{"nodes": [0, 1, 2]}}],
"nodes": [
  {{"mesh": 0}},
  {{"mesh": 1, "translation": [2, 0, 0]}},
  {{"mesh": 2, "translation": [4, 0, 0]}}
],
"materials": [
  {{"pbrMetallicRoughness": {{"baseColorFactor": [1.0, 0.5, 0.0, 1.0]}}}},
  {{"pbrMetallicRoughness": {{"baseColorTexture": {{"index": 0}}}}}}
],
"textures": [{{"source": 0}}],
"images": [{{"uri": "data:image/png;base64,{img}"}}],
"meshes": [
  {{"primitives": [{{"attributes": {{"POSITION": {pos}, "NORMAL": {nrm}, "COLOR_0": {col}}}}}]}},
  {{"primitives": [{{"attributes": {{"POSITION": {pos}, "NORMAL": {nrm}}}, "material": 0}}]}},
  {{"primitives": [{{"attributes": {{"POSITION": {pos}, "NORMAL": {nrm}, "TEXCOORD_0": {uv}}}, "material": 1}}]}}
]"#,
            img = b64(&png)
        ),
    );
}

#[test]
fn vertex_colors_come_from_color0_then_texture_then_factor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("colors.gltf");
    write_color_sources(&path);
    let blob = bake_model(&path, &ModelOptions::default()).unwrap();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    assert_eq!(mesh.vertex_count, 9);
    // Non-indexed primitives get sequential indices.
    assert_eq!(mesh.index_count, 9);
    let rgba = |i: usize| {
        let c = mesh.color(i);
        [c.r, c.g, c.b, c.a]
    };
    // COLOR_0 is linear: 0 → 0, 1 → 255, 0.5 → sRGB ~188; alpha stays linear.
    assert_eq!(rgba(0), [0, 0, 0, 255]);
    assert_eq!(rgba(1), [255, 255, 255, 255]);
    let mid = rgba(2);
    assert!((187..=188).contains(&mid[0]) && mid[3] == 128, "{mid:?}");
    // baseColorFactor.
    assert!((3..6).all(|i| rgba(i) == [255, 127, 0, 255]));
    // Texture sampled at each vertex UV.
    assert_eq!(rgba(6), [255, 0, 0, 255]);
    assert_eq!(rgba(8), [0, 0, 255, 255]);
    // Node transforms are applied while merging.
    assert!(near(mesh.position(7), Vec3::new(5.0, 0.0, 0.0)));
    assert!(!mesh.has_uvs(), "untextured: UVs are dropped");

    // Textured bake: UVs kept, the texture modulates a white vertex color,
    // and the sprite becomes the default texture (TXUV + TXID).
    let opts = ModelOptions {
        texture: Some("sprites/bricks".into()),
        ..ModelOptions::default()
    };
    let blob = bake_model(&path, &opts).unwrap();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    assert!(mesh.has_uvs());
    assert_eq!(mesh.uv(8), Vec2::new(0.75, 0.5));
    assert_eq!(mesh.uv(3), Vec2::ZERO, "no TEXCOORD_0: zero UVs");
    assert_eq!(rgba_of(&mesh, 6), [255, 255, 255, 255]);
    let bricks = SpriteId::from_path("sprites/bricks");
    assert_eq!(mesh.default_texture(), Some(bricks));
    assert_eq!(mesh.texture_for(&ModelParams::DEFAULT), Some(bricks));
    let other = SpriteId::from_path("sprites/moss");
    let params = ModelParams {
        texture: Some(other),
        ..ModelParams::DEFAULT
    };
    assert_eq!(mesh.texture_for(&params), Some(other), "materials override");
    // Textured triangles reach the rasterizer with their UVs.
    let camera = Camera3 {
        eye: Vec3::new(2.5, 0.5, 6.0),
        target: Vec3::new(2.5, 0.5, 0.0),
        ..Camera3::default()
    };
    let (tris, _) = draw(&mesh, &camera, &ModelParams::DEFAULT);
    assert_eq!(tris.len(), 3);
    assert!(
        tris.iter()
            .any(|t| t.uv.iter().any(|uv| (uv.x - 0.75).abs() < 1e-4))
    );
}

fn rgba_of(mesh: &Mesh, i: usize) -> [u8; 4] {
    let c = mesh.color(i);
    [c.r, c.g, c.b, c.a]
}

#[test]
fn line_primitives_are_skipped_and_bad_files_fail_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    // A LINES primitive next to a triangle: only the triangle bakes.
    let mut g = Gltf::new();
    let (pos, nrm) = triangle(&mut g);
    let lines = dir.path().join("lines.gltf");
    g.write(
        &lines,
        &format!(
            r#""scene": 0, "scenes": [{{"nodes": [0]}}], "nodes": [{{"mesh": 0}}],
"meshes": [{{"primitives": [
  {{"attributes": {{"POSITION": {pos}, "NORMAL": {nrm}}}, "mode": 1}},
  {{"attributes": {{"POSITION": {pos}, "NORMAL": {nrm}}}}}
]}}]"#
        ),
    );
    let mesh_blob = bake_model(&lines, &ModelOptions::default()).unwrap();
    assert_eq!(Mesh::from_tmdl(&mesh_blob).unwrap().vertex_count, 3);

    // Only lines: no triangle geometry.
    let only_lines = dir.path().join("only-lines.gltf");
    g.write(
        &only_lines,
        &format!(
            r#""scene": 0, "scenes": [{{"nodes": [0]}}], "nodes": [{{"mesh": 0}}],
"meshes": [{{"primitives": [{{"attributes": {{"POSITION": {pos}, "NORMAL": {nrm}}}, "mode": 1}}]}}]"#
        ),
    );
    let err = bake_model(&only_lines, &ModelOptions::default()).unwrap_err();
    assert!(err.contains("no triangle geometry"), "{err}");

    // No normals.
    let no_normals = dir.path().join("flat.gltf");
    g.write(
        &no_normals,
        &format!(
            r#""scene": 0, "scenes": [{{"nodes": [0]}}], "nodes": [{{"mesh": 0}}],
"meshes": [{{"primitives": [{{"attributes": {{"POSITION": {pos}}}}}]}}]"#
        ),
    );
    let err = bake_model(&no_normals, &ModelOptions::default()).unwrap_err();
    assert!(err.contains("no NORMAL"), "{err}");

    // A mesh no node uses still bakes (no scene graph to walk).
    let orphan = dir.path().join("orphan.gltf");
    g.write(
        &orphan,
        &format!(
            r#""meshes": [{{"primitives": [{{"attributes": {{"POSITION": {pos}, "NORMAL": {nrm}}}}}]}}]"#
        ),
    );
    assert_eq!(
        Mesh::from_tmdl(&bake_model(&orphan, &ModelOptions::default()).unwrap())
            .unwrap()
            .vertex_count,
        3
    );

    // More animated nodes than MAX_JOINTS.
    let many = dir.path().join("many.gltf");
    let times = g.f32s("SCALAR", &[0.0, 1.0]);
    let moves = g.f32s("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let nodes: Vec<String> = (0..=MAX_JOINTS)
        .map(|i| format!("{{\"mesh\": 0, \"translation\": [{i}, 0, 0]}}"))
        .collect();
    let roots: Vec<String> = (0..=MAX_JOINTS).map(|i| i.to_string()).collect();
    g.write(
        &many,
        &format!(
            r#""scene": 0, "scenes": [{{"nodes": [{roots}]}}], "nodes": [{nodes}],
"meshes": [{{"primitives": [{{"attributes": {{"POSITION": {pos}, "NORMAL": {nrm}}}}}]}}],
"animations": [{{"samplers": [{{"input": {times}, "output": {moves}}}],
  "channels": [{{"sampler": 0, "target": {{"node": 0, "path": "translation"}}}}]}}]"#,
            roots = roots.join(","),
            nodes = nodes.join(",")
        ),
    );
    let err = bake_model(&many, &ModelOptions::default()).unwrap_err();
    assert!(err.contains("exceed the engine limit of 32"), "{err}");

    // Not a glTF at all, and a missing file.
    let junk = dir.path().join("junk.gltf");
    std::fs::write(&junk, "{ nope").unwrap();
    assert!(bake_model(&junk, &ModelOptions::default()).is_err());
    assert!(clip_names(&junk).is_empty());
    let missing = dir.path().join("missing.glb");
    let err = bake_model(&missing, &ModelOptions::default()).unwrap_err();
    assert!(err.contains("missing.glb"), "{err}");
}

/// Three animated nodes, one triangle each: `slide` (LINEAR translation
/// 0 → 2 on x), `grow` at y = 5 (STEP scale 1 → 3) and `bob` at y = -5
/// (CUBICSPLINE translation, tangents ignored). One unnamed clip of 1 s.
fn write_channels(path: &Path) {
    let mut g = Gltf::new();
    let (pos, nrm) = triangle(&mut g);
    let times = g.f32s("SCALAR", &[0.0, 1.0]);
    let slide = g.f32s("VEC3", &[0.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
    let grow = g.f32s("VEC3", &[1.0, 1.0, 1.0, 3.0, 3.0, 3.0]);
    // (in-tangent, value, out-tangent) per key.
    let bob = g.f32s(
        "VEC3",
        &[
            9.0, 9.0, 9.0, 0.0, -5.0, 0.0, 9.0, 9.0, 9.0, //
            9.0, 9.0, 9.0, 0.0, -3.0, 0.0, 9.0, 9.0, 9.0,
        ],
    );
    g.write(
        path,
        &format!(
            r#""scene": 0, "scenes": [{{"nodes": [0, 1, 2]}}],
"nodes": [
  {{"name": "slide", "mesh": 0}},
  {{"name": "grow", "mesh": 0, "translation": [0, 5, 0]}},
  {{"name": "bob", "mesh": 0, "translation": [0, -5, 0]}}
],
"meshes": [{{"primitives": [{{"attributes": {{"POSITION": {pos}, "NORMAL": {nrm}}}}}]}}],
"animations": [{{
  "samplers": [
    {{"input": {times}, "output": {slide}, "interpolation": "LINEAR"}},
    {{"input": {times}, "output": {grow}, "interpolation": "STEP"}},
    {{"input": {times}, "output": {bob}, "interpolation": "CUBICSPLINE"}}
  ],
  "channels": [
    {{"sampler": 0, "target": {{"node": 0, "path": "translation"}}}},
    {{"sampler": 1, "target": {{"node": 1, "path": "scale"}}}},
    {{"sampler": 2, "target": {{"node": 2, "path": "translation"}}}}
  ]
}}]"#
        ),
    );
}

#[test]
fn every_channel_and_interpolation_poses_its_joint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("channels.gltf");
    write_channels(&path);
    assert_eq!(
        clip_names(&path),
        ["anim0"],
        "unnamed clips get an index name"
    );
    // One-shot (`once`): the clip holds its last frame.
    let blob = bake_model(
        &path,
        &ModelOptions {
            anim_fps: 10.0,
            once: vec!["anim0".into()],
            ..ModelOptions::default()
        },
    )
    .unwrap();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    assert_eq!(mesh.joint_count(), 3);
    assert_eq!(mesh.clip_count(), 1);
    let clip = mesh.clip(0).unwrap();
    assert_eq!(Some(clip), mesh.find_clip(ClipId::from_name("anim0")));
    assert!(mesh.clip(1).is_none());
    assert_eq!((clip.frame_count, clip.looping), (11, false));
    assert!((clip.duration() - 1.0).abs() < 1e-6);

    // Vertex order follows first use: 3 vertices per node, nodes in order.
    let mut skin = [Mat34::IDENTITY; MAX_JOINTS];
    let at = |skin: &[Mat34; MAX_JOINTS], joint: usize, v: usize| {
        skin[joint].transform_point(mesh.position(joint * 3 + v))
    };
    assert_eq!(mesh.pose(None, &mut skin), 3);
    // Bind pose: every vertex where the bake put it.
    for j in 0..3 {
        for v in 0..3 {
            assert!(near(at(&skin, j, v), mesh.position(j * 3 + v)));
        }
    }
    mesh.pose(Some(AnimRef::new(clip.id, 0.5)), &mut skin);
    // LINEAR: halfway.
    assert!(near(at(&skin, 0, 1), Vec3::new(2.0, 0.0, 0.0)));
    // STEP: holds the first key until the next one.
    assert!(near(at(&skin, 1, 1), Vec3::new(1.0, 5.0, 0.0)));
    // CUBICSPLINE: passes through the values (-5 → -3), tangents dropped.
    assert!(near(at(&skin, 2, 0), Vec3::new(0.0, -4.0, 0.0)));
    mesh.pose(Some(AnimRef::new(clip.id, 1.0)), &mut skin);
    assert!(near(at(&skin, 0, 1), Vec3::new(3.0, 0.0, 0.0)));
    assert!(near(at(&skin, 1, 1), Vec3::new(3.0, 5.0, 0.0)));
    assert!(near(at(&skin, 2, 0), Vec3::new(0.0, -3.0, 0.0)));
    mesh.pose(Some(AnimRef::new(clip.id, 7.0)), &mut skin);
    assert!(near(at(&skin, 0, 1), Vec3::new(3.0, 0.0, 0.0)), "clamped");
    // Looping (the default): 1.5 s plays like 0.5 s.
    let looping_blob = bake_model(
        &path,
        &ModelOptions {
            anim_fps: 10.0,
            ..ModelOptions::default()
        },
    )
    .unwrap();
    let looping = Mesh::from_tmdl(&looping_blob).unwrap();
    assert!(looping.clip(0).unwrap().looping);
    looping.pose(Some(AnimRef::new(clip.id, 1.5)), &mut skin);
    assert!(near(
        skin[0].transform_point(looping.position(1)),
        Vec3::new(2.0, 0.0, 0.0)
    ));

    // The pose reaches the screen: the sliding triangle moves sideways
    // (seen from +z, world +x is screen left).
    let camera = Camera3 {
        eye: Vec3::new(1.0, 0.0, 12.0),
        target: Vec3::new(1.0, 0.0, 0.0),
        ..Camera3::default()
    };
    let centroid_x = |time: f32| {
        let params = ModelParams {
            anim: Some(AnimRef::new(clip.id, time)),
            ..ModelParams::DEFAULT
        };
        let (tris, stats) = draw(&mesh, &camera, &params);
        assert_eq!(stats.emitted, 3);
        // The topmost triangle on screen is `grow` (y = 5); the middle one
        // is `slide`.
        let mut by_y: Vec<&ScreenTri> = tris.iter().collect();
        by_y.sort_by(|a, b| a.pts[0].y.total_cmp(&b.pts[0].y));
        by_y[1].pts.iter().map(|p| p.x).sum::<f32>() / 3.0
    };
    let (start, end) = (centroid_x(0.0), centroid_x(1.0));
    assert!(end < start - 10.0, "{start} -> {end}");

    // The bounding sphere covers every animated frame.
    let (c, r) = mesh.bounds().unwrap();
    for p in [Vec3::new(3.0, 0.0, 0.0), Vec3::new(3.0, 5.0, 0.0)] {
        assert!((p - c).dot(p - c).sqrt() <= r + 1e-4);
    }
}

#[test]
fn rest_rotations_round_trip_for_half_turns_on_every_axis() {
    // Half turns put the rotation matrix's trace at -1: the quaternion
    // extraction must take its x, y and z branches.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("turns.gltf");
    let mut g = Gltf::new();
    let (pos, nrm) = triangle(&mut g);
    let times = g.f32s("SCALAR", &[0.0, 1.0]);
    // Identity → a quarter turn about z, stored as the *negated* quaternion
    // (same rotation, opposite hemisphere): sampling must not take the
    // long way round.
    let q = std::f32::consts::FRAC_PI_4;
    let spin = g.f32s("VEC4", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -q.sin(), -q.cos()]);
    g.write(
        &path,
        &format!(
            r#""scene": 0, "scenes": [{{"nodes": [0, 1, 2, 3]}}],
"nodes": [
  {{"mesh": 0, "rotation": [1, 0, 0, 0]}},
  {{"mesh": 0, "rotation": [0, 1, 0, 0], "translation": [3, 0, 0]}},
  {{"mesh": 0, "rotation": [0, 0, 1, 0], "translation": [6, 0, 0]}},
  {{"mesh": 0, "translation": [9, 0, 0]}}
],
"meshes": [{{"primitives": [{{"attributes": {{"POSITION": {pos}, "NORMAL": {nrm}}}}}]}}],
"animations": [{{"name": "idle",
  "samplers": [{{"input": {times}, "output": {spin}}}],
  "channels": [{{"sampler": 0, "target": {{"node": 3, "path": "rotation"}}}}]}}]"#
        ),
    );
    let blob = bake_model(&path, &ModelOptions::default()).unwrap();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    assert_eq!(mesh.joint_count(), 4);
    // The baked rest vertices are the rotated triangles…
    assert!(
        near(mesh.position(2), Vec3::new(0.0, -1.0, 0.0)),
        "x half turn"
    );
    assert!(
        near(mesh.position(4), Vec3::new(2.0, 0.0, 0.0)),
        "y half turn"
    );
    assert!(
        near(mesh.position(7), Vec3::new(5.0, 0.0, 0.0)),
        "z half turn"
    );
    // …and the clip's frames reproduce the rest pose exactly.
    let clip = mesh.find_clip(ClipId::from_name("idle")).unwrap();
    let mut skin = [Mat34::IDENTITY; MAX_JOINTS];
    mesh.pose(Some(AnimRef::new(clip.id, 0.5)), &mut skin);
    // Halfway through the turn: 45°, the short way.
    let tip = skin[3].transform_point(mesh.position(10));
    let h = std::f32::consts::FRAC_1_SQRT_2;
    assert!(near(tip, Vec3::new(9.0 + h, h, 0.0)), "{tip:?}");
    for (j, joint) in skin.iter().enumerate().take(3) {
        for v in 0..3 {
            let i = j * 3 + v;
            assert!(
                near(joint.transform_point(mesh.position(i)), mesh.position(i)),
                "joint {j} vertex {v}"
            );
        }
    }
}

/// A two-joint skinned strip plus an unskinned `prop` mesh node; optionally
/// without inverse bind matrices.
fn write_skin_with_prop(path: &Path, with_ibm: bool) {
    write_skin(path, with_ibm, false);
}

/// The skinned strip, optionally under an `armature` node moved to z = 5
/// (a non-joint ancestor of the skeleton, as Blender exports it).
fn write_skin(path: &Path, with_ibm: bool, armature: bool) {
    let mut g = Gltf::new();
    let pos = g.f32s("VEC3", &[0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.2, 0.5, 0.0]);
    let nrm = g.f32s("VEC3", &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]);
    let idx = g.u16s("SCALAR", &[0, 2, 1]);
    let joints = g.u16s("VEC4", &[0, 1, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0]);
    let weights = g.f32s(
        "VEC4",
        &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0],
    );
    let ibm = g.f32s(
        "MAT4",
        &[
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, //
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 1.0,
        ],
    );
    let (tpos, tnrm) = triangle(&mut g);
    let skin = if with_ibm {
        format!(r#"{{"joints": [0, 1], "inverseBindMatrices": {ibm}}}"#)
    } else {
        r#"{"joints": [0, 1]}"#.to_string()
    };
    let (roots, extra) = if armature {
        (
            "4, 2, 3",
            r#", {"name": "armature", "children": [0], "translation": [0.0, 0.0, 5.0]}"#,
        )
    } else {
        ("0, 2, 3", "")
    };
    g.write(
        path,
        &format!(
            r#""scene": 0, "scenes": [{{"nodes": [{roots}]}}],
"nodes": [
  {{"name": "root", "children": [1]}},
  {{"name": "tip", "translation": [0.0, 1.0, 0.0]}},
  {{"name": "strip", "mesh": 0, "skin": 0}},
  {{"name": "prop", "mesh": 1, "translation": [5.0, 0.0, 0.0]}}{extra}
],
"skins": [{skin}],
"meshes": [
  {{"primitives": [{{"attributes": {{"POSITION": {pos}, "NORMAL": {nrm}, "JOINTS_0": {joints}, "WEIGHTS_0": {weights}}}, "indices": {idx}}}]}},
  {{"primitives": [{{"attributes": {{"POSITION": {tpos}, "NORMAL": {tnrm}}}}}]}}
]"#
        ),
    );
}

#[test]
fn skinned_files_give_unskinned_parts_a_static_joint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skin.gltf");
    write_skin_with_prop(&path, true);
    let rigid_blob = bake_model(&path, &ModelOptions::default()).unwrap();
    let rigid = Mesh::from_tmdl(&rigid_blob).unwrap();
    // root + tip + the static joint that carries the prop.
    assert_eq!(rigid.joint_count(), 3);
    assert_eq!(rigid.vertex_count, 6);
    assert!(!rigid.has_blend());
    let mut skin = [Mat34::IDENTITY; MAX_JOINTS];
    assert_eq!(rigid.pose(None, &mut skin), 3);
    // The prop keeps its node transform and never moves.
    let prop: Vec<usize> = (0..6)
        .filter(|&i| rigid.position(i).x >= 5.0 - 1e-5)
        .collect();
    assert_eq!(prop.len(), 3);
    for i in prop {
        assert!(near(
            skin[2].transform_point(rigid.position(i)),
            rigid.position(i)
        ));
    }

    let smooth_blob = bake_model(
        &path,
        &ModelOptions {
            smooth: true,
            ..ModelOptions::default()
        },
    )
    .unwrap();
    let smooth = Mesh::from_tmdl(&smooth_blob).unwrap();
    assert!(smooth.has_blend());
    let blended: Vec<(usize, f32)> = (0..smooth.vertex_count)
        .filter_map(|i| smooth.blend_of(i))
        .collect();
    assert_eq!(blended.len(), 1, "only the half/half vertex blends");
    assert!((blended[0].1 - 0.5).abs() < 0.01);

    // Without inverse bind matrices the skin binds at identity: the bind
    // pose maps the tip joint's vertices through its rest transform.
    let no_ibm = dir.path().join("no-ibm.gltf");
    write_skin_with_prop(&no_ibm, false);
    let no_ibm_blob = bake_model(&no_ibm, &ModelOptions::default()).unwrap();
    let mesh = Mesh::from_tmdl(&no_ibm_blob).unwrap();
    assert_eq!(mesh.joint_count(), 3);
    mesh.pose(None, &mut skin);
    let lifted = skin[1].transform_point(Vec3::ZERO);
    assert!(near(lifted, Vec3::new(0.0, 1.0, 0.0)), "{lifted:?}");

    // A non-joint ancestor (the armature) carries the whole skeleton.
    let rigged = dir.path().join("armature.gltf");
    write_skin(&rigged, true, true);
    let blob = bake_model(&rigged, &ModelOptions::default()).unwrap();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    assert_eq!(mesh.joint_count(), 3, "the armature is not a joint");
    mesh.pose(None, &mut skin);
    let root = skin[0].transform_point(Vec3::ZERO);
    assert!(near(root, Vec3::new(0.0, 0.0, 5.0)), "{root:?}");
    let tip = skin[1].transform_point(Vec3::new(0.0, 1.0, 0.0));
    assert!(near(tip, Vec3::new(0.0, 1.0, 5.0)), "{tip:?}");
}

#[test]
fn manifest_model_bounds_cover_every_bakeable_model() {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path().join("assets");
    write_triangle_gltf(&assets.join("shared/models/tri.gltf"));
    std::fs::write(
        assets.join("shared/models/slab.prim.ron"),
        "(shape: Box(size: (4.0, 0.5, 2.0)), pivot: Bottom)",
    )
    .unwrap();
    std::fs::write(
        assets.join("manifest.toml"),
        r#"version = 1
[models.tri]
file = "models/tri.gltf"
[models.slab]
file = "models/slab.prim.ron"
[models.broken]
file = "models/missing.glb"
"#,
    )
    .unwrap();
    let bounds = manifest_model_bounds(&assets);
    assert_eq!(bounds.len(), 2, "unbakeable models are skipped");
    assert_eq!(bounds["models/tri"], ([0.0, 0.0, 0.0], [1.0, 1.0, 0.0]));
    let (min, max) = bounds["models/slab"];
    assert_eq!(min, [-2.0, 0.0, -1.0], "bottom pivot sits on y = 0");
    assert_eq!(max, [2.0, 0.5, 1.0]);
    // No manifest: no bounds.
    assert!(manifest_model_bounds(dir.path()).is_empty());
}

#[test]
fn texture_color_types_are_sampled_into_vertex_colors() {
    // Gray, gray + alpha and RGB textures (the RGBA path is covered above):
    // one triangle per image, UVs in the image's only texel.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("formats.gltf");
    let mut g = Gltf::new();
    let (pos, nrm) = triangle(&mut g);
    let uv = g.f32s("VEC2", &[0.5, 0.5, 0.5, 0.5, 0.5, 0.5]);
    let images = [
        png_bytes_typed(1, 1, png::ColorType::Grayscale, &[90]),
        png_bytes_typed(1, 1, png::ColorType::GrayscaleAlpha, &[40, 128]),
        png_bytes_typed(1, 1, png::ColorType::Rgb, &[1, 2, 3]),
    ];
    let uris: Vec<String> = images
        .iter()
        .map(|b| format!("{{\"uri\": \"data:image/png;base64,{}\"}}", b64(b)))
        .collect();
    let prim = |m: usize| {
        format!(
            r#"{{"primitives": [{{"attributes": {{"POSITION": {pos}, "NORMAL": {nrm}, "TEXCOORD_0": {uv}}}, "material": {m}}}]}}"#
        )
    };
    let mat = |t: usize| {
        format!(r#"{{"pbrMetallicRoughness": {{"baseColorTexture": {{"index": {t}}}}}}}"#)
    };
    g.write(
        &path,
        &format!(
            r#""scene": 0, "scenes": [{{"nodes": [0, 1, 2]}}],
"nodes": [{{"mesh": 0}}, {{"mesh": 1, "translation": [2, 0, 0]}}, {{"mesh": 2, "translation": [4, 0, 0]}}],
"images": [{}],
"textures": [{{"source": 0}}, {{"source": 1}}, {{"source": 2}}],
"materials": [{}, {}, {}],
"meshes": [{}, {}, {}]"#,
            uris.join(", "),
            mat(0),
            mat(1),
            mat(2),
            prim(0),
            prim(1),
            prim(2)
        ),
    );
    let blob = bake_model(&path, &ModelOptions::default()).unwrap();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    assert_eq!(rgba_of(&mesh, 0), [90, 90, 90, 255]);
    assert_eq!(rgba_of(&mesh, 3), [40, 40, 40, 128]);
    assert_eq!(rgba_of(&mesh, 6), [1, 2, 3, 255]);
}

fn trs_at(x: f32) -> Trs {
    Trs {
        t: [x, 0.0, 0.0],
        ..Trs::IDENTITY
    }
}

/// One triangle on two joints, with a clip, blend weights and UVs.
fn rigged_triangle() -> MeshData {
    let joint = |parent: i32| JointData {
        parent,
        inverse_bind: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ],
        rest: Trs::IDENTITY,
    };
    MeshData {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        colors: vec![[255; 4]; 3],
        indices: vec![0, 1, 2],
        skeleton: Some(SkeletonData {
            joint_of_vertex: vec![0, 1, 1],
            joints: vec![joint(-1), joint(0)],
            clips: vec![ClipData {
                name: "push".into(),
                fps: 2.0,
                looping: false,
                frames: vec![
                    vec![Trs::IDENTITY, Trs::IDENTITY],
                    vec![Trs::IDENTITY, trs_at(3.0)],
                ],
            }],
        }),
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        texture: Some(trino_core::asset_id("sprites/skin")),
        blend: vec![[0, 0], [0, 0], [0, 128]],
        slots: vec![0, 2, 4],
    }
}

#[test]
fn mesh_data_encodes_every_tmdl_section_and_rejects_bad_layouts() {
    // SKEL + ANIM + SKW2 + TXUV + TXID + SLOT + BNDS, read back by the core.
    let blob = rigged_triangle().encode().unwrap();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    assert_eq!(mesh.joint_count(), 2);
    let clip = mesh.find_clip(ClipId::from_name("push")).unwrap();
    assert_eq!((clip.frame_count, clip.looping, clip.fps), (2, false, 2.0));
    assert!(mesh.has_blend());
    assert_eq!(
        mesh.blend_of(2).map(|(j, w)| (j, (w * 255.0).round())),
        Some((0, 128.0))
    );
    assert_eq!(mesh.uv(1), Vec2::new(1.0, 0.0));
    assert_eq!(
        mesh.default_texture(),
        Some(SpriteId::from_path("sprites/skin"))
    );
    assert!(mesh.has_slots());
    assert_eq!([mesh.slot(0), mesh.slot(1), mesh.slot(2)], [0, 2, 4]);
    let mut skin = [Mat34::IDENTITY; MAX_JOINTS];
    mesh.pose(Some(AnimRef::new(clip.id, 0.5)), &mut skin);
    assert!(near(
        skin[1].transform_point(Vec3::ZERO),
        Vec3::new(3.0, 0.0, 0.0)
    ));
    // The bounds cover the clip's displaced joint.
    let (c, r) = mesh.bounds().unwrap();
    let far = Vec3::new(4.0, 0.0, 0.0);
    assert!((far - c).dot(far - c).sqrt() <= r + 1e-4);

    // Without blending weights no SKW2 is written.
    let mut rigid = rigged_triangle();
    rigid.blend = vec![[0, 0]; 3];
    assert!(
        !Mesh::from_tmdl(&rigid.encode().unwrap())
            .unwrap()
            .has_blend()
    );

    let fails = |m: MeshData, needle: &str| {
        let err = m.encode().unwrap_err();
        assert!(err.contains(needle), "{err}");
    };
    fails(MeshData::default(), "no triangle geometry");
    let mut huge = rigged_triangle();
    huge.skeleton = None;
    huge.positions = vec![[0.0; 3]; 70_000];
    fails(huge, "exceed the engine's u16 index space");
    let mut joints = rigged_triangle();
    let skel = joints.skeleton.as_mut().unwrap();
    skel.joints = (0..=MAX_JOINTS as i32)
        .map(|i| JointData {
            parent: i - 1,
            ..skel.joints[0].clone()
        })
        .collect();
    fails(joints, "joints exceed the engine limit");
    let mut blend = rigged_triangle();
    blend.blend.remove(0);
    fails(blend, "one blend pair per vertex");
    let mut uvs = rigged_triangle();
    uvs.uvs.pop();
    fails(uvs, "textured model has 2 UVs for 3 vertices");
    let mut slots = rigged_triangle();
    slots.slots.pop();
    fails(slots, "2 color slots for 3 vertices");
    let mut slot5 = rigged_triangle();
    slot5.slots[1] = 5;
    fails(slot5, "color slot 5 is out of range 0..=4");
    // All-zero slots write no SLOT section: the blob is byte-identical to
    // one without slots.
    let mut zeros = rigged_triangle();
    zeros.slots = vec![0; 3];
    let mut none = rigged_triangle();
    none.slots.clear();
    assert_eq!(zeros.encode().unwrap(), none.encode().unwrap());
    assert!(
        !Mesh::from_tmdl(&none.encode().unwrap())
            .unwrap()
            .has_slots()
    );
}
