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

/// Rigid vs smooth skinning on the same 2-joint mesh (a 24-segment capsule
/// bent at its middle): the extra transform per blended vertex is the whole
/// cost difference. Host CPU numbers; the ratio is what carries to consoles.
#[test]
#[ignore = "benchmark: run with --release --ignored --nocapture"]
fn skinning_rigid_vs_smooth() {
    use trino_asset_pipeline::model::{JointData, SkeletonData, Trs};
    use trino_asset_pipeline::{Primitive, Shape};
    let mut data = Primitive {
        shape: Shape::Capsule {
            radius: 0.4,
            height: 2.0,
        },
        segments: 24,
        ..Primitive::default()
    }
    .build();
    // Upper half follows joint 1; worst case: every vertex blends 50/50
    // with the other joint (real rigs blend only around the joints).
    let joint_of_vertex: Vec<u8> = data.positions.iter().map(|p| (p[1] > 0.0) as u8).collect();
    let blend: Vec<[u8; 2]> = data
        .positions
        .iter()
        .map(|p| [(p[1] <= 0.0) as u8, 128])
        .collect();
    let rest = Trs {
        t: [0.0; 3],
        r: [0.0, 0.0, 0.0, 1.0],
        s: [1.0; 3],
    };
    let ident = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ];
    data.skeleton = Some(SkeletonData {
        joint_of_vertex,
        joints: vec![
            JointData {
                parent: -1,
                inverse_bind: ident,
                rest,
            },
            JointData {
                parent: 0,
                inverse_bind: ident,
                rest,
            },
        ],
        clips: Vec::new(),
    });
    let rigid = data.encode().unwrap();
    data.blend = blend;
    let smooth = data.encode().unwrap();
    let camera = Camera3::default();
    let time = |blob: &[u8]| {
        let mesh = Mesh::from_tmdl(blob).unwrap();
        let mut tris = Vec::new();
        const DRAWS: u32 = 4000;
        let t = Instant::now();
        for _ in 0..DRAWS {
            tris.clear();
            trino_core::render3d::tessellate(
                &mesh,
                &Mat34::IDENTITY,
                &camera,
                &DEFAULT_LIGHT,
                &ModelParams::DEFAULT,
                Vec2::new(320.0, 240.0),
                &mut |tri| tris.push(tri),
            );
        }
        (
            t.elapsed().as_secs_f64() * 1e6 / DRAWS as f64,
            mesh.vertex_count,
            tris.len(),
        )
    };
    let (r_us, verts, tris) = time(&rigid);
    let (s_us, _, _) = time(&smooth);
    println!(
        "skinning ({verts} verts, {tris} tris): rigid {r_us:.2} us/draw, smooth {s_us:.2} us/draw \
         ({:+.0}%)",
        (s_us / r_us - 1.0) * 100.0
    );
    assert!(Mesh::from_tmdl(&smooth).unwrap().has_blend());
}
