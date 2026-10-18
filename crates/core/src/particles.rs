//! Fixed-capacity, deterministic particle systems drawn as billboards —
//! smoke, sparks, dust, coin glints. No allocation, no `std`: the same
//! seed and the same `dt` sequence give the same particles on PC, N64 and
//! 3DS (an xorshift RNG and the core's own trig).
//!
//! ```ignore
//! let mut smoke: ParticleSystem<32> = ParticleSystem::new(SMOKE, 1);
//! smoke.origin = chimney;
//! smoke.update(dt);          // in Game::update
//! smoke.draw(renderer);      // in Game::render, after set_camera
//! ```
//!
//! Each particle is one billboard (two triangles), so budget them like
//! geometry: 32 particles = 64 tris.

use crate::math::{Color, Rect, Vec2, Vec3};
use crate::math3d::{cos, sin, sqrt};
use crate::render::{BillboardParams, Renderer, SpriteId};

/// How a system spawns and evolves particles. `const`-constructible so
/// games keep their effects as constants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emitter {
    pub sprite: SpriteId,
    /// Particles spawned per second while `ParticleSystem::emitting`.
    pub rate: f32,
    /// Lifetime range in seconds (min, max).
    pub life: (f32, f32),
    /// Initial speed range (min, max), along `direction` within `spread`.
    pub speed: (f32, f32),
    /// Main emission direction (normalized by the system).
    pub direction: Vec3,
    /// Cone half-angle around `direction`, radians (0 = a straight jet).
    pub spread: f32,
    /// Constant acceleration (gravity, buoyancy, wind).
    pub gravity: Vec3,
    /// Size at birth and at death (world units), interpolated linearly.
    pub size: (f32, f32),
    /// Color at birth and at death (alpha fades), interpolated linearly.
    pub color: (Color, Color),
    /// Flipbook: frames laid out left to right in the sprite, each
    /// `frame_size` pixels; played once over a particle's life. `0` frames
    /// = draw the whole sprite.
    pub frames: u16,
    pub frame_size: Vec2,
}

impl Emitter {
    pub const DEFAULT: Emitter = Emitter {
        sprite: SpriteId(0),
        rate: 10.0,
        life: (1.0, 1.0),
        speed: (1.0, 1.0),
        direction: Vec3::new(0.0, 1.0, 0.0),
        spread: 0.3,
        gravity: Vec3::ZERO,
        size: (0.5, 0.5),
        color: (Color::WHITE, Color::WHITE),
        frames: 0,
        frame_size: Vec2::ZERO,
    };
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    pub pos: Vec3,
    pub vel: Vec3,
    pub age: f32,
    pub life: f32,
    /// Screen roll, fixed per particle (variety at no cost).
    pub roll: f32,
}

const DEAD: Particle = Particle {
    pos: Vec3::ZERO,
    vel: Vec3::ZERO,
    age: 0.0,
    life: 0.0,
    roll: 0.0,
};

/// A particle system of up to `N` live particles.
#[derive(Clone, Copy, Debug)]
pub struct ParticleSystem<const N: usize> {
    pub emitter: Emitter,
    /// World position particles spawn at.
    pub origin: Vec3,
    /// Continuous spawning on/off (bursts work either way).
    pub emitting: bool,
    /// The most particles alive at once (≤ `N`): a platform's cap from a
    /// baked [`Effect`]. 0 = this effect is off here.
    pub limit: usize,
    particles: [Particle; N],
    count: usize,
    carry: f32,
    rng: u32,
}

impl<const N: usize> ParticleSystem<N> {
    /// `seed` picks the random stream (non-zero; 0 is mapped to 1).
    pub const fn new(emitter: Emitter, seed: u32) -> Self {
        ParticleSystem {
            emitter,
            origin: Vec3::ZERO,
            emitting: true,
            limit: N,
            particles: [DEAD; N],
            count: 0,
            carry: 0.0,
            rng: if seed == 0 { 1 } else { seed },
        }
    }

