//! The auto-detect bench (issue #55, ADR 0022 §2): a short, fixed 3D scene
//! drawn at each preset, Low first, on the renderer the game will use.
//!
//! The scene is a typical native frame: 64 lit spheres (~100k triangles)
//! on a floor, four point lights, AO and a HUD rectangle. Each preset draws
//! two warm-up frames (pipelines build lazily) and then up to twelve timed
//! ones, each from `begin_frame` to the GPU's idle (`wait_gpu`): the CPU's
//! and the GPU's work together, which is what a frame costs. The walk
//! stops at the first preset whose median misses the target, or when the
//! budget is spent. Nothing is shown (the window isn't presented to: a
//! FIFO swap chain would round every frame up to the refresh) and the
//! renderer's state comes back as it was.

use trino_core::{
    AmbientOcclusion, Camera3, Color, GpuClass, LocalLight, Material, ModelId, ModelParams,
    PresetTable, QualityPreset, Rect, Renderer, Transform3, Vec3,
};
use web_time::{Duration, Instant};

use super::PcRenderer;

/// What a calibration measured and picked.
#[derive(Clone, Debug, PartialEq)]
pub struct Calibration {
    /// The median frame time per preset walked, µs (Low first; the walk
    /// stops at the first one over the target).
    pub results: Vec<(QualityPreset, u32)>,
    /// The highest preset that held the target (Low when none did; a
    /// software adapter never gets more).
    pub pick: QualityPreset,
    pub gpu: GpuClass,
    /// The target each preset's median was held against, µs.
    pub target_us: u32,
    pub elapsed: Duration,
}

impl Calibration {
    /// The picked preset's median frame time, µs.
    pub fn gpu_us(&self) -> u32 {
        self.results
            .iter()
            .find(|(p, _)| *p == self.pick)
            .or(self.results.first())
            .map_or(0, |(_, us)| *us)
    }

    /// `preset=us` per preset walked, then the pick: one line for logs.
    pub fn summary(&self) -> String {
        let mut s = String::new();
        for (p, us) in &self.results {
            s.push_str(&format!("{}={} ", p.key(), us));
        }
        format!(
            "{s}pick={} gpu={} target_us={} elapsed_ms={}",
            self.pick.key(),
            self.gpu.key(),
            self.target_us,
            self.elapsed.as_millis()
        )
    }
}

const SPHERE: ModelId = ModelId::from_path("trino/bench/sphere");
const FLOOR: ModelId = ModelId::from_path("trino/bench/floor");
const WARM_UP: u32 = 2;
const TIMED: u32 = 12;

/// A calibration in progress, one frame at a time
/// ([`PcRenderer::bench_begin`], [`PcRenderer::bench_step`]): the PC
/// times each frame to the GPU's idle in a loop
/// ([`PcRenderer::calibrate`]); the Web, which can't wait for its GPU,
/// draws one per animation frame, presented, and times the intervals.
pub struct Bench {
    table: PresetTable,
    target_us: u32,
    budget: Duration,
    start: Instant,
    /// The renderer's state before, put back at the end.
    saved: Saved,
    /// The preset being drawn (an index into `QualityPreset::NAMED`).
    preset: usize,
    /// Frames drawn at it.
    frame: u32,
    since: Instant,
    times: Vec<u32>,
    results: Vec<(QualityPreset, u32)>,
    pick: QualityPreset,
}

type Saved = (
    super::QualityState,
    Option<crate::dynres::DynResController>,
    bool,
    Camera3,
    trino_core::render3d::Light,
    trino_core::TexFilter,
);

impl PcRenderer {
    /// Draw the bench at each of `table`'s presets (dynamic resolution off,
    /// no frame cap) at the current picture size and pick the highest whose
    /// median frame holds `target_us` (90% of a 60 Hz frame is the usual
    /// one), spending at most about `budget`. The console profiles keep
    /// their look: they return Low unmeasured.
    pub fn calibrate(
        &mut self,
        table: &PresetTable,
        target_us: u32,
        budget: Duration,
    ) -> Calibration {
        let mut bench = self.bench_begin(table, target_us, budget, false);
        let mut last = None;
        loop {
            let f = Instant::now();
            if let Some(c) = self.bench_step(&mut bench, last) {
                return c;
            }
            self.wait_gpu();
            last = Some(f.elapsed().as_micros().min(u32::MAX as u128) as u32);
        }
    }

