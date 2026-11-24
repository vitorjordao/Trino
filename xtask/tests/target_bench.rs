//! Bongo-Kart #184: what a render target costs on this machine's GPU (the
//! issue asked for the Radeon). castle64's hub at 1080p, as the runner
//! draws it (4x MSAA, the game's SSAO, the GPU T&L path), against the same
//! frame plus a rear-view mirror: the game's whole render again into a
//! target, from the player's camera turned around, shown as a sprite at the
//! top of the screen. Then four 256×144 monitors (the most a frame draws,
//! `Caps::max_targets`). Each frame is the game's update, its renders and
//! the GPU's work (`wait_gpu`). A measurement for ADR 0046 §4, not a gate:
//!
//! ```sh
//! cargo test --release -p xtask --test target_bench -- --ignored --nocapture
//! ```
//!
//! `TRINO_BENCH_FRAMES` (default 240) sets the measured frames per row. The
//! table is written to `target/evidence/render-target/bench-<adapter>.md`
//! and each row's last frame beside it.

use std::path::{Path, PathBuf};
use std::time::Instant;

use trino_core::{
    BillboardParams, Camera3, Caps, Color, Game, InputState, Light, LocalLight, Mat34, Material,
    ModelId, ModelParams, RenderTarget, Renderer, SpriteId, SpriteParams, Transform3, Vec2, Vec3,
};
use trino_platform_pc::{PcRenderer, SimProfile};

