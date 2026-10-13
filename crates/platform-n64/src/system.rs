//! N64 platform services ([`trino_core::Platform`]): clock, ISViewer log,
//! DFS blobs and cartridge-EEPROM saves (framed by `trino_core::save`).

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::Cell;

use trino_core::save::{self, HEADER};
use trino_core::{BlobId, Platform, SaveError};

use crate::{ffi, runtime};

pub struct N64Platform {
    /// Loaded at boot and never freed: `'static` (see `Platform::blob`).
    blobs: BTreeMap<u32, &'static [u8]>,
    /// The hardware tick counter is 32-bit µs (wraps every ~71 min);
    /// widen it to the trait's u64 by tracking wraps.
    last_us: Cell<u32>,
    high: Cell<u64>,
}

impl N64Platform {
    pub fn new() -> Self {
        N64Platform {
            blobs: BTreeMap::new(),
            last_us: Cell::new(runtime::now_us()),
            high: Cell::new(0),
        }
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

    fn save_capacity(&self) -> usize {
        self.eeprom_bytes().saturating_sub(HEADER)
    }

    fn save_read(&mut self, buf: &mut [u8]) -> Result<usize, SaveError> {
        let total = self.eeprom_bytes();
        if total <= HEADER {
            return Err(SaveError::Unsupported);
        }
        let mut header = [0u8; HEADER];
        unsafe { ffi::trino_save_read(header.as_mut_ptr(), 0, HEADER as u32) };
        let Some(len) = save::data_len(&header).filter(|&l| l <= total - HEADER) else {
            return Ok(0); // blank or foreign EEPROM: no save yet
        };
        let mut data = vec![0u8; len];
        unsafe { ffi::trino_save_read(data.as_mut_ptr(), HEADER as u32, len as u32) };
        if !save::verify(&header, &data) {
            return Ok(0); // torn write: treat as no save
        }
        let n = len.min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        Ok(len)
    }

    fn save_write(&mut self, data: &[u8]) -> Result<(), SaveError> {
        let capacity = self.save_capacity();
        if self.eeprom_bytes() <= HEADER {
            return Err(SaveError::Unsupported);
        }
        if data.len() > capacity {
            return Err(SaveError::TooLarge {
                len: data.len(),
                capacity,
            });
        }
        // Data first, header last: a power cut mid-write leaves a header
        // whose checksum no longer matches -> reads as "no save".
        unsafe {
            ffi::trino_save_write(data.as_ptr(), HEADER as u32, data.len() as u32);
            let header = save::header(data);
            ffi::trino_save_write(header.as_ptr(), 0, HEADER as u32);
        }
        Ok(())
    }
}
