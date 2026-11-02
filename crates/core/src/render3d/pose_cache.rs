//! What a layered pose computes once and keeps across frames (issue #136,
//! ADR 0039 §5 revisited): an inertialized transition's per-joint offsets
//! (`From − To`, frozen when the transition fired, so the same for every
//! frame it runs) and an additive clip's first frame (the reference its
//! motion is measured from).
//!
//! The renderer owns a [`PoseCache`] and fills it on its serial draw path
//! ([`PoseCache::prepare`], `&mut`); the mesh reads it
//! ([`Mesh::with_pose_cache`](super::Mesh::with_pose_cache)), so the PC's
//! worker threads, the N64's flush and the GPU batches pose from the same
//! entries. A miss — no cache, a full one, a path that never prepared —
//! samples the clips as before: the cache only stores what the layered
//! path computes anyway, with the same expressions, so a pose is bit for
//! bit the same with it or without it.
//!
//! The keys are the content (the mesh's clip data, the layer and its frozen
//! `From`/`To` samples; a clip id), never an instance: two characters in the
//! same transition share one entry, and nothing in `AnimRef` grows.

use super::{ClipSample, MAX_JOINTS, Mesh, Skeleton};
use crate::anim::{AnimPose, LayerMode, MAX_POSE_SAMPLES, SampleRole};
use crate::math::Vec3;
use crate::math3d::Quat;
use crate::render::AnimRef;

/// Entries a backend's cache holds (8 × ~1.3 KB on the heap, allocated by
/// the first layered pose that needs one; never on the base N64, whose
/// tier folds inertia and layers away).
pub const POSE_CACHE_ENTRIES: usize = 8;

/// `From`/`To` samples a key holds (three of each: `AnimGraphInstance`'s
/// snapshot).
const KEY_SAMPLES: usize = 6;

impl<S> PoseCache<S>
where
    S: AsRef<[PoseCacheEntry]> + AsMut<[PoseCacheEntry]> + Extend<PoseCacheEntry>,
{
    /// [`prepare`](Self::prepare), giving an empty storage its
    /// [`POSE_CACHE_ENTRIES`] the first time a layered pose is drawn (a
    /// game without layers, and the base N64, never allocate it).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn prepare_growing(&mut self, mesh: &Mesh, anim: Option<AnimRef>) {
        if mesh.layered(anim).is_none() {
            return;
        }
        if self.entries.as_ref().is_empty() {
            self.entries
                .extend((0..POSE_CACHE_ENTRIES).map(|_| PoseCacheEntry::EMPTY));
        }
        self.prepare(mesh, anim);
    }
}

/// A local transform.
pub(super) type Trs = (Vec3, Quat, Vec3);

const IDENTITY_TRS: Trs = (Vec3::ZERO, Quat::IDENTITY, Vec3::ZERO);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Empty,
    /// A layer's inertialized offsets.
    Inertial,
    /// A clip's first frame (an additive layer's reference).
    First,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Key {
    kind: Kind,
    /// The mesh's clip data (its address and length: one loaded blob).
    mesh: usize,
    len: usize,
    layer: u8,
    /// Inertial: the From/To samples (clip, time bits, weight bits), and
    /// which of them are `To` (a bit each). First: `data[0]` is the clip.
    n: u8,
    to: u8,
    data: [u32; KEY_SAMPLES * 3],
}

impl Key {
    const EMPTY: Key = Key {
        kind: Kind::Empty,
        mesh: 0,
        len: 0,
        layer: 0,
        n: 0,
        to: 0,
        data: [0; KEY_SAMPLES * 3],
    };

    fn of_mesh(kind: Kind, skel: &Skeleton) -> Key {
        Key {
            kind,
            mesh: skel.clips.as_ptr() as usize,
            len: skel.clips.len(),
            ..Key::EMPTY
        }
    }

    /// Layer `l`'s inertial key (`None`: it has no From/To samples, or
    /// more than a key holds).
    fn inertial(skel: &Skeleton, pose: &AnimPose, l: usize) -> Option<Key> {
        let mut k = Key::of_mesh(Kind::Inertial, skel);
        k.layer = l as u8;
        for s in pose.samples() {
            if s.layer as usize != l || s.role == SampleRole::Pose {
                continue;
            }
            let n = k.n as usize;
            if n >= KEY_SAMPLES {
                return None;
            }
            k.data[n * 3] = s.clip.0;
            k.data[n * 3 + 1] = s.time.to_bits();
            k.data[n * 3 + 2] = s.weight.to_bits();
            if s.role == SampleRole::To {
                k.to |= 1 << n;
            }
            k.n += 1;
        }
        (k.n > 0).then_some(k)
    }

    fn first(skel: &Skeleton, clip: u32) -> Key {
        let mut k = Key::of_mesh(Kind::First, skel);
        k.data[0] = clip;
        k
    }
}

/// One cached table: a transform per joint, and which joints have one.
#[derive(Clone, Copy, Debug)]
pub struct PoseCacheEntry {
    key: Key,
    /// The frame it was last used in.
    last: u32,
    /// Joints with a value (bit `j`).
    has: u32,
    trs: [Trs; MAX_JOINTS],
}

