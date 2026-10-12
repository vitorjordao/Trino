//! glTF animation → TMDL rigid skeleton, end to end: a generated two-node
//! file (a static "body" and a child "arm" whose animation rotates it) must
//! bake to a 2-joint skeleton whose clip moves only the arm's vertices.

use trino_asset_pipeline::{ModelOptions, bake_model};
use trino_core::{AnimRef, ClipId, MAX_JOINTS, Mat34, Mesh, Vec3};

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

/// One triangle mesh (positions, normals, indices), plus animation
/// input/output data, packed into one buffer.
fn write_gltf(path: &std::path::Path, anim_name: &str) {
    write_gltf_at(path, anim_name, 0.0);
}

/// Same model, with the clip's keyframes at `start` and `start + 1` s
/// (exporters like Blender's start at frame 1 = 1/24 s).
fn write_gltf_at(path: &std::path::Path, anim_name: &str, start: f32) {
    let mut buf: Vec<u8> = Vec::new();
    let f = |buf: &mut Vec<u8>, vals: &[f32]| {
        for v in vals {
            buf.extend_from_slice(&v.to_le_bytes());
        }
    };
    // Triangle in the XY plane at x in [0, 1] (facing +Z).
    f(&mut buf, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]); // 0..36 positions
    f(&mut buf, &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]); // 36..72 normals
    for i in [0u16, 1, 2, 0] {
        buf.extend_from_slice(&i.to_le_bytes()); // 72..80 indices (+pad)
    }
    f(&mut buf, &[start, start + 1.0]); // 80..88 times
    let h = std::f32::consts::FRAC_PI_4; // 90 degrees around Z at t = 1
    f(&mut buf, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, h.sin(), h.cos()]); // 88..120 rotations
    let json = format!(
        r#"{{
  "asset": {{"version": "2.0"}},
  "scene": 0,
  "scenes": [{{"nodes": [0]}}],
  "nodes": [
    {{"name": "body", "mesh": 0, "children": [1]}},
    {{"name": "arm", "mesh": 0, "translation": [2.0, 0.0, 0.0]}}
  ],
  "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0, "NORMAL": 1}}, "indices": 2}}]}}],
  "animations": [{{
    "name": "{anim_name}",
    "samplers": [{{"input": 3, "output": 4, "interpolation": "LINEAR"}}],
    "channels": [{{"sampler": 0, "target": {{"node": 1, "path": "rotation"}}}}]
  }}],
  "buffers": [{{"byteLength": {len}, "uri": "data:application/octet-stream;base64,{data}"}}],
  "bufferViews": [
    {{"buffer": 0, "byteOffset": 0, "byteLength": 36}},
    {{"buffer": 0, "byteOffset": 36, "byteLength": 36}},
    {{"buffer": 0, "byteOffset": 72, "byteLength": 6}},
    {{"buffer": 0, "byteOffset": 80, "byteLength": 8}},
    {{"buffer": 0, "byteOffset": 88, "byteLength": 32}}
  ],
  "accessors": [
    {{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0,0,0], "max": [1,1,0]}},
    {{"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"}},
    {{"bufferView": 2, "componentType": 5123, "count": 3, "type": "SCALAR"}},
    {{"bufferView": 3, "componentType": 5126, "count": 2, "type": "SCALAR", "min": [{t0}], "max": [{t1}]}},
    {{"bufferView": 4, "componentType": 5126, "count": 2, "type": "VEC4"}}
  ]
}}"#,
        len = buf.len(),
        data = b64(&buf),
        t0 = start,
        t1 = start + 1.0,
    );
    std::fs::write(path, json).unwrap();
}

#[test]
fn node_animation_bakes_to_a_rigid_skeleton() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("arm.gltf");
    write_gltf(&path, "wave");
    let blob = bake_model(
        &path,
        &ModelOptions {
            anim_fps: 10.0,
            once: vec!["wave".into()],
            ..ModelOptions::default()
        },
    )
    .unwrap();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    assert_eq!(mesh.vertex_count, 6, "two triangles merged");
    assert_eq!(mesh.joint_count(), 2);
    let clip = mesh.find_clip(ClipId::from_name("wave")).unwrap();
    assert_eq!(clip.frame_count, 11, "1 s at 10 fps, inclusive");
    assert!(!clip.looping, "listed in `once`");

    // Bind pose: the arm triangle sits at x = 2..3.
    let mut skin = [Mat34::IDENTITY; MAX_JOINTS];
    mesh.pose(None, &mut skin);
    let arm_tip = |skin: &[Mat34; MAX_JOINTS]| skin[1].transform_point(mesh.position(4));
    let rest = arm_tip(&skin);
    assert!(
        (rest.x - 3.0).abs() < 1e-4 && rest.y.abs() < 1e-4,
        "{rest:?}"
    );

    // End of the clip: the arm turned 90 degrees around its own origin, so
    // its (1, 0, 0) vertex now points up from x = 2.
    mesh.pose(
        Some(AnimRef::new(clip.id, 5.0)), // clamps: one-shot clip
        &mut skin,
    );
    let tip = arm_tip(&skin);
    assert!(
        (tip - Vec3::new(2.0, 1.0, 0.0)).dot(tip - Vec3::new(2.0, 1.0, 0.0)) < 1e-3,
        "{tip:?}"
    );
    // The body never moves.
    assert_eq!(skin[0], Mat34::IDENTITY);

    // Bounds cover the animated extent (the tip reaches y = 1 at x = 2).
    let (c, r) = mesh.bounds().unwrap();
    let d = tip - c;
    assert!(d.dot(d).sqrt() <= r);
}

#[test]
fn clips_start_at_their_first_keyframe() {
    // Blender-style timeline offset: keys at 1/24 s and 1 + 1/24 s. The
    // clip must still be exactly 1 s (no held duplicate of the first pose).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("offset.gltf");
    write_gltf_at(&path, "spin", 1.0 / 24.0);
    let blob = bake_model(
        &path,
        &ModelOptions {
            anim_fps: 10.0,
            ..ModelOptions::default()
        },
    )
    .unwrap();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    let clip = mesh.find_clip(ClipId::from_name("spin")).unwrap();
    assert_eq!(clip.frame_count, 11);
    assert!((clip.duration() - 1.0).abs() < 1e-6);
}

#[test]
fn static_gltf_stays_skeleton_free() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("static.gltf");
    write_gltf(&path, "wave");
    // Strip the animation: same file, no "animations" array.
    let text = std::fs::read_to_string(&path).unwrap();
    let start = text.find("  \"animations\"").unwrap();
    let end = text.find("  \"buffers\"").unwrap();
    std::fs::write(&path, format!("{}{}", &text[..start], &text[end..])).unwrap();
    let blob = bake_model(&path, &ModelOptions::default()).unwrap();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    assert_eq!(mesh.joint_count(), 0);
    // Child transform still applied when merging.
    let max_x = (0..mesh.vertex_count)
        .map(|i| mesh.position(i).x)
        .fold(f32::MIN, f32::max);
    assert!((max_x - 3.0).abs() < 1e-5);
}
