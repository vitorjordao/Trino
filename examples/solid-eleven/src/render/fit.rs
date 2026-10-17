//! The body's fit through every animation: each clip sampled densely,
//! mirrored, and crossfaded into every other, on the PC's 15-part body and
//! the consoles' 9-part one, measured against the solids the meshes are
//! built from (`BODY` in `tools/blender/solid_eleven_anim.py`).
//!
//! The rules, each a way a rigid-part body goes wrong:
//! 1. **Attached:** every joint's pivot lies inside its parent's parts and
//!    on or inside its own, so a limb never floats off the body (the old head did, 4 cm
//!    over the collar).
//! 2. **Hidden stays hidden:** a part's surface that sits inside its
//!    parent at rest (a neck in the collar, a shirt tucked into the shorts,
//!    a shin's top in the knee) stays inside in every pose; skin may show
//!    through cloth by 1 cm at most, cloth through cloth by 3 cm (the old
//!    thighs cut through the shorts, the arms through the sleeves). The
//!    band within 1.5 cm of the parent's surface is the seam: a limb comes
//!    out there and may show more as it turns.
//! 3. **No holes:** a face left out of a mesh stays covered by another part.
//! 4. **No interpenetration:** a visible surface never sinks more than
//!    2.5 cm into another part (arms through the chest, a foot through
//!    the other shin), except right at the joint that bends it, or where a
//!    third part covers the point (a shorts corner in a leg, under the
//!    shorts leg).
//! 5. **Turf:** nothing goes under the ground, and in clips with a foot
//!    down the lowest point stays within 10 cm of it.
//! 6. **Hair over the skull:** every outer face of every PC hair style
//!    stays 2 mm or more off the skull, so skin never shows through hair
//!    (the old cap's front and sides crossed the forehead). The PC body
//!    wears every style at once for rules 1–5 (styles never judge each
//!    other, and a style never hides or covers the bald skull).
//!
//! `cargo test -p solid-eleven fit -- --nocapture` prints the closest call
//! of each rule per clip.

use super::anim::{
    AnimState, Part, Pose, Solid, Tint, body_matrix, clip, joint_matrices, lying, mask, plant,
    route, sample_masked, switch, tier,
};
use super::clips_gen::{CLIPS, ClipName, HAIR, JOINTS, PARTS, SKELETON};
use trino_core::{Mat34, Vec3};

/// How far skin may show through cloth (m), and cloth through cloth.
const TOL_SKIN: f32 = 0.010;
const TOL_SAME: f32 = 0.030;
/// The band along a parent's surface where a child comes out (see rule 2).
const SEAM: f32 = 0.015;
/// How deep a visible surface may sink into another part.
const SINK: f32 = 0.025;
/// How deep inside another part a point is out of sight (rule 4).
const BURIED: f32 = 0.005;
/// Around the joint that bends two parts, they may press into each other
/// (wider at the shoulder: a raised arm bunches into it).
const JOINT_ZONE: f32 = 0.09;
const SHOULDER_ZONE: f32 = 0.13;

fn zone(child: usize) -> f32 {
    use super::anim::joint::{ARM_L, ARM_R};
    if child == ARM_L || child == ARM_R {
        SHOULDER_ZONE
    } else {
        JOINT_ZONE
    }
}
/// Samples per clip.
const STEPS: usize = 48;

#[derive(Clone, Copy)]
struct Plane {
    n: Vec3,
    d: f32,
}

/// The six bounding planes of a solid (outward normals: inside when
/// `n · p <= d`).
fn planes(s: &Solid) -> [Plane; 6] {
    let h = s.y1 - s.y0;
    let side = |dw: f32, sign: f32, axis_x: bool, at: f32| {
        // Outward along ±axis, tilted by the taper.
        let (a, b) = (sign * h, -dw * 0.5);
        let l = (a * a + b * b).sqrt();
        let n = if axis_x {
            Vec3::new(a / l, b / l, 0.0)
        } else {
            Vec3::new(0.0, b / l, a / l)
        };
        let p = if axis_x {
            Vec3::new(at, s.y0, 0.0)
        } else {
            Vec3::new(0.0, s.y0, at)
        };
        Plane { n, d: n.dot(p) }
    };
    [
        side(s.w1 - s.w0, -1.0, true, s.cx - s.w0 * 0.5),
        side(s.w1 - s.w0, 1.0, true, s.cx + s.w0 * 0.5),
        Plane {
            n: Vec3::new(0.0, -1.0, 0.0),
            d: -s.y0,
        },
        Plane {
            n: Vec3::new(0.0, 1.0, 0.0),
            d: s.y1,
        },
        side(s.d1 - s.d0, -1.0, false, s.cz - s.d0 * 0.5),
        side(s.d1 - s.d0, 1.0, false, s.cz + s.d0 * 0.5),
    ]
}

