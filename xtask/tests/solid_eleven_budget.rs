//! Solid Eleven's N64 frame on the host: record every 3D draw of the
//! self-test's scripted match (the N64 profile's draw list), and check that
//! each one stays on the N64's RSP (`trino_core::render3d::rsp_check`), and
//! that the frame's draw and triangle counts stay in budget. A draw that
//! falls back to the CPU costs ~1.5 ms of emulated time (a ground tile
//! under the camera once did, until the engine checked a draw whose
//! bounding sphere crosses the band vertex by vertex); this names the
//! reason without a console build.

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
    // One bake at a time in this test binary (its tests run in
    // parallel and share the directory).
    static BAKE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _bake = BAKE.lock().unwrap_or_else(|e| e.into_inner());
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
                // Off screen is fine (the engine culls it for ~16 µs; the
                // game culls most of those itself). Anything else — the
                // fine ground tile under the camera included, now checked
                // vertex by vertex — must stay on the RSP.
                assert_eq!(
                    e,
                    trino_core::RspReject::Offscreen,
                    "frame {f}: {} leaves the RSP: {e:?} (matrix {mat:?})",
                    names.get(&id.0).map_or("?", |n| n.as_str())
                );
            }
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

const BASELINE: &str = "tests/perf/solid_eleven_work.toml";

