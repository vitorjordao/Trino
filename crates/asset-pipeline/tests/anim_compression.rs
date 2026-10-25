//! Keyframe compression (issue #65, TMDL `ANMC`) on the repo's own clips
//! and on clips built here: every decoded frame within its bounds, the
//! posed vertices next to the raw clip's, less RAM, old `ANIM` models
//! still loading, and a corrupt `ANMC` refused rather than played.

use std::path::{Path, PathBuf};

use trino_asset_pipeline::anim_compress::{self, Bounds};
use trino_asset_pipeline::model::{ClipData, ModelOptions, Trs};
use trino_asset_pipeline::{Manifest, Platform, bake_model, compression_report, resolve_source};
use trino_core::{AnimRef, ClipId, Mesh};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A manifest model's master and options.
fn model(name: &str) -> (PathBuf, ModelOptions) {
    let root = root();
    let m = Manifest::load(&root.join("assets/manifest.toml")).unwrap();
    let d = &m.models[name];
    (
        resolve_source(&root.join("assets"), Platform::Pc, &d.file).unwrap(),
        d.options(),
    )
}

/// Worst distance between the raw and the compressed bake's posed vertices
/// over every clip and frame (and half frames).
fn worst_vertex(raw: &[u8], packed: &[u8]) -> f32 {
    let (a, b) = (
        Mesh::from_tmdl(raw).unwrap(),
        Mesh::from_tmdl(packed).unwrap(),
    );
    assert!(!a.compressed_clips() && b.compressed_clips());
    assert_eq!(a.clip_count(), b.clip_count());
    let mut worst = 0.0f32;
    for n in 0..a.clip_count() {
        let c = a.clip(n).unwrap();
        assert_eq!(b.clip(n), Some(c), "same clip table");
        for f in 0..c.frame_count * 2 {
            let t = f as f32 * 0.5 / c.fps;
            let pa = trino_asset_pipeline::anim::posed(&a, Some(AnimRef::new(c.id, t)));
            let pb = trino_asset_pipeline::anim::posed(&b, Some(AnimRef::new(c.id, t)));
            for (x, y) in pa.iter().zip(&pb) {
                let d = *x - *y;
                worst = worst.max(d.dot(d).sqrt());
            }
        }
    }
    worst
}

#[test]
fn the_strider_compresses_within_its_bounds_and_saves_ram() {
    let (path, mut opts) = model("strider");
    // The manifest compresses the strider: bake it raw too, to compare.
    opts.anim_compression = None;
    let raw = bake_model(&path, &opts).unwrap();
    let bounds = Bounds::default();
    opts.anim_compression = Some(bounds);
    let packed = bake_model(&path, &opts).unwrap();
    assert!(
        packed.len() < raw.len(),
        "{} vs {}",
        packed.len(),
        raw.len()
    );
    let report = compression_report(&path, &opts, &bounds).unwrap();
    assert_eq!(report.len(), 4);
    for r in &report {
        assert!(r.max_rotation_deg <= bounds.rotation, "{r:?}");
        assert!(r.max_translation <= bounds.translation, "{r:?}");
        assert!(r.max_scale <= bounds.scale, "{r:?}");
        assert!(r.bytes < r.raw_bytes, "{r:?}");
    }
    // A joint's error compounds down the hierarchy: the posed vertices
    // stay within a centimetre of the raw clip's at every frame and between.
    let w = worst_vertex(&raw, &packed);
    assert!(w < 0.01, "worst vertex {w} m");
    // Root motion and events ride beside the clips unchanged.
    let meta = |b: &[u8]| {
        let m = trino_core::anim::ClipMeta::from_tmdl(b).unwrap();
        (m.roots.len(), m.events.len())
    };
    assert_eq!(meta(&raw), meta(&packed));
    let total: (usize, usize) = report
        .iter()
        .fold((0, 0), |s, r| (s.0 + r.raw_bytes, s.1 + r.bytes));
    eprintln!(
        "strider clips: {} -> {} bytes ({:.0}%); TMDL {} -> {}",
        total.0,
        total.1,
        100.0 * total.1 as f32 / total.0 as f32,
        raw.len(),
        packed.len()
    );
}

