//! 3DS platform services ([`trino_core::Platform`]): clock, debug-channel
//! log, RomFS blobs and an SD-card save file (framed by `trino_core::save`,
//! capped at `Caps::N3DS.save_bytes`).

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::Cell;

use trino_core::save::{self, HEADER};
use trino_core::{BlobId, Caps, Platform, SaveError};

use crate::{ffi, runtime};

pub struct N3dsPlatform {
    blobs: BTreeMap<u32, Vec<u8>>,
    last_us: Cell<u32>,
    high: Cell<u64>,
}

impl N3dsPlatform {
    pub fn new() -> Self {
        N3dsPlatform {
            blobs: BTreeMap::new(),
            last_us: Cell::new(runtime::now_us()),
            high: Cell::new(0),
        }
    }

    pub(crate) fn insert_blob(&mut self, id: u32, bytes: Vec<u8>) {
        self.blobs.insert(id, bytes);
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

    fn blob(&self, id: BlobId) -> Option<&[u8]> {
        self.blobs.get(&id.0).map(Vec::as_slice)
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
