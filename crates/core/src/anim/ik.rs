//! Foot IK (issue #65): an animated character's feet put on uneven ground —
//! a slope, a step — by an analytic two-bone solve (thigh → shin → foot)
//! after the clip is sampled, inside [`crate::Mesh::pose`], so every backend
//! draws it with no code of its own.
//!
//! The model names its chains (TMDL `IKCH`, from the manifest's `ik`); the
//! pose carries how far the ground under each foot is above or below the
//! floor the clip was made on ([`IkPose`], in the model's millimetres). The
//! game finds those heights where the feet are ([`FootIk::pose`], with the
//! feet an animation graph knows, `AnimGraphInstance::feet`); the pelvis
//! drops for the lower foot, each leg bends to reach its own, the feet keep
//! their animated orientation.
//!
//! Tiers: [`Caps::anim_ik`](crate::Caps::anim_ik) is on for the PC, the Web
//! and the 3DS, off for the N64 (`FootIk::force` turns it on for one
//! character there). [`IkPose::NONE`] is the old path exactly.

use crate::math::Vec3;
use crate::math3d::{Mat34, Quat, sqrt};
use crate::render3d::MAX_JOINTS;

/// How far the ground under each foot is from the clip's floor, in the
/// model's units / 1000 (millimetres for a model in metres): positive
/// raises the foot, negative lowers it (the pelvis drops with it). Both 0
/// is no IK at all (the pose is the clip's, bit for bit).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct IkPose {
    pub left: i16,
    pub right: i16,
}

impl IkPose {
    pub const NONE: IkPose = IkPose { left: 0, right: 0 };

    /// From model units (clamped to ±32 units).
    pub fn from_units(left: f32, right: f32) -> IkPose {
        let q = |v: f32| crate::math3d::to_i32((v * 1000.0).clamp(-32_000.0, 32_000.0)) as i16;
        IkPose {
            left: q(left),
            right: q(right),
        }
    }

    pub fn is_none(&self) -> bool {
        *self == IkPose::NONE
    }

    fn units(v: i16) -> f32 {
        v as f32 * 0.001
    }
}

/// A model's IK chains (TMDL `IKCH`): the pelvis joint (or none) and up to
/// two legs, each hip → knee → foot.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Chains {
    /// Legs (0..=2).
    pub legs: u8,
    /// The joint that drops for the lower foot (`NO_JOINT`: none).
    pub pelvis: u8,
    /// `[hip, knee, foot]` per leg: left, right.
    pub chain: [[u8; 3]; 2],
}

/// No joint.
pub const NO_JOINT: u8 = 0xFF;

impl Chains {
    /// Parse an `IKCH` body: `u8 legs, u8 pelvis (0xFF none), legs × (u8 hip,
    /// u8 knee, u8 foot)`, against a skeleton's parents (`parent(j)`): every
    /// joint exists and each leg descends hip → knee → foot.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn parse(
        body: &[u8],
        joints: usize,
        parent: &dyn Fn(usize) -> Option<usize>,
    ) -> Option<Chains> {
        let legs = *body.first()? as usize;
        let pelvis = *body.get(1)?;
        if legs == 0 || legs > 2 || body.len() < 2 + 3 * legs {
            return None;
        }
        if pelvis != NO_JOINT && pelvis as usize >= joints {
            return None;
        }
        let descends = |child: usize, anc: usize| {
            let mut j = child;
            for _ in 0..MAX_JOINTS {
                match parent(j) {
                    Some(p) if p == anc => return true,
                    Some(p) => j = p,
                    None => return false,
                }
            }
            false
        };
        let mut chain = [[0u8; 3]; 2];
        for (l, c) in chain.iter_mut().enumerate().take(legs) {
            let at = 2 + 3 * l;
            *c = [body[at], body[at + 1], body[at + 2]];
            let [h, k, f] = c.map(|x| x as usize);
            if h >= joints || k >= joints || f >= joints || !descends(k, h) || !descends(f, k) {
                return None;
            }
        }
        Some(Chains {
            legs: legs as u8,
            pelvis,
            chain,
        })
    }
}

#[inline]
fn pos(m: &Mat34) -> Vec3 {
    Vec3::new(m.m[0][3], m.m[1][3], m.m[2][3])
}