/// The corners of face `f` (planes' order), counter-clockwise or not —
/// only used to spread sample points.
fn face(s: &Solid, f: usize) -> [Vec3; 4] {
    let b = |sx: f32, sz: f32| Vec3::new(s.cx + sx * s.w0 * 0.5, s.y0, s.cz + sz * s.d0 * 0.5);
    let t = |sx: f32, sz: f32| Vec3::new(s.cx + sx * s.w1 * 0.5, s.y1, s.cz + sz * s.d1 * 0.5);
    match f {
        0 => [b(-1.0, -1.0), b(-1.0, 1.0), t(-1.0, 1.0), t(-1.0, -1.0)],
        1 => [b(1.0, -1.0), b(1.0, 1.0), t(1.0, 1.0), t(1.0, -1.0)],
        2 => [b(-1.0, -1.0), b(1.0, -1.0), b(1.0, 1.0), b(-1.0, 1.0)],
        3 => [t(-1.0, -1.0), t(1.0, -1.0), t(1.0, 1.0), t(-1.0, 1.0)],
        4 => [b(-1.0, -1.0), b(1.0, -1.0), t(1.0, -1.0), t(-1.0, -1.0)],
        _ => [b(-1.0, 1.0), b(1.0, 1.0), t(1.0, 1.0), t(-1.0, 1.0)],
    }
}

/// Signed depth of `p` (the part's frame) in a solid: > 0 inside.
fn depth_in(pl: &[Plane; 6], p: Vec3) -> f32 {
    let mut m = f32::MAX;
    for q in pl {
        m = m.min(q.d - q.n.dot(p));
    }
    m
}

fn len(v: Vec3) -> f32 {
    v.dot(v).sqrt()
}

/// A world point back into a rigid joint frame.
fn to_local(m: &Mat34, p: Vec3) -> Vec3 {
    let d = Vec3::new(p.x - m.m[0][3], p.y - m.m[1][3], p.z - m.m[2][3]);
    Vec3::new(
        m.m[0][0] * d.x + m.m[1][0] * d.y + m.m[2][0] * d.z,
        m.m[0][1] * d.x + m.m[1][1] * d.y + m.m[2][1] * d.z,
        m.m[0][2] * d.x + m.m[1][2] * d.y + m.m[2][2] * d.z,
    )
}

struct Sample {
    part: usize,
    solid: usize,
    local: Vec3,
    /// On a face the mesh leaves out.
    skipped: bool,
    /// Past the joint, inside the parent (a neck, a tuck, a sleeve's cap).
    plug: bool,
}

struct Body {
    parts: Vec<Part>,
    planes: Vec<Vec<[Plane; 6]>>,
    /// Bounding sphere per part (its frame): center, radius.
    spheres: Vec<(Vec3, f32)>,
    samples: Vec<Sample>,
    mask: u16,
}

