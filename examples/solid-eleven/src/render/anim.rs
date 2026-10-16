//! Rigid-part player animation: the clips authored in
//! `tools/blender/solid_eleven_anim.py` (generated into `clips_gen.rs`),
//! sampled, mirrored, crossfaded and turned into one matrix per joint.
//!
//! No trigonometry per frame: keys are quaternions, poses blend with
//! `nlerp`, and the body's yaw comes straight from the sim's facing vector.
//! The same code poses players on PC, N64 and 3DS.

use super::clips_gen::{CLIPS, ClipName, JOINTS, SKELETON};
use trino_core::{Mat34, Quat, Vec3};

/// One keyframe: the time (0..1 of the clip), the pelvis offset and a
/// rotation per joint.
#[derive(Clone, Copy, Debug)]
pub struct Key {
    pub t: f32,
    pub root: [f32; 3],
    pub q: [[f32; 4]; JOINTS],
}

#[derive(Clone, Copy, Debug)]
pub struct Clip {
    pub name: &'static str,
    pub seconds: f32,
    pub looped: bool,
    pub keys: &'static [Key],
}

/// Joint indices (the order of `solid_eleven_anim.JOINTS`).
pub mod joint {
    pub const PELVIS: usize = 0;
    pub const TORSO: usize = 1;
    pub const HEAD: usize = 2;
    pub const ARM_L: usize = 3;
    pub const ARM_R: usize = 4;
    pub const FOREARM_L: usize = 5;
    pub const FOREARM_R: usize = 6;
    pub const THIGH_L: usize = 7;
    pub const THIGH_R: usize = 8;
    pub const SHIN_L: usize = 9;
    pub const SHIN_R: usize = 10;
}

/// The left-right partner of each joint.
const MIRROR: [usize; JOINTS] = [0, 1, 2, 4, 3, 6, 5, 8, 7, 10, 9];

/// A sampled pose.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Pose {
    pub root: Vec3,
    pub q: [Quat; JOINTS],
}

impl Pose {
    pub const REST: Pose = Pose {
        root: Vec3::ZERO,
        q: [Quat::IDENTITY; JOINTS],
    };
}

pub fn clip(name: ClipName) -> &'static Clip {
    &CLIPS[name as usize]
}

fn quat(a: [f32; 4]) -> Quat {
    Quat::new(a[0], a[1], a[2], a[3])
}

/// `name` at `time` seconds (looped clips wrap, others hold their last
/// key); `mirror` swaps left and right.
pub fn sample(name: ClipName, time: f32, mirror: bool) -> Pose {
    let c = clip(name);
    let mut u = if c.seconds > 0.0 {
        time / c.seconds
    } else {
        0.0
    };
    if c.looped {
        u -= floor(u);
    } else {
        u = u.clamp(0.0, 1.0);
    }
    let keys = c.keys;
    // The segment holding u (wrapping to the first key for loops).
    let mut i = keys.len() - 1;
    for (k, key) in keys.iter().enumerate() {
        if key.t > u {
            i = if k == 0 { keys.len() - 1 } else { k - 1 };
            break;
        }
    }
    let a = &keys[i];
    let (b, span) = if i + 1 < keys.len() {
        (&keys[i + 1], keys[i + 1].t - a.t)
    } else if c.looped {
        (&keys[0], 1.0 - a.t + keys[0].t)
    } else {
        (a, 1.0)
    };
    let mut f = if span > 0.0 {
        let d = u - a.t;
        let d = if d < 0.0 { d + 1.0 } else { d };
        (d / span).clamp(0.0, 1.0)
    } else {
        0.0
    };
    // Ease in and out between keys (smoothstep): no linear "robot" joints.
    f = f * f * (3.0 - 2.0 * f);
    let mut pose = Pose::REST;
    for j in 0..JOINTS {
        pose.q[j] = quat(a.q[j]).nlerp(quat(b.q[j]), f);
    }
    pose.root = Vec3::new(
        a.root[0] + (b.root[0] - a.root[0]) * f,
        a.root[1] + (b.root[1] - a.root[1]) * f,
        a.root[2] + (b.root[2] - a.root[2]) * f,
    );
    if mirror {
        pose = mirrored(&pose);
    }
    pose
}

/// Left-right mirror: swap sides, reflect each rotation across x.
pub fn mirrored(p: &Pose) -> Pose {
    let mut out = Pose::REST;
    for j in 0..JOINTS {
        let q = p.q[MIRROR[j]];
        out.q[j] = Quat::new(q.x, -q.y, -q.z, q.w);
    }
    out.root = Vec3::new(-p.root.x, p.root.y, p.root.z);
    out
}

