//! A store's services behind the game's `Platform` calls (issue #61): the
//! runner forwards what the game changed ([`trino_core::services`]) to a
//! [`StoreApi`] after every frame and brings the store's state in at start.
//! [`StoreSync`] is the logic, written against the trait so it is tested
//! with a mock ([`MockStore`]) and no Steam; `crate::steam` implements the
//! trait over `steamworks` behind the `steam` feature.
//!
//! - **Start:** the store's unlocked achievements and stats merge into the
//!   local ones (no popups); local unlocks the store lacks (earned offline)
//!   are sent up.
//! - **Each frame:** callbacks run; new unlocks, stats, presence and a
//!   reset go up; stats are stored (`StoreStats`) when something changed,
//!   at most once a second, and at once on a cloud hint or an unlock (so
//!   the store's popup shows now).
//! - **Overlay:** the store's overlay being open is published in
//!   `Platform::services_info().overlay_open`; games pause on it.
//! - **Input:** a store controller (Steam Input) is read into the frame's
//!   [`InputState`] by Trino button ([`StorePad`]); its origin names become
//!   the gamepad's binding labels.

use trino_core::services::{AchievementTable, ServiceBackend, ServicesInfo};
use trino_core::{AchievementState, Button, InputState, Vec2};
use trino_platform_pc::PcPlatform;

/// A controller as the store reads it, in Trino's terms.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StorePad {
    /// Bit per `Button` (its discriminant).
    pub buttons: u32,
    pub stick: Vec2,
    pub right_stick: Vec2,
    pub triggers: Vec2,
}

/// The store calls Trino uses (Steamworks' subset). Names are API names.
pub trait StoreApi {
    fn achievement(&self, api: &str) -> Option<bool>;
    fn set_achievement(&mut self, api: &str) -> bool;
    fn stat(&self, api: &str) -> Option<i32>;
    fn set_stat(&mut self, api: &str, value: i32) -> bool;
    fn store_stats(&mut self) -> bool;
    fn reset_all(&mut self) -> bool;
    fn set_presence(&mut self, key: &str, value: Option<&str>) -> bool;
    fn clear_presence(&mut self);
    fn run_callbacks(&mut self);
    fn overlay_open(&self) -> bool;
    fn cloud_enabled(&self) -> bool;
    /// The store's controller this frame, if one is connected through it.
    fn pad(&mut self) -> Option<StorePad> {
        None
    }
    /// What the store calls the control bound to `button` ("A Button",
    /// "Right Trigger"), for a remapping screen.
    fn button_label(&self, _button: Button) -> Option<String> {
        None
    }
}

/// What the runner holds: a [`StoreSync`] over any [`StoreApi`].
pub trait Store {
    /// After a frame (see [`StoreSync::pump`]).
    fn pump(&mut self, platform: &mut PcPlatform);
    /// Before a frame (see [`StoreSync::input`]).
    fn input(&mut self, input: &mut InputState, platform: &mut PcPlatform);
}

impl<A: StoreApi> Store for StoreSync<A> {
    fn pump(&mut self, platform: &mut PcPlatform) {
        StoreSync::pump(self, platform);
    }
    fn input(&mut self, input: &mut InputState, platform: &mut PcPlatform) {
        StoreSync::input(self, input, platform);
    }
}

/// The store this run talks to: Steam when built with `steam`, given an
/// app id (`trino-package.txt`'s `steam_app_id`, else `TRINO_STEAM_APP_ID`)
/// and launched where a Steam client answers; `None` otherwise (the local
/// file keeps the achievements). Says on stderr why Steam is off.
pub fn connect(
    app_id: Option<u32>,
    manifest: Option<&std::path::Path>,
    platform: &mut PcPlatform,
) -> Option<Box<dyn Store>> {
    let app_id = app_id.or_else(|| {
        std::env::var("TRINO_STEAM_APP_ID")
            .ok()
            .and_then(|v| v.parse().ok())
    })?;
    #[cfg(feature = "steam")]
    {
        match crate::steam::SteamStore::init(app_id, manifest) {
            Ok(api) => {
                eprintln!("[trino] Steam: connected as app {app_id}");
                Some(Box::new(StoreSync::start(api, platform)))
            }
            Err(e) => {
                eprintln!("[trino] Steam is off ({e}): achievements stay in the local file");
                None
            }
        }
    }
    #[cfg(not(feature = "steam"))]
    {
        let _ = (manifest, platform);
        eprintln!(
            "[trino] Steam app {app_id} named, but this build has no `steam` feature:              achievements stay in the local file"
        );
        None
    }
}

