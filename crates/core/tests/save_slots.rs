//! Slotted saves on a raw medium (`trino_core::save`), end to end through
//! the public API, with power cuts injected at every byte of a write: the
//! game must always read back either the new save or the previous one —
//! never nothing, never garbage.

use trino_core::save::{
    self, HEADER, Lz, Medium, SLOT_HEADER, capacity, find_newest, plan_write, read_newest,
    slot_capacity, write_now,
};

/// A RAM image of an EEPROM. `cut` = bytes still writable before the power
/// fails (`None` = never): later writes are lost, a write that straddles
/// the cut is torn.
struct Ram {
    bytes: Vec<u8>,
    cut: Option<usize>,
}

impl Ram {
    fn new(size: usize) -> Self {
        Ram {
            bytes: vec![0xFF; size],
            cut: None,
        }
    }
}

impl Medium for Ram {
    fn size(&self) -> usize {
        self.bytes.len()
    }
    fn read(&mut self, at: usize, buf: &mut [u8]) {
        buf.copy_from_slice(&self.bytes[at..at + buf.len()]);
    }
    fn write(&mut self, at: usize, data: &[u8]) {
        for (k, &b) in data.iter().enumerate() {
            match &mut self.cut {
                Some(0) => return,
                Some(n) => *n -= 1,
                None => {}
            }
            self.bytes[at + k] = b;
        }
    }
}

fn game_save(n: usize, seed: u8) -> Vec<u8> {
    // Save-like data: small counters, a few runs of zeros, ids.
    (0..n)
        .map(|i| match i % 11 {
            0 => seed,
            1..=4 => 0,
            5 => (i / 11) as u8,
            _ => (i as u8).wrapping_mul(seed | 1) & 0x3F,
        })
        .collect()
}

fn write(m: &mut Ram, data: &[u8]) -> save::WritePlan {
    let total = m.size();
    let mut body = vec![0u8; total];
    let mut scratch = vec![0u8; total];
    write_now(m, data, &mut body, &mut scratch, &mut Lz::default()).unwrap()
}

fn read(m: &mut Ram) -> Vec<u8> {
    let total = m.size();
    let mut out = vec![0u8; total];
    let mut scratch = vec![0u8; total];
    let n = read_newest(m, &mut out, &mut scratch);
    out.truncate(n);
    out
}

#[test]
fn blank_medium_reads_as_no_save() {
    for size in [512, 2048] {
        assert!(read(&mut Ram::new(size)).is_empty());
        let mut zero = Ram::new(size);
        zero.bytes.fill(0);
        assert!(read(&mut zero).is_empty());
    }
}

#[test]
fn saves_alternate_slots_and_the_newest_wins() {
    let mut m = Ram::new(2048);
    let mut offsets = Vec::new();
    for k in 0..6u8 {
        let data = game_save(700 + k as usize * 10, k + 1);
        let plan = write(&mut m, &data);
        assert!(plan.safe);
        offsets.push(plan.offset);
        assert_eq!(read(&mut m), data, "save {k}");
    }
    assert_eq!(offsets, [0, 1024, 0, 1024, 0, 1024]);
}

#[test]
fn a_power_cut_anywhere_keeps_the_previous_save() {
    let old = game_save(900, 7);
    let new = game_save(950, 9);
    let mut base = Ram::new(2048);
    write(&mut base, &game_save(800, 3)); // slot A
    write(&mut base, &old); // slot B: the newest
    let plan = {
        let mut probe = Ram {
            bytes: base.bytes.clone(),
            cut: None,
        };
        write(&mut probe, &new)
    };
    let written = plan.body_len + SLOT_HEADER;
    // Under Miri (the nightly job, issue #121) every 13th byte of the body
    // and every byte from the header on: the same loop, minutes not hours.
    let stride = if cfg!(miri) { 13 } else { 1 };
    for cut in (0..=written).filter(|c| c.is_multiple_of(stride) || *c >= plan.body_len) {
        let mut m = Ram {
            bytes: base.bytes.clone(),
            cut: Some(cut),
        };
        write(&mut m, &new);
        let back = read(&mut m);
        if cut == written {
            assert_eq!(back, new, "complete write");
        } else {
            assert!(
                back == old || back == new,
                "cut after {cut} bytes read back {} bytes",
                back.len()
            );
            // Until the header is whole, the old save must still be newest.
            if cut < plan.body_len + 8 {
                assert_eq!(back, old, "cut after {cut} bytes");
            }
        }
    }
}