/// Crossfade: `w` = 0 gives `a`, 1 gives `b`.
pub fn blend(a: &Pose, b: &Pose, w: f32) -> Pose {
    if w <= 0.0 {
        return *a;
    }
    if w >= 1.0 {
        return *b;
    }
    let mut out = Pose::REST;
    for j in 0..JOINTS {
        out.q[j] = a.q[j].nlerp(b.q[j], w);
    }
    out.root = a.root + (b.root - a.root) * w;
    out
}

/// Pelvis placement for a body at `feet` (ground point, world) facing the
/// unit ground direction (`fx`, `fz`) and drawn at `scale`: a yaw matrix
/// built from the facing vector itself (no sin/cos).
pub fn body_matrix(feet: Vec3, fx: f32, fz: f32, scale: f32) -> Mat34 {
    // The model's +z goes to (fx, 0, fz); its +x to (fz, 0, -fx).
    Mat34 {
        m: [
            [fz * scale, 0.0, fx * scale, feet.x * scale],
            [0.0, scale, 0.0, feet.y * scale],
            [-fx * scale, 0.0, fz * scale, feet.z * scale],
        ],
    }
}

/// Every joint's world matrix for `pose` on a body placed by `body`.
pub fn joint_matrices(body: &Mat34, pose: &Pose, out: &mut [Mat34; JOINTS]) {
    for j in 0..JOINTS {
        let (parent, pivot) = SKELETON[j];
        let mut t = Vec3::new(pivot[0], pivot[1], pivot[2]);
        if j == joint::PELVIS {
            t += pose.root;
        }
        let local = Mat34::from_trs(t, pose.q[j], Vec3::ONE);
        out[j] = if parent == usize::MAX {
            body.mul(&local)
        } else {
            out[parent].mul(&local)
        };
    }
}

/// `floor` without std (the clip clock is never huge).
fn floor(x: f32) -> f32 {
    let i = x as i32 as f32;
    if i > x { i - 1.0 } else { i }
}

/// Per-player animation state, advanced by the game's update (visual
/// only: the sim never reads it).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AnimState {
    pub clip: ClipName,
    pub time: f32,
    pub mirror: bool,
    /// The clip being faded out, its time, and the fade left (seconds).
    pub prev: ClipName,
    pub prev_time: f32,
    pub prev_mirror: bool,
    pub fade: f32,
}

/// Crossfade length.
pub const FADE: f32 = 0.16;

impl AnimState {
    pub const fn new(clip: ClipName) -> AnimState {
        AnimState {
            clip,
            time: 0.0,
            mirror: false,
            prev: clip,
            prev_time: 0.0,
            prev_mirror: false,
            fade: 0.0,
        }
    }

    /// Switch to `clip` (fading from the current one) unless already on it;
    /// a one-shot `restart`s when asked.
    pub fn play(&mut self, clip: ClipName, mirror: bool, restart: bool) {
        if clip == self.clip && mirror == self.mirror && !restart {
            return;
        }
        self.prev = self.clip;
        self.prev_time = self.time;
        self.prev_mirror = self.mirror;
        self.fade = FADE;
        // Cycles of the same family keep their phase (jog → sprint).
        let keep = is_cycle(clip) && is_cycle(self.clip);
        if keep {
            let from = super::anim::clip(self.clip).seconds;
            let to = super::anim::clip(clip).seconds;
            self.time = if from > 0.0 {
                self.time / from * to
            } else {
                0.0
            };
        } else {
            self.time = 0.0;
        }
        self.clip = clip;
        self.mirror = mirror;
    }

    /// Advance by `dt` seconds at `rate` × the clip's speed.
    pub fn advance(&mut self, dt: f32, rate: f32) {
        self.time += dt * rate;
        self.prev_time += dt * rate;
        self.fade = (self.fade - dt).max(0.0);
    }

    pub fn pose(&self) -> Pose {
        let cur = sample(self.clip, self.time, self.mirror);
        if self.fade <= 0.0 {
            return cur;
        }
        let old = sample(self.prev, self.prev_time, self.prev_mirror);
        blend(&cur, &old, self.fade / FADE)
    }
}

