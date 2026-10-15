//! `trino_core::memo`: a backend's per-draw command memo. A key covers the
//! model, the generation (camera/light/meshes), the tint, the shading and
//! every matrix bit; anything else about the surface keeps a draw out of
//! the memo. Hits return exactly what was stored; a changed draw or a new
//! generation misses.

use trino_core::math3d::Mat34;
use trino_core::memo::{self, MemoKey, MemoSlot};
use trino_core::{AnimRef, ClipId, Color, ModelParams, Shading, SpriteId, Vec3};

fn at(x: f32) -> Mat34 {
    Mat34::from_rotation_scale_translation(Vec3::ZERO, Vec3::ONE, Vec3::new(x, 0.0, 2.0))
}

#[test]
fn a_repeated_draw_hits_and_anything_it_depends_on_misses() {
    let mut slots = vec![MemoSlot::<4>::EMPTY; 64];
    let plain = ModelParams::DEFAULT;
    let k = MemoKey::new(7, 0, &at(1.0), &plain).unwrap();
    assert!(memo::find(&slots, &k).is_none(), "empty");
    memo::store(&mut slots, k, [1, 2, 3, 4], 12);
    let hit = memo::find(&slots, &k).expect("same draw");
    assert_eq!((hit.words, hit.extra), ([1, 2, 3, 4], 12));
    // Every input of the key matters.
    let other = [
        MemoKey::new(8, 0, &at(1.0), &plain),
        MemoKey::new(7, 1, &at(1.0), &plain),
        MemoKey::new(7, 0, &at(1.0 + f32::EPSILON), &plain),
        MemoKey::new(7, 0, &at(1.0), &ModelParams::tinted(Color::rgb(255, 0, 0))),
        MemoKey::new(
            7,
            0,
            &at(1.0),
            &ModelParams {
                shading: Shading::Unlit,
                ..plain
            },
        ),
    ];
    for o in other {
        let o = o.unwrap();
        assert_ne!(o, k);
        assert!(memo::find(&slots, &o).is_none_or(|s| s.words != [1, 2, 3, 4]));
    }
}

#[test]
fn only_plain_draws_are_memoized() {
    let m = at(0.0);
    for p in [
        ModelParams {
            emissive: Color::rgb(0, 0, 1),
            ..ModelParams::DEFAULT
        },
        ModelParams {
            double_sided: true,
            ..ModelParams::DEFAULT
        },
        ModelParams {
            anim: Some(AnimRef {
                clip: ClipId(0),
                time: 0.0,
                blend: None,
            }),
            ..ModelParams::DEFAULT
        },
        ModelParams {
            texture: Some(SpriteId(3)),
            ..ModelParams::DEFAULT
        },
        ModelParams {
            palette: Some([Color::WHITE; 4]),
            ..ModelParams::DEFAULT
        },
    ] {
        assert!(MemoKey::new(1, 0, &m, &p).is_none(), "{p:?}");
    }
    assert!(MemoKey::new(1, 0, &m, &ModelParams::tinted(Color::rgba(9, 9, 9, 9))).is_some());
}

#[test]
fn a_full_memo_replaces_and_an_empty_one_never_hits() {
    let mut none: Vec<MemoSlot<2>> = Vec::new();
    let k = MemoKey::new(1, 0, &at(0.0), &ModelParams::DEFAULT).unwrap();
    memo::store(&mut none, k, [1, 1], 0);
    assert!(memo::find(&none, &k).is_none());
    // 300 draws into 64 slots: each lookup returns its own command or
    // nothing, never another draw's.
    let mut slots = vec![MemoSlot::<2>::EMPTY; 64];
    let keys: Vec<MemoKey> = (0..300)
        .map(|i| MemoKey::new(i, 3, &at(i as f32), &ModelParams::DEFAULT).unwrap())
        .collect();
    for (i, k) in keys.iter().enumerate() {
        memo::store(&mut slots, *k, [i as u32, 0], i as u32);
    }
    let mut hits = 0;
    for (i, k) in keys.iter().enumerate() {
        if let Some(s) = memo::find(&slots, k) {
            assert_eq!(s.words[0], i as u32);
            hits += 1;
        }
    }
    assert!(hits > 0 && hits <= 64, "{hits}");
    // The empty slot never matches a real key (generation 0).
    let fresh = vec![MemoSlot::<2>::EMPTY; 8];
    let zero = MemoKey::new(0, 0, &Mat34 { m: [[0.0; 4]; 3] }, &ModelParams::DEFAULT).unwrap();
    assert!(memo::find(&fresh, &zero).is_none());
}