    /// A system for a baked effect: its emitter, its cap on this platform
    /// (`effect.max`, at most `N`), off when the platform has none.
    pub const fn from_effect(effect: &Effect, seed: u32) -> Self {
        let mut s = Self::new(effect.emitter, seed);
        let max = effect.max as usize;
        s.limit = if max < N { max } else { N };
        s.emitting = max > 0;
        s
    }

    /// Apply a quality's particle scale to the cap
    /// ([`crate::QualitySettings::particle_cap`]: `limit × scale`, at least
    /// 1 when the effect is on here, at most `N`). Call it once after
    /// [`from_effect`](Self::from_effect) with `platform.quality()
    /// .particle_scale` (1.0 on the consoles: no change). Particles alive
    /// past a lower cap finish their lives.
    pub fn scale_limit(&mut self, scale: f32) {
        let q = crate::QualitySettings {
            particle_scale: scale,
            ..crate::QualitySettings::HIGH
        };
        self.limit = q.particle_cap(self.limit).min(N);
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn particles(&self) -> &[Particle] {
        &self.particles[..self.count]
    }

    /// xorshift32 in 0..1.
    fn rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    fn range(&mut self, (lo, hi): (f32, f32)) -> f32 {
        lo + (hi - lo) * self.rand()
    }

    /// Spawn `n` particles now (explosions, impacts). Extra ones past the
    /// capacity are dropped.
    pub fn burst(&mut self, n: usize) {
        let cap = if self.limit < N { self.limit } else { N };
        for _ in 0..n {
            if self.count >= cap {
                return;
            }
            let p = self.spawn();
            self.particles[self.count] = p;
            self.count += 1;
        }
    }

    fn spawn(&mut self) -> Particle {
        let e = self.emitter;
        // Direction inside a cone: pick an angle around the axis and a tilt.
        let d = e.direction.normalized();
        let tilt = e.spread * sqrt(self.rand());
        let around = self.rand() * core::f32::consts::TAU;
        // Any vector not parallel to d builds the cone basis.
        let helper = if d.y.abs() < 0.9 {
            Vec3::new(0.0, 1.0, 0.0)
        } else {
            Vec3::new(1.0, 0.0, 0.0)
        };
        let u = d.cross(helper).normalized();
        let v = d.cross(u);
        let dir = d * cos(tilt) + (u * cos(around) + v * sin(around)) * sin(tilt);
        let speed = self.range(e.speed);
        Particle {
            pos: self.origin,
            vel: dir * speed,
            age: 0.0,
            life: self.range(e.life).max(1e-3),
            roll: self.rand() * core::f32::consts::TAU,
        }
    }

    /// Advance every particle by `dt`, retire the dead, spawn new ones.
    pub fn update(&mut self, dt: f32) {
        let g = self.emitter.gravity;
        let mut i = 0;
        while i < self.count {
            let p = &mut self.particles[i];
            p.age += dt;
            if p.age >= p.life {
                // Swap-remove (order is irrelevant: billboards z-test).
                self.count -= 1;
                self.particles[i] = self.particles[self.count];
                continue;
            }
            p.vel += g * dt;
            p.pos += p.vel * dt;
            i += 1;
        }
        if self.emitting && self.emitter.rate > 0.0 {
            self.carry += self.emitter.rate * dt;
            let n = self.carry as usize;
            self.carry -= n as f32;
            self.burst(n);
        }
    }

    /// Draw every live particle as a billboard (call after `set_camera`).
    pub fn draw(&self, renderer: &mut dyn Renderer) {
        let e = &self.emitter;
        for p in self.particles() {
            let t = (p.age / p.life).clamp(0.0, 1.0);
            let size = e.size.0 + (e.size.1 - e.size.0) * t;
            let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
            let (c0, c1) = e.color;
            let region = (e.frames > 0).then(|| {
                let f = ((t * e.frames as f32) as u16).min(e.frames - 1);
                Rect::new(
                    f as f32 * e.frame_size.x,
                    0.0,
                    e.frame_size.x,
                    e.frame_size.y,
                )
            });
            renderer.draw_billboard(
                e.sprite,
                p.pos,
                &BillboardParams {
                    size: Vec2::new(size, size),
                    rotation: p.roll,
                    tint: Color::rgba(
                        mix(c0.r, c1.r),
                        mix(c0.g, c1.g),
                        mix(c0.b, c1.b),
                        mix(c0.a, c1.a),
                    ),
                    region,
                },
            );
        }
    }
}

/// A baked particle effect (`TPFX`, from an `assets/.../*.particles.ron`
/// through the asset bake): the emitter and the most particles this
/// platform keeps alive (0 = the effect is off on this platform).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effect {
    pub emitter: Emitter,
    pub max: u16,
}

