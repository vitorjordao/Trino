//! Compressed clips (TMDL `ANMC`, issue #65): each joint's translation,
//! rotation and scale is a **track** — one constant value, or the keyframes
//! an error-bounded reduction kept, quantized — instead of a full
//! `t3 q4 s3` f32 sample per joint per frame (`ANIM`, 40 bytes). The pose
//! decodes on the fly: a binary search for the two keys around the frame,
//! a lerp (nlerp for rotations, like the raw clip's frames). The bake checks
//! every frame of the decode against the source within its bound
//! (`trino_asset_pipeline::anim_compress`).
//!
//! ## `ANMC` (replaces `ANIM`, after `SKEL`)
//!
//! ```text
//! u32 clip count, then per clip:
//!   u32 clip id, f32 fps, u32 frame count, u32 flags (bit 0 loop), u32 body bytes
//!   body: per joint, three tracks in order t, r, s; each track:
//!     u8 0 = constant: t/s f32*3 | r i16*4
//!     u8 1 = keyed:    u16 n, u16 frame*n, then
//!                      t/s f32 min*3, f32 step*3, u16*3 per key (min + q * step)
//!                      r   i16*4 per key (q / 32767)
//! ```

use super::{rd_f32, rd_u8, rd_u16, rd_u32};
use crate::math::Vec3;
use crate::math3d::Quat;

/// Clip header bytes (one more `u32` than `ANIM`'s: the body length).
pub const CLIP_HEADER: usize = 20;

/// Track kinds.
pub const CONSTANT: u8 = 0;
pub const KEYED: u8 = 1;

/// Rotation quantization: a component is `q / ROT_SCALE`.
pub const ROT_SCALE: f32 = 32767.0;

#[inline]
fn rd_i16(d: &[u8], at: usize) -> i16 {
    rd_u16(d, at) as i16
}

/// Which channel a track holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Channel {
    Translation,
    Rotation,
    Scale,
}

/// Bytes of the track at `at` (`None` when malformed or past `d`).
pub fn track_len(d: &[u8], at: usize, ch: Channel) -> Option<usize> {
    let kind = *d.get(at)?;
    let len = match (kind, ch) {
        (CONSTANT, Channel::Rotation) => 1 + 8,
        (CONSTANT, _) => 1 + 12,
        (KEYED, _) => {
            if d.len() < at + 3 {
                return None;
            }
            let n = rd_u16(d, at + 1) as usize;
            if n == 0 {
                return None;
            }
            let keys = 3 + 2 * n;
            match ch {
                Channel::Rotation => keys + 8 * n,
                _ => keys + 24 + 6 * n,
            }
        }
        _ => return None,
    };
    (d.len() >= at + len).then_some(len)
}

/// Bytes of one joint's three tracks at `at`.
pub fn joint_len(d: &[u8], at: usize) -> Option<usize> {
    let t = track_len(d, at, Channel::Translation)?;
    let r = track_len(d, at + t, Channel::Rotation)?;
    let s = track_len(d, at + t + r, Channel::Scale)?;
    Some(t + r + s)
}

/// Validate a clip body of `joints` joints whose frames are `0..frames`:
/// every track parses, every key frame is in range and increasing.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn check_body(d: &[u8], joints: usize, frames: usize) -> bool {
    let mut at = 0;
    for _ in 0..joints {
        for ch in [Channel::Translation, Channel::Rotation, Channel::Scale] {
            let Some(len) = track_len(d, at, ch) else {
                return false;
            };
            if rd_u8(d, at) == KEYED {
                let n = rd_u16(d, at + 1) as usize;
                let mut last = -1i32;
                for k in 0..n {
                    let f = rd_u16(d, at + 3 + 2 * k) as i32;
                    if f <= last || f as usize >= frames {
                        return false;
                    }
                    last = f;
                }
                if ch != Channel::Rotation {
                    let v = at + 3 + 2 * n;
                    if (0..6).any(|i| !rd_f32(d, v + 4 * i).is_finite()) {
                        return false;
                    }
                }
            } else if ch != Channel::Rotation
                && (0..3).any(|i| !rd_f32(d, at + 1 + 4 * i).is_finite())
            {
                return false;
            }
            at += len;
        }
    }
    at == d.len()
}