impl Body {
    fn new(tier_bit: u8) -> Body {
        let parts: Vec<Part> = PARTS
            .iter()
            .chain(HAIR.iter())
            .copied()
            .filter(|p| p.tiers & tier_bit != 0)
            .collect();
        let mut planes_v = Vec::new();
        let mut spheres = Vec::new();
        let mut samples = Vec::new();
        let mut mask = 0u16;
        for (pi, p) in parts.iter().enumerate() {
            // Which way the part runs from its pivot (up: head, torso).
            let bulk: f32 = p.solids.iter().map(|s| (s.y0 + s.y1) * (s.y1 - s.y0)).sum();
            mask |= 1 << p.joint;
            let mut j = p.joint;
            while SKELETON[j].0 != usize::MAX {
                j = SKELETON[j].0;
                mask |= 1 << j;
            }
            planes_v.push(p.solids.iter().map(planes).collect::<Vec<_>>());
            let mut lo = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
            let mut hi = Vec3::new(f32::MIN, f32::MIN, f32::MIN);
            for (si, s) in p.solids.iter().enumerate() {
                for f in 0..6 {
                    let c = face(s, f);
                    for v in c {
                        lo = Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z));
                        hi = Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z));
                    }
                    let skipped = s.skip & (1 << f) != 0;
                    for u in 0..3 {
                        for v in 0..3 {
                            let (u, v) = (u as f32 * 0.5, v as f32 * 0.5);
                            let a = c[0] + (c[1] - c[0]) * u;
                            let b = c[3] + (c[2] - c[3]) * u;
                            let local = a + (b - a) * v;
                            samples.push(Sample {
                                part: pi,
                                solid: si,
                                local,
                                skipped,
                                // Past the pivot, or anywhere for a part that
                                // grows up out of its parent (a tucked shirt).
                                plug: p.joint != super::anim::joint::PELVIS
                                    && (bulk > 0.0 || local.y * bulk < -0.005 * bulk.abs()),
                            });
                        }
                    }
                }
            }
            let ctr = (lo + hi) * 0.5;
            spheres.push((ctr, len(hi - ctr)));
        }
        assert_eq!(mask & !mask::ALL, 0);
        Body {
            parts,
            planes: planes_v,
            spheres,
            samples,
            mask,
        }
    }

    /// The joints as the game poses them (planted on the turf).
    fn matrices(&self, pose: &Pose, snap: bool) -> [Mat34; JOINTS] {
        let mut m = [Mat34::IDENTITY; JOINTS];
        joint_matrices(
            &body_matrix(Vec3::ZERO, 0.0, 1.0, 1.0),
            pose,
            &mut m,
            self.mask,
        );
        plant(&mut m, self.mask, snap, 0.0);
        m
    }

    /// Depth of world point `w` in part `q` (a cheap miss outside its
    /// sphere, reported as -1 m).
    fn depth(&self, m: &[Mat34; JOINTS], q: usize, w: Vec3) -> f32 {
        let pq = &self.parts[q];
        let l = to_local(&m[pq.joint], w);
        let (c, r) = self.spheres[q];
        if {
            let d = l - c;
            d.dot(d)
        } > (r + 0.05) * (r + 0.05)
        {
            return -1.0;
        }
        self.planes[q]
            .iter()
            .map(|pl| depth_in(pl, l))
            .fold(f32::MIN, f32::max)
    }

    /// Union depth over the parts on joint `j`.
    fn depth_joint(&self, m: &[Mat34; JOINTS], j: usize, w: Vec3) -> f32 {
        (0..self.parts.len())
            .filter(|&q| self.parts[q].joint == j)
            .map(|q| self.depth(m, q, w))
            .fold(-1.0, f32::max)
    }

    /// Whether part `q` counts for a point of part `p`: a hair style
    /// (one of several a player may wear) never covers, hides or judges
    /// another part's point, and styles never judge each other; a style's
    /// own point is judged against the body.
    fn judges(&self, p: usize, q: usize) -> bool {
        self.parts[q].tint != Tint::Hair || self.parts[p].tint == Tint::Hair && p == q
    }

    fn tint_of_joint(&self, j: usize) -> Option<Tint> {
        self.parts.iter().find(|p| p.joint == j).map(|p| p.tint)
    }
}

fn parent(j: usize) -> usize {
    SKELETON[j].0
}

/// A pose and where it came from.
struct Probe {
    label: String,
    clip: Option<ClipName>,
    pose: Pose,
    snap: bool,
}

fn probes(m: u16) -> Vec<Probe> {
    let mut out = Vec::new();
    for name in ClipName::ALL {
        let c = clip(name);
        for mirror in [false, true] {
            for k in 0..=STEPS {
                let t = c.seconds * k as f32 / STEPS as f32;
                out.push(Probe {
                    label: format!(
                        "{}{} t={:.2}",
                        c.name,
                        if mirror { " mirrored" } else { "" },
                        t
                    ),
                    clip: Some(name),
                    pose: sample_masked(name, t, mirror, m),
                    snap: c.grounded,
                });
            }
        }
    }
    // Crossfades the game makes, frame by frame as it plays them (the
    // fade starts from a few points of the old clip, or its held end).
    for a in ClipName::ALL {
        for b in ClipName::ALL {
            for fa in [0.3f32, 0.7, 1.0] {
                if !switches(a, b, fa) {
                    continue;
                }
                let mut st = AnimState::new(a);
                st.time = clip(a).seconds * fa;
                switch(&mut st, b, false, false);
                let mut f = 0;
                while st.fade > 0.0 {
                    out.push(Probe {
                        label: format!(
                            "{} -> {} frame {f} (from t {fa:.1})",
                            clip(a).name,
                            clip(b).name
                        ),
                        clip: None,
                        pose: st.pose_masked(m),
                        snap: st.grounded(),
                    });
                    st.advance(1.0 / 60.0, 1.0);
                    f += 1;
                }
            }
        }
    }
    out
}