impl PoseCacheEntry {
    pub const EMPTY: PoseCacheEntry = PoseCacheEntry {
        key: Key::EMPTY,
        last: 0,
        has: 0,
        trs: [IDENTITY_TRS; MAX_JOINTS],
    };
}

/// A renderer's cache of layered-pose work that outlives a frame (see the
/// module docs). `S` is its storage (`Vec<PoseCacheEntry>` in the
/// backends: empty until a pose needs it).
#[derive(Debug)]
pub struct PoseCache<S> {
    entries: S,
    frame: u32,
    /// Lookups [`prepare`](Self::prepare) found / computed (tests, stats).
    pub hits: u32,
    pub misses: u32,
}

impl<S: AsRef<[PoseCacheEntry]> + AsMut<[PoseCacheEntry]>> PoseCache<S> {
    pub const fn new(entries: S) -> Self {
        PoseCache {
            entries,
            frame: 0,
            hits: 0,
            misses: 0,
        }
    }

    /// The entries a mesh reads ([`Mesh::with_pose_cache`]).
    pub fn entries(&self) -> &[PoseCacheEntry] {
        self.entries.as_ref()
    }

    /// The storage (a backend sizes it on first use).
    pub fn storage_mut(&mut self) -> &mut S {
        &mut self.entries
    }

    /// A new frame: entries not used in the last two frames may be
    /// replaced.
    pub fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    /// Forget everything (a mesh was uploaded or freed: its address may be
    /// reused by other data).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn clear(&mut self) {
        for e in self.entries.as_mut() {
            e.key = Key::EMPTY;
        }
    }

    /// Fill what drawing `mesh` with `anim` will read: its layers'
    /// inertial offsets and its additive clips' first frames. Cheap when
    /// the pose has neither (no table slot, no inertia, no additive layer).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn prepare(&mut self, mesh: &Mesh, anim: Option<AnimRef>) {
        let (Some(skel), Some(pose)) = (mesh.skeleton, mesh.layered(anim)) else {
            return;
        };
        if self.entries.as_ref().is_empty() {
            return;
        }
        let layers = pose.layers();
        let wants = |p: &AnimPose| {
            p.samples().iter().any(|s| {
                s.role != SampleRole::Pose
                    || layers
                        .get(s.layer as usize)
                        .is_some_and(|l| s.layer > 0 && l.mode == LayerMode::Additive)
            })
        };
        if !wants(pose) {
            return;
        }
        // Hits first: locating the samples (and decoding their tracks) is
        // what the draw does anyway, so only a miss pays for it here.
        let additive = |s: &crate::anim::PoseSample| {
            s.layer > 0
                && layers
                    .get(s.layer as usize)
                    .is_some_and(|l| l.mode == LayerMode::Additive)
        };
        let mut missed = false;
        for (l, layer) in layers.iter().enumerate() {
            if layer.decay > 0.0
                && let Some(key) = Key::inertial(&skel, pose, l)
            {
                missed |= !self.touch(&key);
            }
        }
        for s in pose.samples().iter().filter(|s| additive(s)) {
            missed |= !self.touch(&Key::first(&skel, s.clip.0));
        }
        if !missed {
            return;
        }
        let at = Located::of(&skel, pose);
        for (l, layer) in layers.iter().enumerate() {
            if layer.decay > 0.0
                && let Some(key) = Key::inertial(&skel, pose, l)
                && !self.has(&key)
            {
                let samples = pose.samples();
                self.insert(
                    key,
                    |j| {
                        let f = Mesh::blend_role(
                            &skel,
                            samples,
                            &at.loc,
                            &at.offs,
                            None,
                            j,
                            l,
                            SampleRole::From,
                        )?;
                        let t = Mesh::blend_role(
                            &skel,
                            samples,
                            &at.loc,
                            &at.offs,
                            None,
                            j,
                            l,
                            SampleRole::To,
                        )?;
                        Some((f.0 - t.0, f.1 * t.1.conjugate(), f.2 - t.2))
                    },
                    skel.joint_count,
                );
            }
        }
        for (i, s) in pose.samples().iter().enumerate() {
            if !at.refs.has(i) {
                continue;
            }
            let r = at.refs.at[i];
            let key = Key::first(&skel, s.clip.0);
            if !self.has(&key) {
                let offs = &at.offs[i];
                self.insert(
                    key,
                    |j| Some(Mesh::sample_any(&skel, r, j, offs)),
                    skel.joint_count,
                );
            }
        }
    }

    /// Whether `key` is stored.
    fn has(&self, key: &Key) -> bool {
        self.entries.as_ref().iter().any(|e| e.key == *key)
    }

    /// Every stored value moved (a test proving the mesh reads them).
    #[cfg(test)]
    pub(super) fn poison(&mut self) {
        for e in self.entries.as_mut() {
            for v in e.trs.iter_mut() {
                v.0 += Vec3::new(9.0, 9.0, 9.0);
            }
        }
    }

    /// Mark `key` used this frame; whether it is cached.
    fn touch(&mut self, key: &Key) -> bool {
        let frame = self.frame;
        for e in self.entries.as_mut() {
            if e.key == *key {
                e.last = frame;
                self.hits += 1;
                return true;
            }
        }
        false
    }

    /// Compute and store an entry where one is free or stale (unused for
    /// two frames); none is: skip (the mesh samples it per draw).
    fn insert(&mut self, key: Key, mut value: impl FnMut(usize) -> Option<Trs>, joints: usize) {
        let frame = self.frame;
        let Some(e) = self
            .entries
            .as_mut()
            .iter_mut()
            .find(|e| e.key.kind == Kind::Empty || frame.wrapping_sub(e.last) >= 2)
        else {
            return;
        };
        self.misses += 1;
        e.key = key;
        e.last = frame;
        e.has = 0;
        for j in 0..joints.min(MAX_JOINTS) {
            if let Some(v) = value(j) {
                e.trs[j] = v;
                e.has |= 1 << j;
            }
        }
    }
}