    /// Start a calibration [`bench_step`](Self::bench_step) draws frame by
    /// frame; `present` shows its frames (the Web's, timed by their
    /// intervals: an unpresented frame never paces the page).
    pub fn bench_begin(
        &mut self,
        table: &PresetTable,
        target_us: u32,
        budget: Duration,
        present: bool,
    ) -> Bench {
        let saved = (
            self.quality,
            self.dynres.clone(),
            self.msaa > 1,
            self.camera,
            self.light,
            self.game_filter,
        );
        let mut bench = Bench {
            table: *table,
            target_us,
            budget,
            start: Instant::now(),
            saved,
            preset: 0,
            frame: 0,
            since: Instant::now(),
            times: Vec::new(),
            results: Vec::new(),
            pick: QualityPreset::Low,
        };
        if !self.profile.is_native() {
            bench.preset = QualityPreset::NAMED.len();
            return bench;
        }
        self.upload_mesh(SPHERE, sphere_tmdl(24, 32));
        self.upload_mesh(FLOOR, floor_tmdl(16));
        self.no_present = !present;
        self.bench_preset(&mut bench);
        bench
    }

    /// One bench frame. `last_us` is the previous frame's time (the
    /// time to the GPU's idle, or the interval since it was presented).
    /// `Some` when done: the pick, the renderer as it was.
    pub fn bench_step(&mut self, b: &mut Bench, last_us: Option<u32>) -> Option<Calibration> {
        let presets = QualityPreset::NAMED.len();
        if b.preset < presets {
            if let Some(us) = last_us.filter(|_| b.frame > WARM_UP) {
                b.times.push(us);
            }
            let per_preset = b.budget / presets as u32;
            // However slow the adapter, Low draws a frame and reports it.
            let low = b.preset == 0;
            let spent = b.start.elapsed() >= b.budget && (b.frame > 0 || !low);
            if spent && low && b.times.is_empty() {
                // Out of time inside the warm-up (WARP on CI's Windows VM):
                // its last frame stands in for the median.
                b.times.extend(last_us);
            }
            let enough = b.frame >= WARM_UP + TIMED
                || (b.since.elapsed() >= per_preset && b.times.len() >= 3);
            if spent || enough {
                let mut next = !spent;
                if b.times.is_empty() {
                    next = false;
                } else {
                    b.times.sort_unstable();
                    let median = b.times[b.times.len() / 2];
                    let preset = QualityPreset::NAMED[b.preset];
                    b.results.push((preset, median));
                    if median > b.target_us {
                        next = false;
                    } else {
                        b.pick = preset;
                    }
                }
                b.preset = if next { b.preset + 1 } else { presets };
                if b.preset < presets {
                    self.bench_preset(b);
                }
            }
        }
        if b.preset >= presets {
            return Some(self.bench_end(b));
        }
        self.bench_frame(b.preset as u32 * 16 + b.frame);
        b.frame += 1;
        None
    }

    /// Put the renderer on the bench's current preset, its counters at 0.
    fn bench_preset(&mut self, b: &mut Bench) {
        let mut q = b.table.get(QualityPreset::NAMED[b.preset]);
        q.dynamic_resolution = None;
        q.frame_limit = trino_core::FrameLimit::Unlimited;
        self.apply_quality(&q);
        b.frame = 0;
        b.since = Instant::now();
        b.times.clear();
    }

