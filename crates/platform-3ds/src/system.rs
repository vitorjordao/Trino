//! 3DS platform services ([`trino_core::Platform`]): clock, debug-channel
//! log, RomFS blobs, asset groups (`crate::assets`, issue #56) and an
//! the game's SD-card save files (framed by `trino_core::save`, capped at
//! `Caps::N3DS.save_bytes`, in its own folder: `crate::savedir`).

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::Cell;

use alloc::string::String;

use trino_core::save;
use trino_core::{BlobId, Caps, GroupId, LoadState, MemoryReport, Platform, SaveError};

use crate::{ffi, runtime};

pub struct N3dsPlatform {
    /// Loaded at boot and never freed: `'static` (see `Platform::blob`).
    blobs: BTreeMap<u32, &'static [u8]>,
    last_us: Cell<u32>,
    high: Cell<u64>,
    /// The asset groups and their loader's state (`N3dsAssets::pump`).
    pub(crate) groups: crate::assets::Groups,
    /// Accessibility settings, for the session (`Platform::access`).
    access: trino_core::HostedAccess,
    /// Achievements and stats for the session (issue #61): the game's save
    /// holds them (`AchievementState::pack_for`, `restore_achievements`).
    services: trino_core::HostedServices,
    /// The one player (issue #67), published by the host before each update.
    pub players: trino_core::HostedPlayers,
    /// What the last save read found (`Platform::save_status`).
    save_status: trino_core::SaveStatus,
    /// The game's save folder (`crate::savedir::dir`, GitHub #119).
    save_dir: String,
}

impl N3dsPlatform {
    pub fn new() -> Self {
        N3dsPlatform {
            blobs: BTreeMap::new(),
            last_us: Cell::new(runtime::now_us()),
            high: Cell::new(0),
            groups: crate::assets::Groups::new(),
            access: trino_core::HostedAccess::new(),
            services: trino_core::HostedServices::new(trino_core::ServiceBackend::Session),
            players: trino_core::HostedPlayers::new(),
            save_status: trino_core::SaveStatus::Unknown,
            save_dir: init_save_dir(),
        }
    }

    /// The game's save folder on the SD card (`sdmc:/3ds/trino/<name>`).
    pub fn save_dir(&self) -> &str {
        &self.save_dir
    }

    /// Note the heap's use for `Platform::memory`'s peak (once a frame).
    pub fn sample_heap(&mut self) {
        self.groups.sample();
    }

    pub(crate) fn insert_blob(&mut self, id: u32, bytes: Vec<u8>) {
        self.blobs
            .insert(id, alloc::boxed::Box::leak(bytes.into_boxed_slice()));
    }
}

impl Default for N3dsPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform for N3dsPlatform {
    fn now_micros(&self) -> u64 {
        let now = runtime::now_us();
        if now < self.last_us.get() {
            self.high.set(self.high.get() + (1u64 << 32));
        }
        self.last_us.set(now);
        self.high.get() | now as u64
    }

    fn log(&self, message: &str) {
        runtime::log(message);
    }

