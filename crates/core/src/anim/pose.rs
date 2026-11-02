//! Layered poses (issue #112, ADR 0039): more clips than an
//! [`AnimRef`] carries, each in a **layer** with a per-joint mask and a
//! blend mode, an **inertialized** transition's offsets, and the feet's
//! **slope**.
//!
//! An [`AnimPose`] travels to the backends through the renderer's per-frame
//! pose table ([`Renderer::push_pose`](crate::Renderer::push_pose)): the
//! renderer folds it to its [`Caps::anim_layers`](crate::Caps::anim_layers)
//! and hands back an [`AnimRef`] whose `pose` slot points at the table entry,
//! and whose own three clips are the pose folded to the N64's tier (1 layer,
//! 3 clips: what #65 drew). A backend without a table, or a pose that folds
//! into an `AnimRef`, draws those three clips on the old path, so a #65 pose
//! stays bit for bit what it was.
//!
//! Sampling and blending happen in [`Mesh::pose_world`](crate::Mesh::pose_world),
//! out of line, so the CPU T&L, the N64's RSP skeleton path, the 3DS and the
//! PC's GPU T&L all read the same joints.

use crate::anim::IkPose;
use crate::math::Vec3;
use crate::math3d::sqrt;
use crate::render::{AnimBlend, AnimRef, ClipId};

/// Weighted clip samples one pose holds.
pub const MAX_POSE_SAMPLES: usize = 12;
/// Layers one pose holds (the base layer included).
pub const MAX_POSE_LAYERS: usize = 4;

/// One bit per joint (`MAX_JOINTS` is 32): the joints a layer moves.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct JointMask(pub u32);

impl JointMask {
    pub const ALL: JointMask = JointMask(u32::MAX);
    pub const NONE: JointMask = JointMask(0);

    /// Whether joint `j` is in the mask.
    #[inline]
    pub fn has(self, j: usize) -> bool {
        j < 32 && self.0 & (1 << j) != 0
    }

    /// The mask with joint `j` added.
    pub const fn with(self, j: usize) -> JointMask {
        if j < 32 {
            JointMask(self.0 | (1 << j))
        } else {
            self
        }
    }

    pub fn count(self) -> u32 {
        self.0.count_ones()
    }
}

/// How a layer combines with the layers under it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum LayerMode {
    /// Replace the masked joints by the layer's pose, by its weight.
    #[default]
    Override,
    /// Add the layer's motion relative to each clip's first frame (a
    /// breath, a recoil, a head turn on top of whatever plays).
    Additive,
}

/// What a sample is for in its layer.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum SampleRole {
    /// Part of the layer's pose (the weights of a layer's `Pose` samples
    /// sum to 1).
    #[default]
    Pose,
    /// An inertialized transition's source, frozen where it was when the
    /// transition fired.
    From,
    /// The transition's destination at that same moment: the layer adds
    /// `(From − To) × decay` to its pose, joint by joint.
    To,
}

/// One clip's share of a layer.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PoseSample {
    pub clip: ClipId,
    /// Seconds into the clip (looping clips wrap, one-shots clamp, like
    /// [`AnimRef::time`]).
    pub time: f32,
    pub weight: f32,
    /// The layer (index into [`AnimPose::layers`]).
    pub layer: u8,
    pub role: SampleRole,
}

impl PoseSample {
    pub const NONE: PoseSample = PoseSample {
        clip: ClipId(0),
        time: 0.0,
        weight: 0.0,
        layer: 0,
        role: SampleRole::Pose,
    };
}

/// One layer of a pose: which joints it moves, how, how much.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PoseLayer {
    /// Ignored for the base layer (index 0), which moves every joint.
    pub mask: JointMask,
    pub mode: LayerMode,
    /// 0..1.
    pub weight: f32,
    /// An inertialized transition in progress: how much of its offset is
    /// left (1 when it fired, 0 when it is over).
    pub decay: f32,
}

impl PoseLayer {
    pub const BASE: PoseLayer = PoseLayer {
        mask: JointMask::ALL,
        mode: LayerMode::Override,
        weight: 1.0,
        decay: 0.0,
    };
}

/// The ground's slope under each foot, for foot alignment: the normal's x
/// and z in model space, in 1/127 (y follows). Zero is flat: the foot
/// keeps its clip orientation ([`crate::anim::FootIk::slope`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct FootSlope {
    pub left: [i8; 2],
    pub right: [i8; 2],
}

