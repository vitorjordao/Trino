//! Each game's save folder on the SD card (GitHub #119).
//!
//! A game keeps its A/B slot files (issue #68) in `sdmc:/3ds/trino/<name>/`,
//! `<name>` being the project's save name (`trino.toml` `[3ds] save_name`,
//! else `[project] name`; the engine's castle64 is `castle64`, as on the
//! PC and the Web). `cargo xtask build 3ds` puts it in the RomFS
//! ([`NAME_FILE`]), a self-test build with `-selftest` appended, so a test
//! run never touches the player's save.
//!
//! Before #119 every Trino game shared `sdmc:/3ds/trino/save.{a,b}` (and
//! `save.bin` before the slots). A game whose own folder holds no slot file
//! at all reads that shared save once, as the save from before its folder
//! existed, and never writes it: its next save goes to its own folder. A
//! folder with a damaged slot doesn't migrate (that is the game's own save,
//! reported `Corrupted`), so one game's damaged save never loads another
//! game's.
//!
//! The decisions here are plain Rust over [`Files`], tested on the host;
//! the shim (`trino_save_init`, `trino_save_load`, `trino_save_store`) only
//! opens the files.

use alloc::format;
use alloc::string::String;
use alloc::vec;

use trino_core::save::{self, HEADER, SlotStore};
use trino_core::{Caps, SaveError};

/// Where every game's folder lives.
pub const ROOT: &str = "sdmc:/3ds/trino";
/// The RomFS file holding the save name (romfs-relative).
pub const NAME_FILE: &str = "/save_name";
/// A build without [`NAME_FILE`] (an old xtask) still gets its own folder.
pub const DEFAULT_NAME: &str = "game";
/// Longest folder name kept.
pub const MAX_NAME: usize = 32;

/// `name` as a folder name: lowercase ASCII letters and digits, `-`, `_`
/// and `.`; anything else becomes `_`, at most [`MAX_NAME`] bytes, never
/// empty, `.` or `..`.
pub fn folder(name: &str) -> String {
    let mut out: String = name
        .trim()
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9' | '-' | '_' | '.') => c,
            _ => '_',
        })
        .take(MAX_NAME)
        .collect();
    if out.is_empty() || out.chars().all(|c| c == '.') {
        out = String::from(DEFAULT_NAME);
    }
    out
}

/// The game's save folder: `sdmc:/3ds/trino/<folder(name)>`.
pub fn dir(name: &str) -> String {
    format!("{ROOT}/{}", folder(name))
}

/// The files the shim opens, by the number it takes (`trino_save_load`'s
/// `slot`): the game's own slots, then the shared ones from before #119.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum File {
    /// `<dir>/save.a`.
    OwnA = 0,
    /// `<dir>/save.b`.
    OwnB = 1,
    /// `sdmc:/3ds/trino/save.bin`: the `"TS"`-framed save from before #68.
    SharedBin = 2,
    /// `sdmc:/3ds/trino/save.a`: the shared slots between #68 and #119.
    SharedA = 3,
    /// `sdmc:/3ds/trino/save.b`.
    SharedB = 4,
}

/// The SD card as the slots see it.
pub trait Files {
    /// Read `file` into `buf`: `Ok(None)` when it doesn't exist, else its
    /// full length (only `buf.len()` bytes are copied; an empty `buf` asks
    /// whether it exists).
    fn load(&mut self, file: File, buf: &mut [u8]) -> Result<Option<usize>, SaveError>;
    /// Replace the game's own slot `slot` (0 = A, 1 = B).
    fn store(&mut self, slot: u8, bytes: &[u8]) -> Result<(), SaveError>;
}

/// A game's slot files ([`SlotStore`] for `trino_core::save::read_slot_files`):
/// its own folder, and the shared save once while that folder is empty.
pub struct GameSlots<F>(pub F);