#[test]
fn compression_makes_big_saves_fit_a_slot() {
    let mut m = Ram::new(2048);
    let data = game_save(1400, 5);
    assert!(data.len() > slot_capacity(2048));
    let plan = write(&mut m, &data);
    assert!(plan.body_len <= slot_capacity(2048), "{}", plan.body_len);
    assert!(plan.safe);
    assert_eq!(read(&mut m), data);
}

#[test]
fn incompressible_saves_go_wide_and_still_read_back() {
    let mut m = Ram::new(2048);
    // xorshift noise does not compress.
    let mut x = 0x1234_5678u32;
    let noise: Vec<u8> = (0..1500)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect();
    write(&mut m, &game_save(600, 1));
    let plan = write(&mut m, &noise);
    assert_eq!(plan.offset, 0);
    assert!(!plan.safe, "a wide save has no fallback");
    assert_eq!(read(&mut m), noise);
    // The next small save goes back to the slots (into B: the wide one
    // covered A).
    let small = game_save(500, 2);
    write(&mut m, &small);
    assert_eq!(read(&mut m), small);
    let mut scratch = vec![0u8; 2048];
    assert!(find_newest(&mut m, &mut scratch).is_some());
    assert!(noise.len() <= capacity(2048));
}

#[test]
fn too_large_is_an_error() {
    let mut m = Ram::new(512);
    let mut body = vec![0u8; 512];
    let mut scratch = vec![0u8; 512];
    let r = plan_write(
        &mut m,
        &vec![1u8; 600],
        &mut body,
        &mut scratch,
        &mut Lz::default(),
    );
    assert!(matches!(
        r,
        Err(trino_core::SaveError::TooLarge { len: 600, .. })
    ));
}

#[test]
fn single_frame_saves_still_read_and_migrate_safely() {
    // The layout before slots: "TS" header + data at 0.
    let mut m = Ram::new(2048);
    let legacy = game_save(600, 4);
    m.bytes[..HEADER].copy_from_slice(&save::header(&legacy));
    m.bytes[HEADER..HEADER + legacy.len()].copy_from_slice(&legacy);
    assert_eq!(read(&mut m), legacy);
    // The first slotted save must not overwrite it (it fits in A's half).
    let plan = write(&mut m, &game_save(610, 8));
    assert_eq!(plan.offset, 1024);
    assert!(plan.safe);
    assert_eq!(read(&mut m), game_save(610, 8));
}

#[test]
fn a_4_kbit_eeprom_works_too() {
    let mut m = Ram::new(512);
    for k in 0..3u8 {
        let data = game_save(300, k + 1);
        write(&mut m, &data);
        assert_eq!(read(&mut m), data);
    }
}

#[test]
fn the_write_can_be_split_into_steps() {
    // What a console backend does across frames: plan, the body in 8-byte
    // blocks, the header last.
    let mut m = Ram::new(2048);
    write(&mut m, &game_save(700, 1));
    let data = game_save(720, 2);
    let mut body = vec![0u8; 2048];
    let mut scratch = vec![0u8; 2048];
    let plan = plan_write(&mut m, &data, &mut body, &mut scratch, &mut Lz::default()).unwrap();
    for (k, block) in body[..plan.body_len].chunks(8).enumerate() {
        m.write(plan.offset + SLOT_HEADER + k * 8, block);
        assert_eq!(read(&mut m), game_save(700, 1), "mid-write");
    }
    m.write(plan.offset, &plan.header[..8]);
    m.write(plan.offset + 8, &plan.header[8..]);
    assert_eq!(read(&mut m), data);
}
