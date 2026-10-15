//! The TMDL mesh format through `Mesh::from_tmdl`: the base layout, every
//! optional section (`BNDS`, `SKEL`, `ANIM`, `TXUV`, `TXID`, `SKW2`), the
//! forward-compatible skip of unknown tags, every `MeshError`, and the
//! skeleton evaluation (`pose`, crossfades with `AnimRef::blend_to`).

mod common;

use common::*;
use trino_core::render3d::{MAX_JOINTS, Mesh, MeshError};
use trino_core::{AnimRef, ClipId, Color, Mat34, ModelParams, SpriteId, Vec2, Vec3};

fn translation(m: &Mat34) -> Vec3 {
    Vec3::new(m.m[0][3], m.m[1][3], m.m[2][3])
}

fn near(a: Vec3, b: Vec3) -> bool {
    (a.x - b.x).abs() < 1e-4 && (a.y - b.y).abs() < 1e-4 && (a.z - b.z).abs() < 1e-4
}

#[test]
fn base_layout_vertices_normals_colors_indices() {
    let blob = cube_tmdl();
    let m = Mesh::from_tmdl(&blob).unwrap();
    assert_eq!((m.vertex_count, m.index_count), (24, 36));
    assert_eq!(m.color(0), Color::rgba(230, 80, 80, 255));
    assert_eq!(m.normal(0), Vec3::new(0.0, 0.0, -1.0));
    // The generator's basis uses the core's software sqrt (~1e-5).
    assert!(near(m.position(0), Vec3::new(0.5, -0.5, -0.5)));
    assert_eq!((m.index(0), m.index(1), m.index(2)), (0, 1, 2));
    assert_eq!(m.index(35), 23);
    let (lo, hi) = m.aabb().unwrap();
    assert!(near(lo, Vec3::new(-0.5, -0.5, -0.5)) && near(hi, Vec3::new(0.5, 0.5, 0.5)));
    // No optional sections.
    assert_eq!(m.bounds(), None);
    assert_eq!((m.joint_count(), m.clip_count()), (0, 0));
    assert!(!m.has_uvs() && !m.has_blend());
    assert_eq!(m.uv(3), Vec2::ZERO);
    assert_eq!(m.default_texture(), None);
    assert_eq!(m.clip(0), None);
    let mut out = [Mat34::IDENTITY; MAX_JOINTS];
    assert_eq!(m.pose(None, &mut out), 0, "static mesh: no joints");

    // An empty mesh is valid and has no box.
    let empty = tmdl(&[], &[], &[], &[]);
    let e = Mesh::from_tmdl(&empty).unwrap();
    assert_eq!((e.vertex_count, e.aabb()), (0, None));
}

#[test]
fn bnds_section_carries_the_bounding_sphere() {
    let blob = bounded_cube();
    let m = Mesh::from_tmdl(&blob).unwrap();
    assert_eq!(m.bounds(), Some((Vec3::ZERO, 0.87)));
    // A short BNDS body is ignored, not an error.
    let mut short = cube_tmdl();
    section(&mut short, b"BNDS", &[0; 8]);
    assert_eq!(Mesh::from_tmdl(&short).unwrap().bounds(), None);
}

#[test]
fn txuv_and_txid_sections_make_a_textured_mesh() {
    let blob = textured_cube(SpriteId(42));
    let m = Mesh::from_tmdl(&blob).unwrap();
    assert!(m.has_uvs());
    let uv = m.uv(0); // vertex (0.5, -0.5, ..)
    assert!((uv.x - 1.0).abs() < 1e-4 && uv.y.abs() < 1e-4, "{uv:?}");
    assert_eq!(m.default_texture(), Some(SpriteId(42)));
    assert_eq!(m.texture_for(&ModelParams::DEFAULT), Some(SpriteId(42)));
    let over = ModelParams {
        texture: Some(SpriteId(7)),
        ..ModelParams::DEFAULT
    };
    assert_eq!(m.texture_for(&over), Some(SpriteId(7)), "per-draw override");
    // Without UVs a texture never applies, even when asked for.
    let plain = cube_tmdl();
    assert_eq!(Mesh::from_tmdl(&plain).unwrap().texture_for(&over), None);
}