impl FootSlope {
    pub const FLAT: FootSlope = FootSlope {
        left: [0, 0],
        right: [0, 0],
    };

    pub fn is_flat(&self) -> bool {
        *self == FootSlope::FLAT
    }

    /// From model-space normals (any length; pointing up).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn from_normals(left: Vec3, right: Vec3) -> FootSlope {
        let q = |n: Vec3| -> [i8; 2] {
            let len = sqrt(n.dot(n));
            if len < 1e-6 || n.y <= 0.0 {
                return [0, 0];
            }
            let k = 127.0 / len;
            let c = |v: f32| crate::math3d::to_i32((v * k).clamp(-127.0, 127.0)) as i8;
            [c(n.x), c(n.z)]
        };
        FootSlope {
            left: q(left),
            right: q(right),
        }
    }

    /// Foot `l`'s normal (unit length).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn normal(&self, l: usize) -> Vec3 {
        let [x, z] = if l == 0 { self.left } else { self.right };
        let (x, z) = (x as f32 / 127.0, z as f32 / 127.0);
        let y = sqrt((1.0 - x * x - z * z).max(0.0));
        Vec3::new(x, y, z)
    }
}

/// What a platform's poses may hold ([`crate::Caps::anim_layers`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct AnimLayers {
    /// Layers, the base included (1: no layers).
    pub layers: u8,
    /// Clip samples per pose, inertialized offsets included.
    pub clips: u8,
}

impl AnimLayers {
    /// #65's pose: one layer, three clips (the base N64).
    pub const AREF: AnimLayers = AnimLayers {
        layers: 1,
        clips: 3,
    };
    /// Everything a pose holds.
    pub const MAX: AnimLayers = AnimLayers {
        layers: MAX_POSE_LAYERS as u8,
        clips: MAX_POSE_SAMPLES as u8,
    };
}

/// What [`AnimPose::fold`] took out of a pose.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct FoldReport {
    /// Layers dropped (beyond the tier's count, or emptied).
    pub layers: u8,
    /// Inertialized transitions turned into a crossfade from the frozen
    /// source.
    pub inertial: u8,
    /// Lightest samples dropped (their layer renormalized).
    pub samples: u8,
}

impl FoldReport {
    pub fn is_empty(&self) -> bool {
        *self == FoldReport::default()
    }
}

/// A layered pose: up to [`MAX_POSE_SAMPLES`] clip samples in up to
/// [`MAX_POSE_LAYERS`] layers, foot IK and slopes. Fixed size, `Copy`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AnimPose {
    pub samples: [PoseSample; MAX_POSE_SAMPLES],
    pub len: u8,
    pub layers: [PoseLayer; MAX_POSE_LAYERS],
    pub layer_count: u8,
    pub ik: IkPose,
    pub slope: FootSlope,
}

impl Default for AnimPose {
    fn default() -> Self {
        AnimPose::EMPTY
    }
}

impl AnimPose {
    /// No sample: the bind pose.
    pub const EMPTY: AnimPose = AnimPose {
        samples: [PoseSample::NONE; MAX_POSE_SAMPLES],
        len: 0,
        layers: [PoseLayer::BASE; MAX_POSE_LAYERS],
        layer_count: 1,
        ik: IkPose::NONE,
        slope: FootSlope::FLAT,
    };

