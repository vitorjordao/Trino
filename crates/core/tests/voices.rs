//! Positional voices with a budget (Bongo-Kart #179, ADR 0044 §2) through
//! the public API: `VoicePool` over a mixer with a fixed voice count —
//! priorities, loudness among equals, the backend's own limit, virtual
//! loops, range, doppler and stale handles.

mod common;

use common::MockAudio;
use trino_core::{
    Audio, Doppler, Listener, PITCH_MAX, PoolStats, SoundEmitter, SoundId, Vec3, VoicePool,
};

const ENGINE: SoundId = SoundId::from_path("sounds/engine");
const HORN: SoundId = SoundId::from_path("sounds/horn");

fn listener() -> Listener {
    Listener {
        pos: Vec3::ZERO,
        right: Vec3::new(1.0, 0.0, 0.0),
    }
}

/// A looping engine `x` units to the right, priority `p`, heard within 10.
fn engine(x: f32, p: u8) -> SoundEmitter {
    SoundEmitter {
        looped: true,
        priority: p,
        ..SoundEmitter::new(ENGINE, Vec3::new(x, 0.0, 0.0), 10.0)
    }
}

fn pool<const N: usize>() -> VoicePool<N> {
    let mut p = VoicePool::new();
    p.set_listener(listener(), Vec3::ZERO);
    p
}

#[test]
fn a_full_pool_gives_way_to_a_higher_priority_and_refuses_a_lower_one() {
    let mut mix = MockAudio::new(8);
    let mut pool = pool::<3>();
    let near = pool.play_voice_at(&mut mix, &engine(1.0, 10)).unwrap();
    let mid = pool.play_voice_at(&mut mix, &engine(2.0, 10)).unwrap();
    let far = pool.play_voice_at(&mut mix, &engine(3.0, 10)).unwrap();
    assert_eq!(mix.voices.len(), 3);
    // The pool is the budget: a lower priority is refused.
    assert_eq!(pool.play_voice_at(&mut mix, &engine(0.5, 5)), None);
    assert_eq!(pool.stats().refused, 1);
    // A higher one takes the lowest rank's slot: among equals, the quietest.
    let horn = SoundEmitter {
        priority: 200,
        ..SoundEmitter::new(HORN, Vec3::new(5.0, 0.0, 0.0), 10.0)
    };
    let h = pool.play_voice_at(&mut mix, &horn).unwrap();
    assert!(!pool.is_playing(far), "the farthest engine lost its slot");
    assert!(pool.is_playing(near) && pool.is_playing(mid) && pool.is_audible(h));
    assert_eq!(mix.voices.len(), 3, "its backend voice was stopped");
    assert_eq!(
        pool.stats(),
        PoolStats {
            audible: 3,
            virtual_voices: 0,
            stolen: 1,
            refused: 1,
            inaudible: 0,
        }
    );
    // Equal priority: a louder sound beats the quietest, a quieter one not.
    assert_eq!(pool.play_voice_at(&mut mix, &engine(8.0, 10)), None);
    let close = pool.play_voice_at(&mut mix, &engine(0.2, 10)).unwrap();
    assert!(!pool.is_playing(mid) && pool.is_playing(near) && pool.is_playing(close));
}

/// The backend runs out before the pool (an N64 has four voices): the
/// lowest voiced rank gives its voice up; a loop keeps its slot without
/// one and is heard again when a voice frees.
#[test]
fn the_backend_limit_takes_a_voice_from_the_lowest_rank() {
    let mut mix = MockAudio::new(2);
    let mut pool = pool::<4>();
    let low = pool.play_voice_at(&mut mix, &engine(1.0, 1)).unwrap();
    let high = pool.play_voice_at(&mut mix, &engine(1.0, 2)).unwrap();
    let shot = SoundEmitter {
        priority: 3,
        ..SoundEmitter::new(HORN, Vec3::new(1.0, 0.0, 0.0), 10.0)
    };
    let s = pool.play_voice_at(&mut mix, &shot).unwrap();
    assert!(pool.is_audible(s) && pool.is_audible(high));
    assert!(pool.is_playing(low) && !pool.is_audible(low), "virtual");
    assert_eq!(pool.stats().virtual_voices, 1);
    assert_eq!(pool.stats().stolen, 1);
    // A one-shot below everything voiced gets nothing and is refused.
    let quiet = SoundEmitter {
        priority: 0,
        ..SoundEmitter::new(HORN, Vec3::new(1.0, 0.0, 0.0), 10.0)
    };
    assert_eq!(pool.play_voice_at(&mut mix, &quiet), None);
    // The horn ends: the parked loop gets the voice back.
    mix.voices.retain(|v| v.sound != HORN);
    pool.update(&mut mix);
    assert!(!pool.is_playing(s));
    assert!(pool.is_audible(low));
    assert_eq!(pool.stats().virtual_voices, 0);
    assert_eq!(mix.voices.len(), 2);
}

