//! Animation measurements and pose export on clips built in the test, where
//! the right answers are known: an in-place walk (its stride speed and
//! distance per cycle), a planted foot that slides, a foot through the
//! floor, a crossfade half way, and a pose baked into a static mesh that
//! bakes back to the same triangles.

use trino_asset_pipeline::anim::{self, Limits};
use trino_asset_pipeline::model::{ClipData, JointData, SkeletonData, Trs};
use trino_asset_pipeline::{MeshData, bake_model};
use trino_core::{AnimRef, ClipId, Mesh, Vec3};

const ID: [[f32; 4]; 3] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

/// A box from `lo` to `hi` driven by joint `j`: 8 vertices, 12 triangles.
fn cuboid(d: &mut MeshData, joint: &mut Vec<u8>, j: u8, lo: [f32; 3], hi: [f32; 3]) {
    let base = d.positions.len() as u32;
    for k in 0..8 {
        let p = [
            if k & 1 == 0 { lo[0] } else { hi[0] },
            if k & 2 == 0 { lo[1] } else { hi[1] },
            if k & 4 == 0 { lo[2] } else { hi[2] },
        ];
        d.positions.push(p);
        d.normals.push([0.0, 1.0, 0.0]);
        d.colors.push([200, 120, 60, 255]);
        joint.push(j);
    }
    for f in [
        [0, 1, 3, 0, 3, 2],
        [4, 6, 7, 4, 7, 5],
        [0, 4, 5, 0, 5, 1],
        [2, 3, 7, 2, 7, 6],
        [0, 2, 6, 0, 6, 4],
        [1, 5, 7, 1, 7, 3],
    ] {
        d.indices.extend(f.iter().map(|i| base + i));
    }
}

fn at(t: [f32; 3]) -> Trs {
    Trs { t, ..Trs::IDENTITY }
}

/// A body (joint 0) and two feet (joints 1, 2), and clips over them: each
/// clip is a list of (left foot, right foot) translations per frame.
/// Per frame: the left and the right foot's translation.
type Feet = Vec<([f32; 3], [f32; 3])>;

fn walker(clips: &[(&str, Feet)]) -> Vec<u8> {
    let mut d = MeshData::default();
    let mut joint = Vec::new();
    cuboid(&mut d, &mut joint, 0, [-0.2, 0.4, -0.1], [0.2, 1.0, 0.1]);
    for j in [1, 2] {
        cuboid(
            &mut d,
            &mut joint,
            j,
            [-0.05, 0.0, -0.05],
            [0.05, 0.05, 0.05],
        );
    }
    let joints = (0..3)
        .map(|_| JointData {
            parent: -1,
            inverse_bind: ID,
            rest: Trs::IDENTITY,
        })
        .collect();
    d.skeleton = Some(SkeletonData {
        joint_of_vertex: joint,
        joints,
        clips: clips
            .iter()
            .map(|(name, frames)| ClipData {
                name: name.to_string(),
                fps: 8.0,
                looping: true,
                frames: frames
                    .iter()
                    .map(|(l, r)| vec![Trs::IDENTITY, at(*l), at(*r)])
                    .collect(),
                ..Default::default()
            })
            .collect(),
    });
    d.encode().unwrap()
}

/// An in-place walk: each foot is planted for 4 frames, moving back 5 cm a
/// frame (0.4 m/s at 8 fps), then lifted 10 cm and carried forward. 9
/// frames: a looping clip ends on its first frame (1 s at 8 fps).
fn walk_frames() -> Feet {
    let planted = |k: usize, x: f32| [x, 0.0, 0.1 - 0.05 * k as f32];
    let lifted = |k: usize, x: f32| [x, 0.1, -0.1 + 0.05 * k as f32];
    (0..9)
        .map(|f| f % 8)
        .map(|f| {
            if f < 4 {
                (planted(f, -0.15), lifted(f, 0.15))
            } else {
                (lifted(f - 4, -0.15), planted(f - 4, 0.15))
            }
        })
        .collect()
}

