//! Platform services a shipped game talks to (issue #61, ADR 0029):
//! achievements, stats, rich presence and a cloud-save hint — one API on
//! every target.
//!
//! - **The table** (`assets/achievements.ron`, baked to the
//!   `services/achievements` blob, [`AchievementTable`]) names every
//!   achievement, stat and presence key. An achievement's bit is its place
//!   in the file.
//! - **The state** ([`AchievementState`]): one bit per achievement and an
//!   `i32` per stat. [`AchievementState::pack`] is what a console save
//!   holds: `3 + ⌈achievements / 8⌉ + 4 × stats` bytes (16 achievements and
//!   4 stats: 21 bytes of the N64's 2 KB EEPROM).
//! - **The hosts** hold a [`HostedServices`] and hand it to the
//!   [`Platform`] trait's defaults through
//!   [`Platform::services`]/[`Platform::services_mut`]:
//!   - the PC keeps the state in `<save>.achievements` (and, built with the
//!     `steam` feature on a Steam launch, mirrors it to Steam: its
//!     achievements, stats, Cloud and rich presence);
//!   - the Web in `localStorage`;
//!   - the N64 and the 3DS for the session: a game stores
//!     [`Platform::achievements`]`().pack()` in its own save and gives it
//!     back with [`Platform::restore_achievements`] after loading it (the
//!     same calls are harmless on the PC and the Web, which merge).
//!
//!   Presence and the cloud hint are documented no-ops off Steam.
//! - **The overlay** ([`AchievementToast`], [`AchievementList`]) draws an
//!   unlock popup and the list screen with the renderer's 2D calls, so
//!   every backend shows them; a store with its own popups (Steam) says so
//!   in [`ServicesInfo::store_toasts`].

mod overlay;
pub mod table;

pub use overlay::{AchievementList, AchievementToast, OverlayStyle};
pub use table::{AchievementInfo, AchievementTable, StatInfo, TableError};

use core::fmt::Write as _;

use crate::i18n::{Arg, StrKey, Value};
use crate::platform::Platform;
use crate::text::TextBuf;

/// Most achievements a table holds (16 bytes of bits).
pub const MAX_ACHIEVEMENTS: usize = 128;
/// Most stats a table holds.
pub const MAX_STATS: usize = 16;
/// Most arguments a presence string takes.
pub const MAX_PRESENCE_ARGS: usize = 4;
/// Newly unlocked achievements waiting for [`Platform::next_unlock`].
const TOAST_QUEUE: usize = 8;

/// An achievement: `asset_id` of its API name (`AchievementId::new("FIRST_COIN")`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct AchievementId(pub u32);

impl AchievementId {
    pub const fn new(api_name: &str) -> AchievementId {
        AchievementId(crate::asset::asset_id(api_name))
    }
}

/// A stat: `asset_id` of its API name.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct StatId(pub u32);

impl StatId {
    pub const fn new(api_name: &str) -> StatId {
        StatId(crate::asset::asset_id(api_name))
    }
}

/// Unlocked achievements (a bit each, by table index) and stat values.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AchievementState {
    bits: [u32; MAX_ACHIEVEMENTS / 32],
    stats: [i32; MAX_STATS],
}

/// Why [`AchievementState::unpack`] refused its bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PackError {
    Truncated,
    UnsupportedVersion(u8),
    TooMany,
}

/// The first byte of [`AchievementState::pack`].
const PACK_VERSION: u8 = 1;

impl AchievementState {
    pub const EMPTY: AchievementState = AchievementState {
        bits: [0; MAX_ACHIEVEMENTS / 32],
        stats: [0; MAX_STATS],
    };

    pub fn is_unlocked(&self, index: usize) -> bool {
        index < MAX_ACHIEVEMENTS && self.bits[index / 32] & (1 << (index % 32)) != 0
    }

    pub fn set_unlocked(&mut self, index: usize, on: bool) {
        if index >= MAX_ACHIEVEMENTS {
            return;
        }
        let bit = 1 << (index % 32);
        if on {
            self.bits[index / 32] |= bit;
        } else {
            self.bits[index / 32] &= !bit;
        }
    }

