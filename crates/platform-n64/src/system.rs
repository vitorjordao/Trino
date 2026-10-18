//! N64 platform services ([`trino_core::Platform`]): clock, ISViewer log,
//! DFS blobs and cartridge-EEPROM saves (two slots, compressed, written
//! body-first and header-last by `trino_core::save`, so a power cut keeps
//! the previous save; `save_begin`/`save_step` spread the ~7.5 ms-per-block
//! writes over frames instead of freezing).

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::Cell;

use alloc::boxed::Box;
use trino_core::save::{self, Lz, Medium, SLOT_HEADER, WritePlan};
use trino_core::{BlobId, GroupId, LoadState, MemoryReport, Platform, SaveError};

use crate::{ffi, runtime};

pub struct N64Platform {
    /// Loaded at boot and never freed: `'static` (see `Platform::blob`).
    blobs: BTreeMap<u32, &'static [u8]>,
    /// The hardware tick counter is 32-bit µs (wraps every ~71 min);
    /// widen it to the trait's u64 by tracking wraps.
    last_us: Cell<u32>,
    high: Cell<u64>,
    /// A save being written over frames (`save_begin`).
    pending: Option<Box<Pending>>,
    lz: Option<Box<Lz>>,
    /// Detected at boot (`runtime::hw_tier`).
    tier: trino_core::HwTier,
    /// The asset groups and their loader's state (issue #56,
    /// `N64Assets::pump`).
    pub(crate) groups: crate::assets::Groups,
}

/// An unfinished save: the planned body, how much of it is on the EEPROM,
/// and whether the header (written last) is.
struct Pending {
    plan: WritePlan,
    body: Vec<u8>,
    written: usize,
}

/// The cartridge EEPROM as a [`Medium`].
struct Eeprom(usize);

impl Medium for Eeprom {
    fn size(&self) -> usize {
        self.0
    }
    fn read(&mut self, at: usize, buf: &mut [u8]) {
        unsafe { ffi::trino_save_read(buf.as_mut_ptr(), at as u32, buf.len() as u32) };
    }
    fn write(&mut self, at: usize, data: &[u8]) {
        unsafe { ffi::trino_save_write(data.as_ptr(), at as u32, data.len() as u32) };
    }
}

/// EEPROM block size: the unit `save_step` writes.
const BLOCK: usize = 8;

impl N64Platform {
    pub fn new() -> Self {
        N64Platform {
            blobs: BTreeMap::new(),
            last_us: Cell::new(runtime::now_us()),
            high: Cell::new(0),
            pending: None,
            lz: None,
            tier: runtime::hw_tier(),
            groups: crate::assets::Groups::new(),
        }
    }

    /// Note the heap's use for [`Platform::memory`]'s peak (the hosts call
    /// it at every frame's end).
    pub fn sample_heap(&mut self) {
        self.groups.sample();
    }

    fn plan(&mut self, data: &[u8]) -> Result<Box<Pending>, SaveError> {
        let total = self.eeprom_bytes();
        if total < 2 * SLOT_HEADER {
            return Err(SaveError::Unsupported);
        }
        let lz = self.lz.get_or_insert_with(Box::default);
        let mut body = vec![0u8; total];
        let mut scratch = vec![0u8; total];
        let plan = save::plan_write(&mut Eeprom(total), data, &mut body, &mut scratch, lz)?;
        Ok(Box::new(Pending {
            plan,
            body,
            written: 0,
        }))
    }

    /// Write the next block of `p` (body blocks, then the header's two).
    /// True when the header is on the EEPROM.
    fn write_block(p: &mut Pending, total: usize) -> bool {
        let mut e = Eeprom(total);
        let body = p.plan.body_len;
        if p.written < body {
            let n = BLOCK.min(body - p.written);
            let at = p.plan.offset + SLOT_HEADER + p.written;
            e.write(at, &p.body[p.written..p.written + n]);
            p.written += n;
            return false;
        }
        let h = p.written - body;
        if h < SLOT_HEADER {
            e.write(p.plan.offset + h, &p.plan.header[h..h + BLOCK]);
            p.written += BLOCK;
        }
        p.written >= body + SLOT_HEADER
    }

    pub(crate) fn insert_blob(&mut self, id: u32, bytes: Vec<u8>) {
        self.blobs
            .insert(id, alloc::boxed::Box::leak(bytes.into_boxed_slice()));
    }

    fn eeprom_bytes(&self) -> usize {
        unsafe { ffi::trino_save_capacity() as usize }
    }
}

impl Default for N64Platform {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform for N64Platform {
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
        let ram = crate::hw::caps_of_memory(runtime::memory_size()).ram_bytes;
        self.groups.report(ram)
    }

    fn hw_tier(&self) -> trino_core::HwTier {
        self.tier
    }

    fn save_capacity(&self) -> usize {
        save::capacity(self.eeprom_bytes())
    }

    fn save_read(&mut self, buf: &mut [u8]) -> Result<usize, SaveError> {
        let total = self.eeprom_bytes();
        if total < 2 * SLOT_HEADER {
            return Err(SaveError::Unsupported);
        }
        let mut scratch = vec![0u8; total];
        // Blank, foreign or torn both ways: 0 = no save yet.
        Ok(save::read_newest(&mut Eeprom(total), buf, &mut scratch))
    }

    fn save_write(&mut self, data: &[u8]) -> Result<(), SaveError> {
        self.pending = None;
        let total = self.eeprom_bytes();
        let mut p = self.plan(data)?;
        while !Self::write_block(&mut p, total) {}
        Ok(())
    }

    fn save_begin(&mut self, data: &[u8]) -> Result<(), SaveError> {
        // An unfinished save never got its header: dropping it is safe.
        self.pending = Some(self.plan(data)?);
        Ok(())
    }

    /// One EEPROM block per call, whatever the budget: libdragon's
    /// `eeprom_write` returns at once and the *next* write waits out the
    /// chip's write cycle (~7.5 ms in ares, up to 15 ms on hardware), so a
    /// second block in the same frame costs that whole wait. A frame later
    /// the cycle is over and a block costs one joybus transaction.
    fn save_step(&mut self, _budget_us: u32) -> Result<bool, SaveError> {
        let total = self.eeprom_bytes();
        let Some(p) = self.pending.as_mut() else {
            return Ok(true);
        };
        if Self::write_block(p, total) {
            self.pending = None;
            return Ok(true);
        }
        Ok(false)
    }
}