#[test]
fn range_parks_loops_and_drops_one_shots() {
    let mut mix = MockAudio::new(8);
    let mut pool = pool::<4>();
    let far_shot = SoundEmitter::new(HORN, Vec3::new(50.0, 0.0, 0.0), 10.0);
    assert_eq!(pool.play_voice_at(&mut mix, &far_shot), None);
    assert_eq!(pool.stats().inaudible, 1);
    // A loop out of range holds a slot, voiceless, until it is heard.
    let v = pool.play_voice_at(&mut mix, &engine(50.0, 10)).unwrap();
    assert!(pool.is_playing(v) && !pool.is_audible(v));
    assert!(mix.voices.is_empty());
    assert!(pool.move_voice(v, Vec3::new(-2.0, 0.0, 0.0), Vec3::ZERO));
    pool.update(&mut mix);
    assert!(pool.is_audible(v));
    let heard = mix.voices[0].params;
    assert!(
        heard.volume > 0.5 && heard.pan < 0.0,
        "left of the listener: {heard:?}"
    );
    // Out again: the voice goes, the slot stays.
    pool.move_voice(v, Vec3::new(0.0, 0.0, 30.0), Vec3::ZERO);
    pool.update(&mut mix);
    assert!(pool.is_playing(v) && !pool.is_audible(v));
    assert!(mix.voices.is_empty());
    // A one-shot that leaves the range ends.
    let s = pool
        .play_voice_at(&mut mix, &SoundEmitter::new(HORN, Vec3::ZERO, 10.0))
        .unwrap();
    pool.move_voice(s, Vec3::new(40.0, 0.0, 0.0), Vec3::ZERO);
    pool.update(&mut mix);
    assert!(!pool.is_playing(s));
}

#[test]
fn doppler_raises_an_approaching_sound_and_lowers_a_receding_one() {
    let mut mix = MockAudio::new(8);
    let mut pool = pool::<2>();
    // 10% of the speed of sound straight at the listener.
    let mut e = engine(5.0, 10);
    e.vel = Vec3::new(-34.3, 0.0, 0.0);
    let v = pool.play_voice_at(&mut mix, &e).unwrap();
    let pitch = mix.voices[0].params.pitch;
    assert!((pitch - 343.0 / 308.7).abs() < 1e-3, "{pitch}");
    // Receding, with its own pitch on top.
    let em = pool.emitter_mut(v).unwrap();
    em.vel = Vec3::new(34.3, 0.0, 0.0);
    em.pitch = 1.5;
    pool.update(&mut mix);
    let pitch = mix.voices[0].params.pitch;
    assert!((pitch - 1.5 * 343.0 / 377.3).abs() < 1e-3, "{pitch}");
    // A listener driving after it at the same speed hears no shift.
    pool.set_listener(listener(), Vec3::new(34.3, 0.0, 0.0));
    pool.update(&mut mix);
    assert!((mix.voices[0].params.pitch - 1.5).abs() < 1e-4);
    // Arcade doppler is clamped to the backends' range; off is off.
    pool.set_listener(listener(), Vec3::ZERO);
    pool.set_doppler(Doppler {
        scale: 20.0,
        ..Doppler::PHYSICAL
    });
    pool.emitter_mut(v).unwrap().vel = Vec3::new(-34.3, 0.0, 0.0);
    pool.update(&mut mix);
    assert_eq!(mix.voices[0].params.pitch, PITCH_MAX);
    pool.set_doppler(Doppler::OFF);
    pool.update(&mut mix);
    assert_eq!(mix.voices[0].params.pitch, 1.5);
}

#[test]
fn stopped_and_replaced_handles_go_stale() {
    let mut mix = MockAudio::new(8);
    let mut pool = pool::<1>();
    let a = pool.play_voice_at(&mut mix, &engine(1.0, 10)).unwrap();
    pool.stop(&mut mix, a);
    assert!(!pool.is_playing(a) && mix.voices.is_empty());
    assert!(!pool.move_voice(a, Vec3::ZERO, Vec3::ZERO));
    let b = pool.play_voice_at(&mut mix, &engine(1.0, 10)).unwrap();
    assert_ne!(a, b, "the same slot, a new generation");
    assert!(pool.emitter_mut(a).is_none() && pool.is_playing(b));
    pool.stop(&mut mix, a); // stale: b keeps playing
    assert!(pool.is_audible(b));
    pool.stop_all(&mut mix);
    assert!(!pool.is_playing(b) && mix.voices.is_empty());
    assert_eq!(pool.stats().audible, 0);
    // Without a listener nothing is heard yet.
    let mut deaf = VoicePool::<2>::new();
    assert_eq!(
        deaf.play_voice_at(&mut mix, &SoundEmitter::new(HORN, Vec3::ZERO, 10.0)),
        None
    );
    let audio: &mut dyn Audio = &mut mix;
    assert!(deaf.play_voice_at(audio, &engine(0.0, 1)).is_some());
}