/// Whether the game can switch straight from `a` to `b`: players and
/// officials have their own clips, and from the turf only a get-up or
/// another fall follows ([`route`]).
fn switches(a: ClipName, b: ClipName, from: f32) -> bool {
    use ClipName::*;
    let official = |c| matches!(c, Idle | Walk | Jog | RefPoint | RefCard | FlagUp);
    let player = |c| !matches!(c, RefPoint | RefCard | FlagUp);
    if a == b || !(official(a) && official(b) || player(a) && player(b)) {
        return false;
    }
    // A get-up only follows a fall that played out.
    if b == GetUp {
        return lying(a) && from >= 1.0;
    }
    let mut st = AnimState::new(a);
    st.time = clip(a).seconds * from;
    route(&st, b) == b
}

/// Clips with a foot planted (the lowest point stays near the turf).
fn grounded(c: ClipName) -> bool {
    !matches!(
        c,
        ClipName::Celebrate
            | ClipName::Header
            | ClipName::Slide
            | ClipName::Down
            | ClipName::GkDive
            | ClipName::Sprint
            | ClipName::Jog
    )
}

#[derive(Clone, Debug)]
struct Worst {
    amount: f32,
    detail: String,
}

/// The closest call (or worst breach) per rule and clip: `amount` > 0 is
/// a breach in meters.
type Report = std::collections::BTreeMap<(u8, String), Worst>;

fn note(rep: &mut Report, rule: u8, key: &str, amount: f32, detail: impl FnOnce() -> String) {
    let e = rep.entry((rule, key.to_string())).or_insert(Worst {
        amount: f32::MIN,
        detail: String::new(),
    });
    if amount > e.amount {
        e.amount = amount;
        e.detail = detail();
    }
}

fn name_of(p: &Part) -> String {
    let joint = [
        "pelvis",
        "torso",
        "head",
        "arm_l",
        "arm_r",
        "forearm_l",
        "forearm_r",
        "thigh_l",
        "thigh_r",
        "shin_l",
        "shin_r",
    ][p.joint];
    format!("{:?}@{joint}", p.tint)
}

