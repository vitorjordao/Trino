//! Asset groups and memory (issue #56, ADR 0022): load a level's assets on
//! demand instead of every asset at boot, and see what memory they take.
//!
//! The manifest puts each asset in one or more groups (`groups = ["level1"]`;
//! default `["boot"]`, every earlier project's behaviour). The `boot` group
//! loads at boot and stays; any other group loads when the game asks
//! ([`crate::Platform::load_group`]) and frees its memory when it says so
//! ([`crate::Platform::unload_group`]). Handles never change: an asset is
//! `models/boar` whatever group holds it, and drawing one whose group is not
//! loaded draws nothing and counts [`crate::FrameStats::not_loaded`].
//!
//! Loading never blocks a frame. Each frame the backend spends up to
//! [`crate::Caps::load_budget_us`] reading and decoding the requested
//! groups, so a game can draw a loading screen from
//! [`LoadState::Loading`]'s `progress`. [`GroupTable`] is the request and
//! state bookkeeping every backend shares; the backends do the reading.

/// Handle of an asset group: the [`crate::asset::asset_id`] of
/// `groups/<name>`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct GroupId(pub u32);

impl GroupId {
    /// The group every asset is in unless the manifest says otherwise: loaded
    /// at boot, never unloaded.
    pub const BOOT: GroupId = GroupId::from_name("boot");

    /// `GroupId::from_name("level1")` (the manifest's `[groups.level1]`).
    pub const fn from_name(name: &str) -> GroupId {
        GroupId(crate::bundle::group_id(name))
    }

    /// From the logical path, `groups/level1`.
    pub const fn from_path(logical_path: &str) -> GroupId {
        GroupId(crate::asset::asset_id(logical_path))
    }
}

/// Where a group is ([`crate::Platform::load_group`] /
/// [`group_state`](crate::Platform::group_state)).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LoadState {
    /// Not resident (never loaded, unloaded, or cancelled).
    #[default]
    Unloaded,
    /// Being read; `progress` is the percent of its bytes done so far
    /// (0..=99, never decreasing while it loads).
    Loading { progress: u8 },
    /// Every asset of the group is resident and draws.
    Ready,
    /// The group is unknown to this build, or its bundle is corrupt or
    /// did not fit in memory. What it had loaded is freed again; an
    /// [`unload_group`](crate::Platform::unload_group) clears the state so a
    /// later load retries.
    Failed,
}

impl LoadState {
    pub const fn is_ready(self) -> bool {
        matches!(self, LoadState::Ready)
    }

    /// 0..=100 (100 = ready; a failed or unloaded group is 0).
    pub const fn percent(self) -> u8 {
        match self {
            LoadState::Loading { progress } => progress,
            LoadState::Ready => 100,
            _ => 0,
        }
    }

    /// `unloaded`, `loading`, `ready`, `failed` (logs, `--json`).
    pub const fn label(self) -> &'static str {
        match self {
            LoadState::Unloaded => "unloaded",
            LoadState::Loading { .. } => "loading",
            LoadState::Ready => "ready",
            LoadState::Failed => "failed",
        }
    }
}

/// How many groups a build can hold (boot included).
pub const MAX_GROUPS: usize = 32;

#[derive(Clone, Copy, Debug)]
struct Slot {
    id: GroupId,
    state: LoadState,
    /// What the group holds must be freed before anything else happens to
    /// it (an unload or a cancel the backend hasn't processed yet).
    free: bool,
    /// Request order: loads are served first come, first served.
    order: u32,
}

/// The groups a build knows and where each one is: what a backend's
/// `Platform` keeps for [`crate::Platform::load_group`] and its loader reads
/// every frame. No allocation (a fixed table of [`MAX_GROUPS`]).
#[derive(Clone, Debug)]
pub struct GroupTable {
    slots: [Slot; MAX_GROUPS],
    len: usize,
    next_order: u32,
}

impl Default for GroupTable {
    fn default() -> Self {
        GroupTable::new()
    }
}

impl GroupTable {
    /// A table holding only `boot` (ready).
    pub const fn new() -> GroupTable {
        let empty = Slot {
            id: GroupId(0),
            state: LoadState::Unloaded,
            free: false,
            order: 0,
        };
        let mut slots = [empty; MAX_GROUPS];
        slots[0] = Slot {
            id: GroupId::BOOT,
            state: LoadState::Ready,
            free: false,
            order: 0,
        };
        GroupTable {
            slots,
            len: 1,
            next_order: 1,
        }
    }

