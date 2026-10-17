//! Evidence for water surfaces (issue #49, ADR 0015): the showcase's river
//! on each PC profile, as each tier draws it. Not a gate:
//!
//! ```sh
//! cargo test -p xtask --test water_shots -- --ignored --nocapture
//! ```
//!
//! Writes `target/evidence/water/`:
//! - `tiers_<view>.png`: the same frame on the native profile (per pixel:
//!   normals, Fresnel, glint, depth foam), the N64 look (per vertex, the
//!   2-cycle combiner's blend) and the 3DS profile (per vertex, TexEnv);
//! - `wade_*.png`: the hero wading in, a ripple ring spreading and
//!   the splash's droplets;
//! - `ripples_*.png`: the same, closer;
//! - `bridge.png`, `bridge_upstream.png`: the bridge over the river;
//! - `crate.png`: the loose crate shoved off the bank, floating.

mod ao_evidence;

use ao_evidence::{NullAudio, assets, renderer, root, row};
use showcase::ShowcaseGame;
use trino_core::{Camera3, Color, Game, InputState, Renderer, SpriteId, Vec2, Vec3};
use trino_platform_pc::SimProfile;

const PROFILES: [SimProfile; 3] = [SimProfile::Native, SimProfile::N64, SimProfile::N3ds];

/// A renderer that keeps its own camera: the game draws as always, seen
/// from where the evidence wants to look.
struct FixedCamera<'a> {
    r: &'a mut trino_platform_pc::PcRenderer,
    camera: Camera3,
}

impl Renderer for FixedCamera<'_> {
    fn caps(&self) -> &trino_core::Caps {
        self.r.caps()
    }
    fn begin_frame(&mut self, clear: Color) {
        self.r.begin_frame(clear);
        self.r.set_camera(&self.camera);
    }
    fn draw_sprite(&mut self, s: SpriteId, p: Vec2, params: &trino_core::SpriteParams) {
        self.r.draw_sprite(s, p, params)
    }
    fn set_camera(&mut self, _: &Camera3) {}
    fn set_light(&mut self, l: &trino_core::Light) {
        self.r.set_light(l)
    }
    fn set_local_lights(&mut self, l: &[trino_core::LocalLight]) {
        self.r.set_local_lights(l)
    }
    fn set_ambient_occlusion(&mut self, ao: Option<&trino_core::AmbientOcclusion>) {
        self.r.set_ambient_occlusion(ao)
    }
    fn set_texture_filter(&mut self, f: trino_core::TexFilter) {
        self.r.set_texture_filter(f)
    }
    fn set_water(&mut self, time: f32, ripples: &[trino_core::Ripple]) {
        self.r.set_water(time, ripples)
    }
    fn draw_model(
        &mut self,
        m: trino_core::ModelId,
        t: &trino_core::Transform3,
        mat: trino_core::Material,
        p: &trino_core::ModelParams,
    ) {
        self.r.draw_model(m, t, mat, p)
    }
    fn draw_model_matrix(
        &mut self,
        m: trino_core::ModelId,
        x: &trino_core::Mat34,
        mat: trino_core::Material,
        p: &trino_core::ModelParams,
    ) {
        self.r.draw_model_matrix(m, x, mat, p)
    }
    fn draw_billboard(&mut self, s: SpriteId, p: Vec3, params: &trino_core::BillboardParams) {
        self.r.draw_billboard(s, p, params)
    }
    fn draw_text(
        &mut self,
        f: trino_core::FontId,
        p: Vec2,
        t: &str,
        params: &trino_core::TextParams,
    ) {
        self.r.draw_text(f, p, t, params)
    }
    fn fill_rect(&mut self, rect: trino_core::Rect, c: Color) {
        self.r.fill_rect(rect, c)
    }
    fn end_frame(&mut self) {
        self.r.end_frame()
    }
}

/// [`panels`] seen from `camera`.
fn panels_from(
    assets: &trino_asset_pipeline::LoadedAssets,
    camera: Camera3,
    setup: impl Fn() -> ShowcaseGame,
) -> Vec<Vec<u8>> {
    PROFILES
        .iter()
        .map(|&p| {
            let mut r = renderer(p, assets);
            r.set_internal_size(640, 480);
            let mut g = setup();
            g.render(&mut FixedCamera { r: &mut r, camera });
            r.read_offscreen()
        })
        .collect()
}

