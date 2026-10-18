//! The water table (ADR 0015): every distinct water surface of a frame
//! (its swell, ripples, flowing detail layer, gloss, the light and the eye)
//! becomes one *set* in a uniform buffer, and each water vertex names its
//! set. The per-pixel tier (native profile) evaluates the swell's and the
//! ripples' normals, Fresnel, the glint and depth foam from it; the
//! console looks read only the detail layer's scroll. Pure code: the
//! layout is unit-tested without a GPU.

use trino_core::render3d::{Camera3, Light};
use trino_core::water::{MAX_RIPPLES, WaveSet};
use trino_core::{Water, WaterState};

/// Water sets per frame. A frame with more distinct surfaces draws the
/// rest with the last set.
pub const MAX_SETS: usize = 8;

/// One water surface. Must match `water.wgsl` (`WaterSet`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuWaterSet {
    /// The swell's two directions (x z, x z).
    pub dirs: [f32; 4],
    /// Their frequencies (cycles per unit) and phases (turns).
    pub k_phase: [f32; 4],
    /// Their heights, the ripple response, the gloss (0..1).
    pub amp: [f32; 4],
    /// The detail layer: UV scale (0 = none), scroll u v, ripple count.
    pub detail: [f32; 4],
    /// Ripples: x, z, age, strength.
    pub ripples: [[f32; 4]; MAX_RIPPLES],
    /// Direction the light travels (unit), ambient.
    pub sun_dir: [f32; 4],
    /// The light's color (0..1), the clock (seconds).
    pub sun_color: [f32; 4],
    /// The ambient color (0..1).
    pub ambient: [f32; 4],
    /// The reflected sky (0..1), fog on (1) or off (0).
    pub sky: [f32; 4],
    /// Fog near, far.
    pub fog: [f32; 4],
    /// The eye (world).
    pub eye: [f32; 4],
    /// The view's forward axis (unit, world): view depth = (p - eye) · fwd.
    pub fwd: [f32; 4],
}

pub const SET_BYTES: usize = core::mem::size_of::<GpuWaterSet>();

/// What makes two water draws share a set.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SetKey {
    pub water: Water,
    pub state: WaterState,
    pub gloss: u8,
    /// The detail layer's UV scale (0 = none).
    pub detail_scale: f32,
    pub camera: Camera3,
    pub light: Light,
}

impl SetKey {
    pub fn gpu(&self) -> GpuWaterSet {
        let w = WaveSet::new(&self.water, self.state.time);
        let dscroll = self
            .water
            .detail_offset(self.state.time, self.detail_scale.max(1.0));
        let mut ripples = [[0.0; 4]; MAX_RIPPLES];
        let rs = self.state.ripples();
        for (g, r) in ripples.iter_mut().zip(rs) {
            *g = [r.pos.x, r.pos.y, r.age, r.strength];
        }
        let c = |v: u8| v as f32 / 255.0;
        let l = &self.light;
        let d = l.dir.normalized();
        let fog = l.fog.enabled();
        let sky = if fog { l.fog.color } else { l.ambient_color };
        let fwd = (self.camera.target - self.camera.eye).normalized();
        let eye = self.camera.eye;
        GpuWaterSet {
            dirs: [w.dir[0].x, w.dir[0].y, w.dir[1].x, w.dir[1].y],
            k_phase: [w.k[0], w.k[1], w.phase[0], w.phase[1]],
            amp: [
                w.amp[0],
                w.amp[1],
                self.water.ripple_responsiveness,
                self.gloss as f32 / 255.0,
            ],
            detail: [self.detail_scale, dscroll.x, dscroll.y, rs.len() as f32],
            ripples,
            sun_dir: [d.x, d.y, d.z, l.ambient],
            sun_color: [c(l.color.r), c(l.color.g), c(l.color.b), self.state.time],
            ambient: [
                c(l.ambient_color.r),
                c(l.ambient_color.g),
                c(l.ambient_color.b),
                0.0,
            ],
            sky: [c(sky.r), c(sky.g), c(sky.b), fog as u32 as f32],
            fog: [l.fog.near, l.fog.far, trino_core::render3d::NEAR_PLANE, 0.0],
            eye: [eye.x, eye.y, eye.z, 0.0],
            fwd: [fwd.x, fwd.y, fwd.z, 0.0],
        }
    }
}

/// A frame's water sets.
#[derive(Default)]
pub struct WaterSets {
    keys: Vec<SetKey>,
}

impl WaterSets {
    pub fn clear(&mut self) {
        self.keys.clear();
    }

    /// The set index for `key` (shared with an equal one, the last once
    /// [`MAX_SETS`] are in use).
    pub fn index(&mut self, key: SetKey) -> u32 {
        if let Some(i) = self.keys.iter().position(|k| *k == key) {
            return i as u32;
        }
        if self.keys.len() < MAX_SETS {
            self.keys.push(key);
        }
        self.keys.len() as u32 - 1
    }

    /// The uniform buffer's contents (always `MAX_SETS` sets).
    pub fn bytes(&self) -> Vec<u8> {
        let mut sets = [<GpuWaterSet as bytemuck::Zeroable>::zeroed(); MAX_SETS];
        for (s, k) in sets.iter_mut().zip(&self.keys) {
            *s = k.gpu();
        }
        bytemuck::cast_slice(&sets).to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::render3d::DEFAULT_LIGHT;
    use trino_core::{Ripple, Vec2};

    #[test]
    fn layout_matches_the_shader() {
        // 15 vec4s, 16-byte aligned (WGSL uniform layout).
        assert_eq!(SET_BYTES, 15 * 16);
        assert_eq!(core::mem::offset_of!(GpuWaterSet, ripples), 64);
        assert_eq!(core::mem::offset_of!(GpuWaterSet, sun_dir), 128);
    }

    #[test]
    fn sets_carry_the_swell_the_ripples_and_the_scroll() {
        let r = Ripple {
            pos: Vec2::new(1.0, 2.0),
            age: 0.5,
            strength: 0.1,
        };
        let key = SetKey {
            water: Water::DEFAULT,
            state: WaterState::new(2.0, &[r], 4),
            gloss: 255,
            detail_scale: 2.0,
            camera: Camera3::default(),
            light: DEFAULT_LIGHT,
        };
        let g = key.gpu();
        assert_eq!(g.ripples[0], [1.0, 2.0, 0.5, 0.1]);
        assert_eq!(g.detail[0], 2.0);
        assert_eq!(g.detail[3], 1.0);
        assert_eq!(g.amp[3], 1.0);
        let (b, d) = Water::DEFAULT.scroll(2.0);
        let f = trino_core::water::frac;
        assert_eq!(
            [g.detail[1], g.detail[2]],
            [f(d.x - b.x * 2.0), f(d.y - b.y * 2.0)]
        );
        let mut sets = WaterSets::default();
        assert_eq!(sets.index(key), 0);
        assert_eq!(sets.index(key), 0);
        let other = SetKey { gloss: 3, ..key };
        assert_eq!(sets.index(other), 1);
        assert_eq!(sets.bytes().len(), SET_BYTES * MAX_SETS);
    }
}