    /// One clip at `time`.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn clip(clip: ClipId, time: f32) -> AnimPose {
        let mut p = AnimPose::EMPTY;
        p.push(0, SampleRole::Pose, clip, time, 1.0);
        p
    }

    /// The pose an [`AnimRef`] draws (its three clips' weights).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn from_ref(a: &AnimRef) -> AnimPose {
        let mut p = AnimPose::EMPTY;
        let wb = a.blend.map_or(0.0, |b| b.weight.clamp(0.0, 1.0));
        let wo = a.over.weight.clamp(0.0, 1.0);
        let keep = 1.0 - wo;
        p.push(0, SampleRole::Pose, a.clip, a.time, (1.0 - wb) * keep);
        if let Some(b) = a.blend {
            p.push(0, SampleRole::Pose, b.clip, b.time, wb * keep);
        }
        if wo > 0.0 {
            p.push(0, SampleRole::Pose, a.over.clip, a.over.time, wo);
        }
        p.ik = a.ik;
        p
    }

    pub fn samples(&self) -> &[PoseSample] {
        &self.samples[..self.len as usize]
    }

    pub fn layers(&self) -> &[PoseLayer] {
        &self.layers[..self.layer_count as usize]
    }

    /// Add a sample (ignored when full). Zero weights are kept out.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn push(&mut self, layer: u8, role: SampleRole, clip: ClipId, time: f32, weight: f32) {
        if weight <= 0.0 || (self.len as usize) >= MAX_POSE_SAMPLES {
            return;
        }
        self.samples[self.len as usize] = PoseSample {
            clip,
            time,
            weight,
            layer,
            role,
        };
        self.len += 1;
    }

    /// Add a layer over the others; returns its index (`None` when full).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn add_layer(&mut self, layer: PoseLayer) -> Option<u8> {
        let n = self.layer_count as usize;
        if n >= MAX_POSE_LAYERS {
            return None;
        }
        self.layers[n] = layer;
        self.layer_count += 1;
        Some(n as u8)
    }

    pub fn with_ik(self, ik: IkPose) -> AnimPose {
        AnimPose { ik, ..self }
    }

    pub fn with_slope(self, slope: FootSlope) -> AnimPose {
        AnimPose { slope, ..self }
    }

    /// Whether it draws as an [`AnimRef`] with no loss: one layer, at most
    /// three pose samples, no inertialized offset, flat feet.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn fits_ref(&self) -> bool {
        self.layer_count <= 1
            && self.len <= 3
            && self.slope.is_flat()
            && self.samples().iter().all(|s| s.role == SampleRole::Pose)
    }

    /// The pose cut down to `caps`, deterministically (the same on every
    /// target, so a tier plays a defined pose): layers past the count go
    /// first, then an inertialized transition becomes a crossfade from its
    /// frozen source (its `To` samples dropped, the `From` ones weighted by
    /// the decay), then the lightest samples go, each layer renormalized.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn fold(&self, caps: AnimLayers) -> (AnimPose, FoldReport) {
        let mut p = *self;
        let mut r = FoldReport::default();
        let max_layers = (caps.layers.max(1) as usize).min(MAX_POSE_LAYERS);
        if (p.layer_count as usize) > max_layers {
            r.layers = p.layer_count - max_layers as u8;
            p.layer_count = max_layers as u8;
            p.retain(|s| (s.layer as usize) < max_layers);
        }
        let max = (caps.clips.max(1) as usize).min(MAX_POSE_SAMPLES);
        // The base N64's tier draws through an `AnimRef`: no offsets, no
        // slopes.
        let aref = caps.layers <= 1 && caps.clips <= 3;
        if (p.len as usize) > max || aref {
            // Inertialized offsets need both ends; a crossfade from the
            // frozen source needs one.
            for l in 0..p.layer_count as usize {
                let has = p
                    .samples()
                    .iter()
                    .any(|s| s.layer as usize == l && s.role != SampleRole::Pose);
                if !has {
                    continue;
                }
                let d = p.layers[l].decay.clamp(0.0, 1.0);
                p.retain(|s| !(s.layer as usize == l && s.role == SampleRole::To));
                for s in p.samples[..p.len as usize].iter_mut() {
                    if s.layer as usize == l {
                        match s.role {
                            SampleRole::From => {
                                s.role = SampleRole::Pose;
                                s.weight *= d;
                            }
                            _ => s.weight *= 1.0 - d,
                        }
                    }
                }
                p.retain(|s| s.weight > 0.0);
                p.layers[l].decay = 0.0;
                r.inertial += 1;
            }
        }
        while (p.len as usize) > max {
            // The lightest sample (its weight within its layer, scaled by
            // the layer's weight) goes; ties drop the later one.
            let mut min = 0;
            let eff = |s: &PoseSample| s.weight * p.layers[s.layer as usize].weight;
            for i in 1..p.len as usize {
                if eff(&p.samples[i]) <= eff(&p.samples[min]) {
                    min = i;
                }
            }
            let layer = p.samples[min].layer;
            for i in min..p.len as usize - 1 {
                p.samples[i] = p.samples[i + 1];
            }
            p.len -= 1;
            r.samples += 1;
            p.renormalize(layer);
        }
        // A layer left without samples moves nothing: drop it when it is
        // the last (indices of the others stay).
        while p.layer_count > 1 && !p.samples().iter().any(|s| s.layer + 1 == p.layer_count) {
            p.layer_count -= 1;
            r.layers += 1;
        }
        if aref {
            p.slope = FootSlope::FLAT;
        }
        (p, r)
    }

    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn retain(&mut self, keep: impl Fn(&PoseSample) -> bool) {
        let mut n = 0;
        for i in 0..self.len as usize {
            if keep(&self.samples[i]) {
                self.samples[n] = self.samples[i];
                n += 1;
            }
        }
        self.len = n as u8;
    }

    /// Scale a layer's `Pose` samples to sum to 1.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn renormalize(&mut self, layer: u8) {
        let mut total = 0.0;
        for s in self.samples() {
            if s.layer == layer && s.role == SampleRole::Pose {
                total += s.weight;
            }
        }
        if total > 0.0 {
            let k = 1.0 / total;
            for s in self.samples[..self.len as usize].iter_mut() {
                if s.layer == layer && s.role == SampleRole::Pose {
                    s.weight *= k;
                }
            }
        }
    }

    /// The [`AnimRef`] of the pose folded to [`AnimLayers::AREF`] (what the
    /// base N64 draws, and what a backend without a pose table draws):
    /// `clip`, `blend` and `over` in sample order, as
    /// [`AnimGraphInstance::anim`](crate::anim::AnimGraphInstance::anim)
    /// builds them.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn to_ref(&self) -> AnimRef {
        let (p, _) = self.fold(AnimLayers::AREF);
        let c = p.samples();
        let Some(first) = c.first() else {
            return AnimRef::new(ClipId(0), 0.0).with_ik(p.ik);
        };
        let mut a = AnimRef::new(first.clip, first.time);
        let total: f32 = c.iter().map(|x| x.weight).sum();
        if c.len() >= 2 && total > 0.0 {
            let w01 = c[0].weight + c[1].weight;
            let k1 = if w01 > 0.0 { c[1].weight / w01 } else { 0.0 };
            a = a.blend_to(c[1].clip, c[1].time, k1);
            if c.len() == 3 {
                a.over = AnimBlend {
                    clip: c[2].clip,
                    time: c[2].time,
                    weight: c[2].weight / total,
                };
            }
        }
        a.with_ik(p.ik)
    }

    /// What a backend's [`Renderer::push_pose`](crate::Renderer::push_pose)
    /// does with a pose, given its tier and its table's length: the
    /// `AnimRef` to draw and the entry to append (`None`: the pose fits the
    /// `AnimRef`, or the table is full and the folded clips draw).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn table_entry(&self, caps: AnimLayers, table_len: usize) -> (AnimRef, Option<AnimPose>) {
        let base = self.to_ref();
        let (p, _) = self.fold(caps);
        if p.fits_ref() || table_len >= u16::MAX as usize {
            // The tier's own fold may differ from the base N64's (more
            // clips kept): draw it, still through an `AnimRef` when it can.
            return (if p.fits_ref() { p.to_ref() } else { base }, None);
        }
        (
            AnimRef {
                pose: table_len as u16 + 1,
                ..base
            },
            Some(p),
        )
    }

    /// A digest of the pose (clips, times and weights quantized to 1/4096,
    /// layers, IK, slopes): the self-test compares it across targets.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn digest(&self) -> u32 {
        let mut h = crate::anim::graph::Fnv::new();
        let q = |v: f32| crate::math3d::to_i32(v * 4096.0) as u32;
        h.u32(self.len as u32 | (self.layer_count as u32) << 8);
        for s in self.samples() {
            h.u32(s.clip.0);
            h.u32(q(s.time));
            h.u32(q(s.weight));
            h.u32(s.layer as u32 | (s.role as u32) << 8);
        }
        for l in self.layers() {
            h.u32(l.mask.0);
            h.u32(l.mode as u32);
            h.u32(q(l.weight));
            h.u32(q(l.decay));
        }
        h.u32(self.ik.left as u16 as u32 | (self.ik.right as u16 as u32) << 16);
        let s = self.slope;
        h.u32(u32::from_le_bytes([
            s.left[0] as u8,
            s.left[1] as u8,
            s.right[0] as u8,
            s.right[1] as u8,
        ]));
        h.0
    }
}