fn is_cycle(c: ClipName) -> bool {
    matches!(
        c,
        ClipName::Walk | ClipName::Jog | ClipName::Sprint | ClipName::Dribble
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn every_clip_is_well_formed() {
        for c in CLIPS.iter() {
            assert!(c.seconds > 0.0, "{}", c.name);
            assert!(!c.keys.is_empty(), "{}", c.name);
            let mut last = -1.0;
            for k in c.keys {
                assert!(k.t > last && k.t <= 1.0, "{}: keys out of order", c.name);
                last = k.t;
                for q in k.q {
                    let n = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
                    assert!(close(n, 1.0), "{}: not a unit quaternion", c.name);
                }
            }
            if !c.looped {
                assert_eq!(
                    c.keys.last().unwrap().t,
                    1.0,
                    "{}: one-shots end at 1",
                    c.name
                );
            }
        }
    }

    #[test]
    fn loops_wrap_and_one_shots_hold() {
        let a = sample(ClipName::Jog, 0.1, false);
        let b = sample(
            ClipName::Jog,
            0.1 + clip(ClipName::Jog).seconds * 3.0,
            false,
        );
        for j in 0..JOINTS {
            assert!(close(a.q[j].x, b.q[j].x) && close(a.q[j].w, b.q[j].w));
        }
        let end = sample(ClipName::Shot, 10.0, false);
        let last = clip(ClipName::Shot).keys.last().unwrap();
        assert!(close(end.q[joint::THIGH_R].x, last.q[joint::THIGH_R][0]));
    }

    #[test]
    fn a_run_cycle_swings_the_legs_in_opposition() {
        // At contact the left thigh swings forward (rx < 0), the right back.
        let p = sample(ClipName::Sprint, 0.0, false);
        assert!(p.q[joint::THIGH_L].x < 0.0 && p.q[joint::THIGH_R].x > 0.0);
        // Half a cycle later it is the other way round.
        let half = clip(ClipName::Sprint).seconds * 0.5;
        let p = sample(ClipName::Sprint, half, false);
        assert!(p.q[joint::THIGH_L].x > 0.0 && p.q[joint::THIGH_R].x < 0.0);
    }

    #[test]
    fn mirroring_swaps_sides() {
        let p = sample(ClipName::Shot, 0.1, false);
        let m = sample(ClipName::Shot, 0.1, true);
        assert!(close(m.q[joint::THIGH_L].x, p.q[joint::THIGH_R].x));
        assert!(close(m.q[joint::THIGH_L].y, -p.q[joint::THIGH_R].y));
        let twice = mirrored(&m);
        assert!(close(twice.q[joint::ARM_R].z, p.q[joint::ARM_R].z));
    }

    #[test]
    fn the_rest_pose_puts_the_feet_on_the_ground() {
        let body = body_matrix(Vec3::ZERO, 0.0, 1.0, 1.0);
        let mut m = [Mat34::IDENTITY; JOINTS];
        joint_matrices(&body, &Pose::REST, &mut m);
        // Shin pivot (knee) height + the shin and boot (~0.47 m): the knee
        // sits about half a meter up.
        let knee = m[joint::SHIN_L].transform_point(Vec3::ZERO);
        assert!(close(knee.y, 0.95 - 0.03 - 0.43), "{}", knee.y);
        assert!(knee.x > 0.0, "the left knee is on +x");
        let head = m[joint::HEAD].transform_point(Vec3::ZERO);
        assert!(close(head.y, 0.95 + 0.02 + 0.5));
    }

    #[test]
    fn facing_turns_the_body_without_trig() {
        // Facing +x: the model's front (+z) maps to +x, its left (+x) to -z.
        let b = body_matrix(Vec3::ZERO, 1.0, 0.0, 1.0);
        let front = b.transform_dir(Vec3::new(0.0, 0.0, 1.0));
        assert!(close(front.x, 1.0) && close(front.z, 0.0));
        let left = b.transform_dir(Vec3::new(1.0, 0.0, 0.0));
        assert!(close(left.z, -1.0));
        // Scaled worlds keep the scale uniform (the RSP lights it).
        let s = body_matrix(Vec3::new(64.0, 0.0, 0.0), 0.6, 0.8, 1.0 / 64.0);
        let sc = s.axis_scales();
        assert!(close(sc.x, sc.y) && close(sc.y, sc.z));
        assert!(close(s.m[0][3], 1.0));
    }

    #[test]
    fn crossfades_reach_their_target() {
        let mut a = AnimState::new(ClipName::Idle);
        a.play(ClipName::Sprint, false, false);
        assert!(a.fade > 0.0);
        a.advance(FADE * 2.0, 1.0);
        assert_eq!(a.fade, 0.0);
        let p = a.pose();
        let s = sample(ClipName::Sprint, a.time, false);
        assert!(close(p.q[joint::THIGH_L].x, s.q[joint::THIGH_L].x));
        // Jog → sprint keeps the cycle's phase.
        let mut b = AnimState::new(ClipName::Jog);
        b.time = clip(ClipName::Jog).seconds * 0.5;
        b.play(ClipName::Sprint, false, false);
        assert!(close(b.time, clip(ClipName::Sprint).seconds * 0.5));
    }
}
