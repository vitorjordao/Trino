//! Save recovery (issue #68) through the public API: the slot files of the
//! PC, the Web and the 3DS, the EEPROM's recovery status, fault injection,
//! and versioned saves with migration. Power cuts at every byte, a bit flip
//! at every bit, two machines' slots after a cloud sync: a read returns the
//! newest intact save or nothing, never a mix, and says which.

mod common;

use trino_core::save::{
    self, Lz, Medium, SLOT_HEADER, SlotStore, encode_slot, parse_slot, read_slot_files,
    slot_file_len, slot_files_fault, write_slot_files,
};
use trino_core::{SaveError, SaveFault, SaveStatus};

/// Two slot files and a legacy file in RAM. `cut`: bytes a slot write
/// still lands before the power fails (a store that writes in place, the
/// worst case; the real stores rename a flushed temp file).
#[derive(Default, Clone)]
struct Files {
    slots: [Option<Vec<u8>>; 2],
    legacy: Option<Vec<u8>>,
    cut: Option<usize>,
}

impl SlotStore for Files {
    fn read_slot(&mut self, slot: u8, buf: &mut [u8]) -> Result<Option<usize>, SaveError> {
        Ok(self.slots[slot as usize].as_ref().map(|f| {
            let n = f.len().min(buf.len());
            buf[..n].copy_from_slice(&f[..n]);
            f.len()
        }))
    }
    fn write_slot(&mut self, slot: u8, bytes: &[u8]) -> Result<(), SaveError> {
        let keep = self.cut.map_or(bytes.len(), |c| c.min(bytes.len()));
        let old = self.slots[slot as usize].take().unwrap_or_default();
        // In place: the new bytes over the old ones, up to the cut.
        let mut f = old;
        f.resize(f.len().max(keep), 0);
        f[..keep].copy_from_slice(&bytes[..keep]);
        if keep == bytes.len() {
            f.truncate(bytes.len());
        }
        self.slots[slot as usize] = Some(f);
        if let Some(c) = &mut self.cut {
            *c -= keep;
        }
        Ok(())
    }
    fn read_legacy(&mut self, buf: &mut [u8]) -> Result<Option<usize>, SaveError> {
        Ok(self.legacy.as_ref().map(|f| {
            let n = f.len().min(buf.len());
            buf[..n].copy_from_slice(&f[..n]);
            f.len()
        }))
    }
}

const CAP: usize = 256;

/// Every byte and every bit natively; under Miri (the nightly job, issue
/// #121) a stride through the same loops, which keeps their logic and every
/// slot layout but not the hours an exhaustive run would take there.
const STRIDE: usize = if cfg!(miri) { 7 } else { 1 };

fn read(f: &mut Files) -> (Vec<u8>, SaveStatus) {
    let mut out = [0u8; CAP];
    let mut scratch = [0u8; CAP + SLOT_HEADER];
    let r = read_slot_files(f, &mut out, &mut scratch).unwrap();
    (out[..r.len.min(CAP)].to_vec(), r.status)
}

fn write(f: &mut Files, data: &[u8]) {
    let mut scratch = [0u8; CAP + SLOT_HEADER];
    let mut file = vec![0u8; slot_file_len(data.len())];
    write_slot_files(f, data, &mut scratch, &mut file).unwrap();
}

fn fault(f: &mut Files, fault: SaveFault) -> bool {
    let mut scratch = [0u8; CAP + SLOT_HEADER];
    slot_files_fault(f, fault, &mut scratch).unwrap()
}