#[test]
fn unknown_sections_are_skipped_and_later_ones_still_read() {
    let mut blob = cube_tmdl();
    section(&mut blob, b"ZZZZ", &[1, 2, 3, 4, 5]);
    section(&mut blob, b"BNDS", &bnds(Vec3::new(0.0, 1.0, 0.0), 2.0));
    section(&mut blob, b"TXID", &9u32.to_le_bytes());
    let m = Mesh::from_tmdl(&blob).unwrap();
    assert_eq!(m.bounds(), Some((Vec3::new(0.0, 1.0, 0.0), 2.0)));
    assert_eq!(m.default_texture(), Some(SpriteId(9)));
    // Trailing bytes too short for a section header are ignored.
    blob.extend_from_slice(&[0, 1, 2]);
    assert!(Mesh::from_tmdl(&blob).is_ok());
}

#[test]
fn every_mesh_error() {
    assert_eq!(Mesh::from_tmdl(b"TMD").unwrap_err(), MeshError::BadMagic);
    let mut wrong = cube_tmdl();
    wrong[0] = b'X';
    assert_eq!(Mesh::from_tmdl(&wrong).unwrap_err(), MeshError::BadMagic);
    let cube = cube_tmdl();
    assert_eq!(
        Mesh::from_tmdl(&cube[..cube.len() - 2]).unwrap_err(),
        MeshError::Truncated
    );

    let bad = |extra: &dyn Fn(&mut Vec<u8>)| {
        let mut b = cube_tmdl();
        extra(&mut b);
        Mesh::from_tmdl(&b).unwrap_err()
    };
    // A section longer than the blob.
    assert_eq!(
        bad(&|b| {
            b.extend_from_slice(b"BNDS");
            b.extend_from_slice(&100u32.to_le_bytes());
            b.extend_from_slice(&[0; 16]);
        }),
        MeshError::BadSection
    );
    // ANIM and SKW2 need a skeleton first.
    assert_eq!(
        bad(&|b| section(b, b"ANIM", &0u32.to_le_bytes())),
        MeshError::BadSection
    );
    assert_eq!(
        bad(&|b| section(b, b"SKW2", &[0; 48])),
        MeshError::BadSection
    );
    // TXUV shorter than 8 bytes per vertex.
    assert_eq!(
        bad(&|b| section(b, b"TXUV", &[0; 8])),
        MeshError::BadSection
    );
    let jov = [0u8; 24];
    // A skeleton with zero joints, a vertex naming a missing joint, a body
    // shorter than its joints, and more joints than MAX_JOINTS.
    assert_eq!(
        bad(&|b| section(b, b"SKEL", &skel(&jov, &[]))),
        MeshError::BadSection
    );
    let mut far = jov;
    far[5] = 1;
    assert_eq!(
        bad(&|b| section(b, b"SKEL", &skel(&far, &[(-1, IDENT34, REST)]))),
        MeshError::BadSection
    );
    assert_eq!(
        bad(&|b| {
            let mut s = skel(&jov, &[(-1, IDENT34, REST)]);
            s.truncate(s.len() - 4);
            section(b, b"SKEL", &s)
        }),
        MeshError::BadSection
    );
    assert_eq!(
        bad(&|b| section(b, b"SKEL", &[1, 0])),
        MeshError::BadSection
    );
    let many: Vec<_> = (0..=MAX_JOINTS as i32)
        .map(|j| (j - 1, IDENT34, REST))
        .collect();
    assert_eq!(
        bad(&|b| section(b, b"SKEL", &skel(&jov, &many))),
        MeshError::TooManyJoints
    );
    // ANIM: missing count, a clip header cut short, a zero-frame clip,
    // samples cut short.
    let with_skel = |b: &mut Vec<u8>| section(b, b"SKEL", &skel(&jov, &[(-1, IDENT34, REST)]));
    assert_eq!(
        bad(&|b| {
            with_skel(b);
            section(b, b"ANIM", &[1, 0]);
        }),
        MeshError::BadSection
    );
    assert_eq!(
        bad(&|b| {
            with_skel(b);
            section(b, b"ANIM", &1u32.to_le_bytes());
        }),
        MeshError::BadSection
    );
    let zero = ClipDef {
        name: "idle",
        fps: 10.0,
        looping: true,
        frames: vec![],
    };
    assert_eq!(
        bad(&|b| {
            with_skel(b);
            section(
                b,
                b"ANIM",
                &anim(&[ClipDef {
                    frames: vec![],
                    ..zero
                }]),
            )
        }),
        MeshError::BadSection
    );
    assert_eq!(
        bad(&|b| {
            with_skel(b);
            let mut a = anim(&[ClipDef {
                name: "idle",
                fps: 10.0,
                looping: true,
                frames: vec![vec![REST]; 2],
            }]);
            a.truncate(a.len() - 1);
            section(b, b"ANIM", &a)
        }),
        MeshError::BadSection
    );
    // SKW2 naming a joint the skeleton lacks, or too short.
    assert_eq!(
        bad(&|b| {
            with_skel(b);
            let mut w = vec![0u8; 48];
            w[0] = 3;
            section(b, b"SKW2", &w)
        }),
        MeshError::BadSection
    );
    assert_eq!(
        bad(&|b| {
            with_skel(b);
            section(b, b"SKW2", &[0; 10])
        }),
        MeshError::BadSection
    );
}

