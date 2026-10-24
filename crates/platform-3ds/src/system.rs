//! 3DS platform services ([`trino_core::Platform`]): clock, debug-channel
//! log, RomFS blobs, asset groups (`crate::assets`, issue #56) and an
//! SD-card save file (framed by `trino_core::save`, capped at
//! `Caps::N3DS.save_bytes`).

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::Cell;

use trino_core::save::{self, HEADER};
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
        }
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
        Caps::N3DS.save_bytes as usize - HEADER
    }

    fn save_read(&mut self, buf: &mut [u8]) -> Result<usize, SaveError> {
        let mut file = vec![0u8; Caps::N3DS.save_bytes as usize];
        let size = unsafe { ffi::trino_save_load(file.as_mut_ptr(), file.len() as u32) };
        if size < 0 {
            return Err(SaveError::Io);
        }
        let size = (size as usize).min(file.len());
        if size < HEADER {
            return Ok(0);
        }
        let header: [u8; HEADER] = file[..HEADER].try_into().expect("HEADER bytes");
        let data = &file[HEADER..size];
        if !save::verify(&header, data) {
            return Ok(0); // foreign or torn file: no save
        }
        let n = data.len().min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        Ok(data.len())
    }

    fn save_write(&mut self, data: &[u8]) -> Result<(), SaveError> {
        let capacity = self.save_capacity();
        if data.len() > capacity {
            return Err(SaveError::TooLarge {
                len: data.len(),
                capacity,
            });
        }
        let mut file = Vec::with_capacity(HEADER + data.len());
        file.extend_from_slice(&save::header(data));
        file.extend_from_slice(data);
        match unsafe { ffi::trino_save_store(file.as_ptr(), file.len() as u32) } {
            0 => Ok(()),
            _ => Err(SaveError::Io),
        }
    }
}
