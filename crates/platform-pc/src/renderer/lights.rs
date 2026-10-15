//! The per-pixel lighting tier's light table (`LightTier::PerPixel`, the
//! native profile): every distinct lighting state of a frame (global
//! light, local lights, wetness, eye) becomes one *set* in a uniform
//! buffer, and each lit vertex names its set. Pure code: the layout is
//! unit-tested without a GPU.

use trino_core::render3d::{Camera3, Light};
use trino_core::{LightKind, LocalLight, MAX_LOCAL_LIGHTS, Vec3};

/// Light sets per frame. A frame that changes its lighting state more
/// often than this lights the rest with the last set.
pub const MAX_SETS: usize = 8;

/// One local light. Must match `shaders.wgsl` (`GpuLight`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuLight {
    /// World position, radius.
    pub pos_radius: [f32; 4],
    /// `color × intensity` (0..1 per channel at intensity 1), `w` = 1 for
    /// a spot.
    pub color_spot: [f32; 4],
    /// Spot axis (unit, world), `cos` of the outer half-angle.
    pub dir_cos: [f32; 4],
    /// `1 / (cos inner - cos outer)`, unused.
    pub cone: [f32; 4],
}

/// One lighting state. Must match `shaders.wgsl` (`LightSet`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuSet {
    /// Direction the global light travels (unit), ambient.
    pub sun_dir: [f32; 4],
    /// Global light color (0..1), wetness.
    pub sun_color: [f32; 4],
    /// Camera position (world), number of local lights.
    pub eye: [f32; 4],
    pub lights: [GpuLight; MAX_LOCAL_LIGHTS],
}

pub const SET_BYTES: usize = core::mem::size_of::<GpuSet>();

/// What makes two lit draws share a set.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SetKey {
    pub eye: Vec3,
    pub light: Light,
    pub locals: trino_core::LightList,
    pub wetness: f32,
}

impl SetKey {
    pub fn new(
        camera: &Camera3,
        light: &Light,
        locals: &trino_core::LightList,
        wetness: f32,
    ) -> Self {
        SetKey {
            eye: camera.eye,
            light: *light,
            locals: *locals,
            wetness,
        }
    }

    pub fn gpu(&self) -> GpuSet {
        let d = self.light.dir.normalized();
        let c = |v: u8| v as f32 / 255.0;
        let mut lights = [GpuLight::zeroed(); MAX_LOCAL_LIGHTS];
        for (g, l) in lights.iter_mut().zip(self.locals.as_slice()) {
            *g = gpu_light(l);
        }
        GpuSet {
            sun_dir: [d.x, d.y, d.z, self.light.ambient],
            sun_color: [
                c(self.light.color.r),
                c(self.light.color.g),
                c(self.light.color.b),
                self.wetness.clamp(0.0, 1.0),
            ],
            eye: [
                self.eye.x,
                self.eye.y,
                self.eye.z,
                self.locals.as_slice().len() as f32,
            ],
            lights,
        }
    }
}

use bytemuck::Zeroable;

fn gpu_light(l: &LocalLight) -> GpuLight {
    let k = l.intensity / 255.0;
    let (spot, dir, co, inv) = match l.kind {
        LightKind::Point => (0.0, Vec3::ZERO, 0.0, 0.0),
        LightKind::Spot { dir, inner, outer } => {
            let (co, inv) = trino_core::light::spot_cone(inner, outer);
            (1.0, dir.normalized(), co, inv)
        }
    };
    GpuLight {
        pos_radius: [l.pos.x, l.pos.y, l.pos.z, l.radius.max(0.0)],
        color_spot: [
            l.color.r as f32 * k,
            l.color.g as f32 * k,
            l.color.b as f32 * k,
            spot,
        ],
        dir_cos: [dir.x, dir.y, dir.z, co],
        cone: [inv, 0.0, 0.0, 0.0],
    }
}

/// A frame's light sets.
#[derive(Default)]
pub struct LightSets {
    keys: Vec<SetKey>,
}

impl LightSets {
    pub fn clear(&mut self) {
        self.keys.clear();
    }

    /// The set index for `key`: the last set when it is the same, a new
    /// one otherwise (the last one once [`MAX_SETS`] are in use).
    pub fn index(&mut self, key: SetKey) -> u32 {
        if self.keys.last() != Some(&key) && self.keys.len() < MAX_SETS {
            self.keys.push(key);
        }
        self.keys.len() as u32 - 1
    }

    /// The uniform buffer's contents (always `MAX_SETS` sets).
    pub fn bytes(&self) -> Vec<u8> {
        let mut sets = [GpuSet::zeroed(); MAX_SETS];
        for (s, k) in sets.iter_mut().zip(&self.keys) {
            *s = k.gpu();
        }
        bytemuck::cast_slice(&sets).to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::Color;
    use trino_core::render3d::DEFAULT_LIGHT;

    #[test]
    fn layout_matches_the_shader() {
        assert_eq!(core::mem::size_of::<GpuLight>(), 64);
        assert_eq!(SET_BYTES, 48 + 64 * MAX_LOCAL_LIGHTS);
        assert_eq!(core::mem::offset_of!(GpuSet, lights), 48);
    }

    #[test]
    fn same_state_shares_a_set_and_the_table_caps() {
        let cam = Camera3::default();
        let mut locals = trino_core::LightList::EMPTY;
        let mut sets = LightSets::default();
        let k0 = SetKey::new(&cam, &DEFAULT_LIGHT, &locals, 0.0);
        assert_eq!(sets.index(k0), 0);
        assert_eq!(sets.index(k0), 0);
        locals.set(
            &[LocalLight::point(Vec3::ONE, Color::rgb(255, 0, 0), 3.0).with_intensity(2.0)],
            8,
        );
        let k1 = SetKey::new(&cam, &DEFAULT_LIGHT, &locals, 0.5);
        assert_eq!(sets.index(k1), 1);
        let g = k1.gpu();
        assert_eq!(g.eye[3], 1.0);
        assert_eq!(g.sun_color[3], 0.5);
        assert_eq!(g.lights[0].pos_radius, [1.0, 1.0, 1.0, 3.0]);
        assert_eq!(g.lights[0].color_spot, [2.0, 0.0, 0.0, 0.0]);
        for i in 0..20 {
            sets.index(SetKey::new(&cam, &DEFAULT_LIGHT, &locals, i as f32 / 20.0));
        }
        assert_eq!(sets.index(k0), MAX_SETS as u32 - 1);
        assert_eq!(sets.bytes().len(), SET_BYTES * MAX_SETS);
    }
}