/// Frames between two `StoreStats` (one a second at 60 fps).
const STORE_INTERVAL: u32 = 60;

/// Keeps a platform's services and a store in step.
pub struct StoreSync<A: StoreApi> {
    pub api: A,
    dirty: bool,
    since_store: u32,
    /// Presence text the store has (`steam_display` and its arguments).
    presence_keys: Vec<String>,
}

impl<A: StoreApi> StoreSync<A> {
    /// Merge the store's state into `platform` and send up what it lacks.
    pub fn start(api: A, platform: &mut PcPlatform) -> StoreSync<A> {
        let mut s = StoreSync {
            api,
            dirty: false,
            since_store: STORE_INTERVAL,
            presence_keys: Vec::new(),
        };
        let table = AchievementTable::from_platform(platform).unwrap_or(AchievementTable::EMPTY);
        let mut theirs = AchievementState::EMPTY;
        for a in table.iter() {
            if s.api.achievement(a.api) == Some(true) {
                theirs.set_unlocked(a.index, true);
            }
        }
        for st in table.stats() {
            if let Some(v) = s.api.stat(st.api) {
                theirs.set_stat(st.index, v);
            }
        }
        let local = trino_core::Platform::achievements(platform);
        trino_core::Platform::restore_achievements(platform, &theirs);
        // What the restore sent back up is the store's own: drop it, then
        // send what only the local copy had.
        let _ = platform.take_store_events();
        for a in table.iter() {
            if local.is_unlocked(a.index) && !theirs.is_unlocked(a.index) {
                s.dirty |= s.api.set_achievement(a.api);
            }
        }
        for st in table.stats() {
            let v = trino_core::Platform::achievements(platform).stat(st.index);
            if s.api.stat(st.api) != Some(v) {
                s.dirty |= s.api.set_stat(st.api, v);
            }
        }
        let info = ServicesInfo {
            backend: ServiceBackend::Steam,
            store_toasts: true,
            cloud: s.api.cloud_enabled(),
            presence: true,
            overlay_open: false,
        };
        platform.set_services_info(info);
        s
    }

    /// After a frame: run callbacks, send the frame's changes, store.
    pub fn pump(&mut self, platform: &mut PcPlatform) {
        self.api.run_callbacks();
        let table = AchievementTable::from_platform(platform).unwrap_or(AchievementTable::EMPTY);
        let e = platform.take_store_events();
        let mut now = false;
        if e.reset {
            self.api.reset_all();
            now = true;
        }
        for a in table.iter() {
            if e.unlocked.is_unlocked(a.index) {
                self.dirty |= self.api.set_achievement(a.api);
                now = true;
            }
        }
        let state = trino_core::Platform::achievements(platform);
        for st in table.stats() {
            if e.stats & (1 << st.index) != 0 {
                self.dirty |= self.api.set_stat(st.api, state.stat(st.index));
            }
        }
        if let Some(p) = e.presence {
            self.presence(&table, &p);
        }
        if e.cloud_hint {
            now = true;
        }
        self.since_store = self.since_store.saturating_add(1);
        if (self.dirty || e.cloud_hint) && (now || self.since_store >= STORE_INTERVAL) {
            self.api.store_stats();
            self.dirty = false;
            self.since_store = 0;
        }
        let mut info = trino_core::Platform::services_info(platform);
        let open = self.api.overlay_open();
        if info.overlay_open != open {
            info.overlay_open = open;
            platform.set_services_info(info);
        }
    }

    /// `steam_display` = the key's token (`#presence_stage`), each argument
    /// as `%name%`'s value; no key clears it.
    fn presence(&mut self, table: &AchievementTable<'_>, p: &trino_core::Presence) {
        let Some(key) = p.key.and_then(|k| table.presence_key(k)) else {
            self.api.clear_presence();
            self.presence_keys.clear();
            return;
        };
        for (name, value) in p.args() {
            self.api.set_presence(name, Some(value));
        }
        let token = presence_token(key);
        self.api.set_presence("steam_display", Some(&token));
        self.presence_keys = p.args().map(|(n, _)| n.to_string()).collect();
    }