    pub fn unlocked_count(&self) -> usize {
        self.bits.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// The value of stat `index` (0 when out of range).
    pub fn stat(&self, index: usize) -> i32 {
        self.stats.get(index).copied().unwrap_or(0)
    }

    pub fn set_stat(&mut self, index: usize, value: i32) {
        if let Some(s) = self.stats.get_mut(index) {
            *s = value;
        }
    }

    /// Bytes [`pack`](Self::pack) writes for a table of `achievements` and
    /// `stats`: what a console save must budget.
    pub const fn packed_len(achievements: usize, stats: usize) -> usize {
        3 + achievements.div_ceil(8) + 4 * stats
    }

    /// The state as save bytes: version, counts, the bits, the stats
    /// (little-endian). Returns the length (0 if `out` is too small).
    pub fn pack(&self, achievements: usize, stats: usize, out: &mut [u8]) -> usize {
        let (na, ns) = (achievements.min(MAX_ACHIEVEMENTS), stats.min(MAX_STATS));
        let len = Self::packed_len(na, ns);
        if out.len() < len {
            return 0;
        }
        out[0] = PACK_VERSION;
        out[1] = na as u8;
        out[2] = ns as u8;
        let bytes = na.div_ceil(8);
        for i in 0..bytes {
            out[3 + i] = (self.bits[i / 4] >> (8 * (i % 4))) as u8;
        }
        // Bits past the table stay clear in the save.
        if na % 8 != 0 {
            out[2 + bytes] &= (1u8 << (na % 8)) - 1;
        }
        for i in 0..ns {
            let at = 3 + bytes + 4 * i;
            out[at..at + 4].copy_from_slice(&self.stats[i].to_le_bytes());
        }
        len
    }

    /// [`pack`](Self::pack) for the platform's table (0 bytes without one).
    pub fn pack_for(&self, table: &AchievementTable<'_>, out: &mut [u8]) -> usize {
        if table.is_empty() && table.stat_count() == 0 {
            return 0;
        }
        self.pack(table.len(), table.stat_count(), out)
    }

    /// Read [`pack`](Self::pack)'s bytes; returns the state and the bytes
    /// read.
    pub fn unpack(bytes: &[u8]) -> Result<(AchievementState, usize), PackError> {
        if bytes.len() < 3 {
            return Err(PackError::Truncated);
        }
        if bytes[0] != PACK_VERSION {
            return Err(PackError::UnsupportedVersion(bytes[0]));
        }
        let (na, ns) = (bytes[1] as usize, bytes[2] as usize);
        if na > MAX_ACHIEVEMENTS || ns > MAX_STATS {
            return Err(PackError::TooMany);
        }
        let len = Self::packed_len(na, ns);
        if bytes.len() < len {
            return Err(PackError::Truncated);
        }
        let mut s = AchievementState::EMPTY;
        let nbytes = na.div_ceil(8);
        for i in 0..nbytes {
            s.bits[i / 4] |= (bytes[3 + i] as u32) << (8 * (i % 4));
        }
        for i in 0..ns {
            let at = 3 + nbytes + 4 * i;
            s.stats[i] =
                i32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        }
        Ok((s, len))
    }
}

impl Default for AchievementState {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// Which service layer a platform runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ServiceBackend {
    /// No services (a platform that keeps nothing).
    None,
    /// Kept in memory for the session; the game's save holds them
    /// (the N64, the 3DS).
    Session,
    /// A local file or `localStorage` (the PC without a store, the Web).
    Local,
    /// Steam (the PC built with the `steam` feature, launched by Steam).
    Steam,
}

/// What the platform's services do ([`Platform::services_info`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ServicesInfo {
    pub backend: ServiceBackend,
    /// The store shows its own unlock popups: the game shouldn't draw an
    /// [`AchievementToast`] too.
    pub store_toasts: bool,
    /// Saves are synced by the store (Steam Cloud).
    pub cloud: bool,
    /// Rich presence reaches someone.
    pub presence: bool,
    /// The store's overlay is open now (Steam's Shift+Tab): pause the game.
    pub overlay_open: bool,
}