struct Mute;
impl trino_core::Audio for Mute {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn renderer(size: (u32, u32)) -> PcRenderer {
    let baked = root().join("target/assets/bench-target/pc");
    trino_asset_pipeline::bake_all(
        &root().join("assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let loaded = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    let mut r = pollster::block_on(PcRenderer::new_headless(SimProfile::Native))
        .unwrap_or_else(|e| panic!("no adapter: {e}"));
    r.set_internal_size(size.0, size.1);
    r.set_msaa(std::env::var("TRINO_MSAA").as_deref() != Ok("0"));
    for s in loaded.sprites {
        r.upload_sprite(SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for m in loaded.models {
        r.upload_mesh(ModelId(m.id), m.tmdl);
    }
    for f in loaded.fonts {
        r.upload_font(trino_core::FontId(f.id), f.table);
    }
    r
}

/// The game's render into a target: its frame becomes the target, its
/// camera turns around, and the screen-only work (HUD, AO, shadows) is
/// left to the screen's render.
struct IntoTarget<'a> {
    r: &'a mut PcRenderer,
    target: RenderTarget,
    first: bool,
    view: View,
}

/// What a target sees.
#[derive(Clone, Copy, PartialEq)]
enum View {
    /// The screen's camera (a monitor, a stadium screen).
    Same,
    /// From the player, looking back (a kart's mirror).
    Rear,
    /// From above the player (a live map).
    Map,
}

impl Renderer for IntoTarget<'_> {
    fn caps(&self) -> &Caps {
        self.r.caps()
    }
    fn begin_frame(&mut self, clear: Color) {
        if self.first {
            self.r.begin_frame(clear);
        }
        assert!(self.r.begin_target(&self.target, clear));
    }
    fn end_frame(&mut self) {
        self.r.end_target();
    }
    fn set_camera(&mut self, c: &Camera3) {
        // The game's camera looks at the player from behind.
        let d = c.target - c.eye;
        let len = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
        let seen = match self.view {
            View::Same => *c,
            View::Rear => Camera3 {
                eye: c.target + Vec3::new(0.0, 0.3 * len, 0.0),
                target: c.target + Vec3::new(-d.x, 0.1 * len, -d.z),
                ..*c
            },
            View::Map => Camera3 {
                eye: c.target + Vec3::new(-0.05 * d.x, 3.0 * len, -0.05 * d.z),
                target: c.target + Vec3::new(0.3 * d.x, 0.0, 0.3 * d.z),
                ..*c
            },
        };
        self.r.set_camera(&seen);
    }
    fn set_light(&mut self, l: &Light) {
        self.r.set_light(l)
    }
    fn set_local_lights(&mut self, l: &[LocalLight]) {
        self.r.set_local_lights(l)
    }
    fn draw_model(&mut self, m: ModelId, t: &Transform3, mat: Material, p: &ModelParams) {
        self.r.draw_model(m, t, mat, p)
    }
    fn draw_model_matrix(&mut self, m: ModelId, x: &Mat34, mat: Material, p: &ModelParams) {
        self.r.draw_model_matrix(m, x, mat, p)
    }
    fn draw_billboard(&mut self, s: SpriteId, p: Vec3, params: &BillboardParams) {
        self.r.draw_billboard(s, p, params)
    }
    /// The HUD is the screen's.
    fn draw_sprite(&mut self, _: SpriteId, _: Vec2, _: &SpriteParams) {}
    fn measure_text(&self, f: trino_core::FontId, t: &str, s: f32) -> Vec2 {
        self.r.measure_text(f, t, s)
    }
}

/// The game's render onto the screen after the targets: its own frame
/// already begun, the targets shown at its end.
struct OntoScreen<'a> {
    r: &'a mut PcRenderer,
    shown: &'a [(RenderTarget, Vec2)],
}

impl Renderer for OntoScreen<'_> {
    fn caps(&self) -> &Caps {
        self.r.caps()
    }
    fn begin_frame(&mut self, _: Color) {}
    fn end_frame(&mut self) {
        for (t, at) in self.shown {
            let (w, h) = (f32::from(t.width), f32::from(t.height));
            let frame = trino_core::Rect::new(at.x - 4.0, at.y - 4.0, w + 8.0, h + 8.0);
            self.r.fill_rect(frame, Color::rgb(24, 24, 32));
            self.r.draw_sprite(t.sprite, *at, &SpriteParams::DEFAULT);
        }
        self.r.end_frame();
    }
    fn set_camera(&mut self, c: &Camera3) {
        self.r.set_camera(c)
    }
    fn set_light(&mut self, l: &Light) {
        self.r.set_light(l)
    }
    fn set_local_lights(&mut self, l: &[LocalLight]) {
        self.r.set_local_lights(l)
    }
    fn set_ambient_occlusion(&mut self, ao: Option<&trino_core::AmbientOcclusion>) {
        self.r.set_ambient_occlusion(ao)
    }
    fn set_shadows(&mut self, s: Option<&trino_core::ShadowSettings>) {
        self.r.set_shadows(s)
    }
    fn set_shadow_receivers(&mut self, p: &[trino_core::ShadowPlane]) {
        self.r.set_shadow_receivers(p)
    }
    fn draw_model(&mut self, m: ModelId, t: &Transform3, mat: Material, p: &ModelParams) {
        self.r.draw_model(m, t, mat, p)
    }
    fn draw_model_matrix(&mut self, m: ModelId, x: &Mat34, mat: Material, p: &ModelParams) {
        self.r.draw_model_matrix(m, x, mat, p)
    }
    fn draw_billboard(&mut self, s: SpriteId, p: Vec3, params: &BillboardParams) {
        self.r.draw_billboard(s, p, params)
    }
    fn draw_sprite(&mut self, s: SpriteId, p: Vec2, params: &SpriteParams) {
        self.r.draw_sprite(s, p, params)
    }
    fn fill_rect(&mut self, rect: trino_core::Rect, c: Color) {
        self.r.fill_rect(rect, c)
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
    fn measure_text(&self, f: trino_core::FontId, t: &str, s: f32) -> Vec2 {
        self.r.measure_text(f, t, s)
    }
}

/// One measured frame layout: the targets drawn before the screen, and
/// where each shows.
struct Setup {
    name: &'static str,
    view: View,
    targets: Vec<(RenderTarget, Vec2)>,
    game: castle64::Castle64Game,
    times: Vec<f64>,
    tnl_us: u64,
    stats: trino_core::FrameStats,
}

/// One frame of `s`: the game's update, its renders, the GPU's work.
fn frame(r: &mut PcRenderer, s: &mut Setup) -> f64 {
    let t = Instant::now();
    s.game
        .update(&InputState::default(), &mut Mute, castle64::perf::DT);
    if s.targets.is_empty() {
        s.game.render(r);
    } else {
        for (k, (target, _)) in s.targets.iter().enumerate() {
            s.game.render(&mut IntoTarget {
                r,
                target: *target,
                first: k == 0,
                view: s.view,
            });
        }
        s.game.render(&mut OntoScreen {
            r,
            shown: &s.targets,
        });
    }
    r.wait_gpu();
    t.elapsed().as_secs_f64() * 1000.0
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

fn slug(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

fn write_png(path: &Path, (w, h): (u32, u32), rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(rgba).unwrap();
}

#[test]
#[ignore = "a measurement: run with --release --ignored --nocapture"]
fn render_target_cost() {
    let (w, h) = (1920u32, 1080u32);
    let mut r = renderer((w, h));
    let adapter = r.adapter_info();
    let rounds: usize = std::env::var("TRINO_BENCH_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let per_round: usize = std::env::var("TRINO_BENCH_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    let top = |tw: u16| Vec2::new((w as f32 - f32::from(tw)) / 2.0, 24.0);
    let monitors = |n: usize, tw: u16, th: u16| -> Vec<(RenderTarget, Vec2)> {
        (0..n)
            .map(|k| {
                let t = RenderTarget::new(["mon/a", "mon/b", "mon/c", "mon/d"][k], tw, th);
                let at = Vec2::new(
                    24.0 + k as f32 * (f32::from(tw) + 24.0),
                    h as f32 - f32::from(th) - 24.0,
                );
                (t, at)
            })
            .collect()
    };
    let scene = &castle64::perf::SCENES[0];
    let game = || castle64::perf::scene_game(scene, Vec2::new(w as f32, h as f32), &mut Mute);
    let setup = |name, view, targets| Setup {
        name,
        view,
        targets,
        game: game(),
        times: Vec::new(),
        tnl_us: 0,
        stats: Default::default(),
    };
    let mirror = RenderTarget::new("targets/mirror", 480, 160);
    let map = RenderTarget::new("targets/map", 256, 256);
    let mut setups = vec![
        setup("screen only", View::Same, vec![]),
        setup(
            "rear-view mirror 480x160",
            View::Rear,
            vec![(mirror, top(480))],
        ),
        setup(
            "live map 256x256",
            View::Map,
            vec![(map, Vec2::new(w as f32 - 280.0, h as f32 - 280.0))],
        ),
        setup(
            "monitor 480x270, the same view",
            View::Same,
            monitors(1, 480, 270),
        ),
        setup(
            "monitor 960x540, the same view",
            View::Same,
            monitors(1, 960, 540),
        ),
        setup(
            "4 monitors 256x144, the same view",
            View::Same,
            monitors(4, 256, 144),
        ),
    ];
    // Warm every layout (pipelines, the GPU's clocks), then interleave them
    // round by round so a clock change lands on all of them alike.
    for s in &mut setups {
        for _ in 0..120 {
            frame(&mut r, s);
        }
    }
    for _ in 0..rounds {
        for s in &mut setups {
            for _ in 0..per_round {
                let ms = frame(&mut r, s);
                s.times.push(ms);
                s.tnl_us += r.frame_times().tnl_us as u64;
            }
            s.stats = r.stats();
            assert_eq!(s.stats.targets as usize, s.targets.len(), "{:?}", s.stats);
        }
    }
    let dir = root().join("target/evidence/render-target");
    for s in &mut setups {
        // Each layout's last frame, and its first target's picture.
        frame(&mut r, s);
        write_png(
            &dir.join(format!("frame-{}.png", slug(s.name))),
            (w, h),
            &r.read_offscreen(),
        );
        if let Some((t, _)) = s.targets.first() {
            let px = r.read_target(t).unwrap();
            write_png(
                &dir.join(format!("target-{}.png", slug(s.name))),
                (u32::from(t.width), u32::from(t.height)),
                &px,
            );
        }
        s.times.sort_by(|a, b| a.total_cmp(b));
    }
    let base = quantile(&setups[0].times, 0.5);
    let mut out = format!(
        "Adapter: {adapter}; castle64 hub at {w}x{h}, {}x MSAA, the game's SSAO, GPU T&L. \
         Every layout warmed for 120 frames, then {rounds} interleaved rounds of {per_round} \
         frames each (update + renders + the GPU's work, waited for).\n\n\
         | Frame | Targets | Median (ms) | p95 (ms) | Added (ms) | CPU T&L (ms) | Draws | Triangles |\n\
         |---|---:|---:|---:|---:|---:|---:|---:|\n",
        if std::env::var("TRINO_MSAA").as_deref() == Ok("0") {
            1
        } else {
            4
        },
    );
    for s in &setups {
        let median = quantile(&s.times, 0.5);
        out += &format!(
            "| {} | {} | {:.2} | {:.2} | {:+.2} | {:.3} | {} | {} |\n",
            s.name,
            s.targets.len(),
            median,
            quantile(&s.times, 0.95),
            median - base,
            s.tnl_us as f64 / 1000.0 / s.times.len() as f64,
            s.stats.models,
            s.stats.tris
        );
    }
    println!("{out}");
    let path = dir.join(format!("bench-{}.md", slug(&adapter)));
    std::fs::write(&path, out).unwrap();
    println!("written to {}", path.display());
}