/// The scripted match's cost, pinned: every 30th frame's 3D draws, source
/// triangles, the 3DS GPU's share (`gpu_check` on the 400-wide top
/// screen), the blob shadows cast (ADR 0025: each is a few CPU triangles
/// on the consoles) and the 2D overlay's rectangles and glyphs.
/// Deterministic (the sim is fixed point, the camera and culling are the
/// game's), so a number that moves is a content or engine change: rewrite with
/// `TRINO_BLESS=1 cargo test -p xtask --test solid_eleven_budget` and
/// review — a number going up is a cost the N64 pays.
#[test]
fn the_match_frame_cost_is_pinned() {
    use std::fmt::Write as _;
    let meshes = meshes();
    let mut g = SolidElevenGame::new(Vec2::new(320.0, 240.0));
    selftest::begin(&mut g);
    let mut r = Rec::default();
    let mut out = String::from(
        "# Solid Eleven's self-test match, frame by frame (every 30th of\n\
         # solid_eleven::selftest's script, N64 profile): 3D draws, source\n\
         # triangles, the 3DS GPU's share of them, blob shadows, 2D\n\
         # rectangles + glyphs.\n\
         # Checked for equality by xtask/tests/solid_eleven_budget.rs;\n\
         # rewrite with `TRINO_BLESS=1 cargo test -p xtask --test\n\
         # solid_eleven_budget` and review: a number going up is a cost the\n\
         # N64 pays.\n",
    );
    for f in 0..selftest::FRAMES {
        g.update(&selftest::input(f), &mut Mute, selftest::DT);
        if f % 30 != 29 {
            continue;
        }
        g.render(&mut r);
        let light = r.light.unwrap_or(DEFAULT_LIGHT);
        let (mut tris, mut gpu_draws) = (0u32, 0u32);
        let blobs = r
            .draws
            .iter()
            .filter(|d| matches!(d.2.shadow, trino_core::shadow::ShadowCast::Blob { .. }))
            .count();
        for (id, mat, params, cam) in &r.draws {
            let mesh = Mesh::from_tmdl(&meshes[&id.0]).unwrap();
            tris += (mesh.index_count / 3) as u32;
            let ctx = TnlContext::new(cam, &light, Vec2::new(400.0, 240.0));
            if trino_core::render3d::gpu_mesh_check(&mesh).is_ok()
                && trino_core::render3d::gpu_check(&ctx, &mesh, mat, params).is_ok()
            {
                gpu_draws += 1;
            }
        }
        let _ = writeln!(
            out,
            "\n[frame-{f}]\ndraws = {}\ntris = {tris}\ngpu_draws_3ds = {gpu_draws}\nblobs = {blobs}\nrects = {}\nglyphs = {}",
            r.draws.len(),
            r.rects,
            r.glyphs
        );
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(BASELINE);
    if std::env::var("TRINO_BLESS").is_ok() {
        std::fs::write(&path, &out).unwrap();
        eprintln!("blessed {}", path.display());
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{BASELINE} missing: run with TRINO_BLESS=1 and commit it"));
    if want != out {
        let diff: Vec<String> = want
            .lines()
            .zip(out.lines())
            .filter(|(a, b)| a != b)
            .map(|(a, b)| format!("  {a}  ->  {b}"))
            .collect();
        panic!(
            "Solid Eleven's frame cost changed ({BASELINE}); if intended, re-bless:\n{}",
            diff.join("\n")
        );
    }
}

/// Issue #86: the referee's card, held up where he books a player, is an
/// N64 RSP draw like the bodies (and a 3DS GPU draw), near the camera and
/// far from it (where the referee keeps his arms to hold it: the far
/// console body has none). The scripted match has no booking, so this
/// stages one: a slide from behind, a yellow.
#[test]
fn the_referees_card_stays_on_the_rsp_near_and_far() {
    use solid_eleven::render::{FAR_DEPTH, M_CARD, Sky};
    use solid_eleven::sim::ball::Ball;
    use solid_eleven::sim::engine::{Act, Pad, Phase, SIDE};
    use solid_eleven::sim::fx::{Fx, V2};
    let meshes = meshes();
    let mut g =
        SolidElevenGame::exhibition(Vec2::new(320.0, 240.0), 0, 1, 21, [false, false], Sky::Day);
    let tick = |g: &mut SolidElevenGame| {
        g.update(&trino_core::InputState::default(), &mut Mute, 1.0 / 60.0)
    };
    for _ in 0..60 * 30 {
        if g.session.as_ref().unwrap().m.phase == Phase::Play {
            break;
        }
        tick(&mut g);
    }
    let m = &mut g.session.as_mut().unwrap().m;
    m.humans = [false, true];
    let (carrier, tackler) = (9, SIDE + 4);
    let at = V2::new(-Fx::int(5), Fx::int(6));
    let fwd = m.attacks(0).sign();
    m.players[carrier].pos = at;
    m.players[carrier].face = V2::new(fwd, Fx::ZERO);
    m.players[carrier].vel = V2::ZERO;
    m.players[carrier].act = Act::Free;
    m.ball = Ball::at_rest(at + V2::new(fwd.mul(Fx::milli(1_500)), Fx::ZERO));
    m.owner = Some(carrier as u8);
    m.players[tackler].pos = at - V2::new(fwd.mul(Fx::milli(1_200)), Fx::ZERO);
    m.players[tackler].face = V2::new(fwd, Fx::ZERO);
    m.players[tackler].act = Act::Free;
    m.active[1] = tackler as u8;
    let slide = Pad {
        slide: true,
        mv: V2::new(fwd, Fx::ZERO),
        ..Pad::default()
    };
    m.step([Pad::default(), slide]);
    for _ in 0..60 * 12 {
        // Long enough for the arm to have gone up (the clip and its fade).
        if g.session
            .as_ref()
            .unwrap()
            .m
            .booking
            .is_some_and(|b| b.shown > 40)
        {
            break;
        }
        tick(&mut g);
    }
    assert!(
        g.session
            .as_ref()
            .unwrap()
            .m
            .booking
            .is_some_and(|b| b.shown > 0),
        "no card was shown"
    );
    let mut r = Rec::default();
    for (label, back) in [("near", None), ("far", Some(FAR_DEPTH + 12.0))] {
        let s = g.session.as_mut().unwrap();
        let referee = s.m.officials[0];
        // The broadcast camera on the referee; for the far shot, backed off
        // beyond the far-detail depth.
        s.view.cam.focus = trino_core::Vec3::new(referee.x.to_f32(), 0.9, referee.y.to_f32());
        if let Some(back) = back {
            s.view.cam.back = back;
            s.view.cam.height = 6.0;
        }
        g.render(&mut r);
        let light = r.light.unwrap_or(DEFAULT_LIGHT);
        let cards = r.draws.iter().filter(|d| d.0 == M_CARD).count();
        assert_eq!(cards, 1, "{label}: the card is drawn once");
        for (id, mat, params, cam) in &r.draws {
            let mesh = Mesh::from_tmdl(&meshes[&id.0]).unwrap();
            rsp_mesh_check(&mesh).unwrap();
            let ctx = TnlContext::new(cam, &light, Vec2::new(320.0, 240.0));
            // (The far shot's camera is staged: only the card is checked
            // there; the stadium may cross its near plane.)
            if back.is_some() && *id != M_CARD {
                continue;
            }
            if let Err(e) = rsp_check(&ctx, &mesh, mat, params) {
                assert_eq!(
                    e,
                    trino_core::RspReject::Offscreen,
                    "{label}: {:08x} leaves the RSP",
                    id.0
                );
                assert_ne!(*id, M_CARD, "{label}: the card is off screen");
            }
            if *id == M_CARD {
                // In his raised hand, over his head: not at a joint the far
                // body never posed.
                let w = solid_eleven::render::WORLD;
                let (x, y, z) = (mat.m[0][3] / w, mat.m[1][3] / w, mat.m[2][3] / w);
                let (dx, dz) = (x - referee.x.to_f32(), z - referee.y.to_f32());
                assert!(
                    dx * dx + dz * dz < 1.0 && (1.6..2.9).contains(&y),
                    "{label}: the card at ({x:.2}, {y:.2}, {z:.2}), the referee at {:?}",
                    referee.to_f32()
                );
                let ctx3 = TnlContext::new(cam, &light, Vec2::new(400.0, 240.0));
                assert!(trino_core::render3d::gpu_mesh_check(&mesh).is_ok());
                assert!(
                    trino_core::render3d::gpu_check(&ctx3, &mesh, mat, params).is_ok(),
                    "{label}: the card leaves the 3DS GPU"
                );
            }
        }
    }
}