impl ServicesInfo {
    pub const NONE: ServicesInfo = ServicesInfo {
        backend: ServiceBackend::None,
        store_toasts: false,
        cloud: false,
        presence: false,
        overlay_open: false,
    };
}

/// May the game send its crash reports? The player's answer, kept by the
/// host (`<save>.crash` on the PC).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CrashConsent {
    /// Not asked yet: a game with an upload endpoint asks once.
    Ask,
    Allow,
    Deny,
}

impl CrashConsent {
    pub const fn key(self) -> &'static str {
        match self {
            CrashConsent::Ask => "ask",
            CrashConsent::Allow => "yes",
            CrashConsent::Deny => "no",
        }
    }

    pub fn parse(s: &str) -> Option<CrashConsent> {
        match s.trim() {
            "ask" => Some(CrashConsent::Ask),
            "yes" | "allow" => Some(CrashConsent::Allow),
            "no" | "deny" => Some(CrashConsent::Deny),
            _ => None,
        }
    }
}

/// Crash reporting as the host has it ([`Platform::crash_reports`]):
/// `None` on the consoles and the Web, which write no reports.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CrashReports {
    /// Reports from earlier runs not sent (or not to be sent).
    pub pending: u16,
    pub consent: CrashConsent,
    /// The package names an upload endpoint (`[pc] crash_upload`); without
    /// one reports stay on disk and there is nothing to ask.
    pub upload: bool,
}

impl CrashReports {
    /// Show the "send the report?" question now: reports wait, an endpoint
    /// takes them and the player hasn't answered.
    pub fn should_ask(&self) -> bool {
        self.pending > 0 && self.upload && self.consent == CrashConsent::Ask
    }
}

/// What a game shows its friends ([`Platform::set_presence`]): a
/// string-table key from the table's presence list and up to
/// [`MAX_PRESENCE_ARGS`] arguments, kept as text for the store.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Presence {
    pub key: Option<StrKey>,
    names: [TextBuf<16>; MAX_PRESENCE_ARGS],
    values: [TextBuf<32>; MAX_PRESENCE_ARGS],
    count: u8,
}

impl Presence {
    pub const NONE: Presence = Presence {
        key: None,
        names: [TextBuf::new(); MAX_PRESENCE_ARGS],
        values: [TextBuf::new(); MAX_PRESENCE_ARGS],
        count: 0,
    };

    /// `key` with `args` written as plain text (integers, fixed point with
    /// a dot, money in minor units, `YYYY-MM-DD` dates): the store formats
    /// them in its own localization.
    pub fn new(key: StrKey, args: &[Arg<'_>]) -> Presence {
        let mut p = Presence {
            key: Some(key),
            ..Presence::NONE
        };
        for a in args.iter().take(MAX_PRESENCE_ARGS) {
            let i = p.count as usize;
            let _ = p.names[i].write_str(a.name);
            let v = &mut p.values[i];
            let _ = match a.value {
                Value::Int(n) | Value::Money(n) => write!(v, "{n}"),
                Value::Dec { value, decimals } => {
                    let scale = 10i64.pow(decimals.min(9) as u32);
                    let sign = if value < 0 { "-" } else { "" };
                    let (whole, frac) = (value.abs() / scale, value.abs() % scale);
                    write!(v, "{sign}{whole}.{frac:0w$}", w = decimals as usize)
                }
                Value::Str(s) => v.write_str(s),
                Value::Date(d) => write!(v, "{:04}-{:02}-{:02}", d.year, d.month, d.day),
            };
            p.count += 1;
        }
        p
    }

    /// The arguments as `(name, text)`.
    pub fn args(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        (0..self.count as usize).map(|i| (self.names[i].as_str(), self.values[i].as_str()))
    }
}

/// What changed for the store since the host's last
/// [`HostedServices::take_store_events`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StoreEvents {
    /// Achievements unlocked since then (bits by table index).
    pub unlocked: AchievementState,
    /// Stats set since then (bit per index).
    pub stats: u16,
    pub presence: Option<Presence>,
    pub cloud_hint: bool,
    /// [`Platform::reset_achievements`] ran.
    pub reset: bool,
}

