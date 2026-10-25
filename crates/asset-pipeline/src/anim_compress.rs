//! Keyframe compression for clips (issue #65, TMDL `ANMC`): per joint and
//! channel, a constant or the keys an error-bounded reduction keeps,
//! quantized (rotations 4 × i16, translations and scales u16 over the
//! track's range). Every frame of the result is decoded with the runtime's
//! own code (`trino_core::anim::anmc`) and held to the bounds: a key is
//! added where a frame misses until every frame passes, so the bound is a
//! promise, not a hope.
//!
//! The manifest opts a model in (`anim_compression = true`, or `{ rotation
//! = 0.5, translation = 0.002, scale = 0.001 }`); `cargo xtask model
//! inspect --anim --compressed` reports bytes and errors per clip for any
//! model, opted in or not.

use trino_core::anim::anmc::{self, Channel};
use trino_core::{ClipId, Quat, Vec3};

use crate::model::{ClipData, SkeletonData, Trs};

/// How far a decoded frame may stray from the source.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Bounds {
    /// Degrees between the decoded and the source rotation of a joint.
    pub rotation: f32,
    /// Model units (meters) of a joint's translation.
    pub translation: f32,
    /// Absolute error of a scale component.
    pub scale: f32,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds {
            rotation: 0.5,
            translation: 0.002,
            scale: 0.001,
        }
    }
}

/// The manifest's `anim_compression`: `true` (the default bounds) or a table.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize)]
#[serde(untagged)]
pub enum CompressionDecl {
    On(bool),
    Bounds(Bounds),
}

impl CompressionDecl {
    pub fn bounds(&self) -> Option<Bounds> {
        match self {
            CompressionDecl::On(true) => Some(Bounds::default()),
            CompressionDecl::On(false) => None,
            CompressionDecl::Bounds(b) => Some(*b),
        }
    }
}

/// What compressing one clip did.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipReport {
    pub name: String,
    pub frames: usize,
    /// `ANIM` bytes (header + 40 per joint per frame).
    pub raw_bytes: usize,
    /// `ANMC` bytes (header + tracks).
    pub bytes: usize,
    /// Keys kept, over every keyed track; tracks that are constants.
    pub keys: usize,
    pub constants: usize,
    /// Worst decoded error over every frame and joint.
    pub max_rotation_deg: f32,
    pub max_translation: f32,
    pub max_scale: f32,
}

fn v3(a: [f32; 3]) -> Vec3 {
    Vec3::new(a[0], a[1], a[2])
}

fn q4(a: [f32; 4]) -> Quat {
    Quat::new(a[0], a[1], a[2], a[3])
}

/// Degrees between two unit quaternions.
pub fn angle_deg(a: Quat, b: Quat) -> f32 {
    let d = a.dot(b).abs().min(1.0);
    2.0 * d.acos().to_degrees()
}

/// One channel of one joint over every frame.
enum Values {
    Vec(Vec<[f32; 3]>),
    Rot(Vec<[f32; 4]>),
}

impl Values {
    fn len(&self) -> usize {
        match self {
            Values::Vec(v) => v.len(),
            Values::Rot(v) => v.len(),
        }
    }
}