    /// Declare a group the build holds (its bundle exists). `false` when the
    /// table is full; a group already known is kept as it is.
    pub fn add(&mut self, id: GroupId) -> bool {
        if self.find(id).is_some() {
            return true;
        }
        if self.len == MAX_GROUPS {
            return false;
        }
        self.slots[self.len] = Slot {
            id,
            state: LoadState::Unloaded,
            free: false,
            order: 0,
        };
        self.len += 1;
        true
    }

    fn find(&self, id: GroupId) -> Option<usize> {
        self.slots[..self.len].iter().position(|s| s.id == id)
    }

    /// Does this build hold the group?
    pub fn knows(&self, id: GroupId) -> bool {
        self.find(id).is_some()
    }

    /// The game's [`crate::Platform::load_group`]: start loading an unloaded
    /// group, or report where it is. Unknown groups are [`LoadState::Failed`].
    pub fn request_load(&mut self, id: GroupId) -> LoadState {
        let Some(i) = self.find(id) else {
            return LoadState::Failed;
        };
        let s = &mut self.slots[i];
        if s.state == LoadState::Unloaded {
            s.state = LoadState::Loading { progress: 0 };
            s.order = self.next_order;
            self.next_order = self.next_order.wrapping_add(1);
        }
        s.state
    }

    /// The game's [`crate::Platform::unload_group`]: the group reads as
    /// unloaded at once (a load in progress is cancelled); the backend frees
    /// its memory before the next frame ([`take_free`](Self::take_free)).
    /// `boot` and unknown groups are left alone.
    pub fn request_unload(&mut self, id: GroupId) {
        if id == GroupId::BOOT {
            return;
        }
        if let Some(i) = self.find(id) {
            let s = &mut self.slots[i];
            if s.state != LoadState::Unloaded {
                s.state = LoadState::Unloaded;
                s.free = true;
            }
        }
    }

    /// Where `id` is (`Failed` when unknown).
    pub fn state(&self, id: GroupId) -> LoadState {
        self.find(id)
            .map_or(LoadState::Failed, |i| self.slots[i].state)
    }

    /// The loader: a group whose memory must be freed now (unloaded or
    /// cancelled since the last call). Free it before loading anything.
    pub fn take_free(&mut self) -> Option<GroupId> {
        let s = self.slots[..self.len].iter_mut().find(|s| s.free)?;
        s.free = false;
        Some(s.id)
    }

    /// The loader: the group to work on (the oldest load request), if any.
    pub fn current_load(&self) -> Option<GroupId> {
        self.slots[..self.len]
            .iter()
            .filter(|s| matches!(s.state, LoadState::Loading { .. }) && !s.free)
            .min_by_key(|s| s.order)
            .map(|s| s.id)
    }

    /// The loader: `done` of `total` bytes of `id` are read. Progress only
    /// moves forward and stays below 100 until [`finish`](Self::finish).
    pub fn set_progress(&mut self, id: GroupId, done: u64, total: u64) {
        if let Some(i) = self.find(id)
            && let LoadState::Loading { progress } = self.slots[i].state
        {
            // Nothing to read (`total` 0): as far as it goes before `finish`.
            let p = (done.min(total) * 100)
                .checked_div(total)
                .map_or(99, |p| p.min(99)) as u8;
            self.slots[i].state = LoadState::Loading {
                progress: progress.max(p),
            };
        }
    }

    /// The loader: `id` finished loading (`ok`) or failed. A failed group
    /// also gets its partial memory freed ([`take_free`](Self::take_free)).
    /// Ignored when the group is no longer loading (cancelled meanwhile).
    pub fn finish(&mut self, id: GroupId, ok: bool) {
        if let Some(i) = self.find(id)
            && matches!(self.slots[i].state, LoadState::Loading { .. })
        {
            let s = &mut self.slots[i];
            if ok {
                s.state = LoadState::Ready;
            } else {
                s.state = LoadState::Failed;
                s.free = true;
            }
        }
    }