#[test]
fn a_walk_cycle_reports_its_stride_speed_and_distance() {
    let tmdl = walker(&[("walk", walk_frames())]);
    let m = anim::clip_metrics(&tmdl, "walk", Limits::default()).unwrap();
    assert_eq!(m.frames, 9);
    assert!((m.duration - 1.0).abs() < 1e-6);
    assert!((m.speed - 0.4).abs() < 1e-4, "speed {}", m.speed);
    assert!(
        (m.distance_per_cycle - 0.4).abs() < 1e-4,
        "{}",
        m.distance_per_cycle
    );
    assert!(
        m.foot_slide < 1e-4,
        "a clean walk does not slide: {}",
        m.foot_slide
    );
    assert!(m.lowest.abs() < 1e-6);
    assert_eq!(m.feet, vec![1, 2], "both feet touch, the body never does");
    assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    // Bounds per frame: the lifted foot raises nothing above the body,
    // but the planted one moves the box back and forth.
    assert_eq!(m.bounds.len(), 9);
    assert!((m.bounds[0].max[1] - 1.0).abs() < 1e-6);
    assert!(m.bounds[0].max[2] > m.bounds[3].max[2] - 1e-6);
    // Deterministic.
    assert_eq!(
        anim::clip_metrics(&tmdl, "walk", Limits::default()).unwrap(),
        m
    );
}

#[test]
fn a_sliding_foot_and_a_foot_through_the_floor_warn() {
    // One planted step of the left foot jumps 15 cm instead of 5.
    let mut skate = walk_frames();
    for f in skate.iter_mut().take(4).skip(2) {
        f.0[2] -= 0.1;
    }
    // The right foot pokes 3 cm under the floor at one frame.
    let mut sink = walk_frames();
    sink[5].1[1] = -0.03;
    let tmdl = walker(&[("skate", skate), ("sink", sink)]);
    let s = anim::clip_metrics(&tmdl, "skate", Limits::default()).unwrap();
    assert!(s.foot_slide > 0.05, "{}", s.foot_slide);
    assert!(
        s.warnings.iter().any(|w| w.contains("slides")),
        "{:?}",
        s.warnings
    );
    let k = anim::clip_metrics(&tmdl, "sink", Limits::default()).unwrap();
    assert!((k.lowest + 0.03).abs() < 1e-5, "{}", k.lowest);
    assert!(
        k.warnings
            .iter()
            .any(|w| w.contains("3.0 cm below the floor")),
        "{:?}",
        k.warnings
    );
    // Looser limits silence them; an unknown clip is an error.
    let loose = Limits {
        slide: 1.0,
        below: 1.0,
    };
    assert!(
        anim::clip_metrics(&tmdl, "skate", loose)
            .unwrap()
            .warnings
            .is_empty()
    );
    assert!(anim::clip_metrics(&tmdl, "run", Limits::default()).is_err());
    let all = anim::all_metrics(
        &tmdl,
        &["sink".into(), "skate".into(), "nope".into()],
        loose,
    );
    assert_eq!(all.len(), 2);
}

#[test]
fn a_crossfade_half_way_is_the_interpolated_pose() {
    let front = vec![([-0.15, 0.0, 0.2], [0.15, 0.0, 0.2]); 2];
    let back = vec![([-0.15, 0.2, -0.2], [0.15, 0.0, -0.2]); 2];
    let tmdl = walker(&[("front", front), ("back", back)]);
    let mesh = Mesh::from_tmdl(&tmdl).unwrap();
    let a = AnimRef::new(ClipId::from_name("front"), 0.0);
    let half = a.blend_to(ClipId::from_name("back"), 0.0, 0.5);
    let p_front = anim::posed(&mesh, Some(a));
    let p_back = anim::posed(&mesh, Some(AnimRef::new(ClipId::from_name("back"), 0.0)));
    let p_half = anim::posed(&mesh, Some(half));
    for i in 0..mesh.vertex_count {
        let want = (p_front[i] + p_back[i]) * 0.5;
        let d = p_half[i] - want;
        assert!(d.dot(d) < 1e-10, "vertex {i}: {:?} vs {want:?}", p_half[i]);
    }
    // The left foot half way: z 0, lifted 10 cm.
    let left = (0..mesh.vertex_count)
        .find(|&i| mesh.joint_of_vertex(i) == 1)
        .unwrap();
    assert!((p_half[left].z - (0.0 - 0.05)).abs() < 1e-6);
    assert!((p_half[left].y - 0.1).abs() < 1e-6);
    // Weight 0 and 1 are the two clips.
    let w0 = anim::posed(&mesh, Some(a.blend_to(ClipId::from_name("back"), 0.0, 0.0)));
    assert_eq!(w0, p_front);
}