impl<F: Files> GameSlots<F> {
    /// Whether the game's own folder holds a slot file (intact or not).
    pub fn has_own(&mut self) -> Result<bool, SaveError> {
        Ok(self.0.load(File::OwnA, &mut [])?.is_some()
            || self.0.load(File::OwnB, &mut [])?.is_some())
    }
}

impl<F: Files> SlotStore for GameSlots<F> {
    fn read_slot(&mut self, slot: u8, buf: &mut [u8]) -> Result<Option<usize>, SaveError> {
        let file = if slot == 0 { File::OwnA } else { File::OwnB };
        self.0.load(file, buf)
    }

    fn write_slot(&mut self, slot: u8, bytes: &[u8]) -> Result<(), SaveError> {
        self.0.store(slot, bytes)
    }

    /// The shared save, only while the game's folder holds no slot file.
    fn read_legacy(&mut self, buf: &mut [u8]) -> Result<Option<usize>, SaveError> {
        if self.has_own()? {
            return Ok(None);
        }
        let mut scratch = vec![0u8; Caps::N3DS.save_bytes as usize];
        let r = save::read_slot_files(&mut Shared(&mut self.0), buf, &mut scratch)?;
        Ok((r.len > 0).then_some(r.len))
    }
}

/// The shared folder from before #119, read-only.
struct Shared<'a, F>(&'a mut F);

