//! Particle systems end to end: a const `Emitter` → `ParticleSystem`
//! updated with fixed steps → `draw` as billboards on a renderer that runs
//! the billboard T&L. Deterministic per seed, fixed capacity, flipbooks.

mod common;

use common::Recorder;
use trino_core::render3d::Camera3;
use trino_core::{Color, Emitter, ParticleSystem, Rect, Renderer, SpriteId, Vec2, Vec3};

const SMOKE: Emitter = Emitter {
    sprite: SpriteId(0x5A0E),
    rate: 20.0,
    life: (1.0, 2.0),
    speed: (1.0, 2.0),
    direction: Vec3::new(0.0, 1.0, 0.0),
    spread: 0.4,
    gravity: Vec3::new(0.0, 0.5, 0.0),
    size: (0.2, 1.0),
    color: (
        Color::rgba(255, 255, 255, 255),
        Color::rgba(100, 100, 100, 0),
    ),
    frames: 4,
    frame_size: Vec2::new(16.0, 16.0),
};

fn run(seed: u32, steps: usize) -> ParticleSystem<32> {
    let mut p: ParticleSystem<32> = ParticleSystem::new(SMOKE, seed);
    p.origin = Vec3::new(1.0, 0.0, 2.0);
    for _ in 0..steps {
        p.update(1.0 / 30.0);
    }
    p
}

#[test]
fn same_seed_same_particles_different_seed_different_ones() {
    let a = run(7, 45);
    let b = run(7, 45);
    assert_eq!(a.particles(), b.particles());
    let c = run(8, 45);
    assert_ne!(a.particles(), c.particles());
    // Seed 0 is mapped to 1.
    assert_eq!(run(0, 10).particles(), run(1, 10).particles());
}

#[test]
fn rate_lifetime_and_motion() {
    // 20/s for 1.5 s with lives of 1..2 s: 30 spawned, the oldest third
    // possibly gone.
    let p = run(3, 45);
    assert!((20..=30).contains(&p.len()), "{}", p.len());
    for q in p.particles() {
        assert!(q.age < q.life && (1.0..=2.0).contains(&q.life));
        // Rising (direction up, buoyant) and within the cone; the newest
        // ones have not moved from the origin yet.
        assert!(q.vel.y > 0.0 && q.pos.y >= 0.0);
        let horiz = (q.vel.x * q.vel.x + q.vel.z * q.vel.z).sqrt();
        assert!(horiz <= q.vel.y, "spread 0.4 rad keeps them mostly upward");
    }
    // Stop emitting: everybody dies within the max lifetime.
    let mut p = run(3, 45);
    p.emitting = false;
    for _ in 0..61 {
        p.update(1.0 / 30.0);
    }
    assert!(p.is_empty());

    // Capacity bounds bursts.
    let mut b: ParticleSystem<8> = ParticleSystem::new(SMOKE, 1);
    b.emitting = false;
    b.burst(5);
    assert_eq!(b.len(), 5);
    b.burst(100);
    assert_eq!(b.len(), 8);
    assert!(
        b.particles()
            .iter()
            .all(|q| q.pos == Vec3::ZERO && q.age == 0.0)
    );
    // A zero rate never spawns on its own.
    let mut quiet: ParticleSystem<8> = ParticleSystem::new(Emitter { rate: 0.0, ..SMOKE }, 1);
    quiet.update(1.0);
    assert!(quiet.is_empty());
}

#[test]
fn draw_emits_one_billboard_per_particle_with_fading_and_flipbook() {
    let mut p: ParticleSystem<16> = ParticleSystem::new(SMOKE, 5);
    p.emitting = false;
    p.burst(10);
    let mut r = Recorder::new();
    r.set_camera(&Camera3 {
        eye: Vec3::new(0.0, 0.0, -6.0),
        target: Vec3::ZERO,
        fov_y: 1.0,
    });
    r.begin_frame(Color::BLACK);
    p.draw(&mut r);
    r.end_frame();
    let b = r.billboards();
    assert_eq!(b.len(), 10);
    for (id, pos, params, visible) in &b {
        assert_eq!(*id, SMOKE.sprite);
        assert_eq!(*pos, Vec3::ZERO);
        assert!(*visible, "in front of the camera");
        // Newborn: birth size, birth color, first flipbook frame.
        assert_eq!(params.size, Vec2::new(0.2, 0.2));
        assert_eq!(params.tint, Color::rgba(255, 255, 255, 255));
        assert_eq!(params.region, Some(Rect::new(0.0, 0.0, 16.0, 16.0)));
    }
    assert_eq!(r.stats().tris, 20, "two triangles per particle");
    assert_eq!(r.stats().sprites, 10);

    // Aged to ~60% of a 1..2 s life: grown, faded, a later frame.
    for _ in 0..30 {
        p.update(1.0 / 30.0);
    }
    r.begin_frame(Color::BLACK);
    p.draw(&mut r);
    let later = r.billboards();
    assert_eq!(later.len(), p.len());
    for (q, (_, pos, params, _)) in p.particles().iter().zip(&later) {
        let t = q.age / q.life;
        assert_eq!(*pos, q.pos);
        assert!((params.size.x - (0.2 + 0.8 * t)).abs() < 1e-5);
        assert!(params.tint.a < 255 && params.tint.r < 255);
        let frame = ((t * 4.0) as u16).min(3) as f32;
        assert_eq!(params.region.unwrap().pos.x, frame * 16.0);
        assert_eq!(params.rotation, q.roll);
    }

    // Without a flipbook the whole sprite is drawn.
    let mut whole: ParticleSystem<4> = ParticleSystem::new(Emitter { frames: 0, ..SMOKE }, 2);
    whole.burst(1);
    r.begin_frame(Color::BLACK);
    whole.draw(&mut r);
    assert_eq!(r.billboards()[0].2.region, None);
    assert_eq!(Emitter::DEFAULT.frames, 0);
}

#[test]
fn sideways_jets_spawn_inside_their_cone() {
    let jet = Emitter {
        direction: Vec3::new(1.0, 0.0, 0.0),
        spread: 0.0,
        gravity: Vec3::ZERO,
        speed: (3.0, 3.0),
        ..SMOKE
    };
    let mut p: ParticleSystem<8> = ParticleSystem::new(jet, 9);
    p.burst(4);
    for q in p.particles() {
        assert!((q.vel.x - 3.0).abs() < 1e-3, "{q:?}");
        assert!(q.vel.y.abs() < 1e-3 && q.vel.z.abs() < 1e-3);
    }
}
