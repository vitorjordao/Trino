//! `trino_core::mem::{Blocks, TrimWindow}` through the public API, in the
//! shape of the base N64's trouble (GitHub #110): RSP mesh copies loaded
//! and unloaded with an asset group in the middle of a run, then one big
//! copy (the micro scenes' 45 360-byte flat copy) that must still find a
//! block; and a renderer buffer that a single big frame grew.

use trino_core::mem::{Blocks, TrimWindow, shrink_to};

#[test]
fn a_group_load_and_unload_leaves_room_for_the_big_copy() {
    // 160 KB for the copies, as the base N64 reserves it at boot.
    let mut region = Blocks::<96>::new(160 * 1024);
    // Boot: castle64's copies, a few KB each.
    let boot: Vec<(u32, u32)> = (0..24)
        .map(|i| {
            let len = 1_500 + (i * 211) % 2_600;
            (region.alloc(len).expect("boot copies fit"), len)
        })
        .collect();
    // A group's skinned model: its parts' copies, interleaved with the
    // boot ones' lifetimes, then the group unloads.
    let group: Vec<(u32, u32)> = (0..11)
        .map(|i| {
            let len = 2_000 + i * 97;
            (region.alloc(len).unwrap(), len)
        })
        .collect();
    // Some boot copies go too (a level's props replaced).
    for &(off, len) in boot.iter().step_by(3) {
        region.free(off, len);
    }
    for &(off, len) in &group {
        region.free(off, len);
    }
    // The holes merged: the big copy finds a block.
    assert!(region.largest_free() >= 45_360, "{}", region.largest_free());
    let big = region.alloc(45_360).expect("the micro scenes' flat copy");
    assert_eq!(big % Blocks::<96>::ALIGN, 0);
    assert_eq!(region.lost(), 0);
    assert!(region.high_water() <= region.size());
}

#[test]
fn a_big_frame_gives_its_buffer_back_after_a_window() {
    // 88-byte CPU triangles; 16 rigid models in one frame made 1 024.
    let mut t = TrimWindow::<1>::new(4);
    let mut cap = 1_024usize;
    t.note(0, 1_024);
    let mut shrunk = None;
    for frame in 0..12 {
        if frame > 0 {
            t.note(0, 60);
        }
        if let Some([peak]) = t.end_frame()
            && let Some(n) = shrink_to(cap, peak, 88, 16 * 1024)
        {
            cap = n;
            shrunk.get_or_insert(frame);
        }
    }
    // The window with the big frame keeps it; the next one gives it back.
    assert_eq!(shrunk, Some(7));
    assert_eq!(cap, 60);
}