#[test]
fn a_pose_bakes_into_a_static_mesh_with_the_same_triangles() {
    let tmdl = walker(&[("walk", walk_frames())]);
    let data = anim::pose_mesh(&tmdl, Some("walk"), 0.25).unwrap();
    assert!(data.skeleton.is_none());
    let mesh = Mesh::from_tmdl(&tmdl).unwrap();
    assert_eq!(data.indices.len(), mesh.index_count);
    let posed = anim::posed(&mesh, Some(AnimRef::new(ClipId::from_name("walk"), 0.25)));
    for (i, p) in data.positions.iter().enumerate() {
        assert_eq!(Vec3::new(p[0], p[1], p[2]), posed[i]);
    }
    // Written as glTF, it bakes back to a skeleton-free TMDL of the same
    // triangles at the same places, with the same colors.
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("walk_pose.gltf");
    std::fs::write(&file, anim::write_gltf(&data, "walk_pose")).unwrap();
    let back = bake_model(&file, &Default::default()).unwrap();
    let b = Mesh::from_tmdl(&back).unwrap();
    assert_eq!(b.joint_count(), 0, "static");
    assert_eq!(b.index_count, mesh.index_count);
    let tri = |m: &Mesh, pos: &dyn Fn(usize) -> Vec3| -> Vec<[i64; 9]> {
        let mut v: Vec<[i64; 9]> = (0..m.index_count / 3)
            .map(|t| {
                let mut k = [0i64; 9];
                for c in 0..3 {
                    let p = pos(m.index(t * 3 + c) as usize);
                    for (a, x) in [p.x, p.y, p.z].into_iter().enumerate() {
                        k[c * 3 + a] = (x * 1e4).round() as i64;
                    }
                }
                k
            })
            .collect();
        v.sort();
        v
    };
    assert_eq!(tri(&b, &|i| b.position(i)), tri(&mesh, &|i| posed[i]));
    for i in 0..b.vertex_count {
        let c = b.color(i);
        assert!(
            (c.r as i32 - 200).abs() <= 1
                && (c.g as i32 - 120).abs() <= 1
                && (c.b as i32 - 60).abs() <= 1,
            "{c:?}"
        );
    }
    // The bind pose works too; an unknown clip doesn't.
    assert_eq!(
        anim::pose_mesh(&tmdl, None, 0.0).unwrap().positions.len(),
        data.positions.len()
    );
    assert!(anim::pose_mesh(&tmdl, Some("fly"), 0.0).is_err());
}

