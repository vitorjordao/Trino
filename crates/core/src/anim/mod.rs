//! Animation beyond sampling one clip (issue #65, ADR 0034): what a clip
//! carries besides its joints — **events** on its timeline and the
//! **root motion** the bake took out of it — and, built on those, the
//! **animation graph** (state machines and blend spaces as data), **foot
//! IK** and the **compressed clip** codec.
//!
//! Everything here is `no_std`, allocation-free and deterministic (plain
//! f32 arithmetic and the core's own trig), so a graph driven by the same
//! parameters gives the same poses on PC, N64, 3DS and the Web. The output
//! of a graph is an [`AnimRef`](crate::AnimRef): every backend already
//! poses it, so none of this needs backend code.
//!
//! The byte views ([`EventTrack`], [`RootTrack`]) read the same records
//! from a baked model (TMDL `AEVT` / `ROOT`, for tools) and from a baked
//! graph (`TANG`, for games, which never parse a mesh at run time).

pub mod anmc;
mod clip;
pub mod graph;
pub mod ik;

pub use clip::{
    AnimEvent, ClipMeta, ClipTable, EVENT_BYTES, EventBuf, EventId, EventTrack, MAX_EVENTS,
    ROOT_BYTES, RootDelta, RootPose, RootTrack,
};
pub use graph::{
    AnimGraph, AnimGraphInstance, Contribution, FeetTrack, GraphBlob, GraphError, ParamId,
    ParamKind, StateId,
};
pub use ik::{FootIk, IkPose};

// Readers over blobs the parsers validated: out of range reads 0 instead of
// panicking, which keeps every read one branch (and no panic site) on the
// N64, where code is RAM.

#[inline]
pub(crate) fn rd_u32(d: &[u8], at: usize) -> u32 {
    match d.get(at..at.wrapping_add(4)) {
        Some(&[a, b, c, e]) => u32::from_le_bytes([a, b, c, e]),
        _ => 0,
    }
}

#[inline]
pub(crate) fn rd_f32(d: &[u8], at: usize) -> f32 {
    f32::from_bits(rd_u32(d, at))
}

#[inline]
pub(crate) fn rd_u16(d: &[u8], at: usize) -> u16 {
    match d.get(at..at.wrapping_add(2)) {
        Some(&[a, b]) => u16::from_le_bytes([a, b]),
        _ => 0,
    }
}

#[inline]
pub(crate) fn rd_u8(d: &[u8], at: usize) -> u8 {
    d.get(at).copied().unwrap_or(0)
}