impl Checker {
    /// The body of `tier_bit`, its static rules (1, and overlaps at rest)
    /// noted in `rep`.
    fn new(tier_bit: u8, rep: &mut Report) -> Checker {
        let body = Body::new(tier_bit);

        let rest = body.matrices(&Pose::REST, true);
        let n = body.parts.len();
        let adjacent = |a: usize, b: usize| parent(a) == b || parent(b) == a;
        // The joint two adjacent joints share (the child's pivot).
        let child = |a: usize, b: usize| if parent(a) == b { a } else { b };

        // 1. Attached: each joint's pivot inside its parent's parts and its own.
        for j in 0..JOINTS {
            if body.mask & (1 << j) == 0 || parent(j) == usize::MAX {
                continue;
            }
            if body.tint_of_joint(j).is_none() {
                continue;
            }
            let pivot = rest[j].transform_point(Vec3::ZERO);
            // Inside the parent; on or inside the child (an arm may end at it).
            let up = body.depth_joint(&rest, parent(j), pivot);
            let own = body.depth_joint(&rest, j, pivot);
            note(rep, 1, "rest", (0.005 - up).max(-0.001 - own), || {
                format!("joint {j} pivot depth {up:.3} in its parent, {own:.3} in its own parts")
            });
        }

        // Which samples are meant to be covered at rest: inside their own
        // part's other solids, a part on the same joint, or the next joint's
        // parts close to the bend. Anything else inside another part at rest
        // is an overlap.
        let mut hidden = vec![false; body.samples.len()];
        for (si, s) in body.samples.iter().enumerate() {
            let p = &body.parts[s.part];
            let w = rest[p.joint].transform_point(s.local);
            let own = body.planes[s.part]
                .iter()
                .enumerate()
                .filter(|&(k, _)| k != s.solid)
                .map(|(_, pl)| depth_in(pl, s.local))
                .fold(-1.0, f32::max);
            hidden[si] = own >= 0.003;
            for q in 0..n {
                if q == s.part || !body.judges(s.part, q) {
                    continue;
                }
                let pq = &body.parts[q];
                let d = body.depth(&rest, q, w);
                if d < 0.003 {
                    continue;
                }
                let near_bend = adjacent(p.joint, pq.joint) && {
                    let pivot = rest[child(p.joint, pq.joint)].transform_point(Vec3::ZERO);
                    len(w - pivot) <= zone(child(p.joint, pq.joint))
                };
                if pq.joint == p.joint || near_bend {
                    hidden[si] = true;
                }
            }
        }
        // A plug: past the pivot and inside the parent at rest, deeper than
        // the seam (the band along the parent's surface where a limb comes out
        // and may show more as it turns: a side-raised arm's armpit).
        let plug: Vec<bool> = body
            .samples
            .iter()
            .map(|s| {
                let p = &body.parts[s.part];
                s.plug && {
                    let w = rest[p.joint].transform_point(s.local);
                    body.depth_joint(&rest, parent(p.joint), w) >= SEAM
                }
            })
            .collect();
        for (h, &p) in hidden.iter_mut().zip(&plug) {
            *h |= p;
        }
        for (si, s) in body.samples.iter().enumerate() {
            if hidden[si] || s.skipped {
                continue;
            }
            let p = &body.parts[s.part];
            let w = rest[p.joint].transform_point(s.local);
            for q in 0..n {
                let pq = &body.parts[q];
                if !body.judges(s.part, q) {
                    continue;
                }
                let d = body.depth(&rest, q, w);
                if q != s.part && d > 0.005 && !(adjacent(p.joint, pq.joint) && pq.tint == p.tint) {
                    note(rep, 4, "rest", d - 0.005, || {
                        format!(
                            "{} overlaps {} by {:.3} m at rest",
                            name_of(p),
                            name_of(pq),
                            d
                        )
                    });
                }
            }
        }

        Checker { body, hidden, plug }
    }

    /// Rules 2 to 5 on one pose, noted under `key`.
    fn probe(&self, rep: &mut Report, key: &str, probe: &Probe) {
        let body = &self.body;
        let (hidden, plug) = (&self.hidden, &self.plug);
        let n = body.parts.len();
        let adjacent = |a: usize, b: usize| parent(a) == b || parent(b) == a;
        let child = |a: usize, b: usize| if parent(a) == b { a } else { b };
        let m = body.matrices(&probe.pose, probe.snap);
        let mut low = f32::MAX;
        for (si, s) in body.samples.iter().enumerate() {
            let p = &body.parts[s.part];
            let w = m[p.joint].transform_point(s.local);
            low = low.min(w.y);
            // 2. A plug stays inside its parent.
            if plug[si] {
                let up = parent(p.joint);
                let d = body.depth_joint(&m, up, w);
                let same = body.parts.iter().any(|q| q.joint == up && q.tint == p.tint);
                let tol = if same { TOL_SAME } else { TOL_SKIN };
                note(rep, 2, key, -d - tol, || {
                    format!(
                        "{}: {} pokes {:.3} m out of its parent",
                        probe.label,
                        name_of(p),
                        -d
                    )
                });
                continue;
            }
            // 3. Skipped faces stay covered.
            if s.skipped {
                let own = body.planes[s.part]
                    .iter()
                    .enumerate()
                    .filter(|&(k, _)| k != s.solid)
                    .map(|(_, pl)| depth_in(pl, s.local))
                    .fold(-1.0, f32::max);
                let cover = (0..n)
                    .filter(|&q| q != s.part && body.judges(s.part, q))
                    .map(|q| body.depth(&m, q, w))
                    .fold(own, f32::max);
                note(rep, 3, key, -cover - 0.005, || {
                    format!(
                        "{}: open face of {} shows {:.3} m (solid {} at {:.3},{:.3},{:.3})",
                        probe.label,
                        name_of(p),
                        -cover,
                        s.solid,
                        s.local.x,
                        s.local.y,
                        s.local.z
                    )
                });
                continue;
            }
            if hidden[si] {
                continue;
            }
            // 4. A visible surface never sinks into another part.
            for q in 0..n {
                let pq = &body.parts[q];
                if pq.joint == p.joint || !body.judges(s.part, q) {
                    continue;
                }
                if adjacent(p.joint, pq.joint) {
                    // Cloth on the same cloth folds at a joint (a shorts
                    // leg into the shorts).
                    if pq.tint == p.tint {
                        continue;
                    }
                    let pivot = m[child(p.joint, pq.joint)].transform_point(Vec3::ZERO);
                    if len(w - pivot) <= zone(child(p.joint, pq.joint)) {
                        continue;
                    }
                }
                let d = body.depth(&m, q, w);
                // Inside a third part, the point isn't visible surface: a
                // shorts corner in a leg that its shorts leg covers.
                if d > SINK
                    && (0..n)
                        .filter(|&r| r != s.part && r != q && body.judges(s.part, r))
                        .any(|r| body.depth(&m, r, w) > BURIED)
                {
                    continue;
                }
                note(rep, 4, key, d - SINK, || {
                    format!(
                        "{}: {} {:.3} m into {}",
                        probe.label,
                        name_of(p),
                        d,
                        name_of(pq)
                    )
                });
            }
        }
        // 5. Turf.
        note(rep, 5, key, -0.02 - low, || {
            format!("{}: lowest point {:.3} m", probe.label, low)
        });
        if let Some(c) = probe.clip
            && grounded(c)
        {
            note(rep, 5, &format!("{key} (foot down)"), low - 0.10, || {
                format!("{}: lowest point {:.3} m", probe.label, low)
            });
        }
    }
}