impl StoreEvents {
    pub fn is_empty(&self) -> bool {
        self.unlocked.unlocked_count() == 0
            && self.stats == 0
            && self.presence.is_none()
            && !self.cloud_hint
            && !self.reset
    }
}

/// The services' state a backend holds (every backend: PC, Web, N64, 3DS).
/// The game changes it through the [`Platform`] defaults; the host persists
/// it ([`take_change`](Self::take_change)) and forwards it to a store
/// ([`take_store_events`](Self::take_store_events)).
#[derive(Clone, Copy, Debug)]
pub struct HostedServices {
    state: AchievementState,
    info: ServicesInfo,
    pending: StoreEvents,
    presence: Presence,
    toasts: [u8; TOAST_QUEUE],
    toast_len: u8,
    changed: bool,
}

impl HostedServices {
    pub const fn new(backend: ServiceBackend) -> HostedServices {
        HostedServices {
            state: AchievementState::EMPTY,
            info: ServicesInfo {
                backend,
                ..ServicesInfo::NONE
            },
            pending: StoreEvents {
                unlocked: AchievementState::EMPTY,
                stats: 0,
                presence: None,
                cloud_hint: false,
                reset: false,
            },
            presence: Presence::NONE,
            toasts: [0; TOAST_QUEUE],
            toast_len: 0,
            changed: false,
        }
    }

    pub const fn state(&self) -> &AchievementState {
        &self.state
    }

    pub const fn info(&self) -> ServicesInfo {
        self.info
    }

    /// The host's store layer (Steam) took over: its backend and what it
    /// does.
    pub fn set_info(&mut self, info: ServicesInfo) {
        self.info = info;
    }

    pub const fn presence(&self) -> &Presence {
        &self.presence
    }

    /// The host's start-up state (a saved file, the store's): not a change
    /// to persist.
    pub fn load(&mut self, state: AchievementState) {
        self.state = state;
        self.changed = false;
    }

    /// Unlock `id` (a no-op when it is unknown or already unlocked).
    /// Returns whether it was newly unlocked.
    pub fn unlock(&mut self, table: &AchievementTable<'_>, id: AchievementId) -> bool {
        match table.index(id) {
            Some(i) => self.unlock_index(i),
            None => false,
        }
    }

    fn unlock_index(&mut self, i: usize) -> bool {
        if self.state.is_unlocked(i) || i >= MAX_ACHIEVEMENTS {
            return false;
        }
        self.state.set_unlocked(i, true);
        self.pending.unlocked.set_unlocked(i, true);
        if (self.toast_len as usize) < TOAST_QUEUE {
            self.toasts[self.toast_len as usize] = i as u8;
            self.toast_len += 1;
        }
        self.changed = true;
        true
    }

    /// Set a stat, clamped to its range (an increment-only stat refuses a
    /// lower value), and unlock the achievements whose threshold it
    /// reaches. Returns the value kept.
    pub fn set_stat(&mut self, table: &AchievementTable<'_>, id: StatId, value: i32) -> i32 {
        let Some(info) = table.stat_index(id).and_then(|i| table.stat(i)) else {
            return 0;
        };
        let old = self.state.stat(info.index);
        let mut v = crate::math::clamp_or_i32(value, info.min, info.max);
        if info.increment_only && v < old {
            v = old;
        }
        if v != old {
            self.state.set_stat(info.index, v);
            self.pending.stats |= 1 << info.index;
            self.changed = true;
        }
        self.unlock_thresholds(table);
        v
    }

    fn unlock_thresholds(&mut self, table: &AchievementTable<'_>) {
        for a in table.iter() {
            if let Some(s) = a.stat
                && self.state.stat(s) >= a.threshold
            {
                self.unlock_index(a.index);
            }
        }
    }

