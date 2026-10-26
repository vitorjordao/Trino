//! The Web's save slots (issue #68): `localStorage` keys `<key>.a` and
//! `<key>.b`, each a base64 slot file ([`trino_core::save`]'s slot files,
//! as on the PC and the 3DS). One `setItem` replaces a slot atomically (a
//! quota error leaves the old one), and the slot that holds the newest
//! intact save is never the one written. The bare `<key>` is the save from
//! before slot files (base64 of the raw bytes), read only when no slot is
//! intact. Browser-free, so host tests cover it with a map.

use trino_core::SaveError;
use trino_core::save::SlotStore;

/// A string store: `localStorage` in the browser, a map in tests.
pub trait KeyValue {
    fn get(&self, key: &str) -> Result<Option<String>, SaveError>;
    fn set(&mut self, key: &str, value: &str) -> Result<(), SaveError>;
}

/// The save slots under `key` in `kv`.
pub struct Slots<'a, K: KeyValue> {
    pub kv: &'a mut K,
    pub key: &'a str,
}

impl<K: KeyValue> Slots<'_, K> {
    pub fn slot_key(&self, slot: u8) -> String {
        format!("{}.{}", self.key, if slot == 0 { "a" } else { "b" })
    }

    fn read(&self, key: &str, buf: &mut [u8]) -> Result<Option<usize>, SaveError> {
        let Some(text) = self.kv.get(key)? else {
            return Ok(None);
        };
        // Text that isn't base64 is a damaged value: a file of no bytes
        // that are a save (the slot check refuses it).
        let bytes = crate::b64::decode(&text).unwrap_or_else(|| text.into_bytes());
        let n = bytes.len().min(buf.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(Some(bytes.len()))
    }
}

impl<K: KeyValue> SlotStore for Slots<'_, K> {
    fn read_slot(&mut self, slot: u8, buf: &mut [u8]) -> Result<Option<usize>, SaveError> {
        self.read(&self.slot_key(slot), buf)
    }

    fn write_slot(&mut self, slot: u8, bytes: &[u8]) -> Result<(), SaveError> {
        let key = self.slot_key(slot);
        self.kv.set(&key, &crate::b64::encode(bytes))
    }

    fn read_legacy(&mut self, buf: &mut [u8]) -> Result<Option<usize>, SaveError> {
        let Some(text) = self.kv.get(self.key)? else {
            return Ok(None);
        };
        // A damaged legacy value reads as no save, as it always did.
        let Some(bytes) = crate::b64::decode(&text) else {
            return Ok(None);
        };
        let n = bytes.len().min(buf.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(Some(bytes.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use trino_core::save::{self, slot_file_len};
    use trino_core::{SaveFault, SaveStatus};

    #[derive(Default)]
    struct Map(HashMap<String, String>);

    impl KeyValue for Map {
        fn get(&self, key: &str) -> Result<Option<String>, SaveError> {
            Ok(self.0.get(key).cloned())
        }
        fn set(&mut self, key: &str, value: &str) -> Result<(), SaveError> {
            self.0.insert(key.into(), value.into());
            Ok(())
        }
    }

    fn read(m: &mut Map) -> (Vec<u8>, SaveStatus) {
        let mut out = [0u8; 64];
        let mut scratch = [0u8; 80];
        let r = save::read_slot_files(
            &mut Slots {
                kv: m,
                key: "trino-save:t",
            },
            &mut out,
            &mut scratch,
        )
        .unwrap();
        (out[..r.len].to_vec(), r.status)
    }

    fn write(m: &mut Map, data: &[u8]) {
        let mut scratch = [0u8; 80];
        let mut file = vec![0u8; slot_file_len(data.len())];
        save::write_slot_files(
            &mut Slots {
                kv: m,
                key: "trino-save:t",
            },
            data,
            &mut scratch,
            &mut file,
        )
        .unwrap();
    }

    #[test]
    fn slots_alternate_recover_and_read_the_old_key() {
        let mut m = Map::default();
        assert_eq!(read(&mut m), (vec![], SaveStatus::Empty));
        // A save from before slot files.
        m.0.insert("trino-save:t".into(), crate::b64::encode(b"old"));
        assert_eq!(read(&mut m), (b"old".to_vec(), SaveStatus::Ok));
        write(&mut m, b"one");
        write(&mut m, b"two");
        assert!(m.0.contains_key("trino-save:t.a") && m.0.contains_key("trino-save:t.b"));
        assert_eq!(read(&mut m), (b"two".to_vec(), SaveStatus::Ok));
        let mut scratch = [0u8; 80];
        save::slot_files_fault(
            &mut Slots {
                kv: &mut m,
                key: "trino-save:t",
            },
            SaveFault::FlipNewest,
            &mut scratch,
        )
        .unwrap();
        assert_eq!(
            read(&mut m),
            (b"one".to_vec(), SaveStatus::RecoveredFromBackup)
        );
        // Not base64 at all: damaged too. With both slots damaged the old
        // key's save is what is left.
        m.0.insert("trino-save:t.a".into(), "%%%".into());
        assert_eq!(
            read(&mut m),
            (b"old".to_vec(), SaveStatus::RecoveredFromBackup)
        );
        m.0.remove("trino-save:t");
        assert_eq!(read(&mut m), (vec![], SaveStatus::Corrupted));
        // The next write repairs both slots.
        write(&mut m, b"three");
        assert_eq!(read(&mut m), (b"three".to_vec(), SaveStatus::Ok));
    }
}