#[test]
fn slot_files_alternate_and_each_read_reports_what_it_found() {
    let mut f = Files::default();
    assert_eq!(read(&mut f), (vec![], SaveStatus::Empty));
    write(&mut f, b"day=1");
    write(&mut f, b"day=2");
    assert!(
        f.slots.iter().all(Option::is_some),
        "two saves fill both slots"
    );
    assert_eq!(read(&mut f), (b"day=2".to_vec(), SaveStatus::Ok));
    write(&mut f, b"day=3");
    assert_eq!(read(&mut f), (b"day=3".to_vec(), SaveStatus::Ok));
    // Bit rot in the newest: the previous save, reported.
    assert!(fault(&mut f, SaveFault::FlipNewest));
    assert_eq!(
        read(&mut f),
        (b"day=2".to_vec(), SaveStatus::RecoveredFromBackup)
    );
    // The next save replaces the damaged slot, and the medium is whole.
    write(&mut f, b"day=4");
    assert_eq!(read(&mut f), (b"day=4".to_vec(), SaveStatus::Ok));
    assert!(fault(&mut f, SaveFault::TearNewest));
    assert_eq!(
        read(&mut f),
        (b"day=2".to_vec(), SaveStatus::RecoveredFromBackup)
    );
    // Damage everywhere: nothing, then a write repairs both slots.
    assert!(fault(&mut f, SaveFault::FlipAll));
    assert_eq!(read(&mut f), (vec![], SaveStatus::Corrupted));
    write(&mut f, b"day=5");
    assert_eq!(read(&mut f), (b"day=5".to_vec(), SaveStatus::Ok));
    assert!(
        f.slots
            .iter()
            .all(|s| s.as_deref().is_some_and(|b| parse_slot(b).is_ok()))
    );
}

#[test]
fn a_power_cut_at_every_byte_of_a_slot_write_keeps_a_whole_save() {
    let old = b"stars=12;coins=340;stage=4".to_vec();
    let new = b"stars=13;coins=355;stage=5;lives=2".to_vec();
    // From one save and from two (the cut write lands over the older one).
    for prior in [1usize, 2] {
        let mut base = Files::default();
        for k in 0..prior {
            write(&mut base, if k + 1 == prior { &old } else { b"stars=11" });
        }
        let total = slot_file_len(new.len());
        for cut in (0..=total).filter(|c| c.is_multiple_of(STRIDE) || *c == total) {
            let mut f = base.clone();
            f.cut = Some(cut);
            write(&mut f, &new);
            f.cut = None;
            let (data, status) = read(&mut f);
            if cut == total {
                assert_eq!((data, status), (new.clone(), SaveStatus::Ok));
            } else {
                assert_eq!(data, old, "prior {prior}, cut at byte {cut} of {total}");
                assert_ne!(status, SaveStatus::Corrupted);
            }
            // And the next save always lands.
            write(&mut f, b"after");
            assert_eq!(
                read(&mut f),
                (b"after".to_vec(), SaveStatus::Ok),
                "cut {cut}"
            );
        }
    }
}

#[test]
fn a_bit_flip_anywhere_reads_the_newest_the_previous_or_nothing() {
    let mut base = Files::default();
    write(&mut base, b"older save");
    write(&mut base, b"newest save");
    for slot in 0..2 {
        let len = base.slots[slot].as_ref().unwrap().len();
        for bit in (0..len * 8).step_by(STRIDE) {
            let mut f = base.clone();
            f.slots[slot].as_mut().unwrap()[bit / 8] ^= 1 << (bit % 8);
            let (data, status) = read(&mut f);
            let newest_slot = 1; // the second write went to B
            let in_body = bit >= SLOT_HEADER * 8;
            match (slot == newest_slot, data.as_slice()) {
                // Damage in the newest: the previous save. Its body's damage
                // is reported; a header's may claim an older sequence number
                // (or none, a flipped magic), which can't be told apart.
                (true, b"older save") => {
                    assert!(
                        !in_body || status == SaveStatus::RecoveredFromBackup,
                        "bit {bit}"
                    );
                    assert_ne!(status, SaveStatus::Corrupted);
                }
                // Damage in the older: the newest still reads. A flipped
                // sequence number may claim a newer save was lost.
                (false, b"newest save") => {
                    assert!(
                        !in_body || status == SaveStatus::Ok,
                        "bit {bit}: {status:?}"
                    );
                }
                (_, other) => panic!("slot {slot} bit {bit}: read {other:?} ({status:?})"),
            }
        }
    }
}