    /// Merge a saved state in (the game's console save, a local file next
    /// to the store's): unlocked bits add up, increment-only stats keep the
    /// higher value, the others take the saved one. Newly unlocked
    /// achievements reach the store but show no popup (the player already
    /// saw them).
    pub fn restore(&mut self, table: &AchievementTable<'_>, saved: &AchievementState) {
        for i in 0..table.len() {
            if saved.is_unlocked(i) && !self.state.is_unlocked(i) {
                self.state.set_unlocked(i, true);
                self.pending.unlocked.set_unlocked(i, true);
                self.changed = true;
            }
        }
        for s in table.stats() {
            let (now, then) = (self.state.stat(s.index), saved.stat(s.index));
            let v = if s.increment_only {
                now.max(then)
            } else {
                then
            };
            let v = crate::math::clamp_or_i32(v, s.min, s.max);
            if v != now {
                self.state.set_stat(s.index, v);
                self.pending.stats |= 1 << s.index;
                self.changed = true;
            }
        }
        let queued = self.toast_len;
        self.unlock_thresholds(table);
        self.toast_len = queued;
    }

    /// Lock everything and zero the stats (tests, the editor's "reset").
    pub fn reset(&mut self) {
        self.state = AchievementState::EMPTY;
        self.pending.unlocked = AchievementState::EMPTY;
        self.pending.stats = 0;
        self.pending.reset = true;
        self.toast_len = 0;
        self.changed = true;
    }

    pub fn set_presence(&mut self, presence: Presence) {
        if presence != self.presence {
            self.presence = presence;
            self.pending.presence = Some(presence);
        }
    }

    pub fn cloud_sync_hint(&mut self) {
        self.pending.cloud_hint = true;
    }

    /// The oldest unlock the game hasn't shown yet (its table index).
    pub fn next_unlock(&mut self) -> Option<usize> {
        if self.toast_len == 0 {
            return None;
        }
        let first = self.toasts[0] as usize;
        self.toasts.copy_within(1.., 0);
        self.toast_len -= 1;
        Some(first)
    }

    /// The state to persist, once per change.
    pub fn take_change(&mut self) -> Option<AchievementState> {
        core::mem::take(&mut self.changed).then_some(self.state)
    }

    /// What the store must hear about, once.
    pub fn take_store_events(&mut self) -> StoreEvents {
        let e = self.pending;
        self.pending = StoreEvents {
            unlocked: AchievementState::EMPTY,
            stats: 0,
            presence: None,
            cloud_hint: false,
            reset: false,
        };
        e
    }
}