#[test]
fn export_pose_writes_a_model_and_its_manifest_entry() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let tmp = tempfile::tempdir().unwrap();
    let assets = tmp.path().join("assets");
    std::fs::create_dir_all(assets.join("shared/models/showcase")).unwrap();
    std::fs::copy(
        repo.join("shared/models/showcase/robot.glb"),
        assets.join("shared/models/showcase/robot.glb"),
    )
    .unwrap();
    std::fs::write(
        assets.join("manifest.toml"),
        "version = 1\n\n[models.robot]\nfile = \"models/showcase/robot.glb\"\n",
    )
    .unwrap();
    let e = anim::export_pose(
        &assets,
        "models/robot",
        Some("walk"),
        0.3,
        "robot-walk-pose",
    )
    .unwrap();
    assert_eq!(e.logical, "models/robot-walk-pose");
    assert!(e.manifest_changed && e.tris > 0 && e.n64_us > 0);
    assert!(assets.join(&e.file).exists());
    let manifest = std::fs::read_to_string(assets.join("manifest.toml")).unwrap();
    assert!(manifest.contains("[models.robot-walk-pose]"));
    // It bakes like any model, without a skeleton.
    let baked = bake_model(&assets.join(&e.file), &Default::default()).unwrap();
    assert_eq!(Mesh::from_tmdl(&baked).unwrap().joint_count(), 0);
    // Again: same file, no second manifest entry.
    let again = anim::export_pose(
        &assets,
        "models/robot",
        Some("walk"),
        0.3,
        "robot-walk-pose",
    )
    .unwrap();
    assert!(!again.manifest_changed);
    assert!(anim::export_pose(&assets, "models/robot", Some("nope"), 0.0, "x").is_err());
    assert!(anim::export_pose(&assets, "models/ghost", None, 0.0, "x").is_err());
}

/// A walker whose root carries it (issue #65): the body (joint 0, the root)
/// moves forward with a surge, and each foot (a child) stays planted where
/// it touched down. `surge` = 0 is the constant-speed control.
fn strider(surge: f32, root_motion: bool) -> Vec<u8> {
    let mut d = MeshData::default();
    let mut joint = Vec::new();
    cuboid(&mut d, &mut joint, 0, [-0.2, 0.4, -0.1], [0.2, 1.0, 0.1]);
    for j in [1, 2] {
        cuboid(
            &mut d,
            &mut joint,
            j,
            [-0.05, 0.0, -0.05],
            [0.05, 0.05, 0.05],
        );
    }
    let joint_at = |parent: i32| JointData {
        parent,
        inverse_bind: ID,
        rest: Trs::IDENTITY,
    };
    // The root's z per frame: 5 cm a frame plus a surge, back on the
    // grid every 4 frames (where a foot touches down).
    let r = |f: usize| 0.05 * f as f32 + surge * (std::f32::consts::FRAC_PI_2 * f as f32).sin();
    let frames = (0..9)
        .map(|f| {
            let z = r(f);
            // Left planted 0..=4 at world z 0.1, right 4..=8 at 0.3 (and,
            // from the previous cycle, at -0.1 on frame 0).
            let left = if f <= 4 {
                [-0.15, 0.0, 0.1 - z]
            } else {
                [-0.15, 0.1, 0.1 + 0.4 * (f - 4) as f32 / 4.0 - z]
            };
            let right = if f == 0 || f >= 4 {
                [0.15, 0.0, if f == 0 { -0.1 } else { 0.3 } - z]
            } else {
                [0.15, 0.1, -0.1 + 0.4 * f as f32 / 4.0 - z]
            };
            vec![at([0.0, 0.0, z]), at(left), at(right)]
        })
        .collect();
    d.skeleton = Some(SkeletonData {
        joint_of_vertex: joint,
        joints: vec![joint_at(-1), joint_at(0), joint_at(0)],
        clips: vec![ClipData {
            name: "stride".into(),
            fps: 8.0,
            looping: true,
            frames,
            ..Default::default()
        }],
    });
    let mut opts = trino_asset_pipeline::model::ModelOptions::default();
    if root_motion {
        opts.root_motion = vec!["stride".into()];
    }
    opts.events.insert(
        "stride".into(),
        vec![
            (0.0, "foot_l".into()),
            (0.5, "sfx:step".into()),
            (1.0, "loop_end".into()),
        ],
    );
    trino_asset_pipeline::model::apply_clip_meta(&mut d, &opts).unwrap();
    d.encode().unwrap()
}