impl<F: Files> SlotStore for Shared<'_, F> {
    fn read_slot(&mut self, slot: u8, buf: &mut [u8]) -> Result<Option<usize>, SaveError> {
        let file = if slot == 0 {
            File::SharedA
        } else {
            File::SharedB
        };
        self.0.load(file, buf)
    }

    fn write_slot(&mut self, _slot: u8, _bytes: &[u8]) -> Result<(), SaveError> {
        // Never written: it may be another game's save.
        Err(SaveError::Io)
    }

    /// `save.bin`, the `"TS"`-framed save from before the slots.
    fn read_legacy(&mut self, buf: &mut [u8]) -> Result<Option<usize>, SaveError> {
        let mut file = vec![0u8; Caps::N3DS.save_bytes as usize];
        let Some(size) = self.0.load(File::SharedBin, &mut file)? else {
            return Ok(None);
        };
        let size = size.min(file.len());
        if size < HEADER {
            return Ok(None);
        }
        let mut header = [0u8; HEADER];
        header.copy_from_slice(&file[..HEADER]);
        let data = &file[HEADER..size];
        if !save::verify(&header, data) {
            return Ok(None); // foreign or torn file: no save
        }
        let n = data.len().min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        Ok(Some(data.len()))
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::collections::BTreeMap;
    use std::vec::Vec;

    use trino_core::SaveStatus;
    use trino_core::save::{SlotStore, encode_slot, read_slot_files, write_slot_files};

    use super::*;

    /// An SD card in memory, with a log of every write.
    #[derive(Default, Clone)]
    struct Card {
        files: BTreeMap<u8, Vec<u8>>,
        writes: Vec<u8>,
    }

    impl Files for &mut Card {
        fn load(&mut self, file: File, buf: &mut [u8]) -> Result<Option<usize>, SaveError> {
            Ok(self.files.get(&(file as u8)).map(|b| {
                let n = b.len().min(buf.len());
                buf[..n].copy_from_slice(&b[..n]);
                b.len()
            }))
        }
        fn store(&mut self, slot: u8, bytes: &[u8]) -> Result<(), SaveError> {
            assert!(slot < 2, "only the game's own slots are written");
            self.writes.push(slot);
            self.files.insert(slot, bytes.to_vec());
            Ok(())
        }
    }

    fn slot(seq: u16, data: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; 64 + data.len()];
        let n = encode_slot(seq, data, &mut out).unwrap();
        out.truncate(n);
        out
    }

    fn read(card: &mut Card) -> (Vec<u8>, SaveStatus) {
        let mut out = vec![0u8; 256];
        let mut scratch = vec![0u8; Caps::N3DS.save_bytes as usize];
        let r = read_slot_files(&mut GameSlots(card), &mut out, &mut scratch).unwrap();
        out.truncate(r.len);
        (out, r.status)
    }

    fn write(card: &mut Card, data: &[u8]) {
        let mut scratch = vec![0u8; Caps::N3DS.save_bytes as usize];
        let mut file = vec![0u8; save::slot_file_len(data.len())];
        write_slot_files(&mut GameSlots(card), data, &mut scratch, &mut file).unwrap();
    }

    #[test]
    fn two_games_get_two_folders() {
        assert_eq!(dir("castle64"), "sdmc:/3ds/trino/castle64");
        assert_eq!(dir("solid-eleven"), "sdmc:/3ds/trino/solid-eleven");
        assert_ne!(dir("castle64"), dir("solid-eleven"));
        assert_ne!(dir("castle64"), dir("castle64-selftest"));
        // The folder name is safe on FAT whatever the project calls itself.
        assert_eq!(folder("My Game: Deluxe/2"), "my_game__deluxe_2");
        assert_eq!(folder("  "), "game");
        assert_eq!(folder(".."), "game");
        assert_eq!(folder(&"x".repeat(80)).len(), MAX_NAME);
    }

    #[test]
    fn an_empty_folder_reads_the_shared_slots_once_and_never_writes_them() {
        let mut card = Card::default();
        card.files
            .insert(File::SharedA as u8, slot(4, b"shared-old"));
        card.files
            .insert(File::SharedB as u8, slot(5, b"shared-new"));
        let shared = card.files.clone();
        let (data, status) = read(&mut card);
        // A move, not damage: the game reads it as an intact save.
        assert_eq!(&data[..], b"shared-new");
        assert_eq!(status, SaveStatus::Ok);
        // The next save lands in the game's own folder.
        write(&mut card, b"own-1");
        assert_eq!(card.writes, [0]);
        let (data, status) = read(&mut card);
        assert_eq!((&data[..], status), (&b"own-1"[..], SaveStatus::Ok));
        // The shared files are exactly as they were.
        for f in [File::SharedA, File::SharedB] {
            assert_eq!(card.files.get(&(f as u8)), shared.get(&(f as u8)));
        }
    }

    #[test]
    fn the_shared_save_from_before_the_slots_migrates_too() {
        let mut card = Card::default();
        let mut framed = save::header(b"bin-save").to_vec();
        framed.extend_from_slice(b"bin-save");
        card.files.insert(File::SharedBin as u8, framed.clone());
        assert_eq!(&read(&mut card).0[..], b"bin-save");
        // A torn save.bin is no save.
        let torn = framed[..framed.len() - 1].to_vec();
        card.files.insert(File::SharedBin as u8, torn);
        assert_eq!(read(&mut card), (Vec::new(), SaveStatus::Empty));
    }

    #[test]
    fn a_damaged_own_save_never_loads_another_games() {
        let mut card = Card::default();
        card.files
            .insert(File::SharedA as u8, slot(9, b"other-game"));
        let mut own = slot(1, b"mine");
        let last = own.len() - 1;
        own[last] ^= 0x40;
        card.files.insert(File::OwnA as u8, own);
        let (data, status) = read(&mut card);
        assert!(data.is_empty(), "{data:?}");
        assert_eq!(status, SaveStatus::Corrupted);
        assert!(GameSlots(&mut card).has_own().unwrap());
    }

    #[test]
    fn an_own_save_wins_over_the_shared_one() {
        let mut card = Card::default();
        card.files.insert(File::SharedA as u8, slot(90, b"shared"));
        card.files.insert(File::OwnB as u8, slot(1, b"own"));
        assert_eq!(read(&mut card), (b"own".to_vec(), SaveStatus::Ok));
    }

    #[test]
    fn the_shared_folder_refuses_writes() {
        let mut card = Card::default();
        let mut c = &mut card;
        assert_eq!(Shared(&mut c).write_slot(0, b"x"), Err(SaveError::Io));
    }
}