/// Encode a track keeping `keys` (frame indices, sorted, first and last
/// included): its bytes.
fn encode_track(vals: &Values, keys: &[usize], constant: bool) -> Vec<u8> {
    let mut b = Vec::new();
    match vals {
        Values::Vec(v) if constant => {
            b.push(anmc::CONSTANT);
            for x in v[0] {
                b.extend_from_slice(&x.to_le_bytes());
            }
        }
        Values::Rot(v) if constant => {
            b.push(anmc::CONSTANT);
            for x in v[0] {
                b.extend_from_slice(&quant_rot(x).to_le_bytes());
            }
        }
        Values::Vec(v) => {
            b.push(anmc::KEYED);
            b.extend_from_slice(&(keys.len() as u16).to_le_bytes());
            for &k in keys {
                b.extend_from_slice(&(k as u16).to_le_bytes());
            }
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            for &k in keys {
                for c in 0..3 {
                    lo[c] = lo[c].min(v[k][c]);
                    hi[c] = hi[c].max(v[k][c]);
                }
            }
            let step: [f32; 3] = std::array::from_fn(|c| (hi[c] - lo[c]) / 65535.0);
            for x in lo.iter().chain(step.iter()) {
                b.extend_from_slice(&x.to_le_bytes());
            }
            for &k in keys {
                for c in 0..3 {
                    let q = if step[c] > 0.0 {
                        ((v[k][c] - lo[c]) / step[c]).round().clamp(0.0, 65535.0) as u16
                    } else {
                        0
                    };
                    b.extend_from_slice(&q.to_le_bytes());
                }
            }
        }
        Values::Rot(v) => {
            b.push(anmc::KEYED);
            b.extend_from_slice(&(keys.len() as u16).to_le_bytes());
            for &k in keys {
                b.extend_from_slice(&(k as u16).to_le_bytes());
            }
            for &k in keys {
                for x in v[k] {
                    b.extend_from_slice(&quant_rot(x).to_le_bytes());
                }
            }
        }
    }
    b
}

fn quant_rot(x: f32) -> i16 {
    (x * anmc::ROT_SCALE).round().clamp(-32767.0, 32767.0) as i16
}

/// The worst frame of a decoded track: (frame, error in the channel's unit).
fn worst(vals: &Values, track: &[u8], ch: Channel) -> (usize, f32) {
    let mut out = (0, 0.0f32);
    for f in 0..vals.len() {
        let e = match (vals, ch) {
            (Values::Rot(v), _) => {
                angle_deg(anmc::sample_quat(track, 0, f as f32), q4(v[f]).normalized())
            }
            (Values::Vec(v), Channel::Scale) => {
                let d = anmc::sample_vec(track, 0, f as f32) - v3(v[f]);
                d.x.abs().max(d.y.abs()).max(d.z.abs())
            }
            (Values::Vec(v), _) => {
                let d = anmc::sample_vec(track, 0, f as f32) - v3(v[f]);
                d.dot(d).sqrt()
            }
        };
        if e > out.1 {
            out = (f, e);
        }
    }
    out
}

/// The error of interpolating frame `f` between keys `a` and `b` (the
/// reduction's own estimate, before quantization).
fn lerp_error(vals: &Values, a: usize, b: usize, f: usize, ch: Channel) -> f32 {
    let k = (f - a) as f32 / (b - a) as f32;
    match (vals, ch) {
        (Values::Rot(v), _) => angle_deg(q4(v[a]).nlerp(q4(v[b]), k), q4(v[f]).normalized()),
        (Values::Vec(v), Channel::Scale) => {
            let p = v3(v[a]) + (v3(v[b]) - v3(v[a])) * k - v3(v[f]);
            p.x.abs().max(p.y.abs()).max(p.z.abs())
        }
        (Values::Vec(v), _) => {
            let p = v3(v[a]) + (v3(v[b]) - v3(v[a])) * k - v3(v[f]);
            p.dot(p).sqrt()
        }
    }
}

/// Douglas-Peucker over frames: the keys whose linear interpolation keeps
/// every frame within `bound`.
fn reduce(vals: &Values, ch: Channel, bound: f32) -> Vec<usize> {
    let n = vals.len();
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (mut wf, mut we) = (0, 0.0f32);
        for f in a + 1..b {
            let e = lerp_error(vals, a, b, f, ch);
            if e > we {
                (wf, we) = (f, e);
            }
        }
        if we > bound {
            keep[wf] = true;
            stack.push((a, wf));
            stack.push((wf, b));
        }
    }
    (0..n).filter(|&f| keep[f]).collect()
}