    fn blob(&self, id: BlobId) -> Option<&'static [u8]> {
        self.blobs.get(&id.0).copied()
    }

    fn load_group(&mut self, group: GroupId) -> LoadState {
        self.groups.table.request_load(group)
    }

    fn unload_group(&mut self, group: GroupId) {
        self.groups.table.request_unload(group);
    }

    fn group_state(&self, group: GroupId) -> LoadState {
        self.groups.table.state(group)
    }

    fn group_blob(&self, id: BlobId) -> Option<&[u8]> {
        match self.groups.blobs.get(&id.0) {
            Some(b) => Some(b),
            None => self.blob(id),
        }
    }

    fn memory(&self) -> MemoryReport {
        self.groups.report(runtime::hw().caps().ram_bytes)
    }

    fn hw_tier(&self) -> trino_core::HwTier {
        runtime::hw_tier()
    }

    fn trace_platform(&self) -> trino_core::trace::TracePlatform {
        trino_core::trace::TracePlatform::N3ds
    }

    /// The player's accessibility settings for this session (issue #60):
    /// RAM only; a game keeps them across power cycles by storing
    /// `AccessSettings::pack` in its own save.
    fn access(&self) -> trino_core::AccessSettings {
        self.access.get()
    }

    fn set_access(&mut self, settings: trino_core::AccessSettings) {
        self.access.set(settings);
    }

    fn services(&self) -> Option<&trino_core::HostedServices> {
        Some(&self.services)
    }

    fn services_mut(&mut self) -> Option<&mut trino_core::HostedServices> {
        Some(&mut self.services)
    }

    fn hosted_players(&self) -> Option<&trino_core::HostedPlayers> {
        Some(&self.players)
    }

    fn hosted_players_mut(&mut self) -> Option<&mut trino_core::HostedPlayers> {
        Some(&mut self.players)
    }

    /// The system setting, read at boot: a bare language (`pt`, `en`).
    fn system_language(&self) -> Option<trino_core::LangTag> {
        runtime::system_language()
    }

    /// The 3DS's fixed preset (issue #55): `Low`, `Medium` on a New 3DS
    /// with the speedup, nothing scaled. `set_quality` stays the trait's
    /// no-op.
    fn quality(&self) -> trino_core::QualitySettings {
        trino_core::QualitySettings::console(&trino_core::Caps::N3DS.with_tier(runtime::hw_tier()))
    }

    fn second_screen(&self) -> Option<(u16, u16)> {
        Some((320, 240))
    }

    fn save_capacity(&self) -> usize {
        Caps::N3DS.save_bytes as usize - save::SLOT_HEADER
    }

    fn save_read(&mut self, buf: &mut [u8]) -> Result<usize, SaveError> {
        let mut scratch = vec![0u8; Caps::N3DS.save_bytes as usize];
        let r = save::read_slot_files(&mut crate::savedir::GameSlots(SdFiles), buf, &mut scratch)?;
        self.save_status = r.status;
        Ok(r.len)
    }

    fn save_write(&mut self, data: &[u8]) -> Result<(), SaveError> {
        let capacity = self.save_capacity();
        if data.len() > capacity {
            return Err(SaveError::TooLarge {
                len: data.len(),
                capacity,
            });
        }
        let mut scratch = vec![0u8; Caps::N3DS.save_bytes as usize];
        let mut file = vec![0u8; save::slot_file_len(data.len())];
        save::write_slot_files(
            &mut crate::savedir::GameSlots(SdFiles),
            data,
            &mut scratch,
            &mut file,
        )?;
        Ok(())
    }

    fn save_status(&self) -> trino_core::SaveStatus {
        self.save_status
    }

    fn save_fault(&mut self, fault: trino_core::SaveFault) -> Result<(), SaveError> {
        let mut scratch = vec![0u8; Caps::N3DS.save_bytes as usize];
        match save::slot_files_fault(&mut crate::savedir::GameSlots(SdFiles), fault, &mut scratch)?
        {
            true => Ok(()),
            false => Err(SaveError::Io),
        }
    }
}

/// Point the shim at the game's folder (the RomFS's save name, GitHub
/// #119) and log it (`TRINO_SAVE_PATH`, with whether the folder holds a
/// save yet).
fn init_save_dir() -> String {
    let name = crate::assets::read_text(&alloc::format!("romfs:{}", crate::savedir::NAME_FILE));
    let name = name
        .as_deref()
        .map_or(crate::savedir::DEFAULT_NAME, str::trim);
    let dir = crate::savedir::dir(name);
    if let Ok(c) = alloc::ffi::CString::new(dir.as_str()) {
        unsafe { ffi::trino_save_init(c.as_ptr()) }
    }
    let own = crate::savedir::GameSlots(SdFiles)
        .has_own()
        .unwrap_or(false);
    runtime::log(&alloc::format!("TRINO_SAVE_PATH {dir} own={}\n", own as u8));
    dir
}

/// The SD card's save files through the shim (`crate::savedir::File`).
struct SdFiles;

impl crate::savedir::Files for SdFiles {
    fn load(
        &mut self,
        file: crate::savedir::File,
        buf: &mut [u8],
    ) -> Result<Option<usize>, SaveError> {
        match unsafe { ffi::trino_save_load(file as u32, buf.as_mut_ptr(), buf.len() as u32) } {
            n if n < 0 => Err(SaveError::Io),
            0 => Ok(None),
            n => Ok(Some(n as usize)),
        }
    }

    fn store(&mut self, slot: u8, bytes: &[u8]) -> Result<(), SaveError> {
        match unsafe { ffi::trino_save_store(slot as u32, bytes.as_ptr(), bytes.len() as u32) } {
            0 => Ok(()),
            _ => Err(SaveError::Io),
        }
    }
}