/// The rotation taking direction `u` onto `v` (both nonzero).
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn from_to(u: Vec3, v: Vec3) -> Mat34 {
    let (lu, lv) = (sqrt(u.dot(u)), sqrt(v.dot(v)));
    if lu < 1e-9 || lv < 1e-9 {
        return Mat34::IDENTITY;
    }
    let c = u.cross(v);
    let w = lu * lv + u.dot(v);
    let q = if w < 1e-6 * lu * lv {
        // Opposite: a half turn about any axis normal to u.
        let a = if u.x.abs() < 0.9 {
            Vec3::new(1.0, 0.0, 0.0)
        } else {
            Vec3::new(0.0, 1.0, 0.0)
        };
        let n = u.cross(a).normalized();
        Quat::new(n.x, n.y, n.z, 0.0)
    } else {
        Quat::new(c.x, c.y, c.z, w).normalized()
    };
    Mat34::from_trs(Vec3::ZERO, q, Vec3::ONE)
}

/// The rotation part of `m` applied to `d`.
#[inline]
fn rot(m: &Mat34, d: Vec3) -> Vec3 {
    m.transform_dir(d)
}

/// Transpose of a pure rotation (its inverse).
fn transpose(r: &Mat34) -> Mat34 {
    let mut t = Mat34::IDENTITY;
    for (i, row) in t.m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate().take(3) {
            *v = r.m[j][i];
        }
    }
    t
}

/// Rotate joint `root`'s subtree by `r` about `pivot` (world space).
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn turn_subtree(
    world: &mut [Mat34; MAX_JOINTS],
    count: usize,
    parent: &dyn Fn(usize) -> Option<usize>,
    root: usize,
    pivot: Vec3,
    r: &Mat34,
) {
    let mut inside = [false; MAX_JOINTS];
    for j in root..count {
        inside[j] = j == root || parent(j).is_some_and(|p| p >= root && inside[p]);
        if inside[j] {
            let m = world[j];
            let mut out = r.mul(&m);
            let t = rot(r, pos(&m) - pivot) + pivot;
            out.m[0][3] = t.x;
            out.m[1][3] = t.y;
            out.m[2][3] = t.z;
            world[j] = out;
        }
    }
}

/// Move joint `root`'s subtree by `d`.
fn shift_subtree(
    world: &mut [Mat34; MAX_JOINTS],
    count: usize,
    parent: &dyn Fn(usize) -> Option<usize>,
    root: usize,
    d: Vec3,
) {
    let mut inside = [false; MAX_JOINTS];
    for j in root..count {
        inside[j] = j == root || parent(j).is_some_and(|p| p >= root && inside[p]);
        if inside[j] {
            world[j].m[0][3] += d.x;
            world[j].m[1][3] += d.y;
            world[j].m[2][3] += d.z;
        }
    }
}