#[test]
fn root_motion_removes_the_foot_slide_of_a_surging_walk() {
    // Without the track, a game moves the walker at the clip's mean speed
    // and the planted foot slides with the surge.
    let rm = anim::clip_metrics(&strider(0.03, true), "stride", Limits::default()).unwrap();
    assert!(rm.root_motion);
    assert!(
        (rm.root_distance - 0.4).abs() < 1e-5,
        "{}",
        rm.root_distance
    );
    assert!(
        rm.foot_slide > 0.03,
        "constant speed slides: {}",
        rm.foot_slide
    );
    // Carried by its own track, the foot stays put.
    let slide = rm.root_foot_slide.unwrap();
    assert!(slide < 1e-5, "root motion slide {slide}");
    assert!(rm.warnings.is_empty(), "{:?}", rm.warnings);

    // The control: a constant-speed walk slides at neither, so the surging
    // one above is what root motion fixes.
    let flat = anim::clip_metrics(&strider(0.0, true), "stride", Limits::default()).unwrap();
    assert!(flat.foot_slide < 1e-5, "{}", flat.foot_slide);
    // And a surging walk baked without root motion keeps its root moving:
    // its foot is planted in model space, there is no track to follow.
    let kept = anim::clip_metrics(&strider(0.03, false), "stride", Limits::default()).unwrap();
    assert!(!kept.root_motion && kept.root_foot_slide.is_none());
}

#[test]
fn the_bake_keeps_the_root_in_place_and_writes_the_track_and_events() {
    let tmdl = strider(0.03, true);
    let mesh = Mesh::from_tmdl(&tmdl).unwrap();
    let id = ClipId::from_name("stride");
    // Every frame poses the root where frame 0 had it.
    let first = anim::posed(&mesh, Some(AnimRef::new(id, 0.0)));
    for f in 1..9 {
        let p = anim::posed(&mesh, Some(AnimRef::new(id, f as f32 / 8.0)));
        assert!((p[0].z - first[0].z).abs() < 1e-6, "frame {f}: {:?}", p[0]);
    }
    let meta = trino_core::anim::ClipMeta::from_tmdl(&tmdl).unwrap();
    let track = meta.roots.root_of(id).unwrap();
    assert_eq!(track.frames(), 9);
    assert!((track.at(0.5).z - 0.2).abs() < 1e-6);
    // Events sorted, the loop's end folded onto its start, `sfx:` carries
    // the sound.
    let ev = meta.events.events_of(id);
    assert_eq!(ev.len(), 3);
    let (t0, a, s0) = ev.get(0);
    let (t2, _, s2) = ev.get(2);
    assert_eq!((t0, t2), (0.0, 0.5));
    assert!(s0.is_none());
    assert_eq!(s2, Some(trino_core::SoundId::from_path("sounds/step")));
    assert!(
        a == trino_core::anim::EventId::from_name("foot_l")
            || a == trino_core::anim::EventId::from_name("loop_end")
    );
    // An event past the end is a bake error naming the clip.
    let mut d = MeshData {
        skeleton: Mesh::from_tmdl(&tmdl).ok().map(|_| SkeletonData {
            joint_of_vertex: vec![],
            joints: vec![JointData {
                parent: -1,
                inverse_bind: ID,
                rest: Trs::IDENTITY,
            }],
            clips: vec![ClipData {
                name: "c".into(),
                fps: 8.0,
                frames: vec![vec![Trs::IDENTITY]; 9],
                ..Default::default()
            }],
        }),
        ..Default::default()
    };
    let mut opts = trino_asset_pipeline::model::ModelOptions::default();
    opts.events.insert("c".into(), vec![(1.5, "late".into())]);
    let err = trino_asset_pipeline::model::apply_clip_meta(&mut d, &opts).unwrap_err();
    assert!(err.contains("past the end of clip `c`"), "{err}");
    opts.events.clear();
    opts.root_motion = vec!["nope".into()];
    let err = trino_asset_pipeline::model::apply_clip_meta(&mut d, &opts).unwrap_err();
    assert!(
        err.contains("clip `nope` is not in this model (clips: c)"),
        "{err}"
    );
}