fn out_dir() -> std::path::PathBuf {
    root().join("target/evidence/water")
}

/// One panel per profile, each the game at the same state, resized to the
/// N64's 320x240 frame x2 so the panels line up.
fn panels(
    assets: &trino_asset_pipeline::LoadedAssets,
    setup: impl Fn() -> ShowcaseGame,
) -> Vec<Vec<u8>> {
    PROFILES
        .iter()
        .map(|&p| {
            let mut r = renderer(p, assets);
            r.set_internal_size(640, 480);
            let mut g = setup();
            g.render(&mut r);
            r.read_offscreen()
        })
        .collect()
}

#[test]
#[ignore = "evidence: writes target/evidence/water (needs a GPU)"]
fn water_evidence() {
    let assets = assets();
    let out = out_dir();
    let idle = InputState::default();
    let screen = Vec2::new(640.0, 480.0);

    // The scene camera, a second in.
    let p = panels(&assets, || {
        let mut g = ShowcaseGame::with_screen(screen);
        g.look.camera = showcase::panel::CAMERA_SCENE;
        for _ in 0..60 {
            g.update(&idle, &mut NullAudio, 1.0 / 60.0);
        }
        g
    });
    row(&out.join("tiers_scene.png"), (640, 480), &p);

    // Behind the hero, wading into the river below the bridge.
    let wade = |frames: u32| {
        move || {
            let mut g = ShowcaseGame::with_screen(screen);
            g.look.camera = showcase::panel::CAMERA_HIGH;
            let c = showcase::river::COURSE[8];
            g.pos = Vec3::new(c.x, 0.6, c.z - 3.2);
            let mut input = InputState::default();
            input.stick = Vec2::new(0.0, 1.0);
            for i in 0..frames {
                let inp = if i < 50 { input } else { idle };
                g.update(&inp, &mut NullAudio, 1.0 / 60.0);
            }
            eprintln!(
                "wade {frames}: hero {:?} ripples {:?}",
                g.pos,
                g.ripples.as_slice()
            );
            g
        }
    };
    for (name, frames) in [("wade_enter", 44), ("wade_ring", 80)] {
        let p = panels(&assets, wade(frames));
        row(&out.join(format!("{name}.png")), (640, 480), &p);
    }
    // Close: the ripples around the wading hero.
    let c = showcase::river::COURSE[8];
    let close = Camera3 {
        eye: Vec3::new(c.x + 3.5, 3.2, c.z - 4.0),
        target: Vec3::new(c.x, -0.2, c.z),
        fov_y: 55f32.to_radians(),
    };
    for (name, frames) in [("ripples_enter", 46), ("ripples_ring", 75)] {
        let p = panels_from(&assets, close, wade(frames));
        row(&out.join(format!("{name}.png")), (640, 480), &p);
    }
    // The bridge over the river, from downstream (the robot fording) and
    // from upstream.
    for (name, eye, target) in [
        (
            "bridge",
            Vec3::new(1.0, 1.4, 0.9),
            Vec3::new(-4.6, 0.25, 0.1),
        ),
        (
            "bridge_upstream",
            Vec3::new(-9.0, 1.6, -1.6),
            Vec3::new(-4.6, 0.3, 0.2),
        ),
    ] {
        let camera = Camera3 {
            eye,
            target,
            fov_y: 55f32.to_radians(),
        };
        let p = panels_from(&assets, camera, || {
            let mut g = ShowcaseGame::with_screen(screen);
            for _ in 0..170 {
                g.update(&idle, &mut NullAudio, 1.0 / 60.0);
            }
            g
        });
        row(&out.join(format!("{name}.png")), (640, 480), &p);
    }

    // The crate shoved in, drifting.
    let p = panels(&assets, || {
        let mut g = ShowcaseGame::with_screen(screen);
        g.look.camera = showcase::panel::CAMERA_HIGH;
        let home = g
            .world
            .find("loose-crate")
            .and_then(|id| g.world.get(id))
            .unwrap()
            .position();
        g.pos = Vec3::new(home.x, 0.6, home.z - 1.2);
        let mut input = InputState::default();
        input.stick = Vec2::new(0.0, 1.0);
        for i in 0..150 {
            let inp = if i < 60 { input } else { idle };
            g.update(&inp, &mut NullAudio, 1.0 / 60.0);
        }
        g
    });
    row(&out.join("crate.png"), (640, 480), &p);
}