#[test]
fn every_animated_model_in_the_repo_compresses_within_bounds() {
    let root = root();
    let m = Manifest::load(&root.join("assets/manifest.toml")).unwrap();
    let mut rows = Vec::new();
    for (name, d) in &m.models {
        let Ok(path) = resolve_source(&root.join("assets"), Platform::Pc, &d.file) else {
            continue;
        };
        let opts = d.options();
        let Ok(report) = compression_report(&path, &opts, &Bounds::default()) else {
            continue;
        };
        if report.is_empty() {
            continue;
        }
        for r in &report {
            assert!(
                r.max_rotation_deg <= 0.5 && r.max_translation <= 0.002,
                "{name}: {r:?}"
            );
        }
        let raw: usize = report.iter().map(|r| r.raw_bytes).sum();
        let bytes: usize = report.iter().map(|r| r.bytes).sum();
        rows.push((name.clone(), raw, bytes));
    }
    assert!(rows.iter().any(|r| r.0 == "robot"), "{rows:?}");
    for (n, raw, b) in &rows {
        eprintln!(
            "{n}: clips {raw} -> {b} bytes ({:.0}%)",
            100.0 * *b as f32 / *raw as f32
        );
    }
}

fn trs(t: [f32; 3], deg: f32) -> Trs {
    let h = deg.to_radians() * 0.5;
    Trs {
        t,
        r: [h.sin(), 0.0, 0.0, h.cos()],
        s: [1.0; 3],
    }
}

#[test]
fn tracks_keep_only_the_keys_their_bound_needs() {
    // Joint 0 still, joint 1 a smooth swing, joint 2 a step.
    let frames: Vec<Vec<Trs>> = (0..61)
        .map(|f| {
            let u = f as f32 / 60.0;
            vec![
                trs([0.0, 1.0, 0.0], 0.0),
                trs([0.0, 0.0, 0.0], 40.0 * (u * std::f32::consts::TAU).sin()),
                trs([if f < 30 { 0.0 } else { 0.5 }, 0.0, 0.0], 0.0),
            ]
        })
        .collect();
    let clip = ClipData {
        name: "swing".into(),
        fps: 60.0,
        looping: true,
        frames,
        ..Default::default()
    };
    let enc = |b: Bounds| anim_compress::encode_clip(&clip, 3, &b).unwrap().1;
    let loose = enc(Bounds::default());
    // The still joint and every scale: constants (3 + 3 tracks + the
    // swinging joint's translation).
    assert_eq!(loose.constants, 3 + 1 + 1 + 1 + 1, "{loose:?}");
    let tight = enc(Bounds {
        rotation: 0.05,
        ..Bounds::default()
    });
    assert!(tight.keys > loose.keys, "{} vs {}", tight.keys, loose.keys);
    assert!(tight.max_rotation_deg <= 0.05);
    assert!(loose.max_rotation_deg <= 0.5 && loose.max_translation <= 0.002);
    // The step needs its two sides: keys at 29 and 30 at least.
    assert!(loose.keys >= 4);
}

#[test]
fn a_corrupt_anmc_is_refused_and_old_anim_models_still_load() {
    let (path, mut opts) = model("strider");
    opts.anim_compression = None;
    let raw = bake_model(&path, &opts).unwrap();
    assert!(Mesh::from_tmdl(&raw).is_ok(), "ANIM loads");
    assert!(!Mesh::from_tmdl(&raw).unwrap().compressed_clips());
    opts.anim_compression = Some(Bounds::default());
    let packed = bake_model(&path, &opts).unwrap();
    let body = trino_core::render3d::tmdl_section(&packed, b"ANMC").unwrap();
    let at = body.as_ptr() as usize - packed.as_ptr() as usize;
    let id = ClipId::from_name("walk");
    for k in (0..body.len()).step_by(7) {
        for x in [0x01u8, 0x80, 0xFF] {
            let mut b = packed.clone();
            b[at + k] ^= x;
            if let Ok(m) = Mesh::from_tmdl(&b) {
                // Parsed: it must still pose without a panic.
                let mut j = [trino_core::Mat34::IDENTITY; trino_core::MAX_JOINTS];
                for t in [0.0, 0.3, 2.0] {
                    m.pose(Some(AnimRef::new(id, t)), &mut j);
                }
            }
        }
    }
    // A truncated section is refused.
    let mut cut = packed.clone();
    let len_at = at - 4;
    let short = (body.len() - 3) as u32;
    cut[len_at..len_at + 4].copy_from_slice(&short.to_le_bytes());
    assert!(Mesh::from_tmdl(&cut[..at + body.len() - 3]).is_err());
    let _ = Path::new("");
}