/// Solve the legs on `world` (model-space joint matrices, the pose before
/// the inverse binds). Out of line: only a draw with an [`IkPose`] pays.
#[inline(never)]
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub(crate) fn solve(
    world: &mut [Mat34; MAX_JOINTS],
    count: usize,
    parent: &dyn Fn(usize) -> Option<usize>,
    chains: &Chains,
    ik: IkPose,
) {
    let offsets = [IkPose::units(ik.left), IkPose::units(ik.right)];
    let legs = chains.legs as usize;
    // The animated feet, before anything moves.
    let mut feet = [Vec3::ZERO; 2];
    for (l, f) in feet.iter_mut().enumerate().take(legs) {
        *f = pos(&world[chains.chain[l][2] as usize]);
    }
    // The pelvis drops so the lower foot can reach down.
    let mut drop = 0.0f32;
    for &o in offsets.iter().take(legs) {
        drop = drop.min(o);
    }
    if chains.pelvis != NO_JOINT && drop < 0.0 {
        shift_subtree(
            world,
            count,
            parent,
            chains.pelvis as usize,
            Vec3::new(0.0, drop, 0.0),
        );
    }
    for l in 0..legs {
        let [h, k, f] = chains.chain[l].map(|x| x as usize);
        let target = feet[l] + Vec3::new(0.0, offsets[l], 0.0);
        let (hp, kp, fp) = (pos(&world[h]), pos(&world[k]), pos(&world[f]));
        let (a, b) = (sqrt((kp - hp).dot(kp - hp)), sqrt((fp - kp).dot(fp - kp)));
        if a < 1e-6 || b < 1e-6 {
            continue;
        }
        let to = target - hp;
        let len = sqrt(to.dot(to));
        if len < 1e-6 {
            continue;
        }
        let u = to * (1.0 / len);
        // A bone under 1e-4 crosses the bounds: no clamp then.
        let d = crate::math::clamp_or(len, (a - b).abs() + 1e-4, a + b - 1e-4);
        // The knee bends in the plane it already bends in.
        let mut bend = (kp - hp) - u * (kp - hp).dot(u);
        if bend.dot(bend) < 1e-10 {
            bend = (fp - kp).cross(Vec3::new(1.0, 0.0, 0.0));
            if bend.dot(bend) < 1e-10 {
                bend = Vec3::new(0.0, 0.0, 1.0);
            }
        }
        let bend = bend.normalized();
        let x = (a * a - b * b + d * d) / (2.0 * d);
        let y = sqrt((a * a - x * x).max(0.0));
        let knee = hp + u * x + bend * y;
        let r1 = from_to(kp - hp, knee - hp);
        turn_subtree(world, count, parent, h, hp, &r1);
        let reach = hp + u * d;
        let foot_now = pos(&world[f]);
        let r2 = from_to(foot_now - knee, reach - knee);
        turn_subtree(world, count, parent, k, knee, &r2);
        // The foot keeps its animated orientation (flat stays flat).
        let undo = transpose(&r2.mul(&r1));
        let at = pos(&world[f]);
        turn_subtree(world, count, parent, f, at, &undo);
    }
}

/// A game's foot IK: the [`IkPose`] for a character from the ground under
/// its feet, by tier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FootIk {
    /// Run even where [`Caps::anim_ik`](crate::Caps::anim_ik) is off (the
    /// N64: one character, its cost in `N64_COST.us_per_ik_draw`).
    pub force: bool,
    /// The most a foot moves (model units): ground further away is a ledge,
    /// not a step (the foot stays where the clip has it).
    pub max_step: f32,
}

impl Default for FootIk {
    fn default() -> Self {
        FootIk {
            force: false,
            max_step: 0.5,
        }
    }
}

impl FootIk {
    /// Whether it runs on these caps.
    pub fn enabled(&self, caps: &crate::Caps) -> bool {
        self.force || caps.anim_ik
    }

