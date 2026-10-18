//! Particle effects as text: `assets/shared/particles/<name>.particles.ron`
//! (an [`ParticlesDef`]) declared as `[particles.<name>]` in the manifest,
//! baked per platform into a `TPFX` blob (`trino_core::particles::Effect`)
//! that games load with `Platform::blob(BlobId::from_path("particles/<name>"))`.
//!
//! Each platform can cap the live particles or turn the effect off; the
//! bake writes that platform's cap. The editor's particle inspector edits
//! these files (live preview, burst, cost) and `cargo xtask particles`
//! checks and simulates them — the same functions.

use serde::{Deserialize, Serialize};
use trino_core::particles::{Effect, TPFX_BYTES};
use trino_core::{Color, Emitter, ParticleSystem, SpriteId, Vec2, Vec3};

use crate::Platform;

/// v2: a `web` cap per effect (issue #47); v1 files load unchanged.
pub const DEF_VERSION: u32 = 2;
pub const DEF_EXT: &str = ".particles.ron";
/// The most particles an effect may keep alive (runtime systems are fixed
/// size; past this a console spends its frame on billboards).
pub const MAX_PARTICLES: u16 = 512;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ParticlesDef {
    pub version: u32,
    /// The particle sprite (`sprites/smoke`); a flipbook laid left to right.
    pub sprite: String,
    /// Particles per second while emitting.
    #[serde(default = "d_rate")]
    pub rate: f32,
    /// Lifetime range, seconds.
    #[serde(default = "d_life")]
    pub life: (f32, f32),
    /// Initial speed range along `direction`, within the cone.
    #[serde(default = "d_speed")]
    pub speed: (f32, f32),
    #[serde(default = "d_dir")]
    pub direction: [f32; 3],
    /// Cone half-angle around `direction`, **degrees**.
    #[serde(default = "d_spread")]
    pub spread: f32,
    #[serde(default)]
    pub gravity: [f32; 3],
    /// Size at birth and at death (world units).
    #[serde(default = "d_size")]
    pub size: (f32, f32),
    /// Color at birth and at death (alpha fades).
    #[serde(default = "d_color")]
    pub color: ([u8; 4], [u8; 4]),
    /// Flipbook frames (0 = the whole sprite) and their size in pixels.
    #[serde(default, skip_serializing_if = "is_zero_u16")]
    pub frames: u16,
    #[serde(default, skip_serializing_if = "is_zero2")]
    pub frame_size: [f32; 2],
    /// The most particles alive at once (every platform, unless capped).
    #[serde(default = "d_max")]
    pub max: u16,
    #[serde(default, skip_serializing_if = "ParticlePlatforms::is_empty")]
    pub platforms: ParticlePlatforms,
}

fn is_zero_u16(v: &u16) -> bool {
    *v == 0
}
fn is_zero2(v: &[f32; 2]) -> bool {
    *v == [0.0, 0.0]
}
fn d_rate() -> f32 {
    10.0
}
fn d_life() -> (f32, f32) {
    (1.0, 1.0)
}
fn d_speed() -> (f32, f32) {
    (1.0, 1.0)
}
fn d_dir() -> [f32; 3] {
    [0.0, 1.0, 0.0]
}
fn d_spread() -> f32 {
    17.0
}
fn d_size() -> (f32, f32) {
    (0.5, 0.5)
}
fn d_color() -> ([u8; 4], [u8; 4]) {
    ([255; 4], [255; 4])
}
fn d_max() -> u16 {
    32
}

/// Per-platform caps (`None` = the effect's `max`; the Web takes the PC's
/// cap when it has none, like scene variants).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct ParticlePlatforms {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pc: Option<PlatformCap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n64: Option<PlatformCap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n3ds: Option<PlatformCap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web: Option<PlatformCap>,
}

impl ParticlePlatforms {
    pub fn is_empty(&self) -> bool {
        self.pc.is_none() && self.n64.is_none() && self.n3ds.is_none() && self.web.is_none()
    }

    pub fn get(&self, p: Platform) -> Option<&PlatformCap> {
        match p {
            Platform::Pc => self.pc.as_ref(),
            Platform::N64 => self.n64.as_ref(),
            Platform::N3ds => self.n3ds.as_ref(),
            Platform::Web => self.web.as_ref().or(self.pc.as_ref()),
        }
    }

    pub fn get_mut(&mut self, p: Platform) -> &mut Option<PlatformCap> {
        match p {
            Platform::Pc => &mut self.pc,
            Platform::N64 => &mut self.n64,
            Platform::N3ds => &mut self.n3ds,
            Platform::Web => &mut self.web,
        }
    }
}