#[test]
fn two_machines_slots_after_a_cloud_sync_resolve_the_same_way_everywhere() {
    // Each machine saved once from the same start: both files carry
    // sequence number 1, with different data.
    let mut a = vec![0u8; 64];
    let mut b = vec![0u8; 64];
    let na = encode_slot(1, b"desk", &mut a).unwrap();
    let nb = encode_slot(1, b"laptop", &mut b).unwrap();
    let mut f = Files {
        slots: [Some(a[..na].to_vec()), Some(b[..nb].to_vec())],
        ..Files::default()
    };
    assert_eq!(
        read(&mut f),
        (b"desk".to_vec(), SaveStatus::Ok),
        "a tie goes to slot A"
    );
    // A later save outranks the tie, on either machine's numbering.
    write(&mut f, b"merged");
    assert_eq!(read(&mut f), (b"merged".to_vec(), SaveStatus::Ok));
    // Wrapping sequence numbers: 0 is newer than 0xFFFF.
    let n0 = encode_slot(0xFFFF, b"old", &mut a).unwrap();
    let n1 = encode_slot(0, b"new", &mut b).unwrap();
    let mut f = Files {
        slots: [Some(a[..n0].to_vec()), Some(b[..n1].to_vec())],
        ..Files::default()
    };
    assert_eq!(read(&mut f).0, b"new".to_vec());
}

#[test]
fn a_save_from_before_slot_files_reads_until_the_first_slot_write() {
    let mut f = Files {
        legacy: Some(b"legacy bytes".to_vec()),
        ..Files::default()
    };
    assert_eq!(read(&mut f), (b"legacy bytes".to_vec(), SaveStatus::Ok));
    write(&mut f, b"slots now");
    assert_eq!(read(&mut f), (b"slots now".to_vec(), SaveStatus::Ok));
    assert_eq!(
        f.legacy.as_deref(),
        Some(&b"legacy bytes"[..]),
        "the old file is left alone"
    );
    // Both slots damaged: the legacy save is what is left, reported.
    assert!(fault(&mut f, SaveFault::FlipAll));
    assert_eq!(
        read(&mut f),
        (b"legacy bytes".to_vec(), SaveStatus::RecoveredFromBackup)
    );
    // A slot file longer than any save of this game is not one of its own.
    let mut f = Files {
        slots: [Some(vec![b'T'; CAP * 4]), None],
        ..Files::default()
    };
    assert_eq!(read(&mut f), (vec![], SaveStatus::Corrupted));
}

#[test]
fn slot_files_refuse_every_inconsistency() {
    let mut buf = [0u8; 64];
    let n = encode_slot(7, b"abc", &mut buf).unwrap();
    let good = &buf[..n];
    assert_eq!(parse_slot(good).unwrap().data, b"abc");
    assert_eq!(
        parse_slot(&good[..n - 1]),
        Err(save::SlotError::Damaged(7)),
        "truncated"
    );
    let mut longer = good.to_vec();
    longer.push(0);
    assert_eq!(
        parse_slot(&longer),
        Err(save::SlotError::Damaged(7)),
        "trailing bytes"
    );
    assert_eq!(parse_slot(b"T2"), Err(save::SlotError::Foreign));
    assert_eq!(parse_slot(&[0xFF; 32]), Err(save::SlotError::Foreign));
    for at in 4..SLOT_HEADER {
        let mut bad = good.to_vec();
        bad[at] ^= 0x01;
        assert!(parse_slot(&bad).is_err(), "header byte {at}");
    }
    assert!(
        encode_slot(1, &[0; 70_000], &mut vec![0; 80_000]).is_none(),
        "u16 length"
    );
    assert!(
        encode_slot(1, b"abc", &mut [0u8; 10]).is_none(),
        "short output"
    );
}

// ------------------------------------------------------------ raw medium

struct Ram(Vec<u8>);

impl Medium for Ram {
    fn size(&self) -> usize {
        self.0.len()
    }
    fn read(&mut self, at: usize, buf: &mut [u8]) {
        buf.copy_from_slice(&self.0[at..at + buf.len()]);
    }
    fn write(&mut self, at: usize, data: &[u8]) {
        self.0[at..at + data.len()].copy_from_slice(data);
    }
}