#[test]
fn skel_and_anim_sections_pose_a_rigid_skeleton() {
    let blob = rigged_cube();
    let m = Mesh::from_tmdl(&blob).unwrap();
    assert_eq!((m.joint_count(), m.clip_count()), (2, 2));
    let lift = m.clip(0).unwrap();
    assert_eq!(lift.id, ClipId::from_name("lift"));
    assert_eq!((lift.fps, lift.frame_count, lift.looping), (2.0, 3, true));
    assert_eq!(lift.duration(), 1.0);
    let slide = m.find_clip(ClipId::from_name("slide")).unwrap();
    assert_eq!((slide.frame_count, slide.looping), (2, false));
    assert_eq!(slide.duration(), 1.0);
    assert_eq!(m.find_clip(ClipId::from_name("missing")), None);
    assert_eq!(m.clip(2), None);

    let mut out = [Mat34::IDENTITY; MAX_JOINTS];
    // Bind pose (no clip, or an unknown one): identity skin matrices.
    assert_eq!(m.pose(None, &mut out), 2);
    assert!(near(translation(&out[1]), Vec3::ZERO));
    m.pose(Some(AnimRef::new(ClipId::from_name("nope"), 0.3)), &mut out);
    assert!(near(translation(&out[1]), Vec3::ZERO));

    // `lift` at 2 fps: t=0.5 is frame 1 (child up 1), t=0.75 between 1 and
    // 2, and the loop wraps over its two-frame span (t=1.25 == t=0.25).
    let at = |clip: &str, t: f32, out: &mut [Mat34; MAX_JOINTS]| {
        m.pose(Some(AnimRef::new(ClipId::from_name(clip), t)), out);
        translation(&out[1])
    };
    assert!(near(at("lift", 0.5, &mut out), Vec3::new(0.0, 1.0, 0.0)));
    assert!(near(at("lift", 0.75, &mut out), Vec3::new(0.0, 1.5, 0.0)));
    assert!(near(at("lift", 1.25, &mut out), at("lift", 0.25, &mut out)));
    assert!(near(at("lift", -0.25, &mut out), Vec3::new(0.0, 1.5, 0.0)));
    assert!(near(translation(&out[0]), Vec3::ZERO), "the root stays");
    // `slide` plays once: it clamps to its last frame.
    assert!(near(at("slide", 0.5, &mut out), Vec3::new(2.0, 0.0, 0.0)));
    assert!(near(at("slide", 9.0, &mut out), Vec3::new(4.0, 0.0, 0.0)));
    assert!(near(at("slide", -1.0, &mut out), Vec3::ZERO));
}

#[test]
fn blend_to_crossfades_two_clips_per_joint() {
    let blob = rigged_cube();
    let m = Mesh::from_tmdl(&blob).unwrap();
    let mut out = [Mat34::IDENTITY; MAX_JOINTS];
    let base = AnimRef::new(ClipId::from_name("lift"), 0.5); // child at y=1
    let slide = ClipId::from_name("slide");
    for (w, want) in [
        (0.0, Vec3::new(0.0, 1.0, 0.0)),
        (0.5, Vec3::new(2.0, 0.5, 0.0)),
        (1.0, Vec3::new(4.0, 0.0, 0.0)),
        // The weight is clamped to 0..1.
        (3.0, Vec3::new(4.0, 0.0, 0.0)),
        (-1.0, Vec3::new(0.0, 1.0, 0.0)),
    ] {
        m.pose(Some(base.blend_to(slide, 5.0, w)), &mut out);
        assert!(near(translation(&out[1]), want), "w={w}: {:?}", out[1]);
    }
    let blended = base.blend_to(slide, 5.0, 0.25);
    assert_eq!(blended.blend.unwrap().weight, 0.25);
    assert_eq!(blended.clip, base.clip);
    // An unknown blend target leaves the main clip alone.
    m.pose(
        Some(base.blend_to(ClipId::from_name("ghost"), 0.0, 1.0)),
        &mut out,
    );
    assert!(near(translation(&out[1]), Vec3::new(0.0, 1.0, 0.0)));
}

