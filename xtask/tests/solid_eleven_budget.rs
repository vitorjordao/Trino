//! Solid Eleven's N64 frame on the host: record every 3D draw of the
//! self-test's scripted match (the N64 profile's draw list), and check that
//! each one stays on the N64's RSP (`trino_core::render3d::rsp_check`), and
//! that the frame's draw and triangle counts stay in budget. A draw that
//! falls back to the CPU costs ~1.5 ms of emulated time (two pitch tiles
//! under the camera once did); this names the reason without a console
//! build.

use std::collections::HashMap;

use solid_eleven::SolidElevenGame;
use solid_eleven::selftest;
use trino_core::render3d::{Mesh, TnlContext, rsp_check, rsp_mesh_check};
use trino_core::{
    Camera3, Caps, Color, DEFAULT_LIGHT, FontId, Game, Light, Mat34, Material, ModelId,
    ModelParams, Rect, Renderer, SpriteId, SpriteParams, TextParams, Transform3, Vec2,
};

struct Mute;
impl trino_core::Audio for Mute {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

#[derive(Default)]
struct Rec {
    camera: Option<Camera3>,
    light: Option<Light>,
    draws: Vec<(ModelId, Mat34, ModelParams, Camera3)>,
    rects: u32,
    glyphs: u32,
}

impl Renderer for Rec {
    fn caps(&self) -> &Caps {
        &Caps::N64
    }
    fn begin_frame(&mut self, _: Color) {
        self.draws.clear();
        self.rects = 0;
        self.glyphs = 0;
    }
    fn draw_sprite(&mut self, _: SpriteId, _: Vec2, _: &SpriteParams) {}
    fn set_camera(&mut self, c: &Camera3) {
        self.camera = Some(*c);
    }
    fn set_light(&mut self, l: &Light) {
        self.light = Some(*l);
    }
    fn draw_model(&mut self, m: ModelId, t: &Transform3, _: Material, p: &ModelParams) {
        let cam = self.camera.unwrap();
        self.draws.push((m, t.matrix(), *p, cam));
    }
    fn draw_model_matrix(&mut self, m: ModelId, mat: &Mat34, _: Material, p: &ModelParams) {
        let cam = self.camera.unwrap();
        self.draws.push((m, *mat, *p, cam));
    }
    fn end_frame(&mut self) {}
    fn fill_rect(&mut self, _: Rect, _: Color) {
        self.rects += 1;
    }
    fn draw_text(&mut self, _: FontId, _: Vec2, t: &str, _: &TextParams) {
        self.glyphs += t.chars().filter(|c| *c != ' ').count() as u32;
    }
    fn measure_text(&self, _: FontId, t: &str, scale: f32) -> Vec2 {
        Vec2::new(t.len() as f32 * 6.0 * scale, 11.0 * scale)
    }
}

/// Every model's TMDL, baked like the engine bakes it.
fn meshes() -> HashMap<u32, Vec<u8>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let baked = root.join("target/assets/solid-eleven/pc");
    trino_asset_pipeline::bake_all(
        &root.join("examples/solid-eleven/assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    assets.models.into_iter().map(|m| (m.id, m.tmdl)).collect()
}

#[test]
fn every_match_draw_stays_on_the_rsp() {
    let meshes = meshes();
    let manifest = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/solid-eleven/assets/manifest.toml"),
    )
    .unwrap();
    let names: HashMap<u32, String> = manifest
        .lines()
        .filter_map(|l| l.strip_prefix("[models.")?.strip_suffix(']'))
        .map(|n| (trino_core::asset_id(&format!("models/{n}")), n.to_string()))
        .collect();
    let mut g = SolidElevenGame::new(Vec2::new(320.0, 240.0));
    selftest::begin(&mut g);
    let mut r = Rec::default();
    let mut worst = (0usize, 0u32);
    let mut frames = 0;
    for f in 0..selftest::FRAMES {
        g.update(&selftest::input(f), &mut Mute, selftest::DT);
        if f % 10 != 0 {
            continue;
        }
        g.render(&mut r);
        frames += 1;
        let light = r.light.unwrap_or(DEFAULT_LIGHT);
        let mut tris = 0u32;
        for (id, mat, params, cam) in &r.draws {
            let bytes = meshes
                .get(&id.0)
                .unwrap_or_else(|| panic!("frame {f}: model {:08x} not baked", id.0));
            let mesh = Mesh::from_tmdl(bytes).unwrap();
            tris += (mesh.index_count / 3) as u32;
            rsp_mesh_check(&mesh)
                .unwrap_or_else(|e| panic!("frame {f}: model {:08x}: mesh {e:?}", id.0));
            let ctx = TnlContext::new(cam, &light, Vec2::new(320.0, 240.0));
            if let Err(e) = rsp_check(&ctx, &mesh, mat, params)
                && std::env::var("SE_DIAG").is_ok()
            {
                let (c, rad) = mesh.bounds().unwrap();
                let v = ctx.projection().0.mul(mat).transform_point(c);
                eprintln!(
                    "frame {f}: {:08x} {e:?} view c=({:.3},{:.3},{:.3}) r={:.3}",
                    id.0,
                    v.x * 64.0,
                    v.y * 64.0,
                    v.z * 64.0,
                    rad
                );
                continue;
            }
            if let Err(e) = rsp_check(&ctx, &mesh, mat, params) {
                // Off screen is fine (the engine culls it for ~16 µs); the
                // game culls most of those itself.
                assert_eq!(
                    e,
                    trino_core::RspReject::Offscreen,
                    "frame {f}: model {:08x} leaves the RSP: {e:?} (matrix {mat:?})",
                    id.0
                );
            }
        }
        if std::env::var("SE_DIAG").is_ok() && r.draws.len() > 95 {
            let mut cats: std::collections::BTreeMap<&str, u32> = Default::default();
            for (id, ..) in &r.draws {
                let name = names.get(&id.0).map(String::as_str).unwrap_or("?");
                let cat = if name.starts_with("se_pitch_f") {
                    "pitch fine"
                } else if name.starts_with("se_pitch") {
                    "pitch coarse"
                } else if name.starts_with("se_stand") || name.starts_with("se_flood") {
                    "stadium"
                } else {
                    name
                };
                *cats.entry(cat).or_default() += 1;
            }
            eprintln!("frame {f}: {} draws {cats:?}", r.draws.len());
        }
        if r.draws.len() > worst.0 {
            worst = (r.draws.len(), tris);
        }
        // The N64 frame: under 110 3D draws, under 2 400 source triangles
        // (~14 ms of RSP), and a light 2D overlay.
        assert!(r.draws.len() <= 110, "frame {f}: {} draws", r.draws.len());
        assert!(tris <= 2_400, "frame {f}: {tris} triangles");
        assert!(
            r.rects + r.glyphs <= 140,
            "frame {f}: {} rects + {} glyphs",
            r.rects,
            r.glyphs
        );
    }
    assert!(frames >= 30);
    println!("worst frame: {} draws, {} triangles", worst.0, worst.1);
}