/// The inertialization curve: how much of a transition's offset is left
/// `k` (0..1) of the way through it — 1 at the start, 0 at the end, with no
/// velocity at either end (`1 − smootherstep`).
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn inertial_decay(k: f32) -> f32 {
    let k = k.clamp(0.0, 1.0);
    1.0 - k * k * k * (k * (k * 6.0 - 15.0) + 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(n: &str) -> ClipId {
        ClipId::from_name(n)
    }

    #[test]
    fn a_ref_round_trips_through_a_pose() {
        let a = AnimRef::new(c("walk"), 0.3).blend_to(c("run"), 0.1, 0.25);
        let mut b = a;
        b.over = AnimBlend {
            clip: c("hop"),
            time: 0.2,
            weight: 0.4,
        };
        for r in [AnimRef::new(c("idle"), 1.0), a, b] {
            let p = AnimPose::from_ref(&r);
            assert!(p.fits_ref());
            let back = p.to_ref();
            assert_eq!(back.clip, r.clip);
            assert_eq!(back.over.clip, r.over.clip);
            assert!((back.over.weight - r.over.weight).abs() < 1e-6);
            if let (Some(x), Some(y)) = (back.blend, r.blend) {
                assert_eq!(x.clip, y.clip);
                assert!((x.weight - y.weight).abs() < 1e-6, "{x:?} {y:?}");
            }
        }
    }

    #[test]
    fn folding_drops_layers_then_inertia_then_the_lightest_samples() {
        let mut p = AnimPose::EMPTY;
        for (clip, w) in [("a", 0.5), ("b", 0.3), ("c", 0.2)] {
            p.push(0, SampleRole::Pose, c(clip), 0.0, w);
        }
        p.push(0, SampleRole::From, c("old"), 0.4, 1.0);
        p.push(0, SampleRole::To, c("a"), 0.0, 1.0);
        p.layers[0].decay = 0.25;
        let up = p
            .add_layer(PoseLayer {
                mask: JointMask(0b1110),
                mode: LayerMode::Override,
                weight: 1.0,
                decay: 0.0,
            })
            .unwrap();
        p.push(up, SampleRole::Pose, c("aim"), 0.0, 1.0);
        assert!(!p.fits_ref());

        // Everything fits on the PC.
        let (pc, r) = p.fold(AnimLayers::MAX);
        assert_eq!((pc, r.is_empty()), (p, true));

        // Two layers, six clips: all of it fits.
        let (x, r) = p.fold(AnimLayers {
            layers: 2,
            clips: 6,
        });
        assert!(r.is_empty());
        assert_eq!(x.len, 6);

        // Base N64: the layer goes, the inertia becomes a crossfade from
        // the frozen source (0.25 of it), then the lightest sample goes.
        let (n64, r) = p.fold(AnimLayers::AREF);
        assert_eq!(
            r,
            FoldReport {
                layers: 1,
                inertial: 1,
                samples: 1,
            }
        );
        assert!(n64.fits_ref());
        let w: f32 = n64.samples().iter().map(|s| s.weight).sum();
        assert!((w - 1.0).abs() < 1e-5);
        assert!(n64.samples().iter().any(|s| s.clip == c("old")));
        assert!(!n64.samples().iter().any(|s| s.clip == c("c")));
        // The same fold every time (a defined pose per tier).
        assert_eq!(p.fold(AnimLayers::AREF).0, n64);
        assert_eq!(p.to_ref(), n64.to_ref());
    }

    #[test]
    fn decay_runs_from_one_to_zero_without_a_kink() {
        assert_eq!(inertial_decay(0.0), 1.0);
        assert_eq!(inertial_decay(1.0), 0.0);
        let mut last = 1.0;
        for i in 1..=100 {
            let d = inertial_decay(i as f32 / 100.0);
            assert!(d <= last);
            last = d;
        }
        // Flat at both ends.
        assert!(1.0 - inertial_decay(0.01) < 1e-4);
        assert!(inertial_decay(0.99) < 1e-4);
    }

    #[test]
    fn slopes_quantize_and_come_back() {
        let s = FootSlope::from_normals(Vec3::new(0.2, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(s.right, [0, 0]);
        let n = s.normal(0);
        assert!((n.x - 0.196).abs() < 0.01 && (n.y - 0.98).abs() < 0.01);
        assert!(FootSlope::from_normals(Vec3::new(0.0, -1.0, 0.0), Vec3::ZERO).is_flat());
    }
}
