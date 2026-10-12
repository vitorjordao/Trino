//! CPU cost of the software T&L — what the N64/3DS backends pay per frame
//! (they rasterize on the GPU/RDP, but every transform, light, clip and
//! cull runs on the console CPU through `trino_core::tessellate`).
//!
//! A deterministic castle64 playthrough (the waypoint bot) renders through a
//! CPU-only `Renderer` that does exactly what the console backends do with
//! `draw_model`, minus the rasterizer. Run on demand:
//!
//! ```sh
//! cargo test --release -p xtask --test bench_tnl -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use trino_core::render3d::{Camera3, DEFAULT_LIGHT, Light, Mesh, ScreenTri};
use trino_core::{
    Caps, Color, Game, Mat34, Material, ModelId, ModelParams, Renderer, SpriteId, SpriteParams,
    Transform3, Vec2,
};

struct NullAudio;
impl trino_core::Audio for NullAudio {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// Mirrors the console backends' `draw_model` path without rasterizing.
struct CpuRenderer {
    meshes: HashMap<u32, Vec<u8>>,
    camera: Camera3,
    light: Light,
    tris: Vec<ScreenTri>,
    caps: Caps,
    emitted: u64,
}

impl Renderer for CpuRenderer {
    fn caps(&self) -> &Caps {
        &self.caps
    }
    fn begin_frame(&mut self, _clear: Color) {
        self.tris.clear();
    }
    fn draw_sprite(&mut self, _: SpriteId, _: Vec2, _: &SpriteParams) {}
    fn set_camera(&mut self, camera: &Camera3) {
        self.camera = *camera;
    }
    fn set_light(&mut self, light: &Light) {
        self.light = *light;
    }
    fn draw_model(&mut self, model: ModelId, t: &Transform3, m: Material, p: &ModelParams) {
        self.draw_model_matrix(model, &t.matrix(), m, p);
    }
    fn draw_model_matrix(&mut self, model: ModelId, matrix: &Mat34, _: Material, p: &ModelParams) {
        let Some(blob) = self.meshes.get(&model.0) else {
            return;
        };
        let mesh = Mesh::from_tmdl(blob).unwrap();
        let tris = &mut self.tris;
        trino_core::render3d::tessellate(
            &mesh,
            matrix,
            &self.camera,
            &self.light,
            p,
            Vec2::new(320.0, 240.0),
            &mut |t| tris.push(t),
        );
    }
    fn end_frame(&mut self) {
        // Backends depth-sort each batch: include it in the cost.
        self.tris
            .sort_unstable_by(|a, b| b.depth.partial_cmp(&a.depth).unwrap());
        self.emitted += self.tris.len() as u64;
    }
}

#[test]
#[ignore = "benchmark: run with --release --ignored --nocapture"]
fn castle64_tnl_cost_per_frame() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let baked = root.join("target/assets/pc");
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    let mut r = CpuRenderer {
        meshes: assets.models.into_iter().map(|m| (m.id, m.tmdl)).collect(),
        camera: Camera3::default(),
        light: DEFAULT_LIGHT,
        tris: Vec::new(),
        caps: Caps::N64,
        emitted: 0,
    };
    let mut game = castle64::Castle64Game::new(Vec2::new(320.0, 240.0));
    let mut bot = castle64::bot::Bot::new(castle64::bot::GREEN_RUN);
    let mut audio = NullAudio;
    const FRAMES: u32 = 1500;
    let mut render_time = 0.0f64;
    for _ in 0..FRAMES {
        let input = bot.drive(&game);
        game.update(&input, &mut audio, 1.0 / 60.0);
        let t = Instant::now();
        game.render(&mut r);
        render_time += t.elapsed().as_secs_f64();
    }
    let per_frame = render_time * 1000.0 / FRAMES as f64;
    println!(
        "castle64 T&L: {per_frame:.3} ms/frame (host CPU), {:.0} tris/frame emitted",
        r.emitted as f64 / FRAMES as f64
    );
}