    /// Every known group and its state (boot first).
    pub fn iter(&self) -> impl Iterator<Item = (GroupId, LoadState)> + '_ {
        self.slots[..self.len].iter().map(|s| (s.id, s.state))
    }

    /// Groups resident now (boot included).
    pub fn resident(&self) -> u16 {
        self.iter().filter(|(_, s)| s.is_ready()).count() as u16
    }

    /// Groups being loaded now.
    pub fn loading(&self) -> u16 {
        self.iter()
            .filter(|(_, s)| matches!(s, LoadState::Loading { .. }))
            .count() as u16
    }
}

/// What a backend's memory holds ([`crate::Platform::memory`]). Bytes; zeros
/// where a backend doesn't know.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct MemoryReport {
    /// The hardware's budget ([`crate::Caps::ram_bytes`]; `u32::MAX` = none).
    pub ram_bytes: u32,
    /// The heap in use now: the N64's and the 3DS's `malloc` (on the 3DS plus
    /// the linear heap textures, meshes and sounds live in); the PC's and
    /// the Web's allocator when the host counts it (`trino_core::mem::Counting`).
    pub heap_used: u32,
    /// The largest `heap_used` seen (sampled at every frame and every load
    /// step).
    pub heap_peak: u32,
    /// The heap's size (0 = unknown or unbounded).
    pub heap_total: u32,
    /// Resident assets, by category ([`crate::bundle::MemoryCategory`]).
    pub meshes: u32,
    pub textures: u32,
    pub audio: u32,
    /// Fonts' glyph tables, blobs, baked scenes, particle effects.
    pub data: u32,
    /// Groups resident (boot included) and being loaded.
    pub groups_resident: u16,
    pub groups_loading: u16,
}

impl MemoryReport {
    /// Bytes the resident assets take.
    pub const fn assets(&self) -> u32 {
        self.meshes
            .saturating_add(self.textures)
            .saturating_add(self.audio)
            .saturating_add(self.data)
    }

    /// The heap that is not assets: the engine's own buffers and the game's
    /// state.
    pub const fn game(&self) -> u32 {
        self.heap_used.saturating_sub(self.assets())
    }

    /// What is left of the heap (`u32::MAX` when its size is unknown).
    pub const fn heap_free(&self) -> u32 {
        if self.heap_total == 0 {
            u32::MAX
        } else {
            self.heap_total.saturating_sub(self.heap_used)
        }
    }

    /// Add `bytes` to `category`'s count (a backend registering an asset).
    pub fn add(&mut self, category: crate::bundle::MemoryCategory, bytes: u32) {
        let c = self.slot(category);
        *c = c.saturating_add(bytes);
    }

    /// Take `bytes` off `category`'s count (an asset freed).
    pub fn sub(&mut self, category: crate::bundle::MemoryCategory, bytes: u32) {
        let c = self.slot(category);
        *c = c.saturating_sub(bytes);
    }

    fn slot(&mut self, category: crate::bundle::MemoryCategory) -> &mut u32 {
        use crate::bundle::MemoryCategory as C;
        match category {
            C::Meshes => &mut self.meshes,
            C::Textures => &mut self.textures,
            C::Audio => &mut self.audio,
            C::Data => &mut self.data,
        }
    }

    /// The `heap=used/peak` field of a `TRINO_FRAME` line (bytes).
    pub fn heap_field(&self) -> HeapField {
        HeapField(self.heap_used, self.heap_peak)
    }
}

/// `heap=<used>/<peak>` ([`MemoryReport::heap_field`]).
#[derive(Clone, Copy, Debug)]
pub struct HeapField(pub u32, pub u32);

impl core::fmt::Display for HeapField {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "heap={}/{}", self.0, self.1)
    }
}

/// Follows the heap's peak between samples (backends sample at frame ends
/// and after each load step).
#[derive(Clone, Copy, Debug, Default)]
pub struct HeapMeter {
    peak: u32,
}

impl HeapMeter {
    /// Record `used` and return the peak so far.
    pub fn sample(&mut self, used: u32) -> u32 {
        self.peak = self.peak.max(used);
        self.peak
    }

    pub const fn peak(&self) -> u32 {
        self.peak
    }