#[test]
fn hierarchy_composes_parent_before_child_with_inverse_bind() {
    // Root translated +2 X at rest, child rest +1 Y relative to it; the
    // child's inverse bind undoes its bind position (2, 1, 0).
    let jov = [1u8; 24];
    let mut inv = IDENT34;
    inv[3] = -2.0;
    inv[7] = -1.0;
    let mut blob = cube_tmdl();
    section(
        &mut blob,
        b"SKEL",
        &skel(
            &jov,
            &[
                (-1, IDENT34, sample(Vec3::new(2.0, 0.0, 0.0))),
                (0, inv, sample(Vec3::new(0.0, 1.0, 0.0))),
            ],
        ),
    );
    let m = Mesh::from_tmdl(&blob).unwrap();
    let mut out = [Mat34::IDENTITY; MAX_JOINTS];
    m.pose(None, &mut out);
    // Bind pose skins to identity for the child; the root's skin is its
    // world matrix (its inverse bind is the identity here).
    assert!(near(translation(&out[1]), Vec3::ZERO), "{:?}", out[1]);
    assert!(near(translation(&out[0]), Vec3::new(2.0, 0.0, 0.0)));
}

#[test]
fn skw2_section_adds_a_second_joint_weight() {
    let base = rigged_cube();
    let mesh = Mesh::from_tmdl(&base).unwrap();
    let mut w = Vec::new();
    for i in 0..mesh.vertex_count {
        // Top vertices blend halfway back toward the root.
        if mesh.position(i).y > 0.0 {
            w.extend_from_slice(&[0, 128]);
        } else {
            w.extend_from_slice(&[0, 0]);
        }
    }
    let mut blob = base.clone();
    section(&mut blob, b"SKW2", &w);
    let m = Mesh::from_tmdl(&blob).unwrap();
    assert!(m.has_blend() && !mesh.has_blend());
    let top = (0..m.vertex_count)
        .find(|&i| m.position(i).y > 0.0)
        .unwrap();
    let bottom = (0..m.vertex_count)
        .find(|&i| m.position(i).y < 0.0)
        .unwrap();
    let (j, wt) = m.blend_of(top).unwrap();
    assert_eq!(j, 0);
    assert!((wt - 128.0 / 255.0).abs() < 1e-6);
    assert_eq!(m.blend_of(bottom), None, "weight 0 = rigid");
    assert_eq!(mesh.blend_of(top), None);
}

#[test]
fn layout_rebuilds_an_identical_mesh_without_reparsing() {
    for blob in [
        cube_tmdl(),
        rigged_cube(),
        textured_cube(SpriteId(3)),
        bounded_cube(),
    ] {
        let parsed = Mesh::from_tmdl(&blob).unwrap();
        let layout = parsed.layout(&blob);
        let rebuilt = Mesh::from_layout(&blob, &layout);
        assert_eq!(format!("{parsed:?}"), format!("{rebuilt:?}"));
        assert_eq!(rebuilt.vertex_count, parsed.vertex_count);
        assert_eq!(rebuilt.bounds(), parsed.bounds());
        assert_eq!(rebuilt.joint_count(), parsed.joint_count());
    }
}

#[test]
fn single_frame_clips_and_empty_meshes() {
    let mut blob = cube_tmdl();
    section(&mut blob, b"SKEL", &skel(&[0; 24], &[(-1, IDENT34, REST)]));
    section(
        &mut blob,
        b"ANIM",
        &anim(&[ClipDef {
            name: "pose",
            fps: 12.0,
            looping: false,
            frames: vec![vec![sample(Vec3::new(0.0, 3.0, 0.0))]],
        }]),
    );
    let m = Mesh::from_tmdl(&blob).unwrap();
    let clip = m.clip(0).unwrap();
    assert_eq!((clip.frame_count, clip.duration()), (1, 0.0), "a held pose");
    let mut out = [Mat34::IDENTITY; MAX_JOINTS];
    m.pose(Some(AnimRef::new(clip.id, 99.0)), &mut out);
    assert!(near(translation(&out[0]), Vec3::new(0.0, 3.0, 0.0)));

    // An empty mesh still yields a usable layout.
    let empty = tmdl(&[], &[], &[], &[]);
    let e = Mesh::from_tmdl(&empty).unwrap();
    let again = Mesh::from_layout(&empty, &e.layout(&empty));
    assert_eq!((again.vertex_count, again.index_count), (0, 0));
}
