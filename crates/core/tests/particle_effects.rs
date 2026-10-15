//! Baked particle effects (`TPFX`) at runtime: a platform's cap holds
//! against bursts and continuous emission, an effect off on a platform
//! spawns nothing, and the same seed plays the same particles.

use trino_core::particles::{EffectError, TPFX_BYTES};
use trino_core::{Color, Effect, Emitter, ParticleSystem, SpriteId, Vec3};

fn fx(max: u16) -> Effect {
    Effect {
        emitter: Emitter {
            sprite: SpriteId(4),
            rate: 100.0,
            life: (2.0, 2.0),
            gravity: Vec3::new(0.0, -1.0, 0.0),
            color: (Color::WHITE, Color::rgba(255, 255, 255, 0)),
            ..Emitter::DEFAULT
        },
        max,
    }
}

#[test]
fn a_platform_cap_holds_against_bursts_and_emission() {
    let mut s: ParticleSystem<128> = ParticleSystem::from_effect(&fx(12), 5);
    assert_eq!(s.limit, 12);
    for _ in 0..60 {
        s.update(1.0 / 30.0);
        assert!(s.len() <= 12);
    }
    assert_eq!(s.len(), 12, "100/s fills it");
    s.burst(500);
    assert_eq!(s.len(), 12);
    // Raising the limit at runtime lets more in, never past the capacity.
    s.limit = 1000;
    s.burst(500);
    assert_eq!(s.len(), 128);
}

#[test]
fn an_effect_off_on_a_platform_spawns_nothing() {
    let mut s: ParticleSystem<16> = ParticleSystem::from_effect(&fx(0), 5);
    assert!(!s.emitting);
    s.emitting = true;
    s.burst(10);
    for _ in 0..30 {
        s.update(0.05);
    }
    assert!(s.is_empty());
}

#[test]
fn the_same_seed_plays_the_same_particles() {
    let run = |seed| {
        let mut s: ParticleSystem<32> = ParticleSystem::from_effect(&fx(20), seed);
        for _ in 0..40 {
            s.update(1.0 / 30.0);
        }
        s.particles().to_vec()
    };
    assert_eq!(run(3), run(3));
    assert_ne!(run(3), run(4));
}

#[test]
fn tpfx_blobs_round_trip() {
    let mut b = [0u8; TPFX_BYTES];
    fx(9).write(&mut b);
    assert_eq!(Effect::parse(&b), Ok(fx(9)));
    assert_eq!(Effect::parse(&b[..40]), Err(EffectError::Truncated));
}