fn eeprom_read(m: &mut Ram) -> (Vec<u8>, SaveStatus) {
    let mut out = vec![0u8; m.0.len()];
    let mut scratch = vec![0u8; m.0.len()];
    let (n, st) = save::read_newest_status(m, &mut out, &mut scratch);
    (out[..n].to_vec(), st)
}

fn eeprom_write(m: &mut Ram, data: &[u8]) {
    let mut body = vec![0u8; m.0.len()];
    let mut scratch = vec![0u8; m.0.len()];
    save::write_now(m, data, &mut body, &mut scratch, &mut Lz::default()).unwrap();
}

#[test]
fn the_eeprom_reports_recovery_and_its_faults_are_the_files() {
    for size in [512usize, 2048] {
        let mut m = Ram(vec![0xFF; size]);
        assert_eq!(eeprom_read(&mut m), (vec![], SaveStatus::Empty));
        eeprom_write(&mut m, b"stage=1;stars=3");
        eeprom_write(&mut m, b"stage=2;stars=7");
        assert_eq!(
            eeprom_read(&mut m),
            (b"stage=2;stars=7".to_vec(), SaveStatus::Ok)
        );
        let mut scratch = vec![0u8; size];
        assert!(save::medium_fault(
            &mut m,
            SaveFault::FlipNewest,
            &mut scratch
        ));
        assert_eq!(
            eeprom_read(&mut m),
            (b"stage=1;stars=3".to_vec(), SaveStatus::RecoveredFromBackup)
        );
        eeprom_write(&mut m, b"stage=3;stars=9");
        assert_eq!(
            eeprom_read(&mut m),
            (b"stage=3;stars=9".to_vec(), SaveStatus::Ok)
        );
        assert!(save::medium_fault(
            &mut m,
            SaveFault::TearNewest,
            &mut scratch
        ));
        assert_eq!(
            eeprom_read(&mut m),
            (b"stage=1;stars=3".to_vec(), SaveStatus::RecoveredFromBackup)
        );
        assert!(save::medium_fault(&mut m, SaveFault::FlipAll, &mut scratch));
        assert_eq!(eeprom_read(&mut m), (vec![], SaveStatus::Corrupted));
        // The next write is numbered past the damage: read back intact.
        eeprom_write(&mut m, b"stage=4");
        assert_eq!(eeprom_read(&mut m), (b"stage=4".to_vec(), SaveStatus::Ok));
        // Nothing to damage on a blank medium.
        let mut blank = Ram(vec![0xFF; size]);
        assert!(!save::medium_fault(
            &mut blank,
            SaveFault::FlipNewest,
            &mut scratch
        ));
    }
}

#[test]
fn a_torn_eeprom_write_over_the_older_slot_is_not_a_lost_save() {
    // The power fails while the body of the next save lands over the older
    // slot: its old header now fails its check, but what the read finds
    // is still the last save that finished.
    let mut m = Ram(vec![0xFF; 512]);
    eeprom_write(&mut m, b"first");
    eeprom_write(&mut m, b"second");
    let mut body = vec![0u8; 512];
    let mut scratch = vec![0u8; 512];
    let plan = save::plan_write(
        &mut m,
        b"third, never finished",
        &mut body,
        &mut scratch,
        &mut Lz::default(),
    )
    .unwrap();
    m.write(plan.offset + SLOT_HEADER, &body[..plan.body_len]);
    assert_eq!(eeprom_read(&mut m), (b"second".to_vec(), SaveStatus::Ok));
}

// --------------------------------------------------------- versioned saves