    /// Start a new watermark (a test measuring one stretch).
    pub fn reset(&mut self, used: u32) {
        self.peak = used;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const L1: GroupId = GroupId::from_name("level1");
    const L2: GroupId = GroupId::from_name("level2");

    fn table() -> GroupTable {
        let mut t = GroupTable::new();
        assert!(t.add(L1) && t.add(L2));
        t
    }

    #[test]
    fn ids_are_the_logical_paths_handles() {
        assert_eq!(L1, GroupId::from_path("groups/level1"));
        assert_ne!(L1, L2);
        assert_eq!(GroupId::BOOT, GroupId::from_path("groups/boot"));
    }

    #[test]
    fn boot_is_ready_and_stays() {
        let mut t = table();
        assert_eq!(t.request_load(GroupId::BOOT), LoadState::Ready);
        t.request_unload(GroupId::BOOT);
        assert_eq!(t.state(GroupId::BOOT), LoadState::Ready);
        assert_eq!(t.take_free(), None);
    }

    #[test]
    fn unknown_groups_fail() {
        let mut t = table();
        let x = GroupId::from_name("nope");
        assert_eq!(t.request_load(x), LoadState::Failed);
        assert_eq!(t.state(x), LoadState::Failed);
    }

    #[test]
    fn a_load_progresses_monotonically_to_ready() {
        let mut t = table();
        assert_eq!(t.request_load(L1), LoadState::Loading { progress: 0 });
        assert_eq!(t.current_load(), Some(L1));
        let mut last = 0;
        for done in [10, 50, 30, 90, 100, 100] {
            t.set_progress(L1, done, 100);
            let p = t.state(L1).percent();
            assert!(p >= last && p <= 99, "{p} after {last}");
            last = p;
        }
        t.finish(L1, true);
        assert_eq!(t.request_load(L1), LoadState::Ready);
        assert_eq!(t.current_load(), None);
        assert_eq!(t.resident(), 2);
    }

    #[test]
    fn loads_are_served_in_request_order() {
        let mut t = table();
        t.request_load(L2);
        t.request_load(L1);
        assert_eq!(t.current_load(), Some(L2));
        t.finish(L2, true);
        assert_eq!(t.current_load(), Some(L1));
        assert_eq!(t.loading(), 1);
    }

    #[test]
    fn unload_cancels_a_load_and_frees_first() {
        let mut t = table();
        t.request_load(L1);
        t.set_progress(L1, 40, 100);
        t.request_unload(L1);
        assert_eq!(t.state(L1), LoadState::Unloaded);
        assert_eq!(t.current_load(), None);
        // A finish arriving for the cancelled load is ignored.
        t.finish(L1, true);
        assert_eq!(t.state(L1), LoadState::Unloaded);
        // Loading again before the backend freed: free comes first.
        assert_eq!(t.request_load(L1), LoadState::Loading { progress: 0 });
        assert_eq!(t.current_load(), None, "waits for the free");
        assert_eq!(t.take_free(), Some(L1));
        assert_eq!(t.take_free(), None);
        assert_eq!(t.current_load(), Some(L1));
    }

    #[test]
    fn a_failed_load_frees_and_retries_after_unload() {
        let mut t = table();
        t.request_load(L1);
        t.finish(L1, false);
        assert_eq!(t.state(L1), LoadState::Failed);
        assert_eq!(t.take_free(), Some(L1));
        assert_eq!(t.request_load(L1), LoadState::Failed, "no retry by itself");
        t.request_unload(L1);
        assert_eq!(t.request_load(L1), LoadState::Loading { progress: 0 });
    }

    #[test]
    fn the_table_is_bounded() {
        let mut t = GroupTable::new();
        for i in 1..MAX_GROUPS as u32 {
            assert!(t.add(GroupId(i)));
        }
        assert!(!t.add(GroupId(9999)));
        assert!(t.add(GroupId(1)), "known groups are fine");
    }

    #[test]
    fn memory_report_sums() {
        use crate::bundle::MemoryCategory as C;
        let mut m = MemoryReport {
            heap_used: 1000,
            heap_total: 4000,
            ..MemoryReport::default()
        };
        m.add(C::Meshes, 300);
        m.add(C::Textures, 200);
        m.sub(C::Meshes, 100);
        assert_eq!(m.assets(), 400);
        assert_eq!(m.game(), 600);
        assert_eq!(m.heap_free(), 3000);
        m.heap_peak = 1200;
        assert_eq!(format!("{}", m.heap_field()), "heap=1000/1200");
        let mut h = HeapMeter::default();
        h.sample(10);
        h.sample(5);
        assert_eq!(h.peak(), 10);
    }
}
