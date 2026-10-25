//! Foot IK on the strider (issue #65): its manifest `ik` bakes to `IKCH`,
//! a raised or lowered foot reaches the ground it is given within a few
//! millimetres with the pelvis dropping for the lower one, no IK is the
//! clip's pose bit for bit, a bad chain is a bake error, and the feet an
//! animation graph reports are where the pose puts the ankles.

use std::path::PathBuf;

use trino_asset_pipeline::model::{IkDecl, ModelOptions};
use trino_asset_pipeline::{Manifest, Platform, bake_model, resolve_source};
use trino_core::anim::IkPose;
use trino_core::{AnimRef, ClipId, MAX_JOINTS, Mat34, Mesh, Vec3};

fn strider() -> (PathBuf, ModelOptions, Vec<u8>) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let m = Manifest::load(&root.join("assets/manifest.toml")).unwrap();
    let d = &m.models["strider"];
    let path = resolve_source(&root.join("assets"), Platform::Pc, &d.file).unwrap();
    let opts = d.options();
    let tmdl = bake_model(&path, &opts).unwrap();
    (path, opts, tmdl)
}

fn joint_pos(m: &Mat34) -> Vec3 {
    Vec3::new(m.m[0][3], m.m[1][3], m.m[2][3])
}

#[test]
fn feet_reach_their_ground_and_none_is_the_clip() {
    let (_, _, tmdl) = strider();
    let mesh = Mesh::from_tmdl(&tmdl).unwrap();
    let chains = mesh.ik_chains().expect("IKCH baked");
    assert_eq!(chains.legs, 2);
    let (lf, rf, pelvis) = (
        chains.chain[0][2] as usize,
        chains.chain[1][2] as usize,
        chains.pelvis as usize,
    );
    for (clip, t) in [("idle", 0.3), ("walk", 0.1), ("walk", 0.6), ("run", 0.2)] {
        let a = AnimRef::new(ClipId::from_name(clip), t);
        let mut plain = [Mat34::IDENTITY; MAX_JOINTS];
        mesh.pose_world(Some(a), &mut plain);
        // A step under the left foot, a dip under the right.
        let mut ik = [Mat34::IDENTITY; MAX_JOINTS];
        mesh.pose_world(Some(a.with_ik(IkPose::from_units(0.15, -0.08))), &mut ik);
        let dl = joint_pos(&ik[lf]) - joint_pos(&plain[lf]);
        let dr = joint_pos(&ik[rf]) - joint_pos(&plain[rf]);
        assert!(
            (dl.y - 0.15).abs() < 0.004 && dl.x.abs() < 0.004 && dl.z.abs() < 0.004,
            "{clip}@{t}: left {dl:?}"
        );
        assert!(
            (dr.y + 0.08).abs() < 0.004 && dr.x.abs() < 0.004 && dr.z.abs() < 0.004,
            "{clip}@{t}: right {dr:?}"
        );
        let dp = joint_pos(&ik[pelvis]) - joint_pos(&plain[pelvis]);
        assert!(
            (dp.y + 0.08).abs() < 1e-4,
            "the pelvis drops for the lower foot: {dp:?}"
        );
        // No IK: the clip's pose, bit for bit (skinning matrices too).
        let (mut a1, mut a2) = ([Mat34::IDENTITY; MAX_JOINTS], [Mat34::IDENTITY; MAX_JOINTS]);
        mesh.pose(Some(a), &mut a1);
        mesh.pose(Some(a.with_ik(IkPose::NONE)), &mut a2);
        assert_eq!(a1, a2);
    }
}

#[test]
fn a_chain_that_does_not_descend_is_a_bake_error() {
    let (path, mut opts, _) = strider();
    opts.ik = Some(IkDecl {
        pelvis: None,
        left: Some(["foot_l".into(), "shin_l".into(), "thigh_l".into()]),
        right: None,
    });
    let e = bake_model(&path, &opts).unwrap_err();
    assert!(e.contains("hip -> knee -> foot"), "{e}");
    opts.ik = Some(IkDecl {
        pelvis: Some("tail".into()),
        left: Some(["thigh_l".into(), "shin_l".into(), "foot_l".into()]),
        right: None,
    });
    let e = bake_model(&path, &opts).unwrap_err();
    assert!(e.contains("no joint `tail`"), "{e}");
}

#[test]
fn the_graphs_feet_are_the_poses_ankles() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let (_, _, tmdl) = strider();
    let mesh = Mesh::from_tmdl(&tmdl).unwrap();
    let chains = mesh.ik_chains().unwrap();
    let manifest = Manifest::load(&root.join("assets/manifest.toml")).unwrap();
    let def =
        trino_scene::anim::AnimGraphDef::load(&root.join("assets/anim/strider.anim.ron")).unwrap();
    let tang = trino_asset_pipeline::anim_graph::bake(&root, &manifest, Platform::Pc, &def)
        .unwrap()
        .unwrap();
    let g = trino_core::AnimGraph::parse(&tang).unwrap();
    let mut i = trino_core::AnimGraphInstance::new(&g);
    i.set(&g, "speed", 0.8);
    for _ in 0..37 {
        i.update(&g, 1.0 / 60.0);
        let feet = i.feet(&g).unwrap();
        let mut w = [Mat34::IDENTITY; MAX_JOINTS];
        mesh.pose_world(Some(i.anim(&g)), &mut w);
        for (leg, f) in feet.iter().enumerate() {
            let ankle = joint_pos(&w[chains.chain[leg][2] as usize]);
            let d = *f - ankle;
            // The track is sampled per baked frame and lerped: centimetres.
            assert!(d.dot(d).sqrt() < 0.03, "leg {leg}: {f:?} vs {ankle:?}");
        }
    }
}