/// The keys `(a, b, k)` around `frame` in a keyed track with `n` keys at
/// `keys` (frame indices): blend `k` from key `a` to key `b`.
#[inline]
fn bracket(d: &[u8], keys: usize, n: usize, frame: f32) -> (usize, usize, f32) {
    let first = rd_u16(d, keys) as f32;
    if n == 1 || frame <= first {
        return (0, 0, 0.0);
    }
    let last = rd_u16(d, keys + 2 * (n - 1)) as f32;
    if frame >= last {
        return (n - 1, n - 1, 0.0);
    }
    // Largest key at or before `frame`.
    let (mut lo, mut hi) = (0usize, n - 1);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if rd_u16(d, keys + 2 * mid) as f32 <= frame {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let fa = rd_u16(d, keys + 2 * lo) as f32;
    let fb = rd_u16(d, keys + 2 * hi) as f32;
    (lo, hi, (frame - fa) / (fb - fa))
}

#[inline]
fn vec_key(d: &[u8], vals: usize, n: usize, i: usize) -> Vec3 {
    let _ = n;
    let q = vals + 24 + 6 * i;
    Vec3::new(
        rd_f32(d, vals) + rd_u16(d, q) as f32 * rd_f32(d, vals + 12),
        rd_f32(d, vals + 4) + rd_u16(d, q + 2) as f32 * rd_f32(d, vals + 16),
        rd_f32(d, vals + 8) + rd_u16(d, q + 4) as f32 * rd_f32(d, vals + 20),
    )
}

#[inline]
fn quat_at(d: &[u8], at: usize) -> Quat {
    let s = 1.0 / ROT_SCALE;
    Quat::new(
        rd_i16(d, at) as f32 * s,
        rd_i16(d, at + 2) as f32 * s,
        rd_i16(d, at + 4) as f32 * s,
        rd_i16(d, at + 6) as f32 * s,
    )
}

/// Translation or scale of the track at `at` at frame position `frame`.
pub fn sample_vec(d: &[u8], at: usize, frame: f32) -> Vec3 {
    if rd_u8(d, at) == CONSTANT {
        return Vec3::new(rd_f32(d, at + 1), rd_f32(d, at + 5), rd_f32(d, at + 9));
    }
    let n = rd_u16(d, at + 1) as usize;
    let keys = at + 3;
    let vals = keys + 2 * n;
    let (a, b, k) = bracket(d, keys, n, frame);
    let va = vec_key(d, vals, n, a);
    if a == b {
        return va;
    }
    let vb = vec_key(d, vals, n, b);
    va + (vb - va) * k
}

/// Rotation of the track at `at` at frame position `frame` (normalized).
pub fn sample_quat(d: &[u8], at: usize, frame: f32) -> Quat {
    if rd_u8(d, at) == CONSTANT {
        return quat_at(d, at + 1).normalized();
    }
    let n = rd_u16(d, at + 1) as usize;
    let keys = at + 3;
    let vals = keys + 2 * n;
    let (a, b, k) = bracket(d, keys, n, frame);
    let qa = quat_at(d, vals + 8 * a);
    if a == b {
        return qa.normalized();
    }
    qa.nlerp(quat_at(d, vals + 8 * b), k)
}

/// Joint `(t, q, s)` from its three tracks at `at`, at frame position
/// `frame`.
pub fn sample_joint(d: &[u8], at: usize, frame: f32) -> (Vec3, Quat, Vec3) {
    let tl = track_len(d, at, Channel::Translation).unwrap_or(0);
    let rl = track_len(d, at + tl, Channel::Rotation).unwrap_or(0);
    (
        sample_vec(d, at, frame),
        sample_quat(d, at + tl, frame),
        sample_vec(d, at + tl + rl, frame),
    )
}

/// The byte offset of each joint's tracks in a clip body (`out[j]`, for
/// `joints` joints): computed once per pose so each joint is O(1).
pub fn joint_offsets(d: &[u8], joints: usize, out: &mut [u16; crate::render3d::MAX_JOINTS]) {
    let mut at = 0;
    for o in out.iter_mut().take(joints) {
        *o = at as u16;
        at += joint_len(d, at).unwrap_or(0);
    }
}

/// Clip `n`'s header in an `ANMC` clips area: (id, fps, frames, looping,
/// body offset, body length).
pub fn clip_header(d: &[u8], at: usize) -> (u32, f32, usize, bool, usize, usize) {
    (
        rd_u32(d, at),
        rd_f32(d, at + 4),
        rd_u32(d, at + 8) as usize,
        rd_u32(d, at + 12) & 1 != 0,
        at + CLIP_HEADER,
        rd_u32(d, at + 16) as usize,
    )
}
