//! Kicks on graph layers (issue #136): `assets/anim/player.anim.ron` holds
//! the `strike` layer (hips, legs, torso, head) and the `arms` layer, each
//! a state per kick clip; the game reads them as data from the baked blob
//! ([`PLAYER_TANG`]) and lays the kick over whatever its own clip picking
//! shows (`pick_clip`): a shot at a sprint keeps the run's arm swing, one
//! from a standstill swings the arms with it.
//!
//! No graph instance runs per player: on the N64 one instance's update
//! costs ~400 µs (the runtime decodes the blob byte by byte), and 22 of
//! them took the football template's match from 30.4 to 39.9 ms against a
//! 33.3 ms gate. The layers' masks and clips are what the game needs; the
//! weights are its own (the strike eases in and out over a crossfade, the
//! arms follow it only while slow).

use super::anim::{self, AnimState, Pose, joint};
use super::clips_gen::{CLIPS, ClipName, JOINTS};
use trino_core::ClipId;
use trino_core::anim::AnimGraph;

/// `assets/anim/player.anim.ron` baked against the PC-only QA rig
/// (`cargo xtask anim export player --dir examples/solid-eleven --out
/// examples/solid-eleven/src/render/player.tang`; `solid_eleven_layers`
/// fails on a stale copy).
pub static PLAYER_TANG: &[u8] = include_bytes!("player.tang");

/// The QA rig's joints (`models/se_player_rig`, TMDL order: what the
/// graph's masks count) and this game's joint for each.
pub const RIG_JOINTS: [(&str, usize); JOINTS] = [
    ("rig_pelvis", joint::PELVIS),
    ("rig_thigh_l", joint::THIGH_L),
    ("rig_shin_l", joint::SHIN_L),
    ("rig_thigh_r", joint::THIGH_R),
    ("rig_shin_r", joint::SHIN_R),
    ("rig_torso", joint::TORSO),
    ("rig_arm_l", joint::ARM_L),
    ("rig_forearm_l", joint::FOREARM_L),
    ("rig_arm_r", joint::ARM_R),
    ("rig_forearm_r", joint::FOREARM_R),
    ("rig_head", joint::HEAD),
];

/// A rig joint mask as this game's joints.
pub fn game_mask(rig: u32) -> u16 {
    let mut m = 0u16;
    for (k, (_, j)) in RIG_JOINTS.iter().enumerate() {
        if rig & (1 << k) != 0 {
            m |= 1 << j;
        }
    }
    m
}

/// The kicks the strike layers play.
pub const KICKS: [ClipName; 5] = [
    ClipName::Pass,
    ClipName::Lob,
    ClipName::Shot,
    ClipName::PowerShot,
    ClipName::Throw,
];

/// The graph's two kick layers, as the game uses them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KickLayers {
    /// The joints of the `strike` layer and of the `arms` layer (this
    /// game's joints).
    pub strike: u16,
    pub arms: u16,
    /// The kicks both layers have a state for (bit `k` = [`KICKS`]`[k]`).
    pub clips: u8,
}

impl KickLayers {
    /// No layers: no kick is ever laid over (a broken blob).
    pub const NONE: KickLayers = KickLayers {
        strike: 0,
        arms: 0,
        clips: 0,
    };

    /// Read the layers from [`PLAYER_TANG`] (once a match).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn load() -> KickLayers {
        AnimGraph::parse(PLAYER_TANG)
            .ok()
            .and_then(|g| KickLayers::of(&g))
            .unwrap_or(KickLayers::NONE)
    }

    /// The `strike` and `arms` layers of `g`: their masks and the kicks
    /// they both have a state for.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn of(g: &AnimGraph) -> Option<KickLayers> {
        let (s, a) = (g.layer_named("strike")?, g.layer_named("arms")?);
        let clips_of = |l: usize| {
            let (lo, hi) = g.layer(l).states;
            let mut bits = 0u8;
            for st in lo..hi {
                if let Some(id) = g.state_clip(st) {
                    for (k, c) in KICKS.iter().enumerate() {
                        if ClipId::from_name(CLIPS[*c as usize].name) == id {
                            bits |= 1 << k;
                        }
                    }
                }
            }
            bits
        };
        Some(KickLayers {
            strike: game_mask(g.layer(s).mask.0),
            arms: game_mask(g.layer(a).mask.0),
            clips: clips_of(s) & clips_of(a),
        })
    }

    /// Whether the layers play `c`.
    pub fn plays(&self, c: ClipName) -> bool {
        KICKS
            .iter()
            .position(|k| *k == c)
            .is_some_and(|k| self.clips & (1 << k) != 0)
    }

    /// `st`'s kick laid over `pose`: the strike layer's joints by the
    /// strike weight, the arms by it times how slow the body moves. Out of
    /// line, in place: the match's stack has little room (the N64
    /// template's gate).
    #[inline(never)]
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn overlay(&self, pose: &mut Pose, st: &AnimState, mask: u16) {
        let w = st.strike();
        if w <= 0.0 || !self.plays(st.kick) {
            return;
        }
        let arms_w = w * arms_share(st.speed);
        let joints = mask & (self.strike | if arms_w > 0.0 { self.arms } else { 0 });
        let kick = anim::sample_masked(
            st.kick,
            st.kick_time(),
            false,
            joints | (mask & anim::mask::SHINS),
        );
        for j in 0..JOINTS {
            let k = if self.strike & (1 << j) != 0 {
                w
            } else if self.arms & (1 << j) != 0 {
                arms_w
            } else {
                continue;
            };
            if mask & (1 << j) != 0 && k > 0.0 {
                pose.q[j] = pose.q[j].nlerp(kick.q[j], k);
            }
        }
        if self.strike & (1 << joint::PELVIS) != 0 {
            pose.root = pose.root + (kick.root - pose.root) * w;
        }
    }
}

/// How much of a kick the arms take at `speed` (m/s): all of it from a
/// jog down, none at a sprint (the run's swing stays).
pub fn arms_share(speed: f32) -> f32 {
    1.0 - ((speed - 2.6) / 3.6).clamp(0.0, 1.0)
}