    /// Fold the store's controller into the frame's input; publish its
    /// labels for a remapping screen.
    pub fn input(&mut self, input: &mut InputState, platform: &mut PcPlatform) {
        let Some(pad) = self.api.pad() else {
            return;
        };
        merge_pad(input, &pad);
        for b in Button::ALL {
            if let Some(l) = self.api.button_label(b) {
                platform.set_gamepad_label(b, Some(l));
            }
        }
    }
}

/// `presence.stage` → `#presence_stage` (what `services export steam`
/// writes: `trino_scene::steam::presence_token`).
pub fn presence_token(key: &str) -> String {
    let mut t = String::from("#");
    t.extend(
        key.chars()
            .map(|c| if c == '.' || c == '-' { '_' } else { c }),
    );
    t
}

/// The store's buttons add to the keyboard's and the pads'; its sticks and
/// triggers win when they are pushed further.
pub fn merge_pad(input: &mut InputState, pad: &StorePad) {
    for b in Button::ALL {
        if pad.buttons & (1 << b as u32) != 0 {
            input.set(b, true);
        }
    }
    let further = |a: Vec2, b: Vec2| {
        if b.length_squared() > a.length_squared() {
            b
        } else {
            a
        }
    };
    input.stick = further(input.stick, pad.stick);
    input.right_stick = further(input.right_stick, pad.right_stick);
    input.triggers = Vec2::new(
        input.triggers.x.max(pad.triggers.x),
        input.triggers.y.max(pad.triggers.y),
    );
    if pad.buttons != 0 || pad.stick.length_squared() > 0.04 {
        input.pad = true;
    }
}

/// A store in memory, for tests: what it holds and every call made.
#[derive(Default, Debug)]
pub struct MockStore {
    pub achievements: std::collections::BTreeMap<String, bool>,
    pub stats: std::collections::BTreeMap<String, i32>,
    pub presence: std::collections::BTreeMap<String, String>,
    pub stores: u32,
    pub resets: u32,
    pub overlay: bool,
    pub callbacks: u32,
    pub pad: Option<StorePad>,
}