    /// The pose for an entity drawn at `matrix` (model → world) whose
    /// animated feet are `feet` (model space, `AnimGraphInstance::feet`):
    /// `ground(world point)` answers the world height of the ground below
    /// it (a physics raycast down), `None` for none. [`IkPose::NONE`] where
    /// the tier has no IK.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn pose(
        &self,
        caps: &crate::Caps,
        matrix: &Mat34,
        feet: [Vec3; 2],
        mut ground: impl FnMut(Vec3) -> Option<f32>,
    ) -> IkPose {
        if !self.enabled(caps) {
            return IkPose::NONE;
        }
        // Model units per world unit vertically (the entity's own scale).
        let up = Vec3::new(matrix.m[0][1], matrix.m[1][1], matrix.m[2][1]);
        let scale = sqrt(up.dot(up)).max(1e-6);
        let base = matrix.m[1][3];
        let mut out = [0.0f32; 2];
        for (o, f) in out.iter_mut().zip(feet) {
            // Where the foot's floor is under it: its XZ at the clip's floor.
            let w = matrix.transform_point(Vec3::new(f.x, 0.0, f.z));
            if let Some(y) = ground(w) {
                let d = (y - base) / scale;
                if d.abs() <= self.max_step {
                    *o = d;
                }
            }
        }
        IkPose::from_units(out[0], out[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// hip (root) at y 1, knee at 0.5, foot at 0: a straight leg.
    fn leg() -> ([Mat34; MAX_JOINTS], impl Fn(usize) -> Option<usize>) {
        let mut w = [Mat34::IDENTITY; MAX_JOINTS];
        let at = |y: f32, z: f32| {
            let mut m = Mat34::IDENTITY;
            m.m[1][3] = y;
            m.m[2][3] = z;
            m
        };
        w[0] = at(1.0, 0.0);
        // A slightly bent knee (forward), so the bend plane is known.
        w[1] = at(0.5, 0.05);
        w[2] = at(0.0, 0.0);
        (w, |j: usize| if j == 0 { None } else { Some(j - 1) })
    }

    #[test]
    fn a_raised_foot_reaches_its_target_and_keeps_its_orientation() {
        let (mut w, parent) = leg();
        let chains = Chains {
            legs: 1,
            pelvis: NO_JOINT,
            chain: [[0, 1, 2], [0, 0, 0]],
        };
        let before = w;
        solve(&mut w, 3, &parent, &chains, IkPose::from_units(0.2, 0.0));
        let f = pos(&w[2]);
        assert!((f.y - 0.2).abs() < 2e-3 && f.z.abs() < 2e-3, "{f:?}");
        // Bone lengths kept.
        let l = |a: Vec3, b: Vec3| sqrt((a - b).dot(a - b));
        assert!((l(pos(&w[0]), pos(&w[1])) - l(pos(&before[0]), pos(&before[1]))).abs() < 1e-3);
        // The knee went forward (it bends the way it bent).
        assert!(pos(&w[1]).z > 0.05);
        // The foot's orientation is the clip's.
        for r in 0..3 {
            for c in 0..3 {
                assert!(
                    (w[2].m[r][c] - before[2].m[r][c]).abs() < 2e-3,
                    "{:?}",
                    w[2]
                );
            }
        }
    }

    #[test]
    fn a_lower_foot_drops_the_pelvis_and_none_is_the_clip() {
        let (mut w, parent) = leg();
        let chains = Chains {
            legs: 1,
            pelvis: 0,
            chain: [[0, 1, 2], [0, 0, 0]],
        };
        solve(&mut w, 3, &parent, &chains, IkPose::from_units(-0.15, 0.0));
        assert!((pos(&w[0]).y - 0.85).abs() < 1e-4, "the hips drop 15 cm");
        assert!((pos(&w[2]).y + 0.15).abs() < 2e-3, "{:?}", pos(&w[2]));
        assert_eq!(IkPose::from_units(0.0, 0.0), IkPose::NONE);
        assert_eq!(
            IkPose::from_units(0.0123, -0.5),
            IkPose {
                left: 12,
                right: -500
            }
        );
    }

    #[test]
    fn chains_must_descend_hip_knee_foot() {
        let parent = |j: usize| if j == 0 { None } else { Some(j - 1) };
        assert!(Chains::parse(&[1, 0xFF, 0, 1, 2], 3, &parent).is_some());
        assert!(
            Chains::parse(&[1, 0xFF, 2, 1, 0], 3, &parent).is_none(),
            "upside down"
        );
        assert!(
            Chains::parse(&[1, 0xFF, 0, 1, 9], 3, &parent).is_none(),
            "no joint 9"
        );
        assert!(Chains::parse(&[3, 0xFF], 3, &parent).is_none());
    }

    #[test]
    fn the_tier_decides_unless_forced() {
        let ik = FootIk::default();
        let m = Mat34::IDENTITY;
        let feet = [Vec3::new(0.1, 0.05, 0.0), Vec3::new(-0.1, 0.05, 0.0)];
        let ground = |p: Vec3| Some(if p.x > 0.0 { 0.2 } else { -0.1 });
        assert_eq!(ik.pose(&crate::Caps::N64, &m, feet, ground), IkPose::NONE);
        let on = ik.pose(&crate::Caps::N3DS, &m, feet, ground);
        assert_eq!(
            on,
            IkPose {
                left: 200,
                right: -100
            }
        );
        let forced = FootIk { force: true, ..ik };
        assert_eq!(forced.pose(&crate::Caps::N64, &m, feet, ground), on);
        // A ledge past max_step leaves the foot to the clip.
        let deep = |_: Vec3| Some(-3.0);
        assert_eq!(forced.pose(&crate::Caps::N64, &m, feet, deep), IkPose::NONE);
    }
}