/// The platform's table, or an empty one.
pub(crate) fn table_of(platform: &(impl Platform + ?Sized)) -> AchievementTable<'static> {
    platform
        .blob(table::BLOB)
        .and_then(|b| AchievementTable::parse(b).ok())
        .unwrap_or(AchievementTable::EMPTY)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(blob: &[u8]) -> AchievementTable<'_> {
        AchievementTable::parse(blob).unwrap()
    }

    #[test]
    fn pack_round_trips_within_the_eeprom_budget() {
        let mut s = AchievementState::EMPTY;
        s.set_unlocked(0, true);
        s.set_unlocked(9, true);
        s.set_unlocked(15, true);
        s.set_stat(0, 1234);
        s.set_stat(3, -7);
        let mut buf = [0u8; 64];
        let n = s.pack(16, 4, &mut buf);
        assert_eq!(n, 21, "16 achievements + 4 stats = 21 bytes");
        assert_eq!(n, AchievementState::packed_len(16, 4));
        let (back, read) = AchievementState::unpack(&buf[..n]).unwrap();
        assert_eq!((back, read), (s, 21));
        // The largest table still fits a 512 B EEPROM's slot many times.
        assert_eq!(AchievementState::packed_len(128, 16), 3 + 16 + 64);
        // Bits past the table never reach the save.
        let mut extra = s;
        extra.set_unlocked(20, true);
        extra.set_unlocked(5, true);
        let n = extra.pack(6, 0, &mut buf);
        let (back, _) = AchievementState::unpack(&buf[..n]).unwrap();
        assert!(back.is_unlocked(0) && back.is_unlocked(5) && !back.is_unlocked(20));
        assert_eq!(s.pack(16, 4, &mut [0u8; 8]), 0, "no room: nothing written");
        assert_eq!(
            AchievementState::unpack(&[9, 0, 0]),
            Err(PackError::UnsupportedVersion(9))
        );
        assert_eq!(
            AchievementState::unpack(&[1, 16, 4, 0]),
            Err(PackError::Truncated)
        );
    }

    #[test]
    fn unlock_stats_thresholds_and_toasts() {
        let blob = table::tests::fixture();
        let t = table(&blob);
        let mut h = HostedServices::new(ServiceBackend::Session);
        assert!(h.unlock(&t, AchievementId::new("FIRST_COIN")));
        assert!(!h.unlock(&t, AchievementId::new("FIRST_COIN")), "once");
        assert!(!h.unlock(&t, AchievementId::new("UNKNOWN")));
        assert_eq!(h.next_unlock(), Some(0));
        assert_eq!(h.next_unlock(), None);
        // COINS is increment-only, 0..=9999; COINS_100 unlocks at 100.
        assert_eq!(h.set_stat(&t, StatId::new("COINS"), 99), 99);
        assert!(!h.state().is_unlocked(2));
        assert_eq!(
            h.set_stat(&t, StatId::new("COINS"), 50),
            99,
            "no going back"
        );
        assert_eq!(h.set_stat(&t, StatId::new("COINS"), 120_000), 9999);
        assert!(h.state().is_unlocked(2));
        assert_eq!(h.next_unlock(), Some(2));
        assert_eq!(h.set_stat(&t, StatId::new("BEST_TIME"), 50), 50);
        assert_eq!(h.set_stat(&t, StatId::new("BEST_TIME"), 40), 40);
        let e = h.take_store_events();
        assert!(e.unlocked.is_unlocked(0) && e.unlocked.is_unlocked(2));
        assert_eq!(e.stats, 0b11);
        assert!(h.take_store_events().is_empty(), "once");
        assert!(h.take_change().is_some());
        assert!(h.take_change().is_none());
    }

    #[test]
    fn restore_merges_and_shows_no_popup() {
        let blob = table::tests::fixture();
        let t = table(&blob);
        let mut saved = AchievementState::EMPTY;
        saved.set_unlocked(1, true);
        saved.set_stat(0, 150);
        saved.set_stat(1, 7);
        let mut h = HostedServices::new(ServiceBackend::Local);
        h.unlock(&t, AchievementId::new("FIRST_COIN"));
        h.next_unlock();
        h.set_stat(&t, StatId::new("COINS"), 200);
        h.next_unlock();
        h.take_store_events();
        h.restore(&t, &saved);
        let s = h.state();
        assert!(s.is_unlocked(0) && s.is_unlocked(1) && s.is_unlocked(2));
        assert_eq!(s.stat(0), 200, "increment-only keeps the higher");
        assert_eq!(s.stat(1), 7, "the others take the saved value");
        assert_eq!(h.next_unlock(), None, "restored unlocks show no popup");
        let e = h.take_store_events();
        assert!(e.unlocked.is_unlocked(1) && !e.unlocked.is_unlocked(0));
        // A threshold reached by a restored stat unlocks quietly too.
        let mut fresh = HostedServices::new(ServiceBackend::Session);
        fresh.restore(&t, &saved);
        assert!(fresh.state().is_unlocked(2));
        assert_eq!(fresh.next_unlock(), None);
        fresh.reset();
        assert_eq!(fresh.state().unlocked_count(), 0);
        assert!(fresh.take_store_events().reset);
    }

    #[test]
    fn presence_keeps_arguments_as_text() {
        let p = Presence::new(
            StrKey::new("presence.stage"),
            &[
                Arg::int("stage", 3),
                Arg::dec("time", -1250, 2),
                Arg::str("name", "Castle"),
                Arg::date("day", 2026, 10, 3),
                Arg::int("ignored", 9),
            ],
        );
        let args: Vec<(&str, &str)> = p.args().collect();
        assert_eq!(
            args,
            [
                ("stage", "3"),
                ("time", "-12.50"),
                ("name", "Castle"),
                ("day", "2026-10-03")
            ]
        );
        let mut h = HostedServices::new(ServiceBackend::Local);
        h.set_presence(p);
        h.set_presence(p);
        assert_eq!(h.take_store_events().presence, Some(p));
        assert_eq!(
            h.take_store_events().presence,
            None,
            "unchanged: not resent"
        );
        h.cloud_sync_hint();
        assert!(h.take_store_events().cloud_hint);
    }
}