impl StoreApi for MockStore {
    fn achievement(&self, api: &str) -> Option<bool> {
        Some(self.achievements.get(api).copied().unwrap_or(false))
    }
    fn set_achievement(&mut self, api: &str) -> bool {
        self.achievements.insert(api.into(), true);
        true
    }
    fn stat(&self, api: &str) -> Option<i32> {
        self.stats.get(api).copied()
    }
    fn set_stat(&mut self, api: &str, value: i32) -> bool {
        self.stats.insert(api.into(), value);
        true
    }
    fn store_stats(&mut self) -> bool {
        self.stores += 1;
        true
    }
    fn reset_all(&mut self) -> bool {
        self.resets += 1;
        self.achievements.clear();
        self.stats.clear();
        true
    }
    fn set_presence(&mut self, key: &str, value: Option<&str>) -> bool {
        match value {
            Some(v) => self.presence.insert(key.into(), v.into()),
            None => self.presence.remove(key),
        };
        true
    }
    fn clear_presence(&mut self) {
        self.presence.clear();
    }
    fn run_callbacks(&mut self) {
        self.callbacks += 1;
    }
    fn overlay_open(&self) -> bool {
        self.overlay
    }
    fn cloud_enabled(&self) -> bool {
        true
    }
    fn pad(&mut self) -> Option<StorePad> {
        self.pad
    }
    fn button_label(&self, button: Button) -> Option<String> {
        self.pad.is_some().then(|| format!("Steam {:?}", button))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::services::table::{self, AchievementRecord, StatRecord};
    use trino_core::{AchievementId, Platform, StatId, StrKey};

    fn platform() -> PcPlatform {
        let a = |api, stat| AchievementRecord {
            api,
            name: "n",
            description: "d",
            icon: 0,
            hidden: false,
            stat,
        };
        let ach = [a("A", None), a("B", None), a("C", Some((0, 10)))];
        let stats = [StatRecord {
            api: "N",
            min: 0,
            max: 100,
            increment_only: true,
        }];
        let presence = ["presence.stage"];
        let mut blob = vec![0u8; table::encoded_len(&ach, &stats, &presence)];
        table::encode(&ach, &stats, &presence, &mut blob).unwrap();
        let mut p = PcPlatform::new(&trino_core::Caps::PC);
        p.insert_blob(table::BLOB, blob);
        p
    }

    #[test]
    fn start_merges_both_ways_without_popups() {
        let mut p = platform();
        p.unlock(AchievementId::new("A")); // earned offline
        p.next_unlock();
        let mut store = MockStore::default();
        store.achievements.insert("B".into(), true);
        store.stats.insert("N".into(), 4);
        let s = StoreSync::start(store, &mut p);
        assert!(p.is_unlocked(AchievementId::new("A")) && p.is_unlocked(AchievementId::new("B")));
        assert_eq!(p.stat(StatId::new("N")), 4);
        assert_eq!(
            p.next_unlock(),
            None,
            "the store's unlocks show no popup here"
        );
        assert_eq!(s.api.achievements.get("A"), Some(&true), "sent up");
        let info = p.services_info();
        assert_eq!(info.backend, ServiceBackend::Steam);
        assert!(info.store_toasts && info.cloud && info.presence);
    }

    #[test]
    fn pump_forwards_unlocks_stats_presence_reset_and_overlay() {
        let mut p = platform();
        let mut s = StoreSync::start(MockStore::default(), &mut p);
        let stores = s.api.stores;
        p.set_stat(StatId::new("N"), 12); // C at 10
        p.set_presence(
            StrKey::new("presence.stage"),
            &[trino_core::Arg::int("stage", 3)],
        );
        s.pump(&mut p);
        assert_eq!(s.api.achievements.get("C"), Some(&true));
        assert_eq!(s.api.stats.get("N"), Some(&12));
        assert_eq!(s.api.stores, stores + 1, "an unlock stores now");
        assert_eq!(
            s.api.presence.get("steam_display").map(String::as_str),
            Some("#presence_stage")
        );
        assert_eq!(s.api.presence.get("stage").map(String::as_str), Some("3"));
        // A stat alone waits for the interval.
        p.set_stat(StatId::new("N"), 13);
        s.pump(&mut p);
        assert_eq!(s.api.stores, stores + 1);
        for _ in 0..STORE_INTERVAL {
            s.pump(&mut p);
        }
        assert_eq!(s.api.stores, stores + 2);
        // The cloud hint stores at once.
        p.set_stat(StatId::new("N"), 14);
        p.cloud_sync_hint();
        s.pump(&mut p);
        assert_eq!(s.api.stores, stores + 3);
        // Reset and the overlay.
        p.reset_achievements();
        s.api.overlay = true;
        s.pump(&mut p);
        assert_eq!(s.api.resets, 1);
        assert!(p.services_info().overlay_open);
        s.api.overlay = false;
        s.pump(&mut p);
        assert!(!p.services_info().overlay_open);
        // Presence with an unknown key clears it.
        p.set_presence(StrKey::new("nope"), &[]);
        s.pump(&mut p);
        assert!(s.api.presence.is_empty());
    }

    #[test]
    fn a_store_pad_folds_into_the_frame() {
        let mut p = platform();
        let mut s = StoreSync::start(MockStore::default(), &mut p);
        let mut input = InputState::default();
        s.input(&mut input, &mut p);
        assert_eq!(input, InputState::default(), "no store pad: untouched");
        s.api.pad = Some(StorePad {
            buttons: 1 << Button::A as u32 | 1 << Button::DpadLeft as u32,
            stick: Vec2::new(0.0, 0.9),
            right_stick: Vec2::ZERO,
            triggers: Vec2::new(0.0, 1.0),
        });
        input.stick = Vec2::new(0.3, 0.0);
        s.input(&mut input, &mut p);
        assert!(input.is_down(Button::A) && input.is_down(Button::DpadLeft));
        assert_eq!(input.stick, Vec2::new(0.0, 0.9));
        assert_eq!(input.triggers.y, 1.0);
        assert!(input.pad);
        assert_eq!(
            p.binding_label(Button::A, trino_core::BindDevice::Gamepad),
            Some("Steam A")
        );
    }
}