pub const TPFX_MAGIC: &[u8; 4] = b"TPFX";
pub const TPFX_VERSION: u16 = 1;
/// Size of a `TPFX` blob.
pub const TPFX_BYTES: usize = 88;

/// Why a `TPFX` blob was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EffectError {
    BadMagic,
    UnsupportedVersion(u16),
    Truncated,
}

impl Effect {
    /// Read a `TPFX` blob (little endian):
    ///
    /// ```text
    /// magic "TPFX"  u16 version (1)  u16 max  u32 sprite
    /// f32 rate, life.0, life.1, speed.0, speed.1, direction xyz,
    ///   spread (radians), gravity xyz, size.0, size.1
    /// color.0 rgba, color.1 rgba, u16 frames, u16 reserved, f32 frame w, h
    /// ```
    pub fn parse(b: &[u8]) -> Result<Effect, EffectError> {
        if b.len() < TPFX_BYTES {
            return Err(EffectError::Truncated);
        }
        if &b[0..4] != TPFX_MAGIC {
            return Err(EffectError::BadMagic);
        }
        let u16_at = |at: usize| u16::from_le_bytes([b[at], b[at + 1]]);
        let version = u16_at(4);
        if version != TPFX_VERSION {
            return Err(EffectError::UnsupportedVersion(version));
        }
        let u32_at = |at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
        let f = |k: usize| f32::from_bits(u32_at(12 + k * 4));
        let rgba = |at: usize| Color::rgba(b[at], b[at + 1], b[at + 2], b[at + 3]);
        Ok(Effect {
            max: u16_at(6),
            emitter: Emitter {
                sprite: SpriteId(u32_at(8)),
                rate: f(0),
                life: (f(1), f(2)),
                speed: (f(3), f(4)),
                direction: Vec3::new(f(5), f(6), f(7)),
                spread: f(8),
                gravity: Vec3::new(f(9), f(10), f(11)),
                size: (f(12), f(13)),
                color: (rgba(68), rgba(72)),
                frames: u16_at(76),
                frame_size: Vec2::new(f32::from_bits(u32_at(80)), f32::from_bits(u32_at(84))),
            },
        })
    }