    fn bench_end(&mut self, b: &mut Bench) -> Calibration {
        let gpu = self.gpu_class();
        let mut pick = b.pick;
        if gpu == GpuClass::Software {
            pick = QualityPreset::Low;
        }
        if self.profile.is_native() {
            self.no_present = false;
            self.forget_mesh(SPHERE);
            self.forget_mesh(FLOOR);
            let (quality, dynres, msaa, camera, light, filter) = b.saved.clone();
            match quality.applied {
                Some(q) => self.apply_quality(&q),
                None => {
                    self.set_msaa(msaa);
                    self.set_anisotropy(1);
                    self.quality = quality;
                    self.set_render_scale(quality.scale);
                    self.apply_present_mode();
                }
            }
            self.dynres = dynres;
            // The restored controller's scale, in targets for its range.
            if let Some(c) = &self.dynres {
                self.set_render_scale(c.scale());
            }
            self.camera = camera;
            self.light = light;
            self.set_texture_filter(filter);
        }
        Calibration {
            results: std::mem::take(&mut b.results),
            pick,
            gpu,
            target_us: b.target_us,
            elapsed: b.start.elapsed(),
        }
    }

    /// The kind of adapter this renderer runs on (from its device type,
    /// else its name: llvmpipe, lavapipe, SwiftShader and WARP are
    /// software).
    pub fn gpu_class(&self) -> GpuClass {
        self.gpu_class
    }

    /// Drop a mesh [`upload_mesh`](Self::upload_mesh) registered.
    fn forget_mesh(&mut self, id: ModelId) {
        self.meshes.remove(&id.0);
        if let Some(gpu) = &mut self.gpu {
            gpu.forget_mesh(id.0);
        }
    }

    fn bench_frame(&mut self, i: u32) {
        let (w, h) = self.internal_size;
        self.begin_frame(Color::rgb(40, 52, 70));
        let t = i as f32 * 0.05;
        self.set_camera(&Camera3 {
            eye: Vec3::new(9.0 * t.sin(), 4.5, -9.0 * t.cos()),
            target: Vec3::new(0.0, 0.4, 0.0),
            fov_y: 60f32.to_radians(),
        });
        self.set_light(&trino_core::render3d::DEFAULT_LIGHT);
        let lights = [
            (Vec3::new(-3.0, 1.5, -3.0), Color::rgb(255, 120, 80)),
            (Vec3::new(3.0, 1.5, -3.0), Color::rgb(80, 160, 255)),
            (Vec3::new(-3.0, 1.5, 3.0), Color::rgb(120, 255, 120)),
            (Vec3::new(3.0, 1.5, 3.0), Color::rgb(255, 230, 120)),
        ]
        .map(|(p, c)| LocalLight::point(p, c, 4.0));
        self.set_local_lights(&lights);
        self.set_ambient_occlusion(Some(&AmbientOcclusion::DEFAULT));
        let params = ModelParams {
            gloss: 96,
            ..ModelParams::default()
        };
        self.draw_model(
            FLOOR,
            &Transform3 {
                position: Vec3::ZERO,
                rotation: Vec3::ZERO,
                scale: Vec3::new(12.0, 1.0, 12.0),
            },
            Material::VertexLit,
            &params,
        );
        for z in 0..8 {
            for x in 0..8 {
                let position = Vec3::new(x as f32 * 1.3 - 4.55, 0.5, z as f32 * 1.3 - 4.55);
                self.draw_model(
                    SPHERE,
                    &Transform3 {
                        position,
                        ..Transform3::default()
                    },
                    Material::VertexLit,
                    &params,
                );
            }
        }
        // The HUD: a bar along the bottom, at the picture's resolution.
        self.fill_rect(
            Rect::new(8.0, h as f32 - 28.0, w as f32 - 16.0, 20.0),
            Color::rgba(0, 0, 0, 160),
        );
        self.end_frame();
    }
}

/// A unit-diameter UV sphere, `rings` × `segments`, shaded from blue
/// (top) to orange (bottom).
fn sphere_tmdl(rings: u16, segments: u16) -> Vec<u8> {
    use core::f32::consts::{PI, TAU};
    let (mut pos, mut nrm, mut col, mut idx) = (vec![], vec![], vec![], vec![]);
    for i in 0..=rings {
        let th = PI * i as f32 / rings as f32;
        let k = i as f32 / rings as f32;
        let c = [
            (90.0 + 160.0 * k) as u8,
            (140.0 + 20.0 * k) as u8,
            (230.0 - 170.0 * k) as u8,
            255,
        ];
        for j in 0..=segments {
            let ph = TAU * j as f32 / segments as f32;
            let n = Vec3::new(th.sin() * ph.cos(), th.cos(), th.sin() * ph.sin());
            pos.push(n * 0.5);
            nrm.push(n);
            col.push(c);
        }
    }
    let row = segments + 1;
    for i in 0..rings {
        for j in 0..segments {
            let a = i * row + j;
            // (θ, φ) → (θ, φ+1) → (θ+1, φ+1): outward-facing.
            idx.extend_from_slice(&[a, a + 1, a + row + 1, a, a + row + 1, a + row]);
        }
    }
    with_bounds(tmdl(&pos, &nrm, &col, &idx), Vec3::ZERO, 0.5)
}