#[test]
fn versioned_saves_migrate_from_older_versions_and_refuse_newer_ones() {
    let mut p = common::MemoryPlatform::new(512);
    let mut buf = [0u8; 64];
    let mut v = [0u8; 64];
    // Version 1: coins only. Version 2 appends lives.
    let n = save::write_versioned(1, b"coins=7", &mut v).unwrap();
    assert_eq!(&v[..4], b"SV\x01\x00");
    trino_core::Platform::save_write(&mut p, &v[..n]).unwrap();
    let mut calls = 0;
    let mut migrate = |from: u16, d: &mut [u8], len: usize| {
        calls += 1;
        assert_eq!(from, 1);
        d[len..len + 8].copy_from_slice(b";lives=3");
        Some(len + 8)
    };
    let l = save::load_versioned(&mut p, &mut buf, 2, &mut migrate).unwrap();
    assert_eq!((l.len, l.from, l.migrated), (15, 1, true));
    assert_eq!(&buf[..l.len], b"coins=7;lives=3");
    assert_eq!(calls, 1);
    // The current version loads as is.
    let n = save::write_versioned(2, b"coins=1;lives=1", &mut v).unwrap();
    trino_core::Platform::save_write(&mut p, &v[..n]).unwrap();
    let l =
        save::load_versioned(&mut p, &mut buf, 2, &mut |_, _, _| panic!("no migration")).unwrap();
    assert_eq!(
        (&buf[..l.len], l.migrated),
        (&b"coins=1;lives=1"[..], false)
    );
    // A newer game's save: nothing loads (the game must not overwrite it
    // without asking), and `from` says why.
    let n = save::write_versioned(9, b"future", &mut v).unwrap();
    trino_core::Platform::save_write(&mut p, &v[..n]).unwrap();
    let l =
        save::load_versioned(&mut p, &mut buf, 2, &mut |_, _, _| panic!("no migration")).unwrap();
    assert_eq!((l.len, l.from), (0, 9));
    // A save from before the game tagged versions is version 0.
    trino_core::Platform::save_write(&mut p, b"raw").unwrap();
    let l = save::load_versioned(&mut p, &mut buf, 1, &mut |from, _, len| {
        assert_eq!(from, 0);
        Some(len)
    })
    .unwrap();
    assert_eq!((&buf[..l.len], l.from, l.migrated), (&b"raw"[..], 0, true));
    // A migration that gives up: defaults.
    trino_core::Platform::save_write(&mut p, b"raw").unwrap();
    let l = save::load_versioned(&mut p, &mut buf, 1, &mut |_, _, _| None).unwrap();
    assert_eq!(l.len, 0);
    assert_eq!(
        save::split_version(b"SV"),
        (0, &b"SV"[..]),
        "a short tag is data"
    );
}

struct Versioned {
    seen: Option<u16>,
}

impl trino_core::Game for Versioned {
    fn update(&mut self, _: &trino_core::InputState, _: &mut dyn trino_core::Audio, _: f32) {}
    fn render(&mut self, _: &mut dyn trino_core::Renderer) {}
    fn migrate_save(&mut self, from: u16, data: &mut [u8], len: usize) -> Option<usize> {
        self.seen = Some(from);
        data[len] = b'!';
        Some(len + 1)
    }
}

#[test]
fn games_migrate_through_their_own_hook() {
    let mut p = common::MemoryPlatform::new(512);
    let mut v = [0u8; 32];
    let n = save::write_versioned(3, b"old", &mut v).unwrap();
    trino_core::Platform::save_write(&mut p, &v[..n]).unwrap();
    let mut g = Versioned { seen: None };
    let mut buf = [0u8; 32];
    let l = save::load_game(&mut g, &mut p, &mut buf, 4).unwrap();
    assert_eq!((g.seen, &buf[..l.len]), (Some(3), &b"old!"[..]));
    // The trait's default gives up: defaults.
    struct Plain;
    impl trino_core::Game for Plain {
        fn update(&mut self, _: &trino_core::InputState, _: &mut dyn trino_core::Audio, _: f32) {}
        fn render(&mut self, _: &mut dyn trino_core::Renderer) {}
    }
    let l = save::load_game(&mut Plain, &mut p, &mut buf, 4).unwrap();
    assert_eq!(l.len, 0);
    assert_eq!(trino_core::Platform::save_status(&p), SaveStatus::Unknown);
    assert_eq!(
        trino_core::Platform::save_fault(&mut p, SaveFault::FlipNewest),
        Err(SaveError::Unsupported),
        "the default has no medium to damage"
    );
}