    /// Write `self` as a `TPFX` blob.
    pub fn write(&self, out: &mut [u8; TPFX_BYTES]) {
        let e = &self.emitter;
        out[0..4].copy_from_slice(TPFX_MAGIC);
        out[4..6].copy_from_slice(&TPFX_VERSION.to_le_bytes());
        out[6..8].copy_from_slice(&self.max.to_le_bytes());
        out[8..12].copy_from_slice(&e.sprite.0.to_le_bytes());
        let fs = [
            e.rate,
            e.life.0,
            e.life.1,
            e.speed.0,
            e.speed.1,
            e.direction.x,
            e.direction.y,
            e.direction.z,
            e.spread,
            e.gravity.x,
            e.gravity.y,
            e.gravity.z,
            e.size.0,
            e.size.1,
        ];
        for (k, v) in fs.iter().enumerate() {
            out[12 + k * 4..16 + k * 4].copy_from_slice(&v.to_le_bytes());
        }
        let (c0, c1) = e.color;
        out[68..72].copy_from_slice(&[c0.r, c0.g, c0.b, c0.a]);
        out[72..76].copy_from_slice(&[c1.r, c1.g, c1.b, c1.a]);
        out[76..78].copy_from_slice(&e.frames.to_le_bytes());
        out[78..80].copy_from_slice(&[0, 0]);
        out[80..84].copy_from_slice(&e.frame_size.x.to_le_bytes());
        out[84..88].copy_from_slice(&e.frame_size.y.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPARKS: Emitter = Emitter {
        rate: 50.0,
        life: (0.5, 1.0),
        speed: (2.0, 3.0),
        gravity: Vec3::new(0.0, -10.0, 0.0),
        ..Emitter::DEFAULT
    };

    #[test]
    fn emits_at_rate_and_retires_by_age() {
        let mut s: ParticleSystem<128> = ParticleSystem::new(SPARKS, 7);
        for _ in 0..30 {
            s.update(1.0 / 60.0); // 0.5 s at 50/s -> ~25 spawned, none dead yet
        }
        assert!((24..=25).contains(&s.len()), "{}", s.len());
        s.emitting = false;
        for _ in 0..120 {
            s.update(1.0 / 60.0);
        }
        assert!(s.is_empty(), "everyone dies within max life");
    }

    #[test]
    fn deterministic_and_bounded() {
        let run = || {
            let mut s: ParticleSystem<8> = ParticleSystem::new(SPARKS, 42);
            s.burst(100); // capped at capacity
            for _ in 0..10 {
                s.update(0.02);
            }
            s
        };
        let (a, b) = (run(), run());
        assert_eq!(a.len(), 8);
        assert_eq!(a.particles(), b.particles());
        // Gravity bends them down; the cone keeps them going up first.
        for p in a.particles() {
            assert!(p.vel.y < 3.0);
        }
    }

    #[test]
    fn spread_respects_the_cone() {
        let e = Emitter {
            spread: 0.2,
            speed: (1.0, 1.0),
            ..Emitter::DEFAULT
        };
        let mut s: ParticleSystem<64> = ParticleSystem::new(e, 3);
        s.burst(64);
        for p in s.particles() {
            // Angle to +Y below the spread: cos(angle) = vel.y / |vel|.
            assert!(p.vel.y > cos(0.2) - 1e-3, "{:?}", p.vel);
        }
    }
}

#[cfg(test)]
mod effect_tests {
    use super::*;

    #[test]
    fn tpfx_round_trips_and_caps_the_system() {
        let fx = Effect {
            emitter: Emitter {
                sprite: SpriteId(9),
                rate: 40.0,
                color: (Color::rgba(1, 2, 3, 4), Color::rgba(5, 6, 7, 8)),
                frames: 4,
                frame_size: Vec2::new(8.0, 8.0),
                ..Emitter::DEFAULT
            },
            max: 5,
        };
        let mut buf = [0u8; TPFX_BYTES];
        fx.write(&mut buf);
        assert_eq!(Effect::parse(&buf), Ok(fx));
        assert_eq!(Effect::parse(&buf[..10]), Err(EffectError::Truncated));
        let mut bad = buf;
        bad[0] = b'X';
        assert_eq!(Effect::parse(&bad), Err(EffectError::BadMagic));
        bad = buf;
        bad[4] = 2;
        assert_eq!(Effect::parse(&bad), Err(EffectError::UnsupportedVersion(2)));
        // The platform's cap holds whatever the capacity and the bursts.
        let mut s: ParticleSystem<64> = ParticleSystem::from_effect(&fx, 3);
        s.burst(50);
        assert_eq!(s.len(), 5);
        for _ in 0..30 {
            s.update(0.05);
            assert!(s.len() <= 5);
        }
        // A cap of 0 is "off here": nothing spawns.
        let off: ParticleSystem<64> = ParticleSystem::from_effect(&Effect { max: 0, ..fx }, 3);
        assert!(!off.emitting && off.limit == 0);
        // A cap past the capacity stops at the capacity.
        let big: ParticleSystem<4> = ParticleSystem::from_effect(&Effect { max: 99, ..fx }, 3);
        assert_eq!(big.limit, 4);
    }
}