/// Located clip samples, one per pose sample, with a bit for each that
/// was found: never an `Option<ClipSample>`, whose unset payload LLVM may
/// feed to the FPU on the N64 ([`ClipSample::NONE`]).
#[derive(Clone, Copy)]
pub(super) struct Samples {
    pub at: [ClipSample; MAX_POSE_SAMPLES],
    pub found: u16,
}

impl Samples {
    const NONE: Samples = Samples {
        at: [ClipSample::NONE; MAX_POSE_SAMPLES],
        found: 0,
    };

    #[inline]
    pub fn has(&self, i: usize) -> bool {
        self.found & (1 << i) != 0
    }

    fn set(&mut self, i: usize, c: Option<ClipSample>) {
        if let Some(c) = c {
            self.at[i] = c;
            self.found |= 1 << i;
        }
    }
}

/// A pose's samples located in a mesh: each sample's clip frames, an
/// additive sample's reference (its clip's first frame), the compressed
/// track offsets (once per clip, shared by every sample of it).
pub(super) struct Located {
    pub loc: Samples,
    pub refs: Samples,
    pub offs: [[u16; MAX_JOINTS]; MAX_POSE_SAMPLES],
}

impl Located {
    #[inline(never)]
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn of(skel: &Skeleton, pose: &AnimPose) -> Located {
        let mut r = Located {
            loc: Samples::NONE,
            refs: Samples::NONE,
            offs: [[0u16; MAX_JOINTS]; MAX_POSE_SAMPLES],
        };
        let layers = pose.layers();
        for (i, s) in pose.samples().iter().enumerate() {
            let Some(layer) = layers.get(s.layer as usize) else {
                continue;
            };
            r.loc.set(i, Mesh::locate(skel, s.clip, s.time));
            if layer.mode == LayerMode::Additive && s.layer > 0 {
                r.refs.set(i, Mesh::locate(skel, s.clip, 0.0));
            }
            if skel.compressed {
                // A clip's track offsets don't depend on the time: decode
                // them once per clip (a `To` sample is usually a `Pose`
                // clip, an additive reference its own clip).
                if r.loc.has(i) {
                    let at = r.loc.at[i].at;
                    match (0..i).find(|&k| r.loc.has(k) && r.loc.at[k].at == at) {
                        Some(k) => r.offs[i] = r.offs[k],
                        None => Mesh::offsets_of(skel, Some(r.loc.at[i]), &mut r.offs[i]),
                    }
                }
            }
        }
        r
    }
}

/// What a draw found in the cache for one pose.
pub(super) struct Cached<'a> {
    /// Each layer's inertial offsets.
    pub inertial: [Option<&'a PoseCacheEntry>; crate::anim::MAX_POSE_LAYERS],
    /// Each additive sample's reference frame.
    pub first: [Option<&'a PoseCacheEntry>; MAX_POSE_SAMPLES],
}

impl<'a> Cached<'a> {
    pub const NONE: Cached<'static> = Cached {
        inertial: [None; crate::anim::MAX_POSE_LAYERS],
        first: [None; MAX_POSE_SAMPLES],
    };

    #[inline(never)]
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn find(skel: &Skeleton<'a>, pose: &AnimPose, at: &Located) -> Cached<'a> {
        let mut c = Cached::NONE;
        let entries = skel.cache;
        if entries.is_empty() {
            return c;
        }
        let get = |key: &Key| entries.iter().find(|e| e.key == *key);
        for (l, layer) in pose.layers().iter().enumerate() {
            if layer.decay > 0.0
                && let Some(key) = Key::inertial(skel, pose, l)
            {
                c.inertial[l] = get(&key);
            }
        }
        for (i, s) in pose.samples().iter().enumerate() {
            if at.refs.has(i) {
                c.first[i] = get(&Key::first(skel, s.clip.0));
            }
        }
        c
    }
}

impl PoseCacheEntry {
    /// Joint `j`'s value (`None`: the joint had none when it was filled).
    #[inline]
    pub(super) fn get(&self, j: usize) -> Option<Trs> {
        (j < MAX_JOINTS && self.has & (1 << j) != 0).then(|| self.trs[j])
    }
}