/// One track within `bound`: reduced, quantized, verified (keys added at
/// the worst frame until every frame passes). Bytes, keys (0 = constant),
/// worst error.
fn track(vals: &Values, ch: Channel, bound: f32) -> (Vec<u8>, usize, f32) {
    let n = vals.len();
    // A constant when every frame is within the bound of the first.
    let constant = encode_track(vals, &[0], true);
    let (_, e) = worst(vals, &constant, ch);
    if e <= bound || n == 1 {
        return (constant, 0, e);
    }
    let mut keys = reduce(vals, ch, bound * 0.75);
    loop {
        let bytes = encode_track(vals, &keys, false);
        let (f, e) = worst(vals, &bytes, ch);
        if e <= bound || keys.len() == n {
            return (bytes, keys.len(), e);
        }
        if !keys.contains(&f) {
            keys.push(f);
            keys.sort_unstable();
        } else {
            // Quantization alone misses (a huge range): keep every frame.
            keys = (0..n).collect();
        }
    }
}

/// Compress one clip of `joints` joints: its `ANMC` clip record (header
/// and body) and what it cost.
pub fn encode_clip(
    clip: &ClipData,
    joints: usize,
    b: &Bounds,
) -> Result<(Vec<u8>, ClipReport), String> {
    let frames = clip.frames.len();
    if frames == 0 || frames > u16::MAX as usize {
        return Err(format!(
            "clip `{}`: {frames} frames (1..=65535 compress)",
            clip.name
        ));
    }
    let mut body = Vec::new();
    let mut report = ClipReport {
        name: clip.name.clone(),
        frames,
        raw_bytes: 16 + frames * joints * 40,
        bytes: 0,
        keys: 0,
        constants: 0,
        max_rotation_deg: 0.0,
        max_translation: 0.0,
        max_scale: 0.0,
    };
    for j in 0..joints {
        let col = |f: &dyn Fn(&Trs) -> [f32; 3]| {
            Values::Vec(clip.frames.iter().map(|fr| f(&fr[j])).collect())
        };
        // Rotations sign-continuous (q and -q are one rotation).
        let mut rot: Vec<[f32; 4]> = Vec::with_capacity(frames);
        for fr in &clip.frames {
            let mut q = fr[j].r;
            if let Some(p) = rot.last()
                && p[0] * q[0] + p[1] * q[1] + p[2] * q[2] + p[3] * q[3] < 0.0
            {
                q = q.map(|x| -x);
            }
            rot.push(q);
        }
        for (vals, ch, bound) in [
            (col(&|t: &Trs| t.t), Channel::Translation, b.translation),
            (Values::Rot(rot.clone()), Channel::Rotation, b.rotation),
            (col(&|t: &Trs| t.s), Channel::Scale, b.scale),
        ] {
            let (bytes, keys, e) = track(&vals, ch, bound);
            if keys == 0 {
                report.constants += 1;
            }
            report.keys += keys;
            match ch {
                Channel::Translation => report.max_translation = report.max_translation.max(e),
                Channel::Rotation => report.max_rotation_deg = report.max_rotation_deg.max(e),
                Channel::Scale => report.max_scale = report.max_scale.max(e),
            }
            body.extend_from_slice(&bytes);
        }
    }
    if body.len() > u16::MAX as usize {
        return Err(format!(
            "clip `{}`: {} compressed bytes (at most 65535)",
            clip.name,
            body.len()
        ));
    }
    if !anmc::check_body(&body, joints, frames) {
        return Err(format!(
            "clip `{}`: the compressed tracks do not validate",
            clip.name
        ));
    }
    let mut out = Vec::with_capacity(anmc::CLIP_HEADER + body.len());
    out.extend_from_slice(&ClipId::from_name(&clip.name).0.to_le_bytes());
    out.extend_from_slice(&clip.fps.to_le_bytes());
    out.extend_from_slice(&(frames as u32).to_le_bytes());
    out.extend_from_slice(&(clip.looping as u32).to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    report.bytes = out.len();
    Ok((out, report))
}

/// Every clip of a skeleton: the `ANMC` section body and the reports.
pub fn encode(skel: &SkeletonData, b: &Bounds) -> Result<(Vec<u8>, Vec<ClipReport>), String> {
    let joints = skel.joints.len();
    let mut body = (skel.clips.len() as u32).to_le_bytes().to_vec();
    let mut reports = Vec::new();
    for c in &skel.clips {
        let (bytes, r) = encode_clip(c, joints, b)?;
        body.extend_from_slice(&bytes);
        reports.push(r);
    }
    Ok((body, reports))
}