/// One platform's limit.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub struct PlatformCap {
    /// The most particles alive there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u16>,
    /// The effect does not run there.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
}

impl ParticlesDef {
    pub fn new(sprite: impl Into<String>) -> Self {
        ParticlesDef {
            version: DEF_VERSION,
            sprite: sprite.into(),
            rate: d_rate(),
            life: d_life(),
            speed: d_speed(),
            direction: d_dir(),
            spread: d_spread(),
            gravity: [0.0; 3],
            size: d_size(),
            color: d_color(),
            frames: 0,
            frame_size: [0.0; 2],
            max: d_max(),
            platforms: ParticlePlatforms::default(),
        }
    }

    pub fn from_ron(text: &str) -> Result<Self, String> {
        let mut d: ParticlesDef =
            ron::from_str(text).map_err(|e| format!("particles parse error: {e}"))?;
        if d.version > DEF_VERSION {
            return Err(format!(
                "particles version {} is newer than this engine understands ({DEF_VERSION})",
                d.version
            ));
        }
        // v1 is v2 without the Web: it saves as v2.
        d.version = DEF_VERSION;
        d.validate()?;
        Ok(d)
    }

    pub fn to_ron(&self) -> String {
        let config = ron::ser::PrettyConfig::default().struct_names(false);
        ron::ser::to_string_pretty(self, config).expect("serialization cannot fail")
    }

    /// Values the runtime can play: ranges in order, positive lifetimes,
    /// a cone within 180°, caps within [`MAX_PARTICLES`].
    pub fn validate(&self) -> Result<(), String> {
        let range = |what: &str, (a, b): (f32, f32), min: f32| {
            if !(a.is_finite() && b.is_finite()) || a < min || b < a {
                Err(format!("{what} ({a}, {b}): need {min} <= min <= max"))
            } else {
                Ok(())
            }
        };
        range("life", self.life, 0.001)?;
        range("speed", self.speed, 0.0)?;
        if !(self.size.0 >= 0.0 && self.size.1 >= 0.0) {
            return Err("size must not be negative".into());
        }
        if !(self.rate >= 0.0 && self.rate.is_finite()) {
            return Err(format!("rate {}: must be >= 0", self.rate));
        }
        if !(0.0..=180.0).contains(&self.spread) {
            return Err(format!("spread {}°: 0..=180", self.spread));
        }
        let d = self.direction;
        if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < 1e-8 {
            return Err("direction is zero".into());
        }
        let caps = [Some(self.max)]
            .into_iter()
            .chain(Platform::ALL.map(|p| self.platforms.get(p).and_then(|c| c.max)));
        for c in caps.flatten() {
            if c > MAX_PARTICLES {
                return Err(format!("max {c}: at most {MAX_PARTICLES}"));
            }
        }
        if self.frames > 0 && (self.frame_size[0] <= 0.0 || self.frame_size[1] <= 0.0) {
            return Err("a flipbook needs a frame_size".into());
        }
        Ok(())
    }

    /// The runtime emitter.
    pub fn emitter(&self) -> Emitter {
        let c = |c: [u8; 4]| Color::rgba(c[0], c[1], c[2], c[3]);
        Emitter {
            sprite: SpriteId::from_path(&self.sprite),
            rate: self.rate,
            life: self.life,
            speed: self.speed,
            direction: Vec3::new(self.direction[0], self.direction[1], self.direction[2]),
            spread: self.spread.to_radians(),
            gravity: Vec3::new(self.gravity[0], self.gravity[1], self.gravity[2]),
            size: self.size,
            color: (c(self.color.0), c(self.color.1)),
            frames: self.frames,
            frame_size: Vec2::new(self.frame_size[0], self.frame_size[1]),
        }
    }

    /// The most particles alive on `platform` (0 = off there).
    pub fn cap(&self, platform: Platform) -> u16 {
        match self.platforms.get(platform) {
            Some(c) if c.disabled => 0,
            Some(c) => c.max.unwrap_or(self.max),
            None => self.max,
        }
    }

    /// The baked effect for `platform`.
    pub fn effect(&self, platform: Platform) -> Effect {
        Effect {
            emitter: self.emitter(),
            max: self.cap(platform),
        }
    }

    /// Its `TPFX` blob for `platform`.
    pub fn tpfx(&self, platform: Platform) -> Vec<u8> {
        let mut out = [0u8; TPFX_BYTES];
        self.effect(platform).write(&mut out);
        out.to_vec()
    }
}

/// Bake the effect file at `path` for `platform`.
pub fn bake(path: &std::path::Path, platform: Platform) -> Result<Vec<u8>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let def = ParticlesDef::from_ron(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(def.tpfx(platform))
}