/// A unit floor (y = 0, facing up) of `cells` × `cells` checkered quads.
fn floor_tmdl(cells: u16) -> Vec<u8> {
    let (mut pos, mut nrm, mut col, mut idx) = (vec![], vec![], vec![], vec![]);
    let up = Vec3::new(0.0, 1.0, 0.0);
    for z in 0..cells {
        for x in 0..cells {
            let c = if (x + z) % 2 == 0 {
                [150, 150, 140, 255]
            } else {
                [110, 112, 104, 255]
            };
            let base = pos.len() as u16;
            let (x0, z0) = (x as f32 / cells as f32 - 0.5, z as f32 / cells as f32 - 0.5);
            let d = 1.0 / cells as f32;
            for (dx, dz) in [(0.0, 0.0), (0.0, d), (d, d), (d, 0.0)] {
                pos.push(Vec3::new(x0 + dx, 0.0, z0 + dz));
                nrm.push(up);
                col.push(c);
            }
            idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    with_bounds(tmdl(&pos, &nrm, &col, &idx), Vec3::ZERO, 0.71)
}

/// The base TMDL layout: header, positions, normals, colors, indices.
fn tmdl(positions: &[Vec3], normals: &[Vec3], colors: &[[u8; 4]], indices: &[u16]) -> Vec<u8> {
    let mut blob = Vec::new();
    blob.extend_from_slice(trino_core::render3d::TMDL_MAGIC);
    blob.extend_from_slice(&(positions.len() as u32).to_le_bytes());
    blob.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    for v in positions.iter().chain(normals) {
        for f in [v.x, v.y, v.z] {
            blob.extend_from_slice(&f.to_le_bytes());
        }
    }
    for c in colors {
        blob.extend_from_slice(c);
    }
    for i in indices {
        blob.extend_from_slice(&i.to_le_bytes());
    }
    blob
}

/// Append a `BNDS` section (the bounding sphere culling reads).
fn with_bounds(mut blob: Vec<u8>, center: Vec3, radius: f32) -> Vec<u8> {
    blob.extend_from_slice(b"BNDS");
    blob.extend_from_slice(&16u32.to_le_bytes());
    for f in [center.x, center.y, center.z, radius] {
        blob.extend_from_slice(&f.to_le_bytes());
    }
    blob
}

/// The class of an adapter from what wgpu reports.
pub(super) fn classify_adapter(info: &wgpu::AdapterInfo) -> GpuClass {
    let name = info.name.to_ascii_lowercase();
    if ["llvmpipe", "lavapipe", "swiftshader", "warp", "software"]
        .iter()
        .any(|s| name.contains(s))
    {
        return GpuClass::Software;
    }
    match info.device_type {
        wgpu::DeviceType::Cpu => GpuClass::Software,
        wgpu::DeviceType::IntegratedGpu => GpuClass::Integrated,
        wgpu::DeviceType::DiscreteGpu => GpuClass::Discrete,
        wgpu::DeviceType::VirtualGpu | wgpu::DeviceType::Other => GpuClass::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bench_meshes_parse() {
        for blob in [sphere_tmdl(24, 32), floor_tmdl(16)] {
            let mesh = trino_core::render3d::Mesh::from_tmdl(&blob).expect("valid TMDL");
            assert!(mesh.bounds().is_some());
        }
        let blob = sphere_tmdl(24, 32);
        let s = trino_core::render3d::Mesh::from_tmdl(&blob).unwrap();
        assert_eq!(s.index_count, 24 * 32 * 6);
    }
}