/// A body ready to judge poses: its parts, and which samples are covered
/// at rest (hidden) or reach into the parent (plugs).
struct Checker {
    body: Body,
    hidden: Vec<bool>,
    plug: Vec<bool>,
}

fn check(tier_bit: u8) -> Report {
    let mut rep = Report::new();
    let checker = Checker::new(tier_bit, &mut rep);
    for probe in probes(checker.body.mask) {
        let key = match probe.clip {
            Some(c) => clip(c).name.to_string(),
            None => "crossfades".to_string(),
        };
        checker.probe(&mut rep, &key, &probe);
    }
    rep
}

/// The rules a pose a real match shows breaks (empty: it fits): for the
/// match scenarios, which check the frames they play.
pub(crate) fn pose_breaches(full: bool, pose: &Pose, snap: bool, label: &str) -> Vec<String> {
    static CHECKERS: std::sync::OnceLock<[Checker; 2]> = std::sync::OnceLock::new();
    let checkers = CHECKERS.get_or_init(|| {
        let mut sink = Report::new();
        [
            Checker::new(tier::FULL, &mut sink),
            Checker::new(tier::NEAR, &mut sink),
        ]
    });
    let checker = &checkers[if full { 0 } else { 1 }];
    let mut rep = Report::new();
    checker.probe(
        &mut rep,
        "frame",
        &Probe {
            label: label.to_string(),
            clip: None,
            pose: *pose,
            snap,
        },
    );
    rep.into_values()
        .filter(|w| w.amount > 0.0)
        .map(|w| w.detail)
        .collect()
}