/// What an effect costs on a platform, over a deterministic run.
#[derive(Clone, Debug, PartialEq)]
pub struct SimReport {
    pub platform: Platform,
    pub cap: u16,
    /// Live particles at the end of the run and the most at once.
    pub live: usize,
    pub peak: usize,
    /// Triangles at the peak (2 per particle).
    pub peak_tris: usize,
    /// Estimated N64 time of the peak frame's billboards (µs).
    pub peak_n64_us: u32,
    /// FNV-1a of the final particles' positions (mm): the same on every
    /// target for the same seed.
    pub hash: u32,
}

/// Run the effect for `seconds` at 30 fps with `seed` on `platform`.
pub fn simulate(def: &ParticlesDef, platform: Platform, seconds: f32, seed: u32) -> SimReport {
    let effect = def.effect(platform);
    let mut sys: Box<ParticleSystem<{ MAX_PARTICLES as usize }>> =
        Box::new(ParticleSystem::from_effect(&effect, seed));
    let mut peak = 0;
    let steps = (seconds * 30.0).round() as usize;
    for _ in 0..steps {
        sys.update(1.0 / 30.0);
        peak = peak.max(sys.len());
    }
    SimReport {
        platform,
        cap: effect.max,
        live: sys.len(),
        peak,
        peak_tris: peak * 2,
        peak_n64_us: trino_core::perf::N64_COST.billboards_us(peak as u32),
        hash: position_hash(sys.particles()),
    }
}

/// FNV-1a over particle positions rounded to millimeters.
pub fn position_hash(particles: &[trino_core::particles::Particle]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for p in particles {
        for v in [p.pos.x, p.pos.y, p.pos.z] {
            for b in ((v * 1000.0).round() as i32).to_le_bytes() {
                h ^= b as u32;
                h = h.wrapping_mul(0x0100_0193);
            }
        }
    }
    h
}

/// `assets/shared/particles/<name>.particles.ron`.
pub fn def_file(assets_root: &std::path::Path, name: &str) -> std::path::PathBuf {
    assets_root
        .join("shared/particles")
        .join(format!("{name}{DEF_EXT}"))
}

/// Write `def` as effect `name` and declare `[particles.<name>]` once.
/// Returns whether the manifest changed.
pub fn save(assets_root: &std::path::Path, name: &str, def: &ParticlesDef) -> Result<bool, String> {
    if !crate::manifest::edit::valid_name(name) {
        return Err(format!(
            "effect name `{name}`: lowercase letters, digits, - and _"
        ));
    }
    def.validate()?;
    let path = def_file(assets_root, name);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, def.to_ron()).map_err(|e| format!("{}: {e}", path.display()))?;
    let mpath = assets_root.join("manifest.toml");
    let text = std::fs::read_to_string(&mpath).map_err(|e| format!("{}: {e}", mpath.display()))?;
    if crate::manifest::edit::has_table(&text, "particles", name) {
        return Ok(false);
    }
    let mut out = text.trim_end().to_string();
    out.push_str(&format!(
        "\n\n[particles.{name}]\nfile = \"particles/{name}{DEF_EXT}\"\n"
    ));
    std::fs::write(&mpath, out).map_err(|e| format!("{}: {e}", mpath.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_web_takes_the_pc_cap_unless_it_has_its_own() {
        let mut d = ParticlesDef::new("sprites/smoke");
        d.max = 40;
        d.platforms.pc = Some(PlatformCap {
            max: Some(30),
            disabled: false,
        });
        assert_eq!(d.platforms.get(Platform::Web).and_then(|c| c.max), Some(30));
        d.platforms.web = Some(PlatformCap {
            max: Some(64),
            disabled: false,
        });
        assert_eq!(d.platforms.get(Platform::Web).and_then(|c| c.max), Some(64));
        // v1 files load and save as v2, `web` included.
        let v1 = d.to_ron().replace("version: 2", "version: 1");
        let back = ParticlesDef::from_ron(&v1).unwrap();
        assert_eq!(back.version, DEF_VERSION);
        assert!(back.to_ron().contains("web:"), "{}", back.to_ron());
    }

    #[test]
    fn caps_per_platform() {
        let mut d = ParticlesDef::new("sprites/smoke");
        d.max = 40;
        d.platforms.n64 = Some(PlatformCap {
            max: Some(12),
            disabled: false,
        });
        d.platforms.n3ds = Some(PlatformCap {
            max: None,
            disabled: true,
        });
        assert_eq!(d.cap(Platform::Pc), 40);
        assert_eq!(d.cap(Platform::N64), 12);
        assert_eq!(d.cap(Platform::N3ds), 0);
        assert!((d.emitter().spread - 17f32.to_radians()).abs() < 1e-6);
    }
}