fn print_and_assert(tier_name: &str, rep: &Report) {
    let mut bad = Vec::new();
    println!("body fit, {tier_name}: rule, clip, margin (m, < 0 = within the rule), closest call");
    for ((rule, key), w) in rep {
        println!("  {rule} {key:<22} {:>+7.3}  {}", w.amount, w.detail);
        if w.amount > 0.0 {
            bad.push(format!("rule {rule} {key}: {}", w.detail));
        }
    }
    assert!(
        bad.is_empty(),
        "{tier_name}: {} breaches:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
fn the_pc_body_fits_through_every_clip() {
    print_and_assert("pc", &check(tier::FULL));
}

#[test]
fn the_console_body_fits_through_every_clip() {
    print_and_assert("consoles (near)", &check(tier::NEAR));
}

/// Far players: the near body without arms, kit legs and all; this tier's
/// own table (no shorts legs) is what's drawn when a kit has no baked leg.
#[test]
fn the_far_console_body_fits_through_every_clip() {
    print_and_assert("consoles (far)", &check(tier::FAR));
}

/// Rule 6: the closest any outer face (all but the underside, which meets
/// the skull at the hairline) of `hair` comes to the skull `skull`, in m
/// (> 0: clear of it), and where.
fn hair_clearance(hair: &[Solid], skull: &[Solid]) -> (f32, Vec3) {
    let skull: Vec<[Plane; 6]> = skull.iter().map(planes).collect();
    let mut worst = (f32::MAX, Vec3::ZERO);
    for s in hair {
        for f in [0usize, 1, 3, 4, 5] {
            if s.skip & (1 << f) != 0 {
                continue;
            }
            let c = face(s, f);
            for u in 0..=8 {
                for v in 0..=8 {
                    let (u, v) = (u as f32 / 8.0, v as f32 / 8.0);
                    let a = c[0] + (c[1] - c[0]) * u;
                    let b = c[3] + (c[2] - c[3]) * u;
                    let p = a + (b - a) * v;
                    let inside = skull
                        .iter()
                        .map(|pl| depth_in(pl, p))
                        .fold(f32::MIN, f32::max);
                    if -inside < worst.0 {
                        worst = (-inside, p);
                    }
                }
            }
        }
    }
    worst
}

#[test]
fn every_hair_style_stays_clear_of_the_skull() {
    let skull = PARTS
        .iter()
        .find(|p| {
            p.tint == Tint::Skin && p.joint == super::anim::joint::HEAD && p.tiers == tier::FULL
        })
        .unwrap()
        .solids;
    for h in HAIR.iter() {
        let (gap, at) = hair_clearance(h.solids, skull);
        println!(
            "hair {:?}: {gap:.4} m off the skull (closest at {at:?})",
            h.model
        );
        assert!(
            gap >= 0.002,
            "a hair style's outer face comes within {gap:.4} m of the skull at {at:?}"
        );
    }
    // The mutation the rule exists for: a cap 1 cm smaller than the shell
    // crosses the forehead.
    let mut cap: Vec<Solid> = HAIR[0].solids.to_vec();
    for s in cap.iter_mut() {
        s.w0 -= 0.04;
        s.d0 -= 0.04;
        s.w1 -= 0.04;
        s.d1 -= 0.04;
    }
    assert!(hair_clearance(&cap, skull).0 < 0.0);
}

#[test]
fn every_clip_and_the_body_are_covered() {
    assert_eq!(CLIPS.len(), ClipName::ALL.len());
    // Both sides of every limb, on every tier that draws it.
    for p in PARTS.iter() {
        assert!(p.tiers != 0 && !p.solids.is_empty());
        assert!(p.joint < JOINTS);
    }
}

#[test]
fn the_plane_test_matches_the_frustum() {
    let s = Solid {
        y0: 0.0,
        y1: 1.0,
        w0: 2.0,
        d0: 2.0,
        w1: 1.0,
        d1: 1.0,
        cx: 0.0,
        cz: 0.0,
        skip: 0,
    };
    let pl = planes(&s);
    assert!(depth_in(&pl, Vec3::new(0.0, 0.5, 0.0)) > 0.4);
    // At y = 0.5 the half-width is 0.75.
    assert!(depth_in(&pl, Vec3::new(0.7, 0.5, 0.0)) > 0.0);
    assert!(depth_in(&pl, Vec3::new(0.8, 0.5, 0.0)) < 0.0);
    assert!(depth_in(&pl, Vec3::new(0.0, 0.5, -0.8)) < 0.0);
    assert!(depth_in(&pl, Vec3::new(0.0, 1.01, 0.0)) < 0.0);
    // A mutation the rules must catch: the old head's neck started 4 cm
    // over the collar.
    let mut floating = check_floating_head();
    assert!(
        floating.any(|d| d < 0.0),
        "a floating head must fail rule 1"
    );
}

/// Rule 1 against a head moved 6 cm up (the pre-fix model's gap).
fn check_floating_head() -> impl Iterator<Item = f32> {
    let body = Body::new(tier::FULL);
    let mut pose = Pose::REST;
    pose.root = Vec3::ZERO;
    let m = body.matrices(&pose, true);
    let pivot = m[super::anim::joint::HEAD].transform_point(Vec3::new(0.0, 0.06, 0.0));
    let torso = body.depth_joint(&m, super::anim::joint::TORSO, pivot);
    core::iter::once(torso)
}
